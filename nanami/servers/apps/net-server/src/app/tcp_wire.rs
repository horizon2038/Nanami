use super::*;

pub(crate) fn emit_tcp_segment(
    runtime: &NetRuntime,
    dst_mac: [u8; 6],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    payload: &[u8],
) -> Result<Word, RequestError> {
    let frame_len = ETH_HDR_LEN + IPV4_HDR_LEN + TCP_HDR_LEN + payload.len();
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
            (IPV4_HDR_LEN + TCP_HDR_LEN + payload.len()) as u16,
        );
        write_u16_be(&mut ip[4..6], 0);
        write_u16_be(&mut ip[6..8], 0x4000);
        ip[8] = 64;
        ip[9] = 6;
        ip[10] = 0;
        ip[11] = 0;
        ip[12..16].copy_from_slice(&runtime.ip);
        ip[16..20].copy_from_slice(&dst_ip);
        let ip_csum = ipv4_checksum(ip);
        write_u16_be(&mut ip[10..12], ip_csum);

        let tcp_base = ETH_HDR_LEN + IPV4_HDR_LEN;
        {
            let tcp = &mut frame[tcp_base..tcp_base + TCP_HDR_LEN];
            write_u16_be(&mut tcp[0..2], src_port);
            write_u16_be(&mut tcp[2..4], dst_port);
            write_u32_be(&mut tcp[4..8], seq);
            write_u32_be(&mut tcp[8..12], ack);
            tcp[12] = (5u8 << 4) & 0xf0;
            tcp[13] = flags;
            write_u16_be(&mut tcp[14..16], window);
            write_u16_be(&mut tcp[16..18], 0);
            write_u16_be(&mut tcp[18..20], 0);
        }

        if !payload.is_empty() {
            frame[tcp_base + TCP_HDR_LEN..].copy_from_slice(payload);
        }

        let mut pseudo_sum: u32 = 0;
        pseudo_sum += (((runtime.ip[0] as u16) << 8) | runtime.ip[1] as u16) as u32;
        pseudo_sum += (((runtime.ip[2] as u16) << 8) | runtime.ip[3] as u16) as u32;
        pseudo_sum += (((dst_ip[0] as u16) << 8) | dst_ip[1] as u16) as u32;
        pseudo_sum += (((dst_ip[2] as u16) << 8) | dst_ip[3] as u16) as u32;
        pseudo_sum += 6u32;
        pseudo_sum += (TCP_HDR_LEN + payload.len()) as u32;
        let tcp_csum = checksum16(&frame[tcp_base..], pseudo_sum);
        {
            let tcp = &mut frame[tcp_base..tcp_base + TCP_HDR_LEN];
            write_u16_be(&mut tcp[16..18], tcp_csum);
        }
    }

    emit_frame(runtime, frame_len)
}
