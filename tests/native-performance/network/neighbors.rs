use super::*;
use std::cmp::min;
#[path = "../../../nanami/servers/libs/net-wire/src/lib.rs"]
#[allow(unused_attributes)]
mod wire;
use wire::read_u16_be;
#[path = "../../../nanami/servers/apps/net-server/src/app/arp_cache.rs"]
mod arp_cache;
use arp_cache::ArpCache;
#[path = "../../../nanami/servers/apps/net-server/src/app/arp.rs"]
mod arp;
use arp::{arp_lookup, emit_arp_request};
#[path = "../../../nanami/servers/apps/net-server/src/app/ip.rs"]
mod ip;
#[path = "../../../nanami/servers/apps/net-server/src/app/tcp_send.rs"]
mod tcp_send;

const ETH_HDR_LEN: usize = 14;
const BACKEND_TX_OFFSET: Word = 0;
const IPV4_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;
const TCP_PAYLOAD_MAX: usize = 1460;
const TCP_FLAG_FIN: u8 = 1;
const TCP_STATE_FIN_WAIT1: u8 = 3;
const TCP_STATE_CLOSE_WAIT: u8 = 5;
const TCP_STATE_LAST_ACK: u8 = 6;
#[derive(Clone, Copy)]
struct TcpConnection {
    peer_ip: [u8; 4],
    local_port: u16,
    peer_port: u16,
    snd_nxt: u32,
    rcv_nxt: u32,
    state: u8,
}
#[derive(Clone, Copy)]
struct Session {
    shm_local: Word,
    shm_size: Word,
}
struct NetRuntime {
    mac: [u8; 6],
    ip: [u8; 4],
    gateway_ip: [u8; 4],
    arp: ArpCache,
    dhcp_waiting: bool,
    buffer: Word,
    session: Session,
    tcp_connections: [TcpConnection; 1],
    tcp_rx: RxWindow,
}
struct RxWindow;
impl RxWindow {
    fn window(&self, _: usize) -> u16 {
        TCP_PAYLOAD_MAX as u16
    }
}
#[derive(Default)]
struct NetStats {
    tcp_tx: Word,
}
fn get_backend_shm_ptr(runtime: &NetRuntime, _: Word) -> *mut u8 {
    runtime.buffer as *mut u8
}
fn emit_frame(_: &NetRuntime, len: usize) -> Result<Word, RequestError> {
    Ok(len)
}
fn next_hop_ip(runtime: &NetRuntime, ip: [u8; 4]) -> [u8; 4] {
    if ip[..3] == runtime.ip[..3] {
        ip
    } else {
        runtime.gateway_ip
    }
}
fn session_for(runtime: &NetRuntime, owner: Word) -> Option<Session> {
    (owner == 1).then_some(runtime.session)
}
fn active_tcp_connection_index(_: &NetRuntime, owner: Word, id: Word) -> Option<usize> {
    (owner == 1 && id == 7).then_some(0)
}
fn map_request_error_to_status(_: RequestError) -> Word {
    OS_RESPONSE_FATAL
}
fn emit_tcp_segment(
    _: &NetRuntime,
    _: [u8; 6],
    _: [u8; 4],
    _: u16,
    _: u16,
    _: u32,
    _: u32,
    _: u8,
    _: u16,
    data: &[u8],
) -> Result<Word, RequestError> {
    Ok(data.len())
}

mod dhcp {
    use super::*;
    pub(super) fn process_dhcp_payload(
        _: &mut NetRuntime,
        _: [u8; 4],
        _: u16,
        _: u16,
        _: &[u8],
    ) -> bool {
        false
    }
}
mod dns {
    use super::*;
    pub(super) fn process_dns_payload(
        _: &mut NetRuntime,
        _: [u8; 4],
        _: u16,
        _: u16,
        _: &[u8],
    ) -> bool {
        false
    }
}
mod icmp {
    use super::*;
    pub(super) fn process_icmp(
        _: &mut NetRuntime,
        _: &[u8],
        _: usize,
        _: [u8; 4],
        _: [u8; 4],
        _: usize,
    ) {
    }
}
mod tcp {
    use super::*;
    pub(super) fn process_tcp(
        _: &mut NetRuntime,
        _: &mut NetStats,
        _: &[u8],
        _: usize,
        _: [u8; 4],
        _: usize,
    ) {
    }
}
mod udp {
    use super::*;
    pub(super) fn try_queue_udp(
        _: &mut NetRuntime,
        _: [u8; 4],
        _: [u8; 4],
        _: u16,
        _: u16,
        _: &[u8],
    ) {
    }
}

fn setup() -> (Vec<u8>, NetRuntime) {
    let mut buffer = vec![0u8; 4096];
    let runtime = NetRuntime {
        mac: [2, 0, 0, 0, 0, 26],
        ip: [192, 168, 1, 26],
        gateway_ip: [192, 168, 1, 1],
        arp: ArpCache::EMPTY,
        dhcp_waiting: false,
        buffer: buffer.as_mut_ptr() as Word,
        session: Session {
            shm_local: buffer.as_mut_ptr() as Word,
            shm_size: buffer.len(),
        },
        tcp_connections: [TcpConnection {
            peer_ip: [192, 168, 1, 22],
            local_port: 80,
            peer_port: 1234,
            snd_nxt: 100,
            rcv_nxt: 200,
            state: 2,
        }],
        tcp_rx: RxWindow,
    };
    (buffer, runtime)
}
fn ipv4(src: [u8; 4], dst: [u8; 4], mac: [u8; 6]) -> [u8; 34] {
    let mut frame = [0; 34];
    frame[6..12].copy_from_slice(&mac);
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&20u16.to_be_bytes());
    frame[26..30].copy_from_slice(&src);
    frame[30..34].copy_from_slice(&dst);
    frame
}

