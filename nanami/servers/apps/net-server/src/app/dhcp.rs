use super::arp::emit_gratuitous_arp_reply;
use super::*;

const DHCP_CLIENT_PORT: u16 = 68;
const DHCP_SERVER_PORT: u16 = 67;
const DHCP_MAGIC_COOKIE: u32 = 0x6382_5363;

const DHCP_OPT_SUBNET_MASK: u8 = 1;
const DHCP_OPT_ROUTER: u8 = 3;
const DHCP_OPT_DNS: u8 = 6;
const DHCP_OPT_REQ_IP: u8 = 50;
const DHCP_OPT_LEASE_TIME: u8 = 51;
const DHCP_OPT_MSG_TYPE: u8 = 53;
const DHCP_OPT_SERVER_ID: u8 = 54;
const DHCP_OPT_PARAM_REQ: u8 = 55;
const DHCP_OPT_CLIENT_ID: u8 = 61;
const DHCP_OPT_END: u8 = 255;

const DHCP_MSG_DISCOVER: u8 = 1;
const DHCP_MSG_OFFER: u8 = 2;
const DHCP_MSG_REQUEST: u8 = 3;
const DHCP_MSG_ACK: u8 = 5;
const DHCP_MSG_NAK: u8 = 6;

const DHCP_POLL_STEP_MS: Word = 200;
const DHCP_SEND_ERROR_BACKOFF_MS: Word = 250;
const DHCP_RETX_BACKOFF_INITIAL_MS: Word = 4000;
const DHCP_RETX_BACKOFF_MAX_MS: Word = 64000;
const DHCP_MAX_RETRIES: usize = 6;
const DHCP_PUMP_BURST: usize = 8;

#[derive(Clone, Copy)]
struct DhcpParsed {
    msg_type: u8,
    yiaddr: [u8; 4],
    server_ip: [u8; 4],
    router_ip: [u8; 4],
    dns_ip: [u8; 4],
}

impl DhcpParsed {
    const EMPTY: Self = Self {
        msg_type: 0,
        yiaddr: [0; 4],
        server_ip: [0; 4],
        router_ip: [0; 4],
        dns_ip: [0; 4],
    };
}

