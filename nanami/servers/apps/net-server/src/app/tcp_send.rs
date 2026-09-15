use super::*;

pub(super) fn handle_tcp_send_request(
    runtime: &mut NetRuntime,
    request: libnanami::ipc::ServiceRequest,
    stats: &mut NetStats,
) -> (Word, Word, Word) {
    let Some(session) = session_for(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    };
    let Some(connection_index) =
        active_tcp_connection_index(runtime, request.identifier, request.arg3)
    else {
        return (libnanami::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };

    let payload_offset = request.arg0;
    let payload_len = request.arg1 as usize;
    let flags = (request.arg2 & 0xff) as u8;
    if payload_offset + payload_len as Word > session.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let peer_ip = runtime.tcp_connections[connection_index].peer_ip;
    let next_hop = next_hop_ip(runtime, peer_ip);
    let dst_mac = if let Some(mac) = arp_lookup(runtime, next_hop) {
        mac
    } else {
        let _ = emit_arp_request(runtime, next_hop);
        // No payload or FIN was consumed. The caller retains it until ARP resolves.
        return (nanami_services::net::NET_SERVICE_RESPONSE_WOULD_BLOCK, 0, 0);
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
            runtime.tcp_rx.window(connection_index),
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
            let src = (session.shm_local + current_offset) as *const u8;
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
                runtime.tcp_rx.window(connection_index),
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
