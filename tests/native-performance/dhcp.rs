//! Production DHCP bootstrap with a full NIC RX queue and deterministic time.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
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
#[path = "../../nanami/servers/apps/net-server/src/app/interfaces.rs"]
mod interfaces;
pub mod net {
    pub use nanami_uapi::net::*;
}
fn refresh_dhcp_port(_: &mut NetRuntime, _: bool) -> bool {
    false
}

#[test]
fn physical_port_selection_uses_carrier_not_pci_order_and_rotates_on_timeout() {
    use net::*;
    let mut carrier = [false, false];
    let mut active = 0;
    let mut choose = |carrier: [bool; 2], rotate| {
        interfaces::select_linked(
            |code, index| match code {
                NET_DEVICE_CONTROL_GET_FEATURES => Some((NET_DEVICE_FEATURE_PORTS, 0)),
                NET_DEVICE_CONTROL_PORT_COUNT => Some((2, active)),
                NET_DEVICE_CONTROL_PORT_INFO => Some((index, carrier[index] as Word)),
                NET_DEVICE_CONTROL_SELECT_PORT => {
                    active = index;
                    Some((0x040404040404 + index, 0))
                }
                _ => None,
            },
            rotate,
        )
    };
    assert_eq!(choose(carrier, false), None);
    carrier[1] = true;
    assert_eq!(choose(carrier, false), Some((1, 0x040404040405)));
    assert_eq!(choose(carrier, false), None); // Keep carrier; do not flap.
    carrier[0] = true;
    assert_eq!(choose(carrier, true), Some((0, 0x040404040404)));
    carrier[0] = false;
    assert_eq!(choose(carrier, false), Some((1, 0x040404040405)));
}

#[test]
fn legacy_and_malformed_port_providers_are_not_selected() {
    assert_eq!(interfaces::select_linked(|_, _| Some((0, 0)), false), None);
    assert_eq!(interfaces::select_linked(|_, _| None, false), None);
    assert_eq!(
        interfaces::select_linked(|_, _| Some((99, 99)), false),
        None
    );
}
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
    strict_bootp_minimum: bool,
    transmitted: Vec<Vec<u8>>,
    status_reports: usize,
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
fn report_backend_status(_: &NetRuntime) {
    FAKE.with_borrow_mut(|s| s.status_reports += 1);
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
    let frame = unsafe { std::slice::from_raw_parts(runtime.buffer as *const u8, len) };
    let payload =
        unsafe { std::slice::from_raw_parts((runtime.buffer + 42) as *const u8, len - 42) };
    let kind = payload[242];
    FAKE.with_borrow_mut(|s| {
        s.transmitted.push(frame.to_vec());
        // RFC 1542 section 2.1: strict BOOTP servers/relays discard messages
        // whose IP/UDP lengths do not include 300 octets of BOOTP data.
        if s.strict_bootp_minimum && payload.len() < 300 {
            return;
        }
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
            offer_mask: Some([255, 255, 255, 0]),
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

// Drive the production asynchronous state machine from a simulated service
// loop. The DHCP implementation itself never waits, sleeps or owns an alarm.
fn drive_acquisition(runtime: &mut NetRuntime) {
    runtime.ip = [0; 4];
    runtime.subnet_mask = [0; 4];
    runtime.gateway_ip = [0; 4];
    runtime.dns_ip = [0; 4];
    let now = FAKE.with_borrow(|s| s.now);
    let mut acquisition = dhcp::Acquisition::start(runtime, now);
    for _ in 0..10_000 {
        pump_backend_with_budget(runtime, &mut NetStats, 128);
        let now = FAKE.with_borrow(|s| s.now);
        let state = acquisition.poll(runtime, now);
        if state != net::IPV4_STATE_ACQUIRING {
            return;
        }
        if FAKE.with_borrow(|s| !s.queue.is_empty()) {
            continue;
        }
        timer::timer_service_set_alarm_ticks(1, PROCESS_SLOT_NOTIFICATION, Some(now + 100))
            .unwrap();
        ipc::notification_wait(PROCESS_SLOT_NOTIFICATION).unwrap();
        timer::timer_service_set_alarm_ticks(1, PROCESS_SLOT_NOTIFICATION, None).unwrap();
    }
    panic!("DHCP never completed or timed out");
}
#[test]
fn lan_burst_does_not_insert_minimum_sleeps_before_dhcp_replies() {
    let (_buffer, mut runtime) = setup(true);
    drive_acquisition(&mut runtime);
    assert_eq!(runtime.ip, [192, 168, 1, 50]);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert!(s.now <= 400, "DHCP delayed by LAN frames: {} ms", s.now);
        assert_eq!(s.alarm, None);
    });
}

#[test]
fn strict_bootp_server_accepts_full_length_discover_and_request() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| s.strict_bootp_minimum = true);
    drive_acquisition(&mut runtime);
    assert_eq!(runtime.ip, [192, 168, 1, 50]);
    FAKE.with_borrow(|s| {
        assert_eq!(s.transmitted.len(), 2);
        assert_eq!(s.status_reports, 0);
        for frame in &s.transmitted {
            assert_eq!(frame.len(), 14 + 20 + 8 + 300);
            assert_eq!(&frame[..6], &[255; 6]);
            assert_eq!(&frame[6..12], &runtime.mac);
            let ip = &frame[14..34];
            assert_eq!(wire::checksum16(ip, 0), 0);
            assert_eq!(&ip[12..16], &[0; 4]);
            assert_eq!(&ip[16..20], &[255; 4]);
            assert_eq!(u16::from_be_bytes([ip[2], ip[3]]), 328);
            assert_eq!(&frame[34..38], &[0, 68, 0, 67]);
            assert_eq!(u16::from_be_bytes([frame[38], frame[39]]), 308);
            let payload = &frame[42..];
            assert_eq!(&payload[..3], &[1, 1, 6]);
            assert_eq!(&payload[28..34], &runtime.mac);
            assert_eq!(&payload[236..240], &[99, 130, 83, 99]);
            let end = payload.iter().rposition(|byte| *byte == 255).unwrap();
            assert!(payload[end + 1..].iter().all(|byte| *byte == 0));
        }
    });
}

