use super::*;
use std::{cmp::min, ptr};

const META_OFFSET: Word = 0;
const RX_OFFSET: Word = 4096;
const TX_OFFSET: Word = 8192;
const TCP_FLAG_FIN: Word = 1;
const TCP_FLAG_PSH: Word = 8;
const TCP_FLAG_ACK: Word = 16;
const HTTP_CONNECTIONS: usize = 128;
const RX_BUDGET: usize = 128;
const TX_BUDGET: usize = 128;
const TCP_RECV_MAX: Word = 1400;
const HTTP_HEADER_CAP: usize = 256;
const IDLE_RECV_SPIN_LOOPS: usize = 1;
const IDLE_RECV_SLEEP_MS: Word = 1;
const IDLE_SPIN_BEFORE_SLEEP: usize = 8;
#[derive(Clone, Copy)]
enum ResponseKind {
    NotFound,
}
struct ResponseCache;
fn response_header(_: &ResponseCache, _: ResponseKind, _: bool) -> (&'static [u8], usize) {
    (b"header", 6)
}
fn build_response_cache() -> ResponseCache {
    ResponseCache
}
fn select_response_kind(_: &[u8]) -> ResponseKind {
    ResponseKind::NotFound
}
fn response_body(_: ResponseKind) -> &'static [u8] {
    b"body"
}
fn should_keep_alive(_: &[u8]) -> bool {
    false
}
fn tx_capacity(_: Word) -> usize {
    4096
}
fn sleep_ms(_: Option<Word>, ms: Word) {
    FAKE.with(|fake| fake.borrow_mut().sleeps.push(ms));
}
fn log_request_error(_: &str, _: RequestError) {
    FAKE.with(|fake| fake.borrow_mut().logs += 1);
}

