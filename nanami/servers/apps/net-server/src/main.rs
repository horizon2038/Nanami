#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;

use libnanami::ipc::ServiceEvent;
use libnanami::{self, RequestError, Word};
use net_wire::{checksum16, ipv4_checksum, read_u16_be, read_u32_be, write_u16_be, write_u32_be};

#[path = "app/arp.rs"]
mod arp;
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
#[path = "app/udp.rs"]
mod udp;
#[path = "app/util.rs"]
mod util;

use arp::arp_lookup;
use arp::emit_arp_request;
use dhcp::dhcp_bootstrap;
use dns::handle_dns_query_request;
use ethernet::process_ethernet_frame;
use tcp::emit_tcp_segment;
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
const BACKOFF_MS: Word = 100;
const TIMER_CONNECT_RETRIES: usize = 128;

const BACKEND_SHM_BYTES: Word = 0x20000;
const BACKEND_RX_OFFSET: Word = 0x0000;
const BACKEND_TX_OFFSET: Word = 0x1000;

const CLIENT_DEFAULT_SHM_BYTES: Word = 0x4000;
const ETH_HDR_LEN: usize = 14;
const IPV4_HDR_LEN: usize = 20;
const TCP_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;
const UDP_PAYLOAD_MAX: usize = 1472;
const UDP_RX_META_LEN: usize = 16;
const TCP_PAYLOAD_MAX: usize = 1460;
const TCP_RX_META_LEN: usize = 12;
const TCP_MAX_CONNECTIONS: usize = 128;
const TCP_RX_QUEUE_CAP: usize = 32;
const UDP_RX_QUEUE_CAP: usize = 8;
const RAW_RX_MAX_BYTES: usize = 1536;
const RAW_RX_QUEUE_CAP: usize = 4;
const TCP_LISTEN_PORT: u16 = 80;
const TCP_STATE_CLOSED: u8 = 0;
const TCP_STATE_SYN_RECEIVED: u8 = 1;
const TCP_STATE_ESTABLISHED: u8 = 2;
const TCP_STATE_FIN_WAIT1: u8 = 3;
const TCP_STATE_FIN_WAIT2: u8 = 4;
const TCP_STATE_CLOSE_WAIT: u8 = 5;
const TCP_STATE_LAST_ACK: u8 = 6;
const TCP_FLAG_FIN: u8 = 0x01;
const TCP_FLAG_SYN: u8 = 0x02;
const TCP_FLAG_RST: u8 = 0x04;
const TCP_FLAG_ACK: u8 = 0x10;
const TCP_WINDOW_DEFAULT: u16 = 0xffff;
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
struct TcpConnection {
    active: bool,
    connection_id: Word,
    state: u8,
    peer_ip: [u8; 4],
    peer_port: u16,
    local_port: u16,
    snd_iss: u32,
    snd_nxt: u32,
    snd_una: u32,
    rcv_nxt: u32,
}

impl TcpConnection {
    const EMPTY: Self = Self {
        active: false,
        connection_id: 0,
        state: TCP_STATE_CLOSED,
        peer_ip: [0; 4],
        peer_port: 0,
        local_port: 0,
        snd_iss: 0,
        snd_nxt: 0,
        snd_una: 0,
        rcv_nxt: 0,
    };
}

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    caller_id: Word,
    shm_local: Word,
    shm_size: Word,
    udp_port: u16,
    tcp_port: u16,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        caller_id: 0,
        shm_local: 0,
        shm_size: 0,
        udp_port: 0,
        tcp_port: 0,
    };
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
        while self.count > 0 {
            let idx = self.head;
            let e = self.entries[idx];
            self.entries[idx] = UdpRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
            if e.used && e.pid == pid && e.dst_port == port {
                return Some(e);
            }
        }
        self.tail = self.head;
        None
    }
}

#[derive(Clone, Copy)]
struct TcpRxEntry {
    used: bool,
    connection_id: Word,
    src_ip: [u8; 4],
    src_port: u16,
    len: usize,
    payload: [u8; TCP_PAYLOAD_MAX],
}

impl TcpRxEntry {
    const EMPTY: Self = Self {
        used: false,
        connection_id: 0,
        src_ip: [0; 4],
        src_port: 0,
        len: 0,
        payload: [0; TCP_PAYLOAD_MAX],
    };
}

