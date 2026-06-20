use super::*;

const DNS_SERVER_PORT: u16 = 53;
const DNS_DEFAULT_CLIENT_PORT: u16 = 53053;

pub(crate) fn handle_dns_query_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
    default_timeout_ms: Word,
) -> (Word, Word, Word) {
    if !runtime.session.active || runtime.session.caller_id != request.identifier {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }

    let name_offset = request.arg0;
    let name_len = request.arg1 as usize;
    let out_offset = request.arg2;
    let timeout_ms = if request.arg3 == 0 {
        default_timeout_ms
    } else {
        request.arg3
    };

    if name_len == 0 || name_len > 253 {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if name_offset + name_len as Word > runtime.session.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if out_offset + 4 > runtime.session.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let mut host = [0u8; 253];
    unsafe {
        let src = (runtime.session.shm_local + name_offset) as *const u8;
        ptr::copy_nonoverlapping(src, host.as_mut_ptr(), name_len);
    }

    let resolved = match dns_query_ipv4(runtime, stats, &host[..name_len], timeout_ms) {
        Ok(ip) => ip,
        Err(e) => return (map_request_error_to_status(e), 0, 0),
    };

    let be = ((resolved[0] as Word) << 24)
        | ((resolved[1] as Word) << 16)
        | ((resolved[2] as Word) << 8)
        | (resolved[3] as Word);

    unsafe {
        let out = (runtime.session.shm_local + out_offset) as *mut u8;
        let dst = core::slice::from_raw_parts_mut(out, 4);
        write_u32_be(dst, be as u32);
    }

    (libnanami::OS_RESPONSE_OK, be, 0)
}

pub(crate) fn process_dns_payload(
    runtime: &mut NetRuntime,
    _src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> bool {
    if src_port != DNS_SERVER_PORT {
        return false;
    }
    if !runtime.dns_waiting || dst_port != runtime.dns_src_port {
        return true;
    }

    let Some((txid, answer_ip)) = parse_dns_response(payload) else {
        return true;
    };
    if txid != runtime.dns_txid {
        return true;
    }

    runtime.dns_answer_valid = true;
    runtime.dns_answer_ip = answer_ip;
    runtime.dns_waiting = false;
    true
}

fn dns_query_ipv4(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    host: &[u8],
    timeout_ms: Word,
) -> Result<[u8; 4], RequestError> {
    let server_ip = runtime.dns_ip;
    let dst_mac = match ensure_arp(runtime, stats, server_ip) {
        Some(v) => v,
        None => {
            return Err(RequestError::Status(
                libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
            ))
        }
    };

    runtime.dns_txid = runtime.dns_txid.wrapping_add(1);
    if runtime.dns_txid == 0 {
        runtime.dns_txid = 1;
    }
    runtime.dns_src_port = DNS_DEFAULT_CLIENT_PORT;
    runtime.dns_waiting = true;
    runtime.dns_answer_valid = false;

    send_dns_query(
        runtime,
        dst_mac,
        server_ip,
        runtime.dns_src_port,
        runtime.dns_txid,
        host,
    )?;

    let mut elapsed = 0usize;
    while elapsed < timeout_ms as usize {
        let _ = super::pump_backend(runtime, stats);
        if runtime.dns_answer_valid {
            runtime.dns_answer_valid = false;
            runtime.dns_waiting = false;
            return Ok(runtime.dns_answer_ip);
        }
        super::sleep_ms(runtime.timer_port, 20);
        elapsed += 20;
    }

    runtime.dns_waiting = false;
    Err(RequestError::Status(
        libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
    ))
}

fn ensure_arp(runtime: &mut NetRuntime, stats: &mut NetStats, ip: [u8; 4]) -> Option<[u8; 6]> {
    if let Some(mac) = arp_lookup(runtime, ip) {
        return Some(mac);
    }
    let _ = emit_arp_request(runtime, ip);
    let mut i = 0usize;
    while i < 10 {
        let _ = super::pump_backend(runtime, stats);
        if let Some(mac) = arp_lookup(runtime, ip) {
            return Some(mac);
        }
        super::sleep_ms(runtime.timer_port, 20);
        i += 1;
    }
    None
}

fn send_dns_query(
    runtime: &NetRuntime,
    dst_mac: [u8; 6],
    dst_ip: [u8; 4],
    src_port: u16,
    txid: u16,
    host: &[u8],
) -> Result<(), RequestError> {
    let mut qname = [0u8; 256];
    let qname_len = encode_dns_name(host, &mut qname)?;

    let dns_len = 12 + qname_len + 4;
    let frame_len = ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN + dns_len;
    let tx_ptr = super::get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);

    unsafe {
        let frame = core::slice::from_raw_parts_mut(tx_ptr, frame_len);
        frame[0..6].copy_from_slice(&dst_mac);
        frame[6..12].copy_from_slice(&runtime.mac);
        frame[12..14].copy_from_slice(&[0x08, 0x00]);

        let ip = &mut frame[ETH_HDR_LEN..ETH_HDR_LEN + IPV4_HDR_LEN];
        ip[0] = 0x45;
        ip[1] = 0;
        write_u16_be(&mut ip[2..4], (IPV4_HDR_LEN + UDP_HDR_LEN + dns_len) as u16);
        write_u16_be(&mut ip[4..6], 0);
        write_u16_be(&mut ip[6..8], 0x4000);
        ip[8] = 64;
        ip[9] = 17;
        ip[10] = 0;
        ip[11] = 0;
        ip[12..16].copy_from_slice(&runtime.ip);
        ip[16..20].copy_from_slice(&dst_ip);
        let ip_sum = ipv4_checksum(ip);
        write_u16_be(&mut ip[10..12], ip_sum);

        let udp = &mut frame[ETH_HDR_LEN + IPV4_HDR_LEN..ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN];
        write_u16_be(&mut udp[0..2], src_port);
        write_u16_be(&mut udp[2..4], DNS_SERVER_PORT);
        write_u16_be(&mut udp[4..6], (UDP_HDR_LEN + dns_len) as u16);
        write_u16_be(&mut udp[6..8], 0);

        let dns = &mut frame[ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN..];
        write_u16_be(&mut dns[0..2], txid);
        write_u16_be(&mut dns[2..4], 0x0100); // RD
        write_u16_be(&mut dns[4..6], 1); // QDCOUNT
        write_u16_be(&mut dns[6..8], 0);
        write_u16_be(&mut dns[8..10], 0);
        write_u16_be(&mut dns[10..12], 0);

        dns[12..12 + qname_len].copy_from_slice(&qname[..qname_len]);
        let qtail = 12 + qname_len;
        write_u16_be(&mut dns[qtail..qtail + 2], 1); // A
        write_u16_be(&mut dns[qtail + 2..qtail + 4], 1); // IN
    }

    let _ = super::emit_frame(runtime, frame_len)?;
    Ok(())
}

fn encode_dns_name(host: &[u8], out: &mut [u8]) -> Result<usize, RequestError> {
    let mut pos = 0usize;
    let mut label_start = 0usize;
    while label_start < host.len() {
        let mut label_end = label_start;
        while label_end < host.len() && host[label_end] != b'.' {
            label_end += 1;
        }
        let len = label_end - label_start;
        if len == 0 || len > 63 {
            return Err(RequestError::InvalidArgument);
        }
        if pos + 1 + len >= out.len() {
            return Err(RequestError::InvalidArgument);
        }
        out[pos] = len as u8;
        pos += 1;
        out[pos..pos + len].copy_from_slice(&host[label_start..label_end]);
        pos += len;
        label_start = label_end + 1;
    }
    if pos >= out.len() {
        return Err(RequestError::InvalidArgument);
    }
    out[pos] = 0;
    pos += 1;
    Ok(pos)
}

fn parse_dns_response(payload: &[u8]) -> Option<(u16, [u8; 4])> {
    if payload.len() < 12 {
        return None;
    }
    let txid = read_u16_be(&payload[0..2]);
    let flags = read_u16_be(&payload[2..4]);
    if (flags & 0x8000) == 0 {
        return None;
    }
    if (flags & 0x000f) != 0 {
        return None;
    }

    let qd = read_u16_be(&payload[4..6]) as usize;
    let an = read_u16_be(&payload[6..8]) as usize;
    let mut off = 12usize;

    for _ in 0..qd {
        off = skip_dns_name(payload, off)?;
        if off + 4 > payload.len() {
            return None;
        }
        off += 4;
    }

    for _ in 0..an {
        off = skip_dns_name(payload, off)?;
        if off + 10 > payload.len() {
            return None;
        }
        let typ = read_u16_be(&payload[off..off + 2]);
        let class = read_u16_be(&payload[off + 2..off + 4]);
        let rdlen = read_u16_be(&payload[off + 8..off + 10]) as usize;
        off += 10;
        if off + rdlen > payload.len() {
            return None;
        }
        if typ == 1 && class == 1 && rdlen == 4 {
            return Some((
                txid,
                [
                    payload[off],
                    payload[off + 1],
                    payload[off + 2],
                    payload[off + 3],
                ],
            ));
        }
        off += rdlen;
    }

    None
}

fn skip_dns_name(payload: &[u8], mut off: usize) -> Option<usize> {
    let mut jumps = 0usize;
    loop {
        if off >= payload.len() {
            return None;
        }
        let len = payload[off];
        if (len & 0xc0) == 0xc0 {
            if off + 1 >= payload.len() {
                return None;
            }
            off += 2;
            return Some(off);
        }
        if len == 0 {
            off += 1;
            return Some(off);
        }
        let step = len as usize;
        off += 1;
        if off + step > payload.len() {
            return None;
        }
        off += step;
        jumps += 1;
        if jumps > 128 {
            return None;
        }
    }
}
