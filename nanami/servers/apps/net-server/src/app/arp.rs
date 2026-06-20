use super::*;

pub(crate) fn emit_arp_request(
    runtime: &NetRuntime,
    target_ip: [u8; 4],
) -> Result<(), RequestError> {
    let frame = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);
    unsafe {
        let f = core::slice::from_raw_parts_mut(frame, 42);
        f[0..6].copy_from_slice(&[0xff; 6]);
        f[6..12].copy_from_slice(&runtime.mac);
        f[12..14].copy_from_slice(&[0x08, 0x06]);
        f[14..16].copy_from_slice(&[0x00, 0x01]);
        f[16..18].copy_from_slice(&[0x08, 0x00]);
        f[18] = 6;
        f[19] = 4;
        f[20..22].copy_from_slice(&[0x00, 0x01]);
        f[22..28].copy_from_slice(&runtime.mac);
        f[28..32].copy_from_slice(&runtime.ip);
        f[32..38].copy_from_slice(&[0x00; 6]);
        f[38..42].copy_from_slice(&target_ip);
    }
    let _ = emit_frame(runtime, 42)?;
    Ok(())
}

pub(crate) fn emit_arp_reply(
    runtime: &NetRuntime,
    target_mac: [u8; 6],
    target_ip: [u8; 4],
) -> Result<(), RequestError> {
    let frame = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);
    unsafe {
        let f = core::slice::from_raw_parts_mut(frame, 42);
        f[0..6].copy_from_slice(&target_mac);
        f[6..12].copy_from_slice(&runtime.mac);
        f[12..14].copy_from_slice(&[0x08, 0x06]);
        f[14..16].copy_from_slice(&[0x00, 0x01]);
        f[16..18].copy_from_slice(&[0x08, 0x00]);
        f[18] = 6;
        f[19] = 4;
        f[20..22].copy_from_slice(&[0x00, 0x02]);
        f[22..28].copy_from_slice(&runtime.mac);
        f[28..32].copy_from_slice(&runtime.ip);
        f[32..38].copy_from_slice(&target_mac);
        f[38..42].copy_from_slice(&target_ip);
    }
    let _ = emit_frame(runtime, 42)?;
    Ok(())
}

pub(crate) fn emit_gratuitous_arp_reply(runtime: &NetRuntime) -> Result<(), RequestError> {
    let frame = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);
    unsafe {
        let f = core::slice::from_raw_parts_mut(frame, 42);
        f[0..6].copy_from_slice(&[0xff; 6]); // broadcast
        f[6..12].copy_from_slice(&runtime.mac);
        f[12..14].copy_from_slice(&[0x08, 0x06]);
        f[14..16].copy_from_slice(&[0x00, 0x01]);
        f[16..18].copy_from_slice(&[0x08, 0x00]);
        f[18] = 6;
        f[19] = 4;
        f[20..22].copy_from_slice(&[0x00, 0x02]); // ARP reply
        f[22..28].copy_from_slice(&runtime.mac); // sender mac
        f[28..32].copy_from_slice(&runtime.ip); // sender ip
        f[32..38].copy_from_slice(&[0x00; 6]); // target mac (unknown/broadcast announcement)
        f[38..42].copy_from_slice(&runtime.ip); // target ip = own ip
    }
    let _ = emit_frame(runtime, 42)?;
    Ok(())
}

pub(crate) fn arp_lookup(runtime: &NetRuntime, ip: [u8; 4]) -> Option<[u8; 6]> {
    if runtime.arp.valid && runtime.arp.ip == ip {
        Some(runtime.arp.mac)
    } else {
        None
    }
}

pub(crate) fn update_arp(runtime: &mut NetRuntime, ip: [u8; 4], mac: [u8; 6]) {
    runtime.arp.valid = true;
    runtime.arp.ip = ip;
    runtime.arp.mac = mac;
}

pub(crate) fn process_arp(runtime: &mut NetRuntime, frame: &[u8]) {
    if frame.len() < 42 {
        return;
    }
    let oper = read_u16_be(&frame[20..22]);
    let sender_mac = [
        frame[22], frame[23], frame[24], frame[25], frame[26], frame[27],
    ];
    let sender_ip = [frame[28], frame[29], frame[30], frame[31]];
    let target_ip = [frame[38], frame[39], frame[40], frame[41]];

    if oper == 2 {
        update_arp(runtime, sender_ip, sender_mac);
        return;
    }

    if oper == 1 && target_ip == runtime.ip {
        let _ = emit_arp_reply(runtime, sender_mac, sender_ip);
    }
}