#[derive(Clone, Copy)]
struct TcpRxQueue {
    entries: [TcpRxEntry; TCP_RX_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
}

impl TcpRxQueue {
    const fn new() -> Self {
        Self {
            entries: [TcpRxEntry::EMPTY; TCP_RX_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    fn push(&mut self, mut entry: TcpRxEntry) -> bool {
        if self.count == self.entries.len() {
            return false;
        }
        entry.used = true;
        self.entries[self.tail] = entry;
        self.tail = (self.tail + 1) % self.entries.len();
        self.count += 1;
        true
    }

    fn pop(&mut self) -> Option<TcpRxEntry> {
        if self.count == 0 {
            return None;
        }
        let e = self.entries[self.head];
        if !e.used {
            return None;
        }
        self.entries[self.head] = TcpRxEntry::EMPTY;
        self.head = (self.head + 1) % self.entries.len();
        self.count -= 1;
        Some(e)
    }
}

#[derive(Clone, Copy)]
struct RawRxEntry {
    used: bool,
    len: usize,
    payload: [u8; RAW_RX_MAX_BYTES],
}

impl RawRxEntry {
    const EMPTY: Self = Self {
        used: false,
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

    fn push(&mut self, frame: &[u8]) {
        if self.count == self.entries.len() {
            self.entries[self.head] = RawRxEntry::EMPTY;
            self.head = (self.head + 1) % self.entries.len();
            self.count -= 1;
        }

        let mut entry = RawRxEntry::EMPTY;
        entry.used = true;
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

    fn pop(&mut self) -> Option<RawRxEntry> {
        if self.count == 0 {
            return None;
        }
        let e = self.entries[self.head];
        if !e.used {
            return None;
        }
        self.entries[self.head] = RawRxEntry::EMPTY;
        self.head = (self.head + 1) % self.entries.len();
        self.count -= 1;
        Some(e)
    }
}

#[derive(Clone, Copy)]
struct ArpCache {
    valid: bool,
    ip: [u8; 4],
    mac: [u8; 6],
}

impl ArpCache {
    const EMPTY: Self = Self {
        valid: false,
        ip: [0; 4],
        mac: [0; 6],
    };
}

struct NetRuntime {
    net_device_port: Word,
    timer_port: Option<Word>,
    backend_shm_local: Word,
    mac: [u8; 6],
    ip: [u8; 4],
    gateway_ip: [u8; 4],
    dns_ip: [u8; 4],
    arp: ArpCache,
    tcp_connections: [TcpConnection; TCP_MAX_CONNECTIONS],
    next_tcp_connection_id: Word,
    session: ClientSession,
    udp_rx: UdpRxQueue,
    tcp_rx: TcpRxQueue,
    raw_rx: RawRxQueue,
    raw_rx_enabled: bool,
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

fn pump_backend(runtime: &mut NetRuntime, stats: &mut NetStats) -> Word {
    pump_backend_with_budget(runtime, stats, BACKEND_PUMP_DEFAULT_BURST)
}

fn pump_backend_with_budget(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    max_frames: usize,
) -> Word {
    let mut processed = 0usize;
    while processed < max_frames {
        let received = match nanami_services::net::net_device_recv(
            runtime.net_device_port,
            BACKEND_RX_OFFSET,
            1536,
        ) {
            Ok(n) => n as usize,
            Err(_) => break,
        };
        if received == 0 {
            break;
        }

        unsafe {
            let frame = core::slice::from_raw_parts(
                get_backend_shm_ptr(runtime, BACKEND_RX_OFFSET) as *const u8,
                received,
            );
            if runtime.raw_rx_enabled {
                runtime.raw_rx.push(frame);
            }
            process_ethernet_frame(runtime, stats, frame);
        }

        stats.rx_packets = stats.rx_packets.wrapping_add(1);
        stats.rx_bytes = stats.rx_bytes.wrapping_add(received as Word);
        processed += 1;
    }

    processed as Word
}

fn handle_tcp_recv_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    if !runtime.session.active || runtime.session.caller_id != request.identifier {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }

    let Some(entry) = runtime.tcp_rx.pop() else {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    };

    let meta_offset = request.arg0;
    let payload_offset = request.arg1;
    let max_len = request.arg2 as usize;
    let copy_len = min(entry.len, max_len);
    if meta_offset + TCP_RX_META_LEN as Word > runtime.session.shm_size
        || payload_offset + copy_len as Word > runtime.session.shm_size
    {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    unsafe {
        let meta = (runtime.session.shm_local + meta_offset) as *mut u8;
        let payload = (runtime.session.shm_local + payload_offset) as *mut u8;

        write_u32_be(
            core::slice::from_raw_parts_mut(meta, 4),
            ((entry.src_ip[0] as u32) << 24)
                | ((entry.src_ip[1] as u32) << 16)
                | ((entry.src_ip[2] as u32) << 8)
                | (entry.src_ip[3] as u32),
        );
        write_u16_be(
            core::slice::from_raw_parts_mut(meta.add(4), 2),
            entry.src_port,
        );
        write_u16_be(
            core::slice::from_raw_parts_mut(meta.add(6), 2),
            copy_len as u16,
        );
        write_u32_be(
            core::slice::from_raw_parts_mut(meta.add(8), 4),
            entry.connection_id as u32,
        );
        ptr::copy_nonoverlapping(entry.payload.as_ptr(), payload, copy_len);
    }

    (
        libnanami::OS_RESPONSE_OK,
        copy_len as Word,
        entry.connection_id,
    )
}

fn active_tcp_connection_index(runtime: &NetRuntime, connection_id: Word) -> Option<usize> {
    if connection_id == 0 {
        let mut index = 0usize;
        while index < runtime.tcp_connections.len() {
            if runtime.tcp_connections[index].active {
                return Some(index);
            }
            index += 1;
        }
        return None;
    }
    let index = (connection_id - 1) as usize;
    if index < runtime.tcp_connections.len()
        && runtime.tcp_connections[index].active
        && runtime.tcp_connections[index].connection_id == connection_id
    {
        return Some(index);
    }
    let mut index = 0usize;
    while index < runtime.tcp_connections.len() {
        let conn = runtime.tcp_connections[index];
        if conn.active && conn.connection_id == connection_id {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn handle_tcp_send_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    if !runtime.session.active || runtime.session.caller_id != request.identifier {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    let Some(connection_index) = active_tcp_connection_index(runtime, request.arg3) else {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };

    let payload_offset = request.arg0;
    let payload_len = request.arg1 as usize;
    let flags = (request.arg2 & 0xff) as u8;
    if payload_offset + payload_len as Word > runtime.session.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let peer_ip = runtime.tcp_connections[connection_index].peer_ip;
    let dst_mac = if let Some(mac) = arp_lookup(runtime, peer_ip) {
        mac
    } else {
        let _ = emit_arp_request(runtime, peer_ip);
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };

    let mut total_payload_sent = 0usize;
    let mut current_offset = payload_offset;
    let mut remaining = payload_len;

    if remaining == 0 {
        let conn = runtime.tcp_connections[connection_index];
        match emit_tcp_segment(
            runtime,
            dst_mac,
            conn.peer_ip,
            conn.local_port,
            conn.peer_port,
            conn.snd_nxt,
            conn.rcv_nxt,
            flags,
            &[],
        ) {
            Ok(_) => {
                if (flags & TCP_FLAG_FIN) != 0 {
                    let conn = &mut runtime.tcp_connections[connection_index];
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                    conn.state = if conn.state == TCP_STATE_CLOSE_WAIT {
                        TCP_STATE_LAST_ACK
                    } else {
                        TCP_STATE_FIN_WAIT1
                    };
                }
                stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
                return (libnanami::OS_RESPONSE_OK, 0, 0);
            }
            Err(e) => return (map_request_error_to_status(e), 0, 0),
        }
    }

    while remaining > 0 {
        let chunk = min(remaining, TCP_PAYLOAD_MAX);
        let mut seg_flags = flags;
        if remaining > chunk {
            seg_flags &= !TCP_FLAG_FIN;
        }
        let conn = runtime.tcp_connections[connection_index];

        let send_result = unsafe {
            let src = (runtime.session.shm_local + current_offset) as *const u8;
            let payload = core::slice::from_raw_parts(src, chunk);
            emit_tcp_segment(
                runtime,
                dst_mac,
                conn.peer_ip,
                conn.local_port,
                conn.peer_port,
                conn.snd_nxt,
                conn.rcv_nxt,
                seg_flags,
                payload,
            )
        };
        match send_result {
            Ok(_) => {
                let conn = &mut runtime.tcp_connections[connection_index];
                conn.snd_nxt = conn.snd_nxt.wrapping_add(chunk as u32);
                if (seg_flags & TCP_FLAG_FIN) != 0 {
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                    conn.state = if conn.state == TCP_STATE_CLOSE_WAIT {
                        TCP_STATE_LAST_ACK
                    } else {
                        TCP_STATE_FIN_WAIT1
                    };
                }
                stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
                total_payload_sent += chunk;
                current_offset += chunk as Word;
                remaining -= chunk;
            }
            Err(e) => {
                return (
                    map_request_error_to_status(e),
                    total_payload_sent as Word,
                    0,
                )
            }
        }
    }

    (libnanami::OS_RESPONSE_OK, total_payload_sent as Word, 0)
}

fn handle_network_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    match request.code {
        nanami_services::net::NET_SERVICE_REQUEST_SEND => {
            // Raw L2 frame send: arg0=client shm offset, arg1=len
            if !runtime.session.active || runtime.session.caller_id != request.identifier {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            let n = request.arg1 as usize;
            if n == 0 || request.arg0 + request.arg1 > runtime.session.shm_size {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            unsafe {
                let src = (runtime.session.shm_local + request.arg0) as *const u8;
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
            if !runtime.session.active || runtime.session.caller_id != request.identifier {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            runtime.raw_rx_enabled = true;
            let _ = pump_backend(runtime, stats);
            let Some(entry) = runtime.raw_rx.pop() else {
                return (libnanami::OS_RESPONSE_OK, 0, 0);
            };
            let n = min(entry.len, request.arg1 as usize);
            if request.arg0 + n as Word > runtime.session.shm_size {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            if n > 0 {
                unsafe {
                    let dst = (runtime.session.shm_local + request.arg0) as *mut u8;
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
                        runtime.session = ClientSession {
                            active: true,
                            caller_id: request.identifier,
                            shm_local: local,
                            shm_size: size,
                            udp_port: 0,
                            tcp_port: 0,
                        };
                        runtime.udp_rx = UdpRxQueue::new();
                        runtime.tcp_rx = TcpRxQueue::new();
                        runtime.raw_rx = RawRxQueue::new();
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
            nanami_services::net::NET_SERVICE_CONTROL_UDP_BIND => {
                if !runtime.session.active || runtime.session.caller_id != request.identifier {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                }
                let port = request.arg1 as u16;
                if port == 0 {
                    return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                runtime.session.udp_port = port;
                libnanami::print!("[net-server] udp bind port=");
                libnanami::print!("{}", port as usize);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, port as Word, 0)
            }
            nanami_services::net::NET_SERVICE_CONTROL_TCP_BIND => {
                if !runtime.session.active || runtime.session.caller_id != request.identifier {
                    return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
                }
                let port = request.arg1 as u16;
                if port == 0 {
                    return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                runtime.session.tcp_port = port;
                libnanami::print!("[net-server] tcp bind port=");
                libnanami::print!("{}", port as usize);
                libnanami::print!("\n");
                (libnanami::OS_RESPONSE_OK, port as Word, 0)
            }
            _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        nanami_services::net::NET_SERVICE_REQUEST_STATS => (
            libnanami::OS_RESPONSE_OK,
            stats.rx_packets,
            stats.tx_packets,
        ),
        nanami_services::net::NET_SERVICE_REQUEST_UDP_SEND => {
            if !runtime.session.active || runtime.session.caller_id != request.identifier {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            match emit_udp_from_session(
                runtime,
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
        nanami_services::net::NET_SERVICE_REQUEST_TCP_RECV => {
            if runtime.tcp_rx.count == 0 {
                let _ = pump_backend_with_budget(runtime, stats, REQUEST_PUMP_BURST);
            }
            handle_tcp_recv_request(runtime, request)
        }
        nanami_services::net::NET_SERVICE_REQUEST_TCP_SEND => {
            handle_tcp_send_request(runtime, request, stats)
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

    let mut runtime = NetRuntime {
        net_device_port,
        timer_port,
        backend_shm_local: backend_local_vaddr,
        mac,
        ip: [10, 0, 2, 15],
        gateway_ip: [10, 0, 2, 2],
        dns_ip: [10, 0, 2, 3],
        arp: ArpCache::EMPTY,
        tcp_connections: [TcpConnection::EMPTY; TCP_MAX_CONNECTIONS],
        next_tcp_connection_id: 1,
        session: ClientSession::EMPTY,
        udp_rx: UdpRxQueue::new(),
        tcp_rx: TcpRxQueue::new(),
        raw_rx: RawRxQueue::new(),
        raw_rx_enabled: false,
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
            ServiceEvent::Notification { .. } => {}
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