pub(crate) fn process_dhcp_payload(
    runtime: &mut NetRuntime,
    src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> bool {
    if src_port != DHCP_SERVER_PORT || dst_port != DHCP_CLIENT_PORT {
        return false;
    }
    if !runtime.dhcp_waiting {
        return true;
    }

    let Some(parsed) = parse_dhcp_payload(payload, runtime.dhcp_xid) else {
        if let Some((xid, msg_type)) = inspect_dhcp_packet(payload) {
            libnanami::print!("[net-server][dhcp] ignore packet type=");
            libnanami::print!("{}", msg_type as usize);
            libnanami::print!(" xid=0x");
            libnanami::print!("{:#x}", xid as usize);
            libnanami::print!(" expected=0x");
            libnanami::print!("{:#x}", runtime.dhcp_xid as usize);
            libnanami::print!("\n");
        }
        return true;
    };

    if parsed.msg_type == DHCP_MSG_OFFER {
        runtime.dhcp_offer_valid = true;
        runtime.dhcp_offer_ip = parsed.yiaddr;
        if parsed.server_ip != [0; 4] {
            runtime.dhcp_server_ip = parsed.server_ip;
        } else if src_ip != [0; 4] && src_ip != [255; 4] {
            runtime.dhcp_server_ip = src_ip;
        }
        if parsed.router_ip != [0; 4] {
            runtime.dhcp_router_ip = parsed.router_ip;
        }
        if parsed.dns_ip != [0; 4] {
            runtime.dhcp_dns_ip = parsed.dns_ip;
        }
        libnanami::print!("[net-server][dhcp] offer received ip=");
        log_ip(runtime.dhcp_offer_ip);
        libnanami::print!(" server=");
        log_ip(runtime.dhcp_server_ip);
        libnanami::print!(" gw=");
        log_ip(runtime.dhcp_router_ip);
        libnanami::print!(" dns=");
        log_ip(runtime.dhcp_dns_ip);
        libnanami::print!("\n");
        return true;
    }

    if parsed.msg_type == DHCP_MSG_ACK {
        runtime.dhcp_ack_valid = true;
        runtime.dhcp_offer_ip = parsed.yiaddr;
        if parsed.server_ip != [0; 4] {
            runtime.dhcp_server_ip = parsed.server_ip;
        } else if src_ip != [0; 4] && src_ip != [255; 4] {
            runtime.dhcp_server_ip = src_ip;
        }
        if parsed.router_ip != [0; 4] {
            runtime.dhcp_router_ip = parsed.router_ip;
        }
        if parsed.dns_ip != [0; 4] {
            runtime.dhcp_dns_ip = parsed.dns_ip;
        }
        libnanami::print!("[net-server][dhcp] ack received ip=");
        log_ip(runtime.dhcp_offer_ip);
        libnanami::print!("\n");
        return true;
    }

    if parsed.msg_type == DHCP_MSG_NAK {
        runtime.dhcp_offer_valid = false;
        runtime.dhcp_ack_valid = false;
        libnanami::print!("[net-server][dhcp] nak received\n");
        return true;
    }

    true
}

fn inspect_dhcp_packet(payload: &[u8]) -> Option<(u32, u8)> {
    if payload.len() < 240 {
        return None;
    }
    let xid = read_u32_be(&payload[4..8]);
    let mut msg_type = 0u8;
    let mut i = 240usize;
    while i < payload.len() {
        let code = payload[i];
        i += 1;
        if code == 0 {
            continue;
        }
        if code == DHCP_OPT_END {
            break;
        }
        if i >= payload.len() {
            break;
        }
        let len = payload[i] as usize;
        i += 1;
        if i + len > payload.len() {
            break;
        }
        if code == DHCP_OPT_MSG_TYPE && len >= 1 {
            msg_type = payload[i];
            break;
        }
        i += len;
    }
    Some((xid, msg_type))
}

pub(crate) fn dhcp_bootstrap(runtime: &mut NetRuntime, stats: &mut NetStats) {
    runtime.dhcp_waiting = true;
    runtime.dhcp_offer_valid = false;
    runtime.dhcp_ack_valid = false;
    runtime.dhcp_offer_ip = [0; 4];
    runtime.dhcp_server_ip = [0; 4];
    runtime.dhcp_router_ip = [0; 4];
    runtime.dhcp_dns_ip = [0; 4];
    runtime.dhcp_xid = make_dhcp_xid(runtime.mac);

    libnanami::print!("[net-server][dhcp] bootstrap start xid=0x");
    libnanami::print!("{:#x}", runtime.dhcp_xid as usize);
    libnanami::print!("\n");

    'discover_phase: loop {
        runtime.dhcp_offer_valid = false;
        runtime.dhcp_ack_valid = false;
        runtime.dhcp_server_ip = [0; 4];
        runtime.dhcp_router_ip = [0; 4];
        runtime.dhcp_dns_ip = [0; 4];

        let mut retry = 0usize;
        loop {
            libnanami::print!("[net-server][dhcp] discover");
            if retry > 0 {
                libnanami::print!(" retry=");
                libnanami::print!("{}", retry);
            }
            libnanami::print!("\n");

            let wait_ms = dhcp_backoff_ms(retry);
            if let Err(e) = send_dhcp_discover(runtime) {
                log_request_error("[net-server][dhcp] discover send failed: ", e);
                super::sleep_ms(runtime.timer_port, DHCP_SEND_ERROR_BACKOFF_MS);
            } else if wait_dhcp_offer(runtime, stats, wait_ms) && runtime.dhcp_offer_valid {
                break;
            }

            retry = retry.saturating_add(1);
            if retry >= DHCP_MAX_RETRIES {
                libnanami::print!("[net-server][dhcp] offer timeout, restart discover cycle\n");
                runtime.dhcp_xid = runtime.dhcp_xid.wrapping_add(0x0101_0101);
                continue 'discover_phase;
            }
            libnanami::print!("[net-server][dhcp] offer timeout, retry=");
            libnanami::print!("{}", retry);
            libnanami::print!(" next_wait_ms=");
            libnanami::print!("{}", dhcp_backoff_ms(retry));
            libnanami::print!("\n");
        }

        let mut retry = 0usize;
        loop {
            runtime.dhcp_ack_valid = false;
            libnanami::print!("[net-server][dhcp] request");
            if retry > 0 {
                libnanami::print!(" retry=");
                libnanami::print!("{}", retry);
            }
            libnanami::print!("\n");

            let wait_ms = dhcp_backoff_ms(retry);
            if let Err(e) = send_dhcp_request(runtime) {
                log_request_error("[net-server][dhcp] request send failed: ", e);
                super::sleep_ms(runtime.timer_port, DHCP_SEND_ERROR_BACKOFF_MS);
            } else if wait_dhcp_ack(runtime, stats, wait_ms) && runtime.dhcp_ack_valid {
                apply_lease(runtime);
                runtime.dhcp_waiting = false;
                return;
            }

            if !runtime.dhcp_offer_valid {
                libnanami::print!("[net-server][dhcp] request invalidated; restart discover\n");
                runtime.dhcp_xid = runtime.dhcp_xid.wrapping_add(0x1111_1111);
                continue 'discover_phase;
            }

            retry = retry.saturating_add(1);
            if retry >= DHCP_MAX_RETRIES {
                libnanami::print!("[net-server][dhcp] ack timeout, restart discover cycle\n");
                runtime.dhcp_xid = runtime.dhcp_xid.wrapping_add(0x0011_0011);
                continue 'discover_phase;
            }
            libnanami::print!("[net-server][dhcp] ack timeout, retry=");
            libnanami::print!("{}", retry);
            libnanami::print!(" next_wait_ms=");
            libnanami::print!("{}", dhcp_backoff_ms(retry));
            libnanami::print!("\n");
        }
    }
}

