//! Production DHCP bootstrap with a full NIC RX queue and deterministic time.
#![allow(dead_code)]
extern crate self as libnanami;
extern crate self as nanami_services;
use std::{cell::RefCell, collections::VecDeque, ptr};
pub use std::{print, println};
pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
#[derive(Debug)]
pub enum RequestError {
    Transport,
    Protocol,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
#[path = "../../nanami/servers/libs/net-wire/src/lib.rs"]
#[allow(unused_attributes)]
mod wire;
use wire::{ipv4_checksum, read_u32_be, write_u16_be, write_u32_be};
const ETH_HDR_LEN: usize = 14;
const IPV4_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;
const BACKEND_TX_OFFSET: Word = 0;
struct NetStats;
struct NetRuntime {
    mac: [u8; 6],
    ip: [u8; 4],
    subnet_mask: [u8; 4],
    gateway_ip: [u8; 4],
    dns_ip: [u8; 4],
    timer_port: Option<Word>,
    buffer: Word,
    dhcp_waiting: bool,
    dhcp_offer_valid: bool,
    dhcp_ack_valid: bool,
    dhcp_xid: u32,
    dhcp_offer_ip: [u8; 4],
    dhcp_server_ip: [u8; 4],
    dhcp_router_ip: [u8; 4],
    dhcp_dns_ip: [u8; 4],
    dhcp_subnet_mask: Option<[u8; 4]>,
}
#[derive(Default)]
struct Fake {
    now: Word,
    alarm: Option<Word>,
    alarms: Vec<Option<Word>>,
    queue: VecDeque<Option<Vec<u8>>>,
    reply: bool,
    wakes: usize,
    sleeps: Vec<Word>,
    pump_delay: Word,
    reply_delay: Word,
    pending_rx: Option<(Word, VecDeque<Option<Vec<u8>>>)>,
    flood: bool,
    fail_wait: bool,
    offer_mask: Option<[u8; 4]>,
    ack_mask: Option<[u8; 4]>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::default(); }
pub mod ipc {
    use super::*;
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn notification_wait(slot: Word) -> Result<Word, RequestError> {
        assert_eq!(slot, PROCESS_SLOT_NOTIFICATION);
        FAKE.with_borrow_mut(|s| {
            s.wakes += 1;
            let alarm = s.alarm.expect("blocking without a fallback wakeup");
            if s.fail_wait {
                return Err(RequestError::Transport);
            }
            if s.pending_rx.as_ref().is_some_and(|(at, _)| *at < alarm) {
                let (at, packets) = s.pending_rx.take().unwrap();
                s.now = at;
                s.queue.extend(packets);
                Ok(1)
            } else {
                s.now = alarm;
                Ok(1 << 63)
            }
        })
    }
}
pub mod timer {
    use super::*;
    pub fn timer_service_monotonic_ticks(_: Word) -> Result<(Word, Word), RequestError> {
        Ok((FAKE.with_borrow(|s| s.now), 1000))
    }
    pub fn timer_service_set_alarm_ticks(
        _: Word,
        slot: Word,
        deadline: Option<Word>,
    ) -> Result<(), RequestError> {
        assert_eq!(slot, PROCESS_SLOT_NOTIFICATION);
        FAKE.with_borrow_mut(|s| {
            s.alarm = deadline;
            s.alarms.push(deadline);
        });
        Ok(())
    }
}
mod arp {
    pub(super) fn emit_gratuitous_arp_reply(
        _: &super::NetRuntime,
    ) -> Result<(), super::RequestError> {
        Ok(())
    }
}
fn emit_arp_request(_: &NetRuntime, _: [u8; 4]) -> Result<(), RequestError> {
    Ok(())
}
fn sleep_ms(_: Option<Word>, milliseconds: Word) {
    FAKE.with_borrow_mut(|s| {
        s.now += milliseconds;
        s.sleeps.push(milliseconds);
    });
}
fn log_request_error(_: &str, _: RequestError) {}
fn get_backend_shm_ptr(runtime: &NetRuntime, _: Word) -> *mut u8 {
    runtime.buffer as *mut u8
}
fn response(xid: u32, kind: u8, mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut packet = vec![0; 240];
    packet[..3].copy_from_slice(&[2, 1, 6]);
    packet[4..8].copy_from_slice(&xid.to_be_bytes());
    packet[16..20].copy_from_slice(&[192, 168, 1, 50]);
    packet[236..240].copy_from_slice(&0x63825363u32.to_be_bytes());
    packet.extend_from_slice(&[
        53, 1, kind, 54, 4, 192, 168, 1, 1, 3, 4, 192, 168, 1, 1, 6, 4, 192, 168, 1, 1, 255,
    ]);
    if let Some(mask) = mask {
        packet.pop();
        packet.extend_from_slice(&[1, 4]);
        packet.extend_from_slice(&mask);
        packet.push(255);
    }
    packet
}
fn emit_frame(runtime: &NetRuntime, len: usize) -> Result<Word, RequestError> {
    let payload =
        unsafe { std::slice::from_raw_parts((runtime.buffer + 42) as *const u8, len - 42) };
    let kind = payload[242];
    FAKE.with_borrow_mut(|s| {
        if s.reply && matches!(kind, 1 | 3) {
            // 63 unrelated LAN packets precede our DHCP reply in a 64-frame queue.
            let mut packets: VecDeque<_> = (0..63).map(|_| None).collect();
            packets.push_back(Some(response(
                runtime.dhcp_xid,
                if kind == 1 { 2 } else { 5 },
                if kind == 1 { s.offer_mask } else { s.ack_mask },
            )));
            if s.reply_delay == 0 {
                s.queue.extend(packets);
            } else {
                s.pending_rx = Some((s.now + s.reply_delay, packets));
            }
        }
    });
    Ok(len)
}
fn pump_backend_with_budget(runtime: &mut NetRuntime, _: &mut NetStats, budget: usize) -> Word {
    FAKE.with_borrow_mut(|s| {
        s.now += s.pump_delay;
        if s.flood {
            s.queue.extend((0..budget).map(|_| None));
            assert!(s.now <= 200_000, "LAN traffic extended the DHCP deadline");
        }
    });
    let mut count = 0;
    while count < budget {
        let Some(frame) = FAKE.with_borrow_mut(|s| s.queue.pop_front()) else {
            break;
        };
        if let Some(payload) = frame {
            dhcp::process_dhcp_payload(runtime, [192, 168, 1, 1], 67, 68, &payload);
        }
        count += 1;
    }
    count
}
#[path = "../../nanami/servers/apps/net-server/src/app/dhcp.rs"]
mod dhcp;
#[path = "../../nanami/servers/apps/net-server/src/app/routing.rs"]
mod routing;
fn setup(reply: bool) -> (Vec<u8>, NetRuntime) {
    FAKE.with_borrow_mut(|s| {
        *s = Fake {
            reply,
            ..Fake::default()
        }
    });
    let mut buffer = vec![0; 4096];
    let runtime = NetRuntime {
        mac: [2, 0, 0, 0, 0, 1],
        ip: [10, 0, 2, 15],
        subnet_mask: [255, 255, 255, 0],
        gateway_ip: [10, 0, 2, 2],
        dns_ip: [10, 0, 2, 3],
        timer_port: Some(1),
        buffer: buffer.as_mut_ptr() as usize,
        dhcp_waiting: false,
        dhcp_offer_valid: false,
        dhcp_ack_valid: false,
        dhcp_xid: 0,
        dhcp_offer_ip: [0; 4],
        dhcp_server_ip: [0; 4],
        dhcp_router_ip: [0; 4],
        dhcp_dns_ip: [0; 4],
        dhcp_subnet_mask: None,
    };
    (buffer, runtime)
}
#[test]
fn lan_burst_does_not_insert_minimum_sleeps_before_dhcp_replies() {
    let (_buffer, mut runtime) = setup(true);
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.ip, [192, 168, 1, 50]);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert!(s.now <= 400, "DHCP delayed by LAN frames: {} ms", s.now);
        assert_eq!(s.alarm, None);
    });
}
#[test]
fn absent_server_uses_bounded_monotonic_timeouts_and_cancels_alarm() {
    let (_buffer, mut runtime) = setup(false);
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.ip, [10, 0, 2, 15]);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert_eq!(s.now, 188_000);
        assert!(s.wakes > 0);
        assert_eq!(s.alarms.last(), Some(&None));
        assert_eq!(s.alarm, None);
    });
}

