use super::*;

pub(crate) fn tcp_reset(runtime: &mut NetRuntime, connection_index: usize) {
    if connection_index < runtime.tcp_connections.len() {
        runtime
            .tcp_index
            .remove(connection_index, &runtime.tcp_connections[connection_index]);
        runtime.tcp_rx.reset(connection_index);
        runtime.tcp_connections[connection_index] = TcpConnection::EMPTY;
    }
}

fn find_tcp_connection(
    runtime: &NetRuntime,
    src_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
) -> Option<usize> {
    runtime
        .tcp_index
        .find(&runtime.tcp_connections, src_ip, src_port, dst_port)
}

pub(crate) fn allocate_tcp_connection(runtime: &NetRuntime) -> Option<usize> {
    runtime.tcp_index.vacant().or_else(|| {
        // Only reclaim after our data and FIN were acknowledged.
        runtime
            .tcp_connections
            .iter()
            .position(|conn| conn.state == TCP_STATE_FIN_WAIT2)
    })
}

pub(super) fn emit_ack(
    runtime: &NetRuntime,
    stats: &mut NetStats,
    connection_index: usize,
    dst_mac: [u8; 6],
) {
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
        runtime.tcp_rx.window(connection_index),
        &[],
    );
    stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
}

fn receive_segment(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    index: usize,
    src_mac: [u8; 6],
    seq: u32,
    flags: u8,
    payload: &[u8],
) {
    let expected_seq = runtime.tcp_connections[index].rcv_nxt;
    if !payload.is_empty() {
        // Retransmissions can overlap bytes already acknowledged, including
        // when the previous segment only partly fitted the receive window.
        let skip = expected_seq.wrapping_sub(seq) as usize;
        if skip < payload.len() {
            let accepted = runtime.tcp_rx.push(index, &payload[skip..]);
            runtime.tcp_connections[index].rcv_nxt = expected_seq.wrapping_add(accepted as u32);
        }
        stats.tcp_rx = stats.tcp_rx.wrapping_add(1);
    }
    if flags & TCP_FLAG_FIN != 0 {
        let conn = &mut runtime.tcp_connections[index];
        // Do not report EOF across a hole or when its preceding data did not fit.
        if seq.wrapping_add(payload.len() as u32) == conn.rcv_nxt {
            conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
            conn.state = TCP_STATE_CLOSE_WAIT;
            conn.eof_pending = true;
        }
    }
    if !payload.is_empty() || flags & TCP_FLAG_FIN != 0 || seq != expected_seq {
        // Also answer zero-window probes / duplicate data with the current window.
        emit_ack(runtime, stats, index, src_mac);
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

    if (flags & TCP_FLAG_SYN) != 0 && (flags & TCP_FLAG_ACK) == 0 {
        let Some(session) = session_for_tcp_port(runtime, dst_port) else {
            return;
        };
        if let Some(index) = find_tcp_connection(runtime, src_ip, src_port, dst_port) {
            let conn = runtime.tcp_connections[index];
            if conn.state == TCP_STATE_SYN_RECEIVED && seq.wrapping_add(1) == conn.rcv_nxt {
                let _ = emit_tcp_segment(
                    runtime,
                    src_mac,
                    src_ip,
                    dst_port,
                    src_port,
                    conn.snd_iss,
                    conn.rcv_nxt,
                    TCP_FLAG_SYN | TCP_FLAG_ACK,
                    runtime.tcp_rx.window(index),
                    &[],
                );
                stats.tcp_tx = stats.tcp_tx.wrapping_add(1);
            } else {
                emit_ack(runtime, stats, index, src_mac);
            }
            // A retransmitted SYN must not replace a live application's ID/state.
            return;
        }
        let Some(connection_index) = allocate_tcp_connection(runtime) else {
            return;
        };
        tcp_reset(runtime, connection_index);
        let connection_id = runtime
            .tcp_index
            .insert(connection_index, src_ip, src_port, dst_port);
        let iss = 0x4e41_4e41u32.wrapping_add((connection_index as u32) << 12);
        runtime.tcp_connections[connection_index] = TcpConnection {
            active: true,
            owner_id: session.caller_id,
            connection_id,
            accepted: false,
            eof_pending: false,
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
            runtime.tcp_rx.window(connection_index),
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
        TCP_STATE_SYN_SENT => {
            let conn = runtime.tcp_connections[connection_index];
            if (flags & (TCP_FLAG_SYN | TCP_FLAG_ACK)) == (TCP_FLAG_SYN | TCP_FLAG_ACK)
                && ack_num == conn.snd_nxt
            {
                let conn = &mut runtime.tcp_connections[connection_index];
                conn.snd_una = ack_num;
                conn.rcv_nxt = seq.wrapping_add(1);
                conn.state = TCP_STATE_ESTABLISHED;
                emit_ack(runtime, stats, connection_index, src_mac);
            }
        }
        TCP_STATE_SYN_RECEIVED => {
            let conn = &mut runtime.tcp_connections[connection_index];
            if (flags & TCP_FLAG_ACK) != 0 && ack_num == conn.snd_nxt && seq == conn.rcv_nxt {
                conn.state = TCP_STATE_ESTABLISHED;
                conn.snd_una = ack_num;
                receive_segment(
                    runtime,
                    stats,
                    connection_index,
                    src_mac,
                    seq,
                    flags,
                    payload,
                );
            }
        }
        TCP_STATE_ESTABLISHED => {
            let conn = &mut runtime.tcp_connections[connection_index];
            if (flags & TCP_FLAG_ACK) != 0
                && ack_num.wrapping_sub(conn.snd_una) <= conn.snd_nxt.wrapping_sub(conn.snd_una)
            {
                conn.snd_una = ack_num;
            }
            receive_segment(
                runtime,
                stats,
                connection_index,
                src_mac,
                seq,
                flags,
                payload,
            );
        }
        TCP_STATE_CLOSE_WAIT => {
            if (flags & TCP_FLAG_ACK) != 0 {
                let conn = &mut runtime.tcp_connections[connection_index];
                if ack_num.wrapping_sub(conn.snd_una) <= conn.snd_nxt.wrapping_sub(conn.snd_una) {
                    conn.snd_una = ack_num;
                }
            }
            if payload_len != 0 || (flags & TCP_FLAG_FIN) != 0 {
                emit_ack(runtime, stats, connection_index, src_mac);
            }
        }
        TCP_STATE_FIN_WAIT1 | TCP_STATE_FIN_WAIT2 => {
            {
                let conn = &mut runtime.tcp_connections[connection_index];
                if (flags & TCP_FLAG_ACK) != 0 && ack_num == conn.snd_nxt {
                    conn.snd_una = ack_num;
                    conn.state = TCP_STATE_FIN_WAIT2;
                }
            }
            if (flags & TCP_FLAG_FIN) != 0 {
                if seq.wrapping_add(payload_len)
                    == runtime.tcp_connections[connection_index].rcv_nxt
                {
                    let conn = &mut runtime.tcp_connections[connection_index];
                    conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
                    emit_ack(runtime, stats, connection_index, src_mac);
                    let conn = &runtime.tcp_connections[connection_index];
                    if conn.snd_una == conn.snd_nxt {
                        tcp_reset(runtime, connection_index);
                    } else {
                        runtime.tcp_connections[connection_index].state = TCP_STATE_LAST_ACK;
                    }
                } else {
                    emit_ack(runtime, stats, connection_index, src_mac);
                }
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