fn wait_dhcp_offer(runtime: &mut NetRuntime, stats: &mut NetStats, timeout_ms: Word) -> bool {
    let mut elapsed = 0usize;
    while elapsed < timeout_ms as usize {
        let _ = super::pump_backend_with_budget(runtime, stats, DHCP_PUMP_BURST);
        if runtime.dhcp_offer_valid {
            return true;
        }
        super::sleep_ms(runtime.timer_port, DHCP_POLL_STEP_MS);
        elapsed = elapsed.saturating_add(DHCP_POLL_STEP_MS as usize);
    }
    false
}

fn wait_dhcp_ack(runtime: &mut NetRuntime, stats: &mut NetStats, timeout_ms: Word) -> bool {
    let mut elapsed = 0usize;
    while elapsed < timeout_ms as usize {
        let _ = super::pump_backend_with_budget(runtime, stats, DHCP_PUMP_BURST);
        if runtime.dhcp_ack_valid {
            return true;
        }
        if !runtime.dhcp_offer_valid {
            return false;
        }
        super::sleep_ms(runtime.timer_port, DHCP_POLL_STEP_MS);
        elapsed = elapsed.saturating_add(DHCP_POLL_STEP_MS as usize);
    }
    false
}

fn dhcp_backoff_ms(retry: usize) -> Word {
    if retry == 0 {
        return DHCP_RETX_BACKOFF_INITIAL_MS;
    }
    let shift = core::cmp::min(retry, 4);
    let mut wait_ms = DHCP_RETX_BACKOFF_INITIAL_MS << shift;
    if wait_ms > DHCP_RETX_BACKOFF_MAX_MS {
        wait_ms = DHCP_RETX_BACKOFF_MAX_MS;
    }
    wait_ms
}

fn apply_lease(runtime: &mut NetRuntime) {
    runtime.ip = runtime.dhcp_offer_ip;
    if runtime.dhcp_router_ip != [0; 4] {
        runtime.gateway_ip = runtime.dhcp_router_ip;
    }
    if runtime.dhcp_dns_ip != [0; 4] {
        runtime.dns_ip = runtime.dhcp_dns_ip;
    }

    libnanami::print!("[net-server][dhcp] lease ip=");
    log_ip(runtime.ip);
    libnanami::print!(" gw=");
    log_ip(runtime.gateway_ip);
    libnanami::print!(" dns=");
    log_ip(runtime.dns_ip);
    libnanami::print!("\n");
    announce_lease(runtime);
}

fn make_dhcp_xid(mac: [u8; 6]) -> u32 {
    0x4e65_7453
        ^ ((mac[0] as u32) << 24)
        ^ ((mac[1] as u32) << 16)
        ^ ((mac[4] as u32) << 8)
        ^ (mac[5] as u32)
}