mod reactor {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../nanami/servers/apps/http-server/src/reactor.rs"
    ));

    fn setup() -> (Vec<u8>, [HttpConnection; HTTP_CONNECTIONS]) {
        let mut shm = vec![0u8; 16384];
        FAKE.with(|fake| {
            *fake.borrow_mut() = Fake {
                shm: shm.as_mut_ptr() as Word,
                ..Fake::default()
            }
        });
        (shm, [HttpConnection::EMPTY; HTTP_CONNECTIONS])
    }

    #[test]
    fn arp_wait_retains_response_and_retries_without_new_requests() {
        let (mut shm, mut conns) = setup();
        prepare_response(&mut conns[0], 7, b"GET /", &ResponseCache);
        FAKE.with(|fake| {
            fake.borrow_mut().send_error =
                Some(RequestError::Status(net::NET_SERVICE_RESPONSE_WOULD_BLOCK))
        });
        for _ in 0..3 {
            assert_eq!(
                flush_http_tx(
                    1,
                    shm.as_mut_ptr() as Word,
                    4096,
                    &ResponseCache,
                    &mut conns
                ),
                (false, true)
            );
            assert!(conns[0].active);
            assert_eq!((conns[0].header_sent, conns[0].body_sent), (0, 0));
            idle_recv_backoff(1, Some(2), &mut Some(3), &mut 0, true);
        }
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            assert_eq!(fake.waits, 0);
            assert_eq!(fake.sleeps, [TX_RETRY_MS; 3]);
            assert_eq!(fake.polls, 3);
            assert_eq!(fake.logs, 0);
            fake.send_error = None;
        });
        assert_eq!(
            flush_http_tx(
                1,
                shm.as_mut_ptr() as Word,
                4096,
                &ResponseCache,
                &mut conns
            ),
            (true, false)
        );
        assert!(!conns[0].active);
        FAKE.with(|fake| {
            assert_eq!(
                fake.borrow().sent,
                [(
                    7,
                    b"headerbody".to_vec(),
                    TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN
                )]
            )
        });
    }

    #[test]
    fn arp_wait_after_first_chunk_does_not_repeat_sent_bytes() {
        let (mut shm, mut conns) = setup();
        prepare_response(&mut conns[0], 7, b"GET /", &ResponseCache);
        let ptr = shm.as_mut_ptr() as Word;
        assert_eq!(
            flush_http_tx(1, ptr, 4, &ResponseCache, &mut conns),
            (true, true)
        );
        FAKE.with(|fake| {
            fake.borrow_mut().send_error =
                Some(RequestError::Status(net::NET_SERVICE_RESPONSE_WOULD_BLOCK))
        });
        assert_eq!(
            flush_http_tx(1, ptr, 4, &ResponseCache, &mut conns),
            (false, true)
        );
        assert_eq!((conns[0].header_sent, conns[0].body_sent), (4, 0));
        FAKE.with(|fake| fake.borrow_mut().send_error = None);
        while conns[0].active {
            flush_http_tx(1, ptr, 4, &ResponseCache, &mut conns);
        }
        FAKE.with(|fake| {
            let fake = fake.borrow();
            let bytes: Vec<u8> = fake
                .sent
                .iter()
                .flat_map(|entry| entry.1.iter().copied())
                .collect();
            assert_eq!(bytes, b"headerbody");
            assert_eq!(
                fake.sent
                    .iter()
                    .filter(|entry| entry.2 & TCP_FLAG_FIN != 0)
                    .count(),
                1
            );
        });
    }

    #[test]
    fn full_response_slots_leave_requests_queued_until_a_slot_is_available() {
        let (mut shm, mut conns) = setup();
        for (i, conn) in conns.iter_mut().enumerate() {
            prepare_response(conn, i + 1, b"GET /", &ResponseCache);
        }
        FAKE.with(|fake| {
            fake.borrow_mut()
                .incoming
                .push_back((1000, b"GET /".to_vec()))
        });
        assert!(!drain_tcp_rx(
            1,
            shm.as_mut_ptr() as Word,
            &ResponseCache,
            &mut conns
        ));
        FAKE.with(|fake| assert_eq!(fake.borrow().recv_calls, 0));
        conns[0] = HttpConnection::EMPTY;
        assert!(drain_tcp_rx(
            1,
            shm.as_mut_ptr() as Word,
            &ResponseCache,
            &mut conns
        ));
        assert_eq!(conns[0].connection_id, 1000);
        FAKE.with(|fake| assert_eq!(fake.borrow().recv_calls, 1));
    }

    #[test]
    fn fin_only_is_also_retained_during_arp_resolution() {
        let (mut shm, mut conns) = setup();
        finish_peer_close(&mut conns[0], 77);
        FAKE.with(|fake| {
            fake.borrow_mut().send_error =
                Some(RequestError::Status(net::NET_SERVICE_RESPONSE_WOULD_BLOCK))
        });
        assert_eq!(
            flush_http_tx(
                1,
                shm.as_mut_ptr() as Word,
                4096,
                &ResponseCache,
                &mut conns
            ),
            (false, true)
        );
        FAKE.with(|fake| fake.borrow_mut().send_error = None);
        assert_eq!(
            flush_http_tx(
                1,
                shm.as_mut_ptr() as Word,
                4096,
                &ResponseCache,
                &mut conns
            ),
            (true, false)
        );
        FAKE.with(|fake| {
            assert_eq!(
                fake.borrow().sent,
                [(77, vec![], TCP_FLAG_ACK | TCP_FLAG_FIN)]
            )
        });
    }

    #[test]
    fn peer_fin_keeps_the_pending_response() {
        let (_, mut conns) = setup();
        prepare_response(&mut conns[0], 7, b"GET /", &ResponseCache);
        conns[0].keep_alive = true;
        conns[0].header_sent = 3;
        finish_peer_close(&mut conns[0], 7);
        assert!(!conns[0].close_only);
        assert!(!conns[0].keep_alive);
        assert_eq!(conns[0].header_sent, 3);
    }

    #[test]
    fn ordinary_idle_uses_notifications_without_timer_or_poll() {
        let _ = setup();
        idle_recv_backoff(1, Some(2), &mut Some(3), &mut 0, false);
        FAKE.with(|fake| {
            let fake = fake.borrow();
            assert_eq!(fake.waits, 1);
            assert!(fake.sleeps.is_empty());
            assert_eq!(fake.polls, 0);
        });
    }

    #[test]
    fn missing_timer_still_retries_and_polls_without_blocking_on_rx() {
        let _ = setup();
        idle_recv_backoff(1, None, &mut Some(3), &mut 0, true);
        FAKE.with(|fake| {
            let fake = fake.borrow();
            assert_eq!(fake.waits, 0);
            assert!(fake.sleeps.is_empty());
            assert_eq!(fake.polls, 1);
        });
    }

    #[test]
    fn fatal_errors_are_not_misclassified_as_arp_wait() {
        let (mut shm, mut conns) = setup();
        prepare_response(&mut conns[0], 7, b"GET /", &ResponseCache);
        FAKE.with(|fake| fake.borrow_mut().send_error = Some(RequestError::Protocol));
        assert_eq!(
            flush_http_tx(
                1,
                shm.as_mut_ptr() as Word,
                4096,
                &ResponseCache,
                &mut conns
            ),
            (true, false)
        );
        assert!(!conns[0].active);
        FAKE.with(|fake| assert_eq!(fake.borrow().logs, 1));
    }
}