#[test]
fn rx_notification_wakes_before_the_polling_alarm() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| s.reply_delay = 10);
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.ip, [192, 168, 1, 50]);
    FAKE.with_borrow(|s| {
        assert_eq!(s.now, 170); // Two 10-ms RX wakes and three 50-ms announcements.
        assert_eq!(s.wakes, 2);
        assert_eq!(s.alarm, None);
        assert_eq!(s.alarms, [Some(200), None, Some(210), None]);
    });
}

#[test]
fn continuous_lan_traffic_and_pump_cost_do_not_extend_deadlines() {
    let (_buffer, mut runtime) = setup(false);
    FAKE.with_borrow_mut(|s| {
        s.flood = true;
        s.pump_delay = 25;
    });
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert_eq!(s.now, 188_000);
        assert_eq!(s.wakes, 0);
        assert_eq!(s.alarm, None);
    });
}

#[test]
fn notification_errors_cancel_each_outstanding_alarm() {
    let (_buffer, mut runtime) = setup(false);
    FAKE.with_borrow_mut(|s| s.fail_wait = true);
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert_eq!(s.wakes, 6);
        assert_eq!(s.alarm, None);
        assert_eq!(s.alarms, [Some(200), None].repeat(6));
    });
}

#[test]
fn dhcp_offer_mask_is_retained_when_ack_omits_it() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| s.offer_mask = Some([255, 255, 254, 0]));
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.subnet_mask, [255, 255, 254, 0]);
    assert_eq!(
        routing::next_hop_ip(&runtime, [192, 168, 0, 9]),
        [192, 168, 0, 9]
    );
    assert_eq!(
        routing::next_hop_ip(&runtime, [192, 168, 2, 9]),
        runtime.gateway_ip
    );
}

