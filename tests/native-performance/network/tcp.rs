//! Real TCP wire/state/receive code. Only IPC/shared memory and the NIC are faked.
use super::*;
use std::cmp::min;
#[path = "../../../nanami/servers/libs/net-wire/src/lib.rs"]
#[allow(unused_attributes)]
mod wire;
use wire::{checksum16, ipv4_checksum, read_u16_be, read_u32_be, write_u16_be, write_u32_be};
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp_state.rs"]
mod state;
use state::*;
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp_rx.rs"]
mod rx;
use rx::{TcpRxBuffer, TcpRxQueue};
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp_wire.rs"]
mod tcp_wire;
use tcp_wire::emit_tcp_segment;
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp.rs"]
mod tcp;
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp_index.rs"]
mod tcp_index;
use tcp_index::{active_tcp_connection_index, TcpIndex};

const ETH_HDR_LEN: usize = 14;
const IPV4_HDR_LEN: usize = 20;
const BACKEND_TX_OFFSET: Word = 0;
const PEER_IP: [u8; 4] = [10, 0, 2, 2];
#[derive(Clone, Copy)]
struct Session {
    caller_id: Word,
    shm_local: Word,
    shm_size: Word,
}
struct NetRuntime {
    mac: [u8; 6],
    ip: [u8; 4],
    tcp_connections: [TcpConnection; TCP_MAX_CONNECTIONS],
    tcp_index: TcpIndex,
    tcp_rx: TcpRxQueue,
    session: Session,
    tx: std::cell::UnsafeCell<[u8; 2048]>,
    emitted: RefCell<Vec<Vec<u8>>>,
}
#[derive(Default)]
struct NetStats {
    tcp_rx: Word,
    tcp_tx: Word,
}
fn get_backend_shm_ptr(runtime: &NetRuntime, _: Word) -> *mut u8 {
    runtime.tx.get().cast()
}
fn emit_frame(runtime: &NetRuntime, len: usize) -> Result<Word, RequestError> {
    let frame = unsafe { std::slice::from_raw_parts(runtime.tx.get().cast::<u8>(), len) };
    runtime.emitted.borrow_mut().push(frame.to_vec());
    Ok(len)
}
fn session_for(runtime: &NetRuntime, owner: Word) -> Option<Session> {
    (owner == runtime.session.caller_id).then_some(runtime.session)
}
fn session_for_tcp_port(runtime: &NetRuntime, port: u16) -> Option<Session> {
    (port == 80).then_some(runtime.session)
}
fn next_hop_ip(_: &NetRuntime, ip: [u8; 4]) -> [u8; 4] {
    ip
}
fn arp_lookup(_: &NetRuntime, _: [u8; 4]) -> Option<[u8; 6]> {
    Some([2; 6])
}

fn setup() -> (Vec<u8>, NetRuntime, NetStats) {
    let mut shm = vec![0; 4096];
    let buffers: Box<[TcpRxBuffer]> = (0..TCP_MAX_CONNECTIONS)
        .map(|_| TcpRxBuffer::EMPTY)
        .collect();
    let runtime = NetRuntime {
        mac: [4; 6],
        ip: [10, 0, 2, 15],
        tcp_connections: [TcpConnection::EMPTY; TCP_MAX_CONNECTIONS],
        tcp_index: TcpIndex::EMPTY,
        tcp_rx: TcpRxQueue::new(
            Box::leak(buffers)
                .try_into()
                .unwrap_or_else(|_| unreachable!()),
        ),
        session: Session {
            caller_id: 1,
            shm_local: shm.as_mut_ptr() as Word,
            shm_size: shm.len(),
        },
        tx: std::cell::UnsafeCell::new([0; 2048]),
        emitted: RefCell::default(),
    };
    (shm, runtime, NetStats::default())
}

#[test]
fn indexed_ids_survive_reuse_but_reject_stale_and_foreign_handles() {
    let (_shm, mut runtime, mut stats) = setup();
    let mut previous = 0;
    for _ in 0..2048 {
        connect(&mut runtime, &mut stats, 0, 100);
        let id = runtime.tcp_connections[0].connection_id;
        assert!(id <= u32::MAX as Word);
        assert_ne!(id, 0);
        assert_ne!(id, previous);
        assert_eq!(active_tcp_connection_index(&runtime, 1, id), Some(0));
        assert_eq!(active_tcp_connection_index(&runtime, 2, id), None);
        if previous != 0 {
            assert_eq!(active_tcp_connection_index(&runtime, 1, previous), None);
        }
        tcp::tcp_reset(&mut runtime, 0);
        assert_eq!(active_tcp_connection_index(&runtime, 1, id), None);
        assert_eq!(
            runtime
                .tcp_index
                .find(&runtime.tcp_connections, PEER_IP, 1000, 80),
            None
        );
        assert_eq!(runtime.tcp_index.vacant(), Some(0));
        previous = id;
    }
}