fn announce_lease(runtime: &mut NetRuntime) {
    let mut i = 0usize;
    while i < 3 {
        let _ = emit_gratuitous_arp_reply(runtime);
        let _ = emit_arp_request(runtime, runtime.ip);
        super::sleep_ms(runtime.timer_port, 50);
        i += 1;
    }
}

fn send_dhcp_discover(runtime: &NetRuntime) -> Result<(), RequestError> {
    let mut payload = [0u8; 300];
    let mut p = 0usize;

    payload[p] = 1;
    payload[p + 1] = 1;
    payload[p + 2] = 6;
    payload[p + 3] = 0;
    write_u32_be(&mut payload[p + 4..p + 8], runtime.dhcp_xid);
    write_u16_be(&mut payload[p + 8..p + 10], 0);
    write_u16_be(&mut payload[p + 10..p + 12], 0x8000);
    payload[p + 28..p + 34].copy_from_slice(&runtime.mac);
    p = 236;
    write_u32_be(&mut payload[p..p + 4], DHCP_MAGIC_COOKIE);
    p += 4;

    payload[p] = DHCP_OPT_MSG_TYPE;
    payload[p + 1] = 1;
    payload[p + 2] = DHCP_MSG_DISCOVER;
    p += 3;

    payload[p] = DHCP_OPT_CLIENT_ID;
    payload[p + 1] = 7;
    payload[p + 2] = 1;
    payload[p + 3..p + 9].copy_from_slice(&runtime.mac);
    p += 9;

    payload[p] = DHCP_OPT_PARAM_REQ;
    payload[p + 1] = 4;
    payload[p + 2] = DHCP_OPT_SUBNET_MASK;
    payload[p + 3] = DHCP_OPT_ROUTER;
    payload[p + 4] = DHCP_OPT_DNS;
    payload[p + 5] = DHCP_OPT_LEASE_TIME;
    p += 6;

    payload[p] = DHCP_OPT_END;
    p += 1;

    send_dhcp_udp(
        runtime,
        [0xff; 6],
        [0, 0, 0, 0],
        [255, 255, 255, 255],
        DHCP_CLIENT_PORT,
        DHCP_SERVER_PORT,
        &payload[..p],
    )
}

fn send_dhcp_request(runtime: &NetRuntime) -> Result<(), RequestError> {
    let mut payload = [0u8; 300];
    let mut p = 0usize;

    payload[p] = 1;
    payload[p + 1] = 1;
    payload[p + 2] = 6;
    payload[p + 3] = 0;
    write_u32_be(&mut payload[p + 4..p + 8], runtime.dhcp_xid);
    write_u16_be(&mut payload[p + 8..p + 10], 0);
    write_u16_be(&mut payload[p + 10..p + 12], 0x8000);
    payload[p + 28..p + 34].copy_from_slice(&runtime.mac);
    p = 236;
    write_u32_be(&mut payload[p..p + 4], DHCP_MAGIC_COOKIE);
    p += 4;

    payload[p] = DHCP_OPT_MSG_TYPE;
    payload[p + 1] = 1;
    payload[p + 2] = DHCP_MSG_REQUEST;
    p += 3;

    payload[p] = DHCP_OPT_REQ_IP;
    payload[p + 1] = 4;
    payload[p + 2..p + 6].copy_from_slice(&runtime.dhcp_offer_ip);
    p += 6;

    if runtime.dhcp_server_ip != [0; 4] {
        payload[p] = DHCP_OPT_SERVER_ID;
        payload[p + 1] = 4;
        payload[p + 2..p + 6].copy_from_slice(&runtime.dhcp_server_ip);
        p += 6;
    }

    payload[p] = DHCP_OPT_CLIENT_ID;
    payload[p + 1] = 7;
    payload[p + 2] = 1;
    payload[p + 3..p + 9].copy_from_slice(&runtime.mac);
    p += 9;

    payload[p] = DHCP_OPT_PARAM_REQ;
    payload[p + 1] = 4;
    payload[p + 2] = DHCP_OPT_SUBNET_MASK;
    payload[p + 3] = DHCP_OPT_ROUTER;
    payload[p + 4] = DHCP_OPT_DNS;
    payload[p + 5] = DHCP_OPT_LEASE_TIME;
    p += 6;

    payload[p] = DHCP_OPT_END;
    p += 1;

    send_dhcp_udp(
        runtime,
        [0xff; 6],
        [0, 0, 0, 0],
        [255, 255, 255, 255],
        DHCP_CLIENT_PORT,
        DHCP_SERVER_PORT,
        &payload[..p],
    )
}

