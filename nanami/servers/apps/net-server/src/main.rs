#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;

use libnanami::ipc::ServiceEvent;
use libnanami::{self, RequestError, Word};
use net_wire::{checksum16, ipv4_checksum, read_u16_be, read_u32_be, write_u16_be, write_u32_be};

#[path = "app/arp.rs"]
mod arp;
#[path = "app/arp_cache.rs"]
mod arp_cache;
#[path = "app/backend.rs"]
mod backend;
#[path = "app/dhcp.rs"]
mod dhcp;
#[path = "app/dns.rs"]
mod dns;
#[path = "app/ethernet.rs"]
mod ethernet;
#[path = "app/icmp.rs"]
mod icmp;
#[path = "app/ip.rs"]
mod ip;
#[path = "app/tcp.rs"]
mod tcp;
#[path = "app/tcp_index.rs"]
mod tcp_index;
#[path = "app/tcp_send.rs"]
mod tcp_send;
#[path = "app/tcp_state.rs"]
mod tcp_state;
#[path = "app/tcp_rx.rs"]
mod tcp_rx;
#[path = "app/tcp_wire.rs"]
mod tcp_wire;
#[path = "app/udp.rs"]
mod udp;
#[path = "app/util.rs"]
mod util;

use arp::arp_lookup;
use arp::emit_arp_request;
use arp_cache::ArpCache;
use backend::{pump_backend, pump_backend_with_budget};
use dhcp::dhcp_bootstrap;
use dns::handle_dns_query_request;
use ethernet::process_ethernet_frame;
use icmp::{emit_icmp_echo_from_session, handle_icmp_recv_request};
use tcp_index::{active_tcp_connection_index, TcpIndex};
use tcp_rx::{handle_tcp_recv_request, TcpRxBuffer, TcpRxQueue};
use tcp_send::handle_tcp_send_request;
use tcp_state::*;
use tcp_wire::emit_tcp_segment;
use udp::{emit_udp_from_session, handle_udp_recv_request};
use util::{log_request_error, map_request_error_to_status};

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[net-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

const SLOT_SERVICE_PORT: Word = 20;
const SLOT_NET_DEVICE_PORT: Word = 23;
const SLOT_TIMER_SERVICE_PORT: Word = 22;
const SLOT_CLIENT_RX_NOTIFICATION_BASE: Word = 32;
const BACKOFF_MS: Word = 100;
const TIMER_CONNECT_RETRIES: usize = 128;
const RX_MAINTENANCE_INTERVAL_MS: Word = 10;

const BACKEND_SHM_BYTES: Word = 0x20000;
const BACKEND_RX_OFFSET: Word = 0x0000;
const BACKEND_TX_OFFSET: Word = 0x1000;

const CLIENT_DEFAULT_SHM_BYTES: Word = 0x4000;
const ETH_HDR_LEN: usize = 14;
const IPV4_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;
const UDP_PAYLOAD_MAX: usize = 1472;
const UDP_RX_META_LEN: usize = 16;
// The service loop is the sole owner; keep packet storage off the user stack.
static mut TCP_RX_BUFFERS: [TcpRxBuffer; TCP_MAX_CONNECTIONS] =
    [const { TcpRxBuffer::EMPTY }; TCP_MAX_CONNECTIONS];
const UDP_RX_QUEUE_CAP: usize = 8;
const ICMP_PAYLOAD_MAX: usize = 1480;
const ICMP_RX_META_LEN: usize = 8;
const ICMP_RX_QUEUE_CAP: usize = 8;
const CLIENT_SESSION_MAX: usize = 4;
const CLIENT_BIND_MAX: usize = 16;
const RAW_RX_MAX_BYTES: usize = 1536;
const RAW_RX_QUEUE_CAP: usize = 4;
const DNS_QUERY_TIMEOUT_MS: Word = 1200;
const REQUEST_PUMP_BURST: usize = 64;
const BACKEND_PUMP_DEFAULT_BURST: usize = 128;
const SPIN_LOOPS_PER_MS_FALLBACK: usize = 2_000;

#[derive(Clone, Copy)]
struct NetStats {
    tx_packets: Word,
    tx_bytes: Word,
    rx_packets: Word,
    rx_bytes: Word,
    udp_tx: Word,
    udp_rx: Word,
    tcp_tx: Word,
    tcp_rx: Word,
}

impl NetStats {
    const fn new() -> Self {
        Self {
            tx_packets: 0,
            tx_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            udp_tx: 0,
            udp_rx: 0,
            tcp_tx: 0,
            tcp_rx: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    caller_id: Word,
    shm_local: Word,
    shm_size: Word,
    udp_ports: [u16; CLIENT_BIND_MAX],
    tcp_ports: [u16; CLIENT_BIND_MAX],
    icmp_identifiers: [u16; CLIENT_BIND_MAX],
    raw_rx_enabled: bool,
    rx_notification: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        caller_id: 0,
        shm_local: 0,
        shm_size: 0,
        udp_ports: [0; CLIENT_BIND_MAX],
        tcp_ports: [0; CLIENT_BIND_MAX],
        icmp_identifiers: [0; CLIENT_BIND_MAX],
        raw_rx_enabled: false,
        rx_notification: 0,
    };
}

#[derive(Clone, Copy)]
struct IcmpRxEntry {
    used: bool,
    owner_id: Word,
    identifier: u16,
    src_ip: [u8; 4],
    ttl: u8,
    len: usize,
    payload: [u8; ICMP_PAYLOAD_MAX],
}

impl IcmpRxEntry {
    const EMPTY: Self = Self {
        used: false,
        owner_id: 0,
        identifier: 0,
        src_ip: [0; 4],
        ttl: 0,
        len: 0,
        payload: [0; ICMP_PAYLOAD_MAX],
    };
}

#[derive(Clone, Copy)]
struct IcmpRxQueue {
    entries: [IcmpRxEntry; ICMP_RX_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl IcmpRxQueue {
    const fn new() -> Self {
        Self {
            entries: [IcmpRxEntry::EMPTY; ICMP_RX_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, mut entry: IcmpRxEntry) {
        if self.count == self.entries.len() {
            self.entries[self.head] = IcmpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
        }
        entry.used = true;
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % self.entries.len();
        self.count += 1;
    }

    fn pop_for(&mut self, owner_id: Word, identifier: u16) -> Option<IcmpRxEntry> {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = IcmpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used
                && entry.owner_id == owner_id
                && (identifier == 0 || entry.identifier == identifier)
            {
                return Some(entry);
            }
            if entry.used {
                self.push(entry);
            }
            checked += 1;
        }
        None
    }

    fn remove_owner(&mut self, owner_id: Word) {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = IcmpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used && entry.owner_id != owner_id {
                self.push(entry);
            }
            checked += 1;
        }
    }
}

#[derive(Clone, Copy)]
struct UdpRxEntry {
    used: bool,
    pid: Word,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    len: usize,
    payload: [u8; UDP_PAYLOAD_MAX],
}

impl UdpRxEntry {
    const EMPTY: Self = Self {
        used: false,
        pid: 0,
        src_ip: [0; 4],
        dst_ip: [0; 4],
        src_port: 0,
        dst_port: 0,
        len: 0,
        payload: [0; UDP_PAYLOAD_MAX],
    };
}

#[derive(Clone, Copy)]
struct UdpRxQueue {
    entries: [UdpRxEntry; UDP_RX_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl UdpRxQueue {
    const fn new() -> Self {
        Self {
            entries: [UdpRxEntry::EMPTY; UDP_RX_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, mut entry: UdpRxEntry) {
        if self.count == self.entries.len() {
            self.entries[self.head] = UdpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
        }
        entry.used = true;
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % self.entries.len();
        self.count += 1;
    }

    fn pop_for(&mut self, pid: Word, port: u16) -> Option<UdpRxEntry> {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = UdpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used && entry.pid == pid && entry.dst_port == port {
                return Some(entry);
            }
            if entry.used {
                self.push(entry);
            }
            checked += 1;
        }
        None
    }

    fn remove_owner(&mut self, pid: Word) {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = UdpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used && entry.pid != pid {
                self.push(entry);
            }
            checked += 1;
        }
    }
}

#[derive(Clone, Copy)]
struct RawRxEntry {
    used: bool,
    owner_id: Word,
    len: usize,
    payload: [u8; RAW_RX_MAX_BYTES],
}

impl RawRxEntry {
    const EMPTY: Self = Self {
        used: false,
        owner_id: 0,
        len: 0,
        payload: [0; RAW_RX_MAX_BYTES],
    };
}

#[derive(Clone, Copy)]
struct RawRxQueue {
    entries: [RawRxEntry; RAW_RX_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl RawRxQueue {
    const fn new() -> Self {
        Self {
            entries: [RawRxEntry::EMPTY; RAW_RX_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, owner_id: Word, frame: &[u8]) {
        if self.count == self.entries.len() {
            self.entries[self.head] = RawRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
        }

        let mut entry = RawRxEntry::EMPTY;
        entry.used = true;
        entry.owner_id = owner_id;
        entry.len = min(frame.len(), RAW_RX_MAX_BYTES);
        if entry.len > 0 {
            unsafe {
                ptr::copy_nonoverlapping(frame.as_ptr(), entry.payload.as_mut_ptr(), entry.len);
            }
        }
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % self.entries.len();
        self.count += 1;
    }

    fn pop_for(&mut self, owner_id: Word) -> Option<RawRxEntry> {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = RawRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used && entry.owner_id == owner_id {
                return Some(entry);
            }
            if entry.used {
                self.push(entry.owner_id, &entry.payload[..entry.len]);
            }
            checked += 1;
        }
        None
    }

    fn remove_owner(&mut self, owner_id: Word) {
        let pending = self.count;
        let mut checked = 0usize;
        while checked < pending {
            let entry = self.entries[self.head];
            self.entries[self.head] = RawRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if entry.used && entry.owner_id != owner_id {
                self.push(entry.owner_id, &entry.payload[..entry.len]);
            }
            checked += 1;
        }
    }
}

struct NetRuntime {
    net_device_port: Word,
    timer_port: Option<Word>,
    backend_shm_local: Word,
    backend_rx_batch: bool,
    mac: [u8; 6],
    ip: [u8; 4],
    gateway_ip: [u8; 4],
    dns_ip: [u8; 4],
    arp: ArpCache,
    tcp_connections: [TcpConnection; TCP_MAX_CONNECTIONS],
    tcp_index: TcpIndex,
    sessions: [ClientSession; CLIENT_SESSION_MAX],
    udp_rx: UdpRxQueue,
    icmp_rx: IcmpRxQueue,
    tcp_rx: TcpRxQueue,
    raw_rx: RawRxQueue,
    dhcp_waiting: bool,
    dhcp_xid: u32,
    dhcp_offer_valid: bool,
    dhcp_ack_valid: bool,
    dhcp_offer_ip: [u8; 4],
    dhcp_server_ip: [u8; 4],
    dhcp_router_ip: [u8; 4],
    dhcp_dns_ip: [u8; 4],
    dns_waiting: bool,
    dns_txid: u16,
    dns_src_port: u16,
    dns_answer_valid: bool,
    dns_answer_ip: [u8; 4],
}

fn session_index(runtime: &NetRuntime, caller_id: Word) -> Option<usize> {
    runtime
        .sessions
        .iter()
        .position(|session| session.active && session.caller_id == caller_id)
}

fn session_for(runtime: &NetRuntime, caller_id: Word) -> Option<ClientSession> {
    session_index(runtime, caller_id).map(|index| runtime.sessions[index])
}

fn session_for_udp_port(runtime: &NetRuntime, port: u16) -> Option<ClientSession> {
    runtime
        .sessions
        .iter()
        .copied()
        .find(|session| session.active && session.udp_ports.contains(&port))
}

fn session_for_tcp_port(runtime: &NetRuntime, port: u16) -> Option<ClientSession> {
    runtime
        .sessions
        .iter()
        .copied()
        .find(|session| session.active && session.tcp_ports.contains(&port))
}

fn cleanup_client_session(runtime: &mut NetRuntime, index: usize) {
    let session = runtime.sessions[index];
    if !session.active {
        return;
    }

    runtime.udp_rx.remove_owner(session.caller_id);
    runtime.icmp_rx.remove_owner(session.caller_id);
    runtime.raw_rx.remove_owner(session.caller_id);
    for index in 0..runtime.tcp_connections.len() {
        let connection = &runtime.tcp_connections[index];
        if connection.active && connection.owner_id == session.caller_id {
            tcp::tcp_reset(runtime, index);
        }
    }
    if session.shm_local != 0 && session.shm_size != 0 {
        let _ = libnanami::request_mapping_release(session.shm_local, session.shm_size);
    }
    runtime.sessions[index] = ClientSession::EMPTY;
}

fn cleanup_stale_client_sessions(runtime: &mut NetRuntime) {
    let mut index = 0usize;
    while index < runtime.sessions.len() {
        let session = runtime.sessions[index];
        if session.active
            && matches!(
                libnanami::request_process_alive(session.caller_id),
                Ok(false)
            )
        {
            cleanup_client_session(runtime, index);
        }
        index += 1;
    }
}

fn bind_port(ports: &mut [u16; CLIENT_BIND_MAX], port: u16) -> bool {
    if ports.contains(&port) {
        return true;
    }
    let Some(slot) = ports.iter_mut().find(|bound| **bound == 0) else {
        return false;
    };
    *slot = port;
    true
}

fn unbind_port(ports: &mut [u16; CLIENT_BIND_MAX], port: u16) -> bool {
    let Some(slot) = ports.iter_mut().find(|bound| **bound == port) else {
        return false;
    };
    *slot = 0;
    true
}

fn unpack_mac(detail0: Word) -> [u8; 6] {
    [
        (detail0 & 0xff) as u8,
        ((detail0 >> 8) & 0xff) as u8,
        ((detail0 >> 16) & 0xff) as u8,
        ((detail0 >> 24) & 0xff) as u8,
        ((detail0 >> 32) & 0xff) as u8,
        ((detail0 >> 40) & 0xff) as u8,
    ]
}

fn pack_mac(mac: [u8; 6]) -> Word {
    ((mac[0] as Word) << 40)
        | ((mac[1] as Word) << 32)
        | ((mac[2] as Word) << 24)
        | ((mac[3] as Word) << 16)
        | ((mac[4] as Word) << 8)
        | mac[5] as Word
}

fn pack_ipv4(ip: [u8; 4]) -> u32 {
    ((ip[0] as u32) << 24) | ((ip[1] as u32) << 16) | ((ip[2] as u32) << 8) | ip[3] as u32
}

fn get_backend_shm_ptr(runtime: &NetRuntime, offset: Word) -> *mut u8 {
    (runtime.backend_shm_local + offset) as *mut u8
}

fn emit_frame(runtime: &NetRuntime, frame_len: usize) -> Result<Word, RequestError> {
    nanami_services::net::net_device_send(
        runtime.net_device_port,
        BACKEND_TX_OFFSET,
        frame_len as Word,
    )
}

fn next_hop_ip(runtime: &NetRuntime, destination: [u8; 4]) -> [u8; 4] {
    if destination[0..3] == runtime.ip[0..3] {
        destination
    } else {
        runtime.gateway_ip
    }
}


fn notify_client_sessions(runtime: &NetRuntime) {
    for session in runtime.sessions {
        if session.active && session.rx_notification != 0 {
            let _ = libnanami::ipc::notification_notify(session.rx_notification);
        }
    }
}


fn handle_tcp_accept_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    let Some(session) = session_for(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    };
    if request.arg0 + TCP_RX_META_LEN as Word > session.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let mut index = 0usize;
    while index < runtime.tcp_connections.len() {
        let conn = runtime.tcp_connections[index];
        if conn.active
            && conn.owner_id == request.identifier
            && !conn.accepted
            && (request.arg1 == 0 || conn.local_port == request.arg1 as u16)
            && (conn.state == TCP_STATE_ESTABLISHED || conn.state == TCP_STATE_CLOSE_WAIT)
        {
            runtime.tcp_connections[index].accepted = true;
            unsafe {
                let meta = (session.shm_local + request.arg0) as *mut u8;
                write_u32_be(
                    core::slice::from_raw_parts_mut(meta, 4),
                    pack_ipv4(conn.peer_ip),
                );
                write_u16_be(
                    core::slice::from_raw_parts_mut(meta.add(4), 2),
                    conn.peer_port,
                );
                write_u16_be(
                    core::slice::from_raw_parts_mut(meta.add(6), 2),
                    conn.local_port,
                );
                write_u32_be(
                    core::slice::from_raw_parts_mut(meta.add(8), 4),
                    conn.connection_id as u32,
                );
            }
            return (
                libnanami::OS_RESPONSE_OK,
                conn.connection_id,
                pack_ipv4(conn.peer_ip) as Word,
            );
        }
        index += 1;
    }
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

fn handle_tcp_connect_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    if session_for(runtime, request.identifier).is_none() {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    let peer_ip = [
        ((request.arg0 >> 24) & 0xff) as u8,
        ((request.arg0 >> 16) & 0xff) as u8,
        ((request.arg0 >> 8) & 0xff) as u8,
        (request.arg0 & 0xff) as u8,
    ];
    let local_port = ((request.arg1 >> 16) & 0xffff) as u16;
    let peer_port = (request.arg1 & 0xffff) as u16;
    if local_port == 0 || peer_port == 0 || peer_ip == [0; 4] {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    for conn in &runtime.tcp_connections {
        if conn.active
            && conn.owner_id == request.identifier
            && conn.local_port == local_port
            && conn.peer_port == peer_port
            && conn.peer_ip == peer_ip
        {
            let id = if conn.state == TCP_STATE_ESTABLISHED {
                conn.connection_id
            } else {
                0
            };
            return (libnanami::OS_RESPONSE_OK, id, 0);
        }
    }

    let next_hop = next_hop_ip(runtime, peer_ip);
    let Some(dst_mac) = arp_lookup(runtime, next_hop) else {
        let _ = emit_arp_request(runtime, next_hop);
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    let Some(index) = tcp::allocate_tcp_connection(runtime) else {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };
    let iss = 0x414c_5445u32.wrapping_add((index as u32) << 12);
    tcp::tcp_reset(runtime, index);
    let connection_id = runtime
        .tcp_index
        .insert(index, peer_ip, peer_port, local_port);
    runtime.tcp_connections[index] = TcpConnection {
        active: true,
        owner_id: request.identifier,
        connection_id,
        accepted: true,
        eof_pending: false,
        state: TCP_STATE_SYN_SENT,
        peer_ip,
        peer_port,
        local_port,
        snd_iss: iss,
        snd_nxt: iss.wrapping_add(1),
        snd_una: iss,
        rcv_nxt: 0,
    };
    match emit_tcp_segment(
        runtime,
        dst_mac,
        peer_ip,
        local_port,
        peer_port,
        iss,
        0,
        TCP_FLAG_SYN,
        runtime.tcp_rx.window(index),
        &[],
    ) {
        Ok(_) => {
            stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Err(error) => {
            tcp::tcp_reset(runtime, index);
            (map_request_error_to_status(error), 0, 0)
        }
    }
}

fn handle_network_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    match request.code {
        nanami_services::net::NET_SERVICE_REQUEST_SEND => {
            // Raw L2 frame send: arg0=client shm offset, arg1=len
            let Some(session) = session_for(runtime, request.identifier) else {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            };
            let n = request.arg1 as usize;
            if n == 0 || request.arg0 + request.arg1 > session.shm_size {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            unsafe {
                let src = (session.shm_local + request.arg0) as *const u8;
                let dst = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);
                ptr::copy_nonoverlapping(src, dst, n);
            }
            match emit_frame(runtime, n) {
                Ok(sent) => {
                    stats.tx_packets = stats.tx_packets.wrapping_add(1);
                    stats.tx_bytes = stats.tx_bytes.wrapping_add(sent);
                    (libnanami::OS_RESPONSE_OK, sent, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::net::NET_SERVICE_REQUEST_RECV => {
            // Raw L2 recv: arg0=client shm offset, arg1=max len
            let Some(index) = session_index(runtime, request.identifier) else {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            };
            runtime.sessions[index].raw_rx_enabled = true;
            let session = runtime.sessions[index];
            let _ = pump_backend(runtime, stats);
            let Some(entry) = runtime.raw_rx.pop_for(request.identifier) else {
                return (libnanami::OS_RESPONSE_OK, 0, 0);
            };
            let n = min(entry.len, request.arg1 as usize);
            if request.arg0 + n as Word > session.shm_size {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if n > 0 {
                unsafe {
                    let dst = (session.shm_local + request.arg0) as *mut u8;
                    ptr::copy_nonoverlapping(entry.payload.as_ptr(), dst, n);
                }
            }
            (libnanami::OS_RESPONSE_OK, n as Word, 0)
        }
        nanami_services::net::NET_SERVICE_REQUEST_CONTROL => match request.arg0 {
            nanami_services::net::NET_SERVICE_CONTROL_LINK_UP => {
                match nanami_services::net::net_device_control(
                    runtime.net_device_port,
                    nanami_services::net::NET_DEVICE_CONTROL_LINK_UP,
                    request.arg1,
                    request.arg2,
                ) {
                    Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                    Err(e) => (map_request_error_to_status(e), 0, 0),
                }
            }
            nanami_services::net::NET_SERVICE_CONTROL_LINK_DOWN => {
                match nanami_services::net::net_device_control(
                    runtime.net_device_port,
                    nanami_services::net::NET_DEVICE_CONTROL_LINK_DOWN,
                    request.arg1,
                    request.arg2,
                ) {
                    Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
                    Err(e) => (map_request_error_to_status(e), 0, 0),
                }
            }
            nanami_services::net::NET_SERVICE_CONTROL_POLL => {
                let polled = pump_backend(runtime, stats);
                (
                    libnanami::OS_RESPONSE_OK,
                    polled,
                    runtime.udp_rx.count as Word,
                )
            }
            nanami_services::net::NET_SERVICE_CONTROL_GET_IPV4_CONFIG => {
                let ip = pack_ipv4(runtime.ip);
                let gateway = pack_ipv4(runtime.gateway_ip);
                let dns = pack_ipv4(runtime.dns_ip);
                (
                    libnanami::OS_RESPONSE_OK,
                    ((ip as Word) << 32) | gateway as Word,
                    dns as Word,
                )
            }
            nanami_services::net::NET_SERVICE_CONTROL_GET_MAC => {
                (libnanami::OS_RESPONSE_OK, pack_mac(runtime.mac), 0)
            }
            nanami_services::net::NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY => {
                cleanup_stale_client_sessions(runtime);
                let peer_pid = request.arg1;
                if peer_pid == 0 {
                    return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                let size = if request.arg2 == 0 {
                    CLIENT_DEFAULT_SHM_BYTES
                } else {
                    request.arg2
                };
                match libnanami::request_shared_memory(peer_pid, size) {
                    Ok((local, peer)) => {
                        let index = session_index(runtime, request.identifier).or_else(|| {
                            runtime.sessions.iter().position(|session| !session.active)
                        });
                        let Some(index) = index else {
                            return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
                        };
                        runtime.sessions[index] = ClientSession {
                            active: true,
                            caller_id: request.identifier,
                            shm_local: local,
                            shm_size: size,
                            udp_ports: [0; CLIENT_BIND_MAX],
                            tcp_ports: [0; CLIENT_BIND_MAX],
                            icmp_identifiers: [0; CLIENT_BIND_MAX],
                            raw_rx_enabled: false,
                            rx_notification: 0,
                        };
                        libnanami::print!("[net-server] client shm attached pid=");
                        libnanami::print!("{}", peer_pid as usize);
                        libnanami::print!(" local=");
                        libnanami::print!("{:#x}", local);
                        libnanami::print!(" peer=");
                        libnanami::print!("{:#x}", peer);
                        libnanami::print!("\n");
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    Err(e) => (map_request_error_to_status(e), 0, 0),
                }
            }
            nanami_services::net::NET_SERVICE_CONTROL_ATTACH_RX_NOTIFICATION => {
                let Some(index) = session_index(runtime, request.identifier) else {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                };
                let source_slot = if request.arg1 == 0 {
                    libnanami::PROCESS_SLOT_NOTIFICATION
                } else {
                    request.arg1
                };
                let destination_slot = SLOT_CLIENT_RX_NOTIFICATION_BASE + index as Word;
                match libnanami::request_notification_port_copy(
                    request.identifier,
                    source_slot,
                    destination_slot,
                    nanami_services::net::NET_NOTIFICATION_RX,
                ) {
                    Ok(()) => {
                        runtime.sessions[index].rx_notification =
                            libnanami::ipc::process_slot_descriptor(destination_slot);
                        (libnanami::OS_RESPONSE_OK, 0, 0)
                    }
                    Err(error) => (map_request_error_to_status(error), 0, 0),
                }
            }
            nanami_services::net::NET_SERVICE_CONTROL_UDP_BIND => {
                let Some(index) = session_index(runtime, request.identifier) else {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                };
                let port = request.arg1 as u16;
                if port == 0 {
                    return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                if runtime.sessions.iter().enumerate().any(|(other, session)| {
                    other != index && session.active && session.udp_ports.contains(&port)
                }) {
                    return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
                }
                if !bind_port(&mut runtime.sessions[index].udp_ports, port) {
                    return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
                }
                libnanami::print!("[net-server] udp bind port=");
                libnanami::print!("{}", port as usize);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, port as Word, 0)
            }
            nanami_services::net::NET_SERVICE_CONTROL_TCP_BIND => {
                let Some(index) = session_index(runtime, request.identifier) else {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                };
                let port = request.arg1 as u16;
                if port == 0 {
                    return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                if runtime.sessions.iter().enumerate().any(|(other, session)| {
                    other != index && session.active && session.tcp_ports.contains(&port)
                }) {
                    return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
                }
                if !bind_port(&mut runtime.sessions[index].tcp_ports, port) {
                    return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
                }
                libnanami::print!("[net-server] tcp bind port=");
                libnanami::print!("{}", port as usize);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, port as Word, 0)
            }
            nanami_services::net::NET_SERVICE_CONTROL_UDP_UNBIND => {
                let Some(index) = session_index(runtime, request.identifier) else {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                };
                let removed =
                    unbind_port(&mut runtime.sessions[index].udp_ports, request.arg1 as u16);
                (libnanami::OS_RESPONSE_OK, removed as Word, 0)
            }
            nanami_services::net::NET_SERVICE_CONTROL_TCP_UNBIND => {
                let Some(index) = session_index(runtime, request.identifier) else {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                };
                let removed =
                    unbind_port(&mut runtime.sessions[index].tcp_ports, request.arg1 as u16);
                (libnanami::OS_RESPONSE_OK, removed as Word, 0)
            }
            _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        nanami_services::net::NET_SERVICE_REQUEST_STATS => (
            libnanami::OS_RESPONSE_OK,
            stats.rx_packets,
            stats.tx_packets,
        ),
        nanami_services::net::NET_SERVICE_REQUEST_UDP_SEND => {
            if session_for(runtime, request.identifier).is_none() {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            match emit_udp_from_session(
                runtime,
                request.identifier,
                request.arg0,
                request.arg1,
                request.arg2,
                request.arg3,
            ) {
                Ok(sent) => {
                    stats.tx_packets = stats.tx_packets.wrapping_add(1);
                    stats.tx_bytes = stats.tx_bytes.wrapping_add(sent);
                    stats.udp_tx = stats.udp_tx.wrapping_add(1);
                    (libnanami::OS_RESPONSE_OK, sent, 0)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        nanami_services::net::NET_SERVICE_REQUEST_UDP_RECV => {
            if runtime.udp_rx.count == 0 {
                let _ = pump_backend(runtime, stats);
            }
            let rsp = handle_udp_recv_request(runtime, request);
            if rsp.0 == libnanami::OS_RESPONSE_OK && rsp.1 > 0 {
                stats.udp_rx = stats.udp_rx.wrapping_add(1);
            }
            rsp
        }
        nanami_services::net::NET_SERVICE_REQUEST_ICMP_SEND => {
            match emit_icmp_echo_from_session(
                runtime,
                request.identifier,
                request.arg0,
                request.arg1,
                request.arg2 as u16,
                request.arg3 as u32,
            ) {
                Ok(sent) => {
                    stats.tx_packets = stats.tx_packets.wrapping_add(1);
                    stats.tx_bytes = stats.tx_bytes.wrapping_add(sent);
                    (libnanami::OS_RESPONSE_OK, request.arg1, 0)
                }
                Err(error) => (map_request_error_to_status(error), 0, 0),
            }
        }
        nanami_services::net::NET_SERVICE_REQUEST_ICMP_RECV => {
            if runtime.icmp_rx.count == 0 {
                let _ = pump_backend_with_budget(runtime, stats, REQUEST_PUMP_BURST);
            }
            handle_icmp_recv_request(runtime, request)
        }
        nanami_services::net::NET_SERVICE_REQUEST_TCP_RECV => {
            if runtime.tcp_rx.count == 0 {
                let _ = pump_backend_with_budget(runtime, stats, REQUEST_PUMP_BURST);
            }
            handle_tcp_recv_request(runtime, request, stats)
        }
        nanami_services::net::NET_SERVICE_REQUEST_TCP_SEND => {
            handle_tcp_send_request(runtime, request, stats)
        }
        nanami_services::net::NET_SERVICE_REQUEST_TCP_ACCEPT => {
            let _ = pump_backend_with_budget(runtime, stats, REQUEST_PUMP_BURST);
            handle_tcp_accept_request(runtime, request)
        }
        nanami_services::net::NET_SERVICE_REQUEST_TCP_CONNECT => {
            let _ = pump_backend_with_budget(runtime, stats, REQUEST_PUMP_BURST);
            handle_tcp_connect_request(runtime, request, stats)
        }
        nanami_services::net::NET_SERVICE_REQUEST_DNS_QUERY => {
            handle_dns_query_request(runtime, request, stats, DNS_QUERY_TIMEOUT_MS)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

pub(crate) fn sleep_ms(timer_port: Option<Word>, milliseconds: Word) {
    if let Some(port) = timer_port {
        let _ = nanami_services::timer::timer_service_sleep_milliseconds(port, milliseconds);
        return;
    }
    let mut remaining = milliseconds as usize;
    while remaining > 0 {
        let mut spin = 0usize;
        while spin < SPIN_LOOPS_PER_MS_FALLBACK {
            core::hint::spin_loop();
            spin += 1;
        }
        remaining -= 1;
    }
}

fn connect_timer_service() -> Option<Word> {
    let mut retry = 0usize;
    loop {
        match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE_PORT) {
            Ok(()) => {
                libnanami::print!("[net-server] timer connected\n");
                return Some(libnanami::ipc::process_slot_descriptor(
                    SLOT_TIMER_SERVICE_PORT,
                ));
            }
            Err(e) => {
                retry += 1;
                if retry >= TIMER_CONNECT_RETRIES {
                    log_request_error(
                        "[net-server] timer connect failed (continue without timer): ",
                        e,
                    );
                    return None;
                }
                core::hint::spin_loop();
            }
        }
    }
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[net-server] bootstrap start\n");

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    libnanami::ipc::bind_current_thread_notification(notification)?;

    match nanami_services::registry::register_network_service() {
        Ok(()) => libnanami::print!("[net-server] service registered: network-service\n"),
        Err(e) => {
            log_request_error("[net-server] register failed: ", e);
            return Err(e.into());
        }
    }
    let timer_port = connect_timer_service();

    libnanami::print!("[net-server] connect backend: net-device\n");
    let backend_pid = loop {
        match nanami_services::registry::connect_net_device_with_pid(SLOT_NET_DEVICE_PORT) {
            Ok(pid) => {
                libnanami::print!("[net-server] backend connected pid=");
                libnanami::print!("{:#x}", pid);
                libnanami::print!("\n");
                break pid;
            }
            Err(e) => {
                log_request_error("[net-server] backend connect failed: ", e);
                sleep_ms(timer_port, BACKOFF_MS);
            }
        }
    };

    let net_device_port = libnanami::ipc::process_slot_descriptor(SLOT_NET_DEVICE_PORT);
    let (backend_local_vaddr, backend_peer_vaddr) = loop {
        match libnanami::request_shared_memory(backend_pid, BACKEND_SHM_BYTES) {
            Ok(v) => break v,
            Err(e) => {
                log_request_error("[net-server] backend shm create failed: ", e);
                sleep_ms(timer_port, BACKOFF_MS);
            }
        }
    };

    loop {
        match nanami_services::net::net_device_control(
            net_device_port,
            nanami_services::net::NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY,
            backend_peer_vaddr,
            BACKEND_SHM_BYTES,
        ) {
            Ok(()) => {
                libnanami::print!("[net-server] backend shm attached\n");
                break;
            }
            Err(e) => {
                log_request_error("[net-server] backend shm attach failed: ", e);
                sleep_ms(timer_port, BACKOFF_MS);
            }
        }
    }

    let irq_driven = match nanami_services::net::net_device_attach_rx_notification(
        net_device_port,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    ) {
        Ok(irq_driven) => irq_driven,
        Err(error) => {
            log_request_error(
                "[net-server] backend rx notification attach failed: ",
                error,
            );
            return Err(error.into());
        }
    };

    let mac = match nanami_services::net::net_device_control_ex(
        net_device_port,
        nanami_services::net::NET_DEVICE_CONTROL_GET_MAC,
        0,
        0,
    ) {
        Ok((status, detail0, _)) if status == libnanami::OS_RESPONSE_OK => unpack_mac(detail0),
        _ => [0; 6],
    };

    loop {
        match nanami_services::net::net_device_control(
            net_device_port,
            nanami_services::net::NET_DEVICE_CONTROL_LINK_UP,
            0,
            0,
        ) {
            Ok(()) => {
                libnanami::print!("[net-server] backend link-up ok\n");
                break;
            }
            Err(e) => {
                log_request_error("[net-server] backend link-up failed: ", e);
                sleep_ms(timer_port, BACKOFF_MS);
            }
        }
    }

    let backend_rx_batch = matches!(
        nanami_services::net::net_device_control_ex(
            net_device_port,
            nanami_services::net::NET_DEVICE_CONTROL_GET_FEATURES,
            0,
            0,
        ),
        Ok((libnanami::OS_RESPONSE_OK, features, _))
            if features & nanami_services::net::NET_DEVICE_FEATURE_RECV_BATCH != 0
    );
    let mut runtime = NetRuntime {
        net_device_port,
        timer_port,
        backend_shm_local: backend_local_vaddr,
        backend_rx_batch,
        mac,
        ip: [10, 0, 2, 15],
        gateway_ip: [10, 0, 2, 2],
        dns_ip: [10, 0, 2, 3],
        arp: ArpCache::EMPTY,
        tcp_connections: [TcpConnection::EMPTY; TCP_MAX_CONNECTIONS],
        tcp_index: TcpIndex::EMPTY,
        sessions: [ClientSession::EMPTY; CLIENT_SESSION_MAX],
        udp_rx: UdpRxQueue::new(),
        icmp_rx: IcmpRxQueue::new(),
        tcp_rx: TcpRxQueue::new(unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX_BUFFERS) }),
        raw_rx: RawRxQueue::new(),
        dhcp_waiting: false,
        dhcp_xid: 0,
        dhcp_offer_valid: false,
        dhcp_ack_valid: false,
        dhcp_offer_ip: [0; 4],
        dhcp_server_ip: [0; 4],
        dhcp_router_ip: [0; 4],
        dhcp_dns_ip: [0; 4],
        dns_waiting: false,
        dns_txid: 0,
        dns_src_port: 0,
        dns_answer_valid: false,
        dns_answer_ip: [0; 4],
    };
    let mut pending_status = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;
    let mut stats = NetStats::new();
    dhcp_bootstrap(&mut runtime, &mut stats);

    if !irq_driven {
        if let Some(timer_port) = runtime.timer_port {
            nanami_services::timer::timer_service_interval_on_notification_milliseconds(
                timer_port,
                RX_MAINTENANCE_INTERVAL_MS,
                libnanami::PROCESS_SLOT_NOTIFICATION,
            )?;
            libnanami::print!("[net-server] rx fallback polling enabled\n");
        } else {
            libnanami::print!("[net-server] rx polling unavailable\n");
        }
    }

    libnanami::print!("[net-server] enter service loop\n");
    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => {
                    log_request_error("[net-server] reply_receive failed: ", e);
                    return Err(e.into());
                }
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => {
                    log_request_error("[net-server] receive failed: ", e);
                    return Err(e.into());
                }
            }
        };

        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(req) => {
                pending_status = handle_network_request(&mut runtime, req, &mut stats);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {
                let _ = pump_backend(&mut runtime, &mut stats);
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[net-server] fault id=");
                libnanami::print!("{:#x}", identifier);
                libnanami::print!(" reason=");
                libnanami::print!("{:#x}", reason);
                libnanami::print!("\n");
                has_pending_reply = false;
            }
        }
    }
}

libnanami::nanami_entry!(nanami_main);