#[test]
fn discover_and_request_use_the_current_physical_ports_mac_and_transaction_id() {
    // Addresses differ from the backend's initial port. A late port selection
    // must update Ethernet SA, BOOTP chaddr AND client-id, not only one layer.
    for mac in [
        [0, 4, 0x5f, 0x7e, 0xb8, 0xda],
        [2, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5],
    ] {
        let (_buffer, mut runtime) = setup(true);
        runtime.mac = mac;
        drive_acquisition(&mut runtime);
        assert_eq!(runtime.ip, [192, 168, 1, 50]);
        FAKE.with_borrow(|s| {
            assert_eq!(s.transmitted.len(), 2);
            for frame in &s.transmitted {
                assert_eq!(&frame[6..12], &mac);
                let payload = &frame[42..];
                assert_eq!(&payload[28..34], &mac);
                assert_eq!(read_u32_be(&payload[4..8]), runtime.dhcp_xid);
                let mut offset = 240;
                let mut client_id = None;
                while payload[offset] != 255 {
                    let code = payload[offset];
                    if code == 0 {
                        offset += 1;
                        continue;
                    }
                    let length = payload[offset + 1] as usize;
                    if code == 61 {
                        client_id = Some(&payload[offset + 2..offset + 2 + length]);
                    }
                    offset += 2 + length;
                }
                let mut expected = vec![1];
                expected.extend_from_slice(&mac);
                assert_eq!(client_id, Some(expected.as_slice()));
            }
        });
    }
}
#[test]
fn absent_server_uses_bounded_monotonic_timeouts_and_cancels_alarm() {
    let (_buffer, mut runtime) = setup(false);
    drive_acquisition(&mut runtime);
    assert_eq!(runtime.ip, [0; 4]); // DHCP mode must not silently use a static fallback.
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert_eq!(s.now, 188_000);
        assert_eq!(s.status_reports, 6);
        assert!(s.wakes > 0);
        assert_eq!(s.alarms.last(), Some(&None));
        assert_eq!(s.alarm, None);
    });
}

#[test]
fn rx_notification_wakes_before_the_polling_alarm() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| s.reply_delay = 10);
    drive_acquisition(&mut runtime);
    assert_eq!(runtime.ip, [192, 168, 1, 50]);
    FAKE.with_borrow(|s| {
        assert_eq!(s.now, 20); // Two 10-ms RX wakes; no blocking announcements.
        assert_eq!(s.wakes, 2);
        assert_eq!(s.alarm, None);
        assert_eq!(s.alarms, [Some(100), None, Some(110), None]);
    });
}

#[test]
fn continuous_lan_traffic_and_pump_cost_do_not_extend_deadlines() {
    let (_buffer, mut runtime) = setup(false);
    FAKE.with_borrow_mut(|s| {
        s.flood = true;
        s.pump_delay = 25;
    });
    drive_acquisition(&mut runtime);
    assert!(!runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert!((188_000..189_000).contains(&s.now));
        assert!(s.wakes > 0);
        assert_eq!(s.alarm, None);
    });
}

#[test]
fn acquisition_step_never_waits_on_notifications_or_sleeps() {
    let (_buffer, mut runtime) = setup(false);
    FAKE.with_borrow_mut(|s| s.fail_wait = true);
    let mut acquisition = dhcp::Acquisition::start(&mut runtime, 0);
    for now in 0..100 {
        assert_eq!(
            acquisition.poll(&mut runtime, now),
            net::IPV4_STATE_ACQUIRING
        );
    }
    assert!(runtime.dhcp_waiting);
    FAKE.with_borrow(|s| {
        assert_eq!(s.wakes, 0);
        assert_eq!(s.alarm, None);
        assert!(s.alarms.is_empty());
        assert!(s.sleeps.is_empty());
    });
}

#[test]
fn dhcp_offer_mask_is_retained_when_ack_omits_it() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| s.offer_mask = Some([255, 255, 254, 0]));
    drive_acquisition(&mut runtime);
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
    drive_acquisition(&mut runtime);
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
fn malformed_lease_mask_leaves_dhcp_unconfigured_instead_of_using_old_static_settings() {
    let (_buffer, mut runtime) = setup(true);
    FAKE.with_borrow_mut(|s| {
        s.offer_mask = Some([255, 0, 255, 0]);
        s.ack_mask = Some([255, 255, 255, 1]);
    });
    drive_acquisition(&mut runtime);
    assert_eq!(runtime.subnet_mask, [0; 4]);
    assert_eq!(runtime.ip, [0; 4]);
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