fn send_dhcp_udp(
    runtime: &NetRuntime,
    dst_mac: [u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Result<(), RequestError> {
    let frame_len = ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN + payload.len();
    let tx_ptr = super::get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);

    unsafe {
        let frame = core::slice::from_raw_parts_mut(tx_ptr, frame_len);
        frame[0..6].copy_from_slice(&dst_mac);
        frame[6..12].copy_from_slice(&runtime.mac);
        frame[12..14].copy_from_slice(&[0x08, 0x00]);

        let ip = &mut frame[ETH_HDR_LEN..ETH_HDR_LEN + IPV4_HDR_LEN];
        ip[0] = 0x45;
        ip[1] = 0;
        write_u16_be(
            &mut ip[2..4],
            (IPV4_HDR_LEN + UDP_HDR_LEN + payload.len()) as u16,
        );
        write_u16_be(&mut ip[4..6], 0);
        write_u16_be(&mut ip[6..8], 0x4000);
        ip[8] = 64;
        ip[9] = 17;
        ip[10] = 0;
        ip[11] = 0;
        ip[12..16].copy_from_slice(&src_ip);
        ip[16..20].copy_from_slice(&dst_ip);
        let ip_sum = ipv4_checksum(ip);
        write_u16_be(&mut ip[10..12], ip_sum);

        let udp = &mut frame[ETH_HDR_LEN + IPV4_HDR_LEN..ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN];
        write_u16_be(&mut udp[0..2], src_port);
        write_u16_be(&mut udp[2..4], dst_port);
        write_u16_be(&mut udp[4..6], (UDP_HDR_LEN + payload.len()) as u16);
        write_u16_be(&mut udp[6..8], 0);

        let dst = frame
            .as_mut_ptr()
            .add(ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN);
        ptr::copy_nonoverlapping(payload.as_ptr(), dst, payload.len());
    }

    let _ = super::emit_frame(runtime, frame_len)?;
    Ok(())
}

fn parse_dhcp_payload(payload: &[u8], expected_xid: u32) -> Option<DhcpParsed> {
    if payload.len() < 240 {
        return None;
    }
    if payload[0] != 2 || payload[1] != 1 || payload[2] != 6 {
        return None;
    }
    let xid = read_u32_be(&payload[4..8]);
    if xid != expected_xid {
        return None;
    }
    let cookie = read_u32_be(&payload[236..240]);
    if cookie != DHCP_MAGIC_COOKIE {
        return None;
    }

    let mut out = DhcpParsed::EMPTY;
    out.yiaddr.copy_from_slice(&payload[16..20]);

    let mut i = 240usize;
    while i < payload.len() {
        let code = payload[i];
        i += 1;
        if code == 0 {
            continue;
        }
        if code == DHCP_OPT_END {
            break;
        }
        if i >= payload.len() {
            break;
        }
        let len = payload[i] as usize;
        i += 1;
        if i + len > payload.len() {
            break;
        }
        let data = &payload[i..i + len];
        match code {
            DHCP_OPT_MSG_TYPE if len >= 1 => out.msg_type = data[0],
            DHCP_OPT_SERVER_ID if len >= 4 => out.server_ip.copy_from_slice(&data[0..4]),
            DHCP_OPT_ROUTER if len >= 4 => out.router_ip.copy_from_slice(&data[0..4]),
            DHCP_OPT_DNS if len >= 4 => out.dns_ip.copy_from_slice(&data[0..4]),
            _ => {}
        }
        i += len;
    }

    if out.msg_type == 0 {
        return None;
    }
    Some(out)
}

fn log_ip(ip: [u8; 4]) {
    libnanami::print!("{}", ip[0] as usize);
    libnanami::print!(".");
    libnanami::print!("{}", ip[1] as usize);
    libnanami::print!(".");
    libnanami::print!("{}", ip[2] as usize);
    libnanami::print!(".");
    libnanami::print!("{}", ip[3] as usize);
}