#[test]
fn tcp_index_removal_preserves_colliding_connections_and_reuses_free_slots() {
    let (_shm, mut runtime, mut stats) = setup();
    for index in 0..TCP_MAX_CONNECTIONS {
        connect(&mut runtime, &mut stats, index, 100);
    }
    assert_eq!(runtime.tcp_index.vacant(), None);
    for index in (0..TCP_MAX_CONNECTIONS).step_by(3) {
        tcp::tcp_reset(&mut runtime, index);
    }
    for index in 0..TCP_MAX_CONNECTIONS {
        let found =
            runtime
                .tcp_index
                .find(&runtime.tcp_connections, PEER_IP, 1000 + index as u16, 80);
        assert_eq!(found, if index % 3 == 0 { None } else { Some(index) });
    }
    for index in (0..TCP_MAX_CONNECTIONS).step_by(3) {
        assert_eq!(runtime.tcp_index.vacant(), Some(index));
        connect(&mut runtime, &mut stats, index, 200);
    }
    assert_eq!(runtime.tcp_index.vacant(), None);
}
fn packet(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    data: &[u8],
) {
    let mut frame = vec![0; ETH_HDR_LEN + IPV4_HDR_LEN + TCP_HDR_LEN + data.len()];
    frame[6..12].copy_from_slice(&[2; 6]);
    let tcp = &mut frame[ETH_HDR_LEN + IPV4_HDR_LEN..];
    write_u16_be(&mut tcp[..2], port);
    write_u16_be(&mut tcp[2..4], 80);
    write_u32_be(&mut tcp[4..8], seq);
    write_u32_be(&mut tcp[8..12], ack);
    tcp[12] = 0x50;
    tcp[13] = flags;
    tcp[TCP_HDR_LEN..].copy_from_slice(data);
    tcp::process_tcp(runtime, stats, &frame, IPV4_HDR_LEN, PEER_IP, frame.len());
}
fn connect(runtime: &mut NetRuntime, stats: &mut NetStats, index: usize, seq: u32) -> u32 {
    let port = 1000 + index as u16;
    packet(
        runtime,
        stats,
        port,
        seq.wrapping_sub(1),
        0,
        TCP_FLAG_SYN,
        b"",
    );
    let ack = runtime.tcp_connections[index].snd_nxt;
    packet(runtime, stats, port, seq, ack, TCP_FLAG_ACK, b"");
    assert_eq!(runtime.tcp_connections[index].state, TCP_STATE_ESTABLISHED);
    ack
}
fn receive(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    id: Word,
    max: Word,
) -> (Word, Word, Word) {
    rx::handle_tcp_recv_request(
        runtime,
        ipc::ServiceRequest {
            identifier: 1,
            arg0: 0,
            arg1: 128,
            arg2: max,
            arg3: id,
        },
        stats,
    )
}
fn ack_window(runtime: &NetRuntime) -> (u32, u16) {
    let frames = runtime.emitted.borrow();
    let tcp = &frames.last().unwrap()[ETH_HDR_LEN + IPV4_HDR_LEN..];
    (read_u32_be(&tcp[8..12]), read_u16_be(&tcp[14..16]))
}

#[test]
fn simultaneous_gets_have_reserved_capacity_not_a_shared_32_packet_limit() {
    let (shm, mut runtime, mut stats) = setup();
    for index in 0..TCP_MAX_CONNECTIONS {
        let ack = connect(&mut runtime, &mut stats, index, 100);
        packet(
            &mut runtime,
            &mut stats,
            1000 + index as u16,
            100,
            ack,
            TCP_FLAG_ACK,
            b"GET / HTTP/1.0\r\n\r\n",
        );
        assert_eq!(runtime.tcp_connections[index].rcv_nxt, 118);
    }
    assert_eq!(runtime.tcp_rx.count, TCP_MAX_CONNECTIONS);
    for id in 1..=TCP_MAX_CONNECTIONS {
        assert_eq!(
            receive(&mut runtime, &mut stats, 0, 1400),
            (OS_RESPONSE_OK, 18, id)
        );
        assert_eq!(&shm[128..146], b"GET / HTTP/1.0\r\n\r\n");
    }
    assert_eq!(runtime.tcp_rx.count, 0);
}