#[test]
fn dhcp_ack_mask_overrides_offer_mask() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| {
        s.offer_mask = Some([255, 255, 0, 0]);
        s.ack_mask = Some([255, 255, 255, 128]);
    });
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.subnet_mask, [255, 255, 255, 128]);
    assert_eq!(
        routing::next_hop_ip(&runtime, [192, 168, 1, 100]),
        [192, 168, 1, 100]
    );
    assert_eq!(
        routing::next_hop_ip(&runtime, [192, 168, 1, 200]),
        runtime.gateway_ip
    );
}

#[test]
fn malformed_mask_does_not_replace_configured_subnet() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| {
        s.offer_mask = Some([255, 0, 255, 0]);
        s.ack_mask = Some([255, 255, 255, 1]);
    });
    dhcp::dhcp_bootstrap(&mut runtime, &mut NetStats);
    assert_eq!(runtime.subnet_mask, [255, 255, 255, 0]);
}

#[test]
fn routing_accepts_all_contiguous_prefixes_and_rejects_invalid_masks() {
    let (_buffer, mut runtime) = setup(false);
    for prefix in 0..=32 {
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        runtime.subnet_mask = mask.to_be_bytes();
        assert!(routing::valid_subnet_mask(runtime.subnet_mask));
        for destination in [[10, 0, 2, 16], [172, 16, 1, 20], [1, 1, 1, 1], runtime.ip] {
            let expected =
                if (u32::from_be_bytes(destination) ^ u32::from_be_bytes(runtime.ip)) & mask == 0 {
                    destination
                } else {
                    runtime.gateway_ip
                };
            assert_eq!(routing::next_hop_ip(&runtime, destination), expected);
        }
    }
    for mask in [[255, 0, 255, 0], [255, 255, 255, 1], [0, 0, 0, 1]] {
        assert!(!routing::valid_subnet_mask(mask));
    }
}