#[test]
fn multiple_neighbors_and_mac_updates_do_not_overwrite_the_http_peer() {
    let (_, mut runtime) = setup();
    let peer = [192, 168, 1, 22];
    runtime.arp.update(peer, [2; 6]);
    for host in 30..50 {
        runtime.arp.update([192, 168, 1, host], [4; 6]);
    }
    assert_eq!(arp_lookup(&runtime, peer), Some([2; 6]));
    runtime.arp.update(peer, [6; 6]);
    assert_eq!(arp_lookup(&runtime, peer), Some([6; 6]));
    assert_eq!(arp_lookup(&runtime, [192, 168, 1, 30]), Some([4; 6]));
}

#[test]
fn bounded_cache_handles_replacement_and_relearning() {
    let mut cache = ArpCache::EMPTY;
    for host in 1..=200 {
        cache.update([10, 0, 0, host], [2, 0, 0, 0, 0, host]);
    }
    assert_eq!(cache.lookup([10, 0, 0, 1]), None);
    assert_eq!(cache.lookup([10, 0, 0, 200]), Some([2, 0, 0, 0, 0, 200]));
    cache.update([10, 0, 0, 1], [4; 6]);
    assert_eq!(cache.lookup([10, 0, 0, 1]), Some([4; 6]));
}

#[test]
fn unrelated_multicast_broadcast_and_unicast_do_not_pollute_neighbors() {
    let (_, mut runtime) = setup();
    let peer = [192, 168, 1, 22];
    runtime.arp.update(peer, [2; 6]);
    for dst in [
        [224, 0, 0, 251],
        [224, 0, 0, 22],
        [255; 4],
        [192, 168, 1, 99],
    ] {
        for host in 30..100 {
            ip::process_ipv4(
                &mut runtime,
                &mut NetStats::default(),
                &ipv4([192, 168, 1, host], dst, [4; 6]),
            );
        }
    }
    assert_eq!(arp_lookup(&runtime, peer), Some([2; 6]));
    assert_eq!(arp_lookup(&runtime, [192, 168, 1, 30]), None);
}

#[test]
fn addressed_packets_learn_local_peers_and_routed_next_hops() {
    let (_, mut runtime) = setup();
    let dst = runtime.ip;
    ip::process_ipv4(
        &mut runtime,
        &mut NetStats::default(),
        &ipv4([192, 168, 1, 22], dst, [2; 6]),
    );
    ip::process_ipv4(
        &mut runtime,
        &mut NetStats::default(),
        &ipv4([10, 0, 0, 2], dst, [4; 6]),
    );
    assert_eq!(arp_lookup(&runtime, [192, 168, 1, 22]), Some([2; 6]));
    assert_eq!(arp_lookup(&runtime, runtime.gateway_ip), Some([4; 6]));
    assert_eq!(arp_lookup(&runtime, [10, 0, 0, 2]), None);
}

#[test]
fn arp_miss_does_not_consume_sequence_numbers_or_fin_and_retry_succeeds() {
    let (_buffer, mut runtime) = setup();
    let mut stats = NetStats::default();
    let request = || ipc::ServiceRequest {
        identifier: 1,
        arg0: 512,
        arg1: 10,
        arg2: 0x19,
        arg3: 7,
    };
    assert_eq!(
        tcp_send::handle_tcp_send_request(&mut runtime, request(), &mut stats),
        (net::NET_SERVICE_RESPONSE_WOULD_BLOCK, 0, 0)
    );
    assert_eq!(
        (
            runtime.tcp_connections[0].snd_nxt,
            runtime.tcp_connections[0].state,
            stats.tcp_tx
        ),
        (100, 2, 0)
    );
    runtime.arp.update([192, 168, 1, 22], [2; 6]);
    assert_eq!(
        tcp_send::handle_tcp_send_request(&mut runtime, request(), &mut stats),
        (OS_RESPONSE_OK, 10, 0)
    );
    assert_eq!(
        (
            runtime.tcp_connections[0].snd_nxt,
            runtime.tcp_connections[0].state,
            stats.tcp_tx
        ),
        (111, TCP_STATE_FIN_WAIT1, 1)
    );
}

#[test]
fn closed_connection_is_not_reported_as_arp_wait() {
    let (_buffer, mut runtime) = setup();
    let request = ipc::ServiceRequest {
        identifier: 1,
        arg3: 8,
        ..Default::default()
    };
    assert_eq!(
        tcp_send::handle_tcp_send_request(&mut runtime, request, &mut NetStats::default()).0,
        OS_RESPONSE_ILLEGAL_OPERATION
    );
}

#[test]
fn arp_request_to_us_learns_sender_but_malformed_arp_does_not() {
    let (_buffer, mut runtime) = setup();
    let mut frame = [0u8; 42];
    frame[14..20].copy_from_slice(&[0, 1, 8, 0, 6, 4]);
    frame[21] = 1;
    frame[22..28].copy_from_slice(&[2; 6]);
    frame[28..32].copy_from_slice(&[192, 168, 1, 22]);
    frame[38..42].copy_from_slice(&runtime.ip);
    arp::process_arp(&mut runtime, &frame);
    assert_eq!(arp_lookup(&runtime, [192, 168, 1, 22]), Some([2; 6]));
    frame[18] = 8;
    frame[22..28].copy_from_slice(&[4; 6]);
    arp::process_arp(&mut runtime, &frame);
    assert_eq!(arp_lookup(&runtime, [192, 168, 1, 22]), Some([2; 6]));
}