#[test]
fn full_window_does_not_ack_and_drop_and_read_sends_window_update() {
    let (shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, 100);
    let data = vec![b'x'; TCP_PAYLOAD_MAX];
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK,
        &data,
    );
    let end = 100 + data.len() as u32;
    assert_eq!(ack_window(&runtime), (end, 0));
    packet(
        &mut runtime,
        &mut stats,
        1000,
        end,
        ack,
        TCP_FLAG_ACK,
        b"next",
    );
    assert_eq!(ack_window(&runtime), (end, 0));
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1400),
        (OS_RESPONSE_OK, 1400, 1)
    );
    assert_eq!(&shm[128..1528], &data[..1400]);
    assert_eq!(ack_window(&runtime), (end, 1400));
    packet(
        &mut runtime,
        &mut stats,
        1000,
        end,
        ack,
        TCP_FLAG_ACK,
        b"next",
    );
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1400),
        (OS_RESPONSE_OK, 64, 1)
    );
    assert_eq!(&shm[128..188], &data[1400..]);
    assert_eq!(&shm[188..192], b"next");
}

#[test]
fn small_reads_do_not_shrink_or_prematurely_open_the_advertised_right_edge() {
    let (_shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, 100);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK,
        &[0; TCP_PAYLOAD_MAX],
    );
    let sent = runtime.emitted.borrow().len();
    for _ in 0..7 {
        assert_eq!(receive(&mut runtime, &mut stats, 1, 100).1, 100);
    }
    assert_eq!(runtime.emitted.borrow().len(), sent);
    assert_eq!(runtime.tcp_rx.window(0), 0);
    receive(&mut runtime, &mut stats, 1, 100);
    assert_eq!(ack_window(&runtime), (1560, 800));
    packet(
        &mut runtime,
        &mut stats,
        1000,
        1559,
        ack,
        TCP_FLAG_ACK,
        b"x",
    );
    assert_eq!(ack_window(&runtime), (1560, 800));
}

#[test]
fn partial_window_acceptance_retries_only_the_tail_before_fin() {
    let (shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, 100);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK,
        &[b'a'; 1300],
    );
    packet(
        &mut runtime,
        &mut stats,
        1000,
        1400,
        ack,
        TCP_FLAG_ACK | TCP_FLAG_FIN,
        &[b'b'; 300],
    );
    assert_eq!(ack_window(&runtime), (1560, 0));
    assert!(!runtime.tcp_connections[0].eof_pending);
    assert_eq!(receive(&mut runtime, &mut stats, 1, 1460).1, 1460);
    assert_eq!(&shm[128..1428], &[b'a'; 1300]);
    assert_eq!(&shm[1428..1588], &[b'b'; 160]);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        1400,
        ack,
        TCP_FLAG_ACK | TCP_FLAG_FIN,
        &[b'b'; 300],
    );
    assert_eq!(ack_window(&runtime).0, 1701);
    assert_eq!(receive(&mut runtime, &mut stats, 1, 1460).1, 140);
    assert_eq!(&shm[128..268], &[b'b'; 140]);
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1460),
        (OS_RESPONSE_OK, 0, 1)
    );
}

#[test]
fn retransmission_overlap_and_sequence_wrap_preserve_stream_bytes_once() {
    let (shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, u32::MAX - 2);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        u32::MAX - 2,
        ack,
        TCP_FLAG_ACK,
        b"abc",
    );
    packet(
        &mut runtime,
        &mut stats,
        1000,
        u32::MAX - 2,
        ack,
        TCP_FLAG_ACK,
        b"abcdef",
    );
    assert_eq!(ack_window(&runtime).0, 3);
    assert_eq!(receive(&mut runtime, &mut stats, 1, 1400).1, 6);
    assert_eq!(&shm[128..134], b"abcdef");
}

#[test]
fn duplicate_syn_does_not_replace_connection_id_or_discard_buffered_data() {
    let (shm, mut runtime, mut stats) = setup();
    packet(&mut runtime, &mut stats, 1000, 99, 0, TCP_FLAG_SYN, b"");
    packet(&mut runtime, &mut stats, 1000, 99, 0, TCP_FLAG_SYN, b"");
    assert_eq!(runtime.tcp_index.vacant(), Some(1));
    let ack = runtime.tcp_connections[0].snd_nxt;
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK,
        b"GET",
    );
    packet(&mut runtime, &mut stats, 1000, 99, 0, TCP_FLAG_SYN, b"");
    assert_eq!(runtime.tcp_index.vacant(), Some(1));
    assert_eq!(runtime.tcp_connections[0].state, TCP_STATE_ESTABLISHED);
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1400),
        (OS_RESPONSE_OK, 3, 1)
    );
    assert_eq!(&shm[128..131], b"GET");
}

