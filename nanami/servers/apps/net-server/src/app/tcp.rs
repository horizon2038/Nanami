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
            write_u16_be(&mut tcp[14..16], TCP_WINDOW_DEFAULT);
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

pub(crate) fn tcp_reset(runtime: &mut NetRuntime, connection_index: usize) {
    if connection_index < runtime.tcp_connections.len() {
        runtime.tcp_connections[connection_index] = TcpConnection::EMPTY;
    }
}

fn listen_port(runtime: &NetRuntime) -> u16 {
    if runtime.session.tcp_port != 0 {
        runtime.session.tcp_port
    } else {
        TCP_LISTEN_PORT
    }
}

fn find_tcp_connection(
    runtime: &NetRuntime,
    src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.tcp_connections.len() {
        let conn = runtime.tcp_connections[i];
        if conn.active
            && conn.peer_ip == src_ip
            && conn.peer_port == src_port
            && conn.local_port == dst_port
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn allocate_tcp_connection(runtime: &NetRuntime) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.tcp_connections.len() {
        if !runtime.tcp_connections[i].active {
            return Some(i);
        }
        i += 1;
    }
    i = 0;
    while i < runtime.tcp_connections.len() {
        let state = runtime.tcp_connections[i].state;
        if state == TCP_STATE_LAST_ACK
            || state == TCP_STATE_FIN_WAIT1
            || state == TCP_STATE_FIN_WAIT2
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn emit_ack(runtime: &NetRuntime, stats: &mut NetStats, connection_index: usize, dst_mac: [u8; 6]) {
    let conn = runtime.tcp_connections[connection_index];
    let _ = emit_tcp_segment(
        runtime,
        dst_mac,
        conn.peer_ip,
        conn.local_port,
        conn.peer_port,
        conn.snd_nxt,
        conn.rcv_nxt,
        TCP_FLAG_ACK,
        &[],
    );
    stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
}

fn ack_and_maybe_queue_payload(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    connection_index: usize,
    src_mac: [u8; 6],
    src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    seq: u32,
    payload: &[u8],
    payload_len: u32,
) {
    let expected_seq = runtime.tcp_connections[connection_index].rcv_nxt;
    if payload_len > 0 && seq == expected_seq {
        // Only ACK payload we can enqueue, so we never ACK-and-drop.
        let can_queue_for_session = runtime.session.active && runtime.session.tcp_port == dst_port;
        let payload_fits_entry = (payload_len as usize) <= TCP_PAYLOAD_MAX;
        if can_queue_for_session && payload_fits_entry {
            let mut entry = TcpRxEntry::EMPTY;
            entry.connection_id = runtime.tcp_connections[connection_index].connection_id;
            entry.src_ip = src_ip;
            entry.src_port = src_port;
            entry.len = payload_len as usize;
            entry.payload[..entry.len].copy_from_slice(payload);
            if runtime.tcp_rx.push(entry) {
                let conn = &mut runtime.tcp_connections[connection_index];
                conn.rcv_nxt = conn.rcv_nxt.wrapping_add(payload_len);
            }
        }
    }

    emit_ack(runtime, stats, connection_index, src_mac);
    if payload_len > 0 {
        stats.tcp_rx = stats.tcp_rx.wrapping_add(1);
    }
}

pub(crate) fn process_tcp(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    frame: &[u8],
    ip_header_len: usize,
    src_ip: [u8; 4],
    ip_end: usize,
) {
    let tcp_base = ETH_HDR_LEN + ip_header_len;
    if ip_end < tcp_base + TCP_HDR_LEN || frame.len() < ip_end {
        return;
    }

    let src_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
    let src_port = read_u16_be(&frame[tcp_base..tcp_base + 2]);
    let dst_port = read_u16_be(&frame[tcp_base + 2..tcp_base + 4]);
    let seq = read_u32_be(&frame[tcp_base + 4..tcp_base + 8]);
    let ack_num = read_u32_be(&frame[tcp_base + 8..tcp_base + 12]);
    let data_offset = ((frame[tcp_base + 12] >> 4) as usize) * 4;
    if data_offset < TCP_HDR_LEN || ip_end < tcp_base + data_offset {
        return;
    }
    let flags = frame[tcp_base + 13];
    let payload = &frame[tcp_base + data_offset..ip_end];
    let payload_len = payload.len() as u32;

    if (flags & TCP_FLAG_SYN) != 0
        && (flags & TCP_FLAG_ACK) == 0
        && dst_port == listen_port(runtime)
    {
        let connection_index = match find_tcp_connection(runtime, src_ip, src_port, dst_port) {
            Some(index) => index,
            None => match allocate_tcp_connection(runtime) {
                Some(index) => index,
                None => return,
            },
        };
        let connection_id = runtime.next_tcp_connection_id;
        runtime.next_tcp_connection_id = runtime.next_tcp_connection_id.wrapping_add(1);
        if runtime.next_tcp_connection_id == 0 {
            runtime.next_tcp_connection_id = 1;
        }
        let iss = 0x4e41_4e41u32.wrapping_add((connection_index as u32) << 12);
        runtime.tcp_connections[connection_index] = TcpConnection {
            active: true,
            connection_id,
            state: TCP_STATE_SYN_RECEIVED,
            peer_ip: src_ip,
            peer_port: src_port,
            local_port: dst_port,
            snd_iss: iss,
            snd_nxt: iss.wrapping_add(1),
            snd_una: iss,
            rcv_nxt: seq.wrapping_add(1),
        };
        let conn = runtime.tcp_connections[connection_index];
        let _ = emit_tcp_segment(
            runtime,
            src_mac,
            src_ip,
            dst_port,
            src_port,
            conn.snd_iss,
            conn.rcv_nxt,
            TCP_FLAG_SYN | TCP_FLAG_ACK,
            &[],
        );
        stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
        return;
    }

    let Some(connection_index) = find_tcp_connection(runtime, src_ip, src_port, dst_port) else {
        return;
    };

    if (flags & TCP_FLAG_RST) != 0 {
        tcp_reset(runtime, connection_index);
        return;
    }

    match runtime.tcp_connections[connection_index].state {
        TCP_STATE_SYN_RECEIVED => {
            let conn = runtime.tcp_connections[connection_index];
            if (flags & TCP_FLAG_SYN) != 0
                && (flags & TCP_FLAG_ACK) == 0
                && seq.wrapping_add(1) == conn.rcv_nxt
            {
                let _ = emit_tcp_segment(
                    runtime,
                    src_mac,
                    src_ip,
                    dst_port,
                    src_port,
                    conn.snd_iss,
                    conn.rcv_nxt,
                    TCP_FLAG_SYN | TCP_FLAG_ACK,
                    &[],
                );
                stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
                return;
            }
            if (flags & TCP_FLAG_ACK) != 0 && ack_num == conn.snd_nxt {
                let conn = &mut runtime.tcp_connections[connection_index];
                conn.state = TCP_STATE_ESTABLISHED;
                conn.snd_una = ack_num;
                if payload_len > 0 {
                    ack_and_maybe_queue_payload(
                        runtime,
                        stats,
                        connection_index,
                        src_mac,
                        src_ip,
                        src_port,
                        dst_port,
                        seq,
                        payload,
                        payload_len,
                    );
                }
            }
        }
        TCP_STATE_ESTABLISHED => {
            {
                let conn = &mut runtime.tcp_connections[connection_index];
                if (flags & TCP_FLAG_ACK) != 0 && ack_num >= conn.snd_una && ack_num <= conn.snd_nxt
                {
                    conn.snd_una = ack_num;
                }
            }
            if payload_len > 0 {
                ack_and_maybe_queue_payload(
                    runtime,
                    stats,
                    connection_index,
                    src_mac,
                    src_ip,
                    src_port,
                    dst_port,
                    seq,
                    payload,
                    payload_len,
                );
            }

            if (flags & TCP_FLAG_FIN) != 0 {
                if seq == runtime.tcp_connections[connection_index].rcv_nxt {
                    let conn = &mut runtime.tcp_connections[connection_index];
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                }
                emit_ack(runtime, stats, connection_index, src_mac);
                runtime.tcp_connections[connection_index].state = TCP_STATE_CLOSE_WAIT;
            }
        }
        TCP_STATE_CLOSE_WAIT => {
            if (flags & TCP_FLAG_ACK) != 0 {
                let conn = &mut runtime.tcp_connections[connection_index];
                if ack_num >= conn.snd_una && ack_num <= conn.snd_nxt {
                    conn.snd_una = ack_num;
                }
            }
            emit_ack(runtime, stats, connection_index, src_mac);
        }
        TCP_STATE_FIN_WAIT1 => {
            {
                let conn = &mut runtime.tcp_connections[connection_index];
                if (flags & TCP_FLAG_ACK) != 0 && ack_num == conn.snd_nxt {
                    conn.snd_una = ack_num;
                    conn.state = TCP_STATE_FIN_WAIT2;
                }
            }
            if (flags & TCP_FLAG_FIN) != 0 {
                if seq == runtime.tcp_connections[connection_index].rcv_nxt {
                    let conn = &mut runtime.tcp_connections[connection_index];
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                }
                emit_ack(runtime, stats, connection_index, src_mac);
                tcp_reset(runtime, connection_index);
            }
        }
        TCP_STATE_FIN_WAIT2 => {
            if (flags & TCP_FLAG_FIN) != 0 {
                if seq == runtime.tcp_connections[connection_index].rcv_nxt {
                    let conn = &mut runtime.tcp_connections[connection_index];
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                }
                emit_ack(runtime, stats, connection_index, src_mac);
                tcp_reset(runtime, connection_index);
            }
        }
        TCP_STATE_LAST_ACK => {
            if (flags & TCP_FLAG_ACK) != 0
                && ack_num == runtime.tcp_connections[connection_index].snd_nxt
            {
                tcp_reset(runtime, connection_index);
            }
        }
        _ => {
            tcp_reset(runtime, connection_index);
        }
    }
}
