use super::arp::{arp_lookup, emit_arp_request};
use super::*;

pub(crate) fn emit_udp_from_session(
    runtime: &mut NetRuntime,
    payload_offset: Word,
    payload_len: Word,
    ports: Word,
    dst_ip_be: Word,
) -> Result<Word, RequestError> {
    if !runtime.session.active || runtime.session.shm_local == 0 {
        return Err(RequestError::Status(
            libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        ));
    }
    let payload_len = min(payload_len as usize, UDP_PAYLOAD_MAX);
    if payload_len == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if payload_offset + payload_len as Word > runtime.session.shm_size {
        return Err(RequestError::InvalidArgument);
    }

    let src_port = ((ports >> 16) & 0xffff) as u16;
    let dst_port = (ports & 0xffff) as u16;
    let dst_ip = [
        ((dst_ip_be >> 24) & 0xff) as u8,
        ((dst_ip_be >> 16) & 0xff) as u8,
        ((dst_ip_be >> 8) & 0xff) as u8,
        (dst_ip_be & 0xff) as u8,
    ];

    let dst_mac = if let Some(mac) = arp_lookup(runtime, dst_ip) {
        mac
    } else {
        let _ = emit_arp_request(runtime, dst_ip);
        return Err(RequestError::Status(
            libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        ));
    };

    let frame_len = ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN + payload_len;
    let tx_ptr = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);
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
            (IPV4_HDR_LEN + UDP_HDR_LEN + payload_len) as u16,
        );
        write_u16_be(&mut ip[4..6], 0);
        write_u16_be(&mut ip[6..8], 0x4000);
        ip[8] = 64;
        ip[9] = 17;
        ip[10] = 0;
        ip[11] = 0;
        ip[12..16].copy_from_slice(&runtime.ip);
        ip[16..20].copy_from_slice(&dst_ip);
        let csum = ipv4_checksum(ip);
        write_u16_be(&mut ip[10..12], csum);

        let udp_base = ETH_HDR_LEN + IPV4_HDR_LEN;
        let udp = &mut frame[udp_base..udp_base + UDP_HDR_LEN];
        write_u16_be(&mut udp[0..2], src_port);
        write_u16_be(&mut udp[2..4], dst_port);
        write_u16_be(&mut udp[4..6], (UDP_HDR_LEN + payload_len) as u16);
        write_u16_be(&mut udp[6..8], 0);

        let src = (runtime.session.shm_local + payload_offset) as *const u8;
        let dst = frame
            .as_mut_ptr()
            .add(ETH_HDR_LEN + IPV4_HDR_LEN + UDP_HDR_LEN);
        ptr::copy_nonoverlapping(src, dst, payload_len);
    }

    let sent = emit_frame(runtime, frame_len)?;
    Ok(sent)
}

pub(crate) fn try_queue_udp(
    runtime: &mut NetRuntime,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) {
    if !runtime.session.active || runtime.session.udp_port == 0 {
        return;
    }
    if runtime.session.udp_port != dst_port {
        return;
    }

    let mut entry = UdpRxEntry::EMPTY;
    entry.pid = runtime.session.caller_id;
    entry.src_ip = src_ip;
    entry.dst_ip = dst_ip;
    entry.src_port = src_port;
    entry.dst_port = dst_port;
    entry.len = min(payload.len(), UDP_PAYLOAD_MAX);
    let mut i = 0usize;
    while i < entry.len {
        entry.payload[i] = payload[i];
        i += 1;
    }
    runtime.udp_rx.push(entry);
}

pub(crate) fn handle_udp_recv_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    if !runtime.session.active || runtime.session.caller_id != request.identifier {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }

    let meta_offset = request.arg0;
    let payload_offset = request.arg1;
    let max_len = request.arg2 as usize;

    let Some(entry) = runtime
        .udp_rx
        .pop_for(runtime.session.caller_id, runtime.session.udp_port)
    else {
        return (libnanami::OS_RESPONSE_OK, 0, 0);
    };

    let copy_len = min(entry.len, max_len);
    if meta_offset + UDP_RX_META_LEN as Word > runtime.session.shm_size
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
            entry.dst_port,
        );
        write_u32_be(
            core::slice::from_raw_parts_mut(meta.add(8), 4),
            ((entry.dst_ip[0] as u32) << 24)
                | ((entry.dst_ip[1] as u32) << 16)
                | ((entry.dst_ip[2] as u32) << 8)
                | (entry.dst_ip[3] as u32),
        );
        write_u16_be(
            core::slice::from_raw_parts_mut(meta.add(12), 2),
            copy_len as u16,
        );
        write_u16_be(core::slice::from_raw_parts_mut(meta.add(14), 2), 0);

        ptr::copy_nonoverlapping(entry.payload.as_ptr(), payload, copy_len);
    }

    (libnanami::OS_RESPONSE_OK, copy_len as Word, 0)
}