#[test]
fn fin_cannot_skip_a_receive_hole_and_payload_fin_orders_data_before_eof() {
    let (shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, 100);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        103,
        ack,
        TCP_FLAG_ACK | TCP_FLAG_FIN,
        b"",
    );
    assert!(!runtime.tcp_connections[0].eof_pending);
    assert_eq!(runtime.tcp_connections[0].state, TCP_STATE_ESTABLISHED);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK | TCP_FLAG_FIN,
        b"GET",
    );
    assert_eq!(ack_window(&runtime).0, 104);
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1400),
        (OS_RESPONSE_OK, 3, 1)
    );
    assert_eq!(&shm[128..131], b"GET");
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 1400),
        (OS_RESPONSE_OK, 0, 1)
    );
    let sent = runtime.emitted.borrow().len();
    packet(&mut runtime, &mut stats, 1000, 104, ack, TCP_FLAG_ACK, b"");
    assert_eq!(
        runtime.emitted.borrow().len(),
        sent,
        "do not ACK a pure ACK in CLOSE_WAIT"
    );
}

#[test]
fn rejected_receive_and_zero_length_receive_leave_data_queued() {
    let (_shm, mut runtime, mut stats) = setup();
    let ack = connect(&mut runtime, &mut stats, 0, 100);
    packet(
        &mut runtime,
        &mut stats,
        1000,
        100,
        ack,
        TCP_FLAG_ACK,
        b"GET",
    );
    for offset in [4090, usize::MAX] {
        let request = ipc::ServiceRequest {
            identifier: 1,
            arg0: offset,
            arg1: 128,
            arg2: 10,
            arg3: 1,
        };
        assert_eq!(
            rx::handle_tcp_recv_request(&mut runtime, request, &mut stats).0,
            OS_RESPONSE_INVALID_ARGUMENT
        );
    }
    assert_eq!(
        receive(&mut runtime, &mut stats, 1, 0),
        (OS_RESPONSE_OK, 0, 0)
    );
    assert_eq!(receive(&mut runtime, &mut stats, 1, 1400).1, 3);
}

#[test]
fn reset_removes_only_that_connections_ready_index_before_reuse() {
    let (shm, mut runtime, mut stats) = setup();
    for index in 0..4 {
        let ack = connect(&mut runtime, &mut stats, index, 100);
        packet(
            &mut runtime,
            &mut stats,
            1000 + index as u16,
            100,
            ack,
            TCP_FLAG_ACK,
            &[index as u8],
        );
    }
    tcp::tcp_reset(&mut runtime, 1);
    assert_eq!(runtime.tcp_rx.count, 3);
    assert_eq!(runtime.tcp_rx.window(1), TCP_PAYLOAD_MAX as u16);
    for index in [0, 2, 3] {
        assert_eq!(
            receive(&mut runtime, &mut stats, 0, 10),
            (OS_RESPONSE_OK, 1, index + 1)
        );
        assert_eq!(shm[128], index as u8);
    }
}

#[test]
fn slow_reader_cannot_exhaust_another_connections_receive_credit() {
    let (_shm, mut runtime, mut stats) = setup();
    for index in 0..2 {
        let ack = connect(&mut runtime, &mut stats, index, 100);
        packet(
            &mut runtime,
            &mut stats,
            1000 + index as u16,
            100,
            ack,
            TCP_FLAG_ACK,
            &[0; TCP_PAYLOAD_MAX],
        );
    }
    assert_eq!(
        receive(&mut runtime, &mut stats, 2, 1460),
        (OS_RESPONSE_OK, 1460, 2)
    );
    assert_eq!(runtime.tcp_rx.window(0), 0);
    assert_eq!(runtime.tcp_rx.window(1), TCP_PAYLOAD_MAX as u16);
    assert_eq!(runtime.tcp_rx.count, 1);
}

#[test]
fn allocation_does_not_reclaim_unacknowledged_response_or_fin() {
    let (_shm, mut runtime, mut stats) = setup();
    for index in 0..TCP_MAX_CONNECTIONS {
        connect(&mut runtime, &mut stats, index, 100);
    }
    runtime.tcp_connections[0].state = TCP_STATE_FIN_WAIT1;
    runtime.tcp_connections[1].state = TCP_STATE_LAST_ACK;
    assert_eq!(tcp::allocate_tcp_connection(&runtime), None);
    runtime.tcp_connections[2].state = TCP_STATE_FIN_WAIT2;
    assert_eq!(tcp::allocate_tcp_connection(&runtime), Some(2));
}
