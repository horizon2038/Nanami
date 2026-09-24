use super::*;
use nanami_services::net::*;

pub(super) fn handle_readiness_request(
    runtime: &NetRuntime,
    request: libnanami::ipc::ServiceRequest,
) -> (Word, Word, Word) {
    if session_for(runtime, request.identifier).is_none() {
        return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    let owner = request.identifier;
    let id = request.arg1;
    let ready = match request.arg0 {
        NET_READINESS_UDP => {
            let readable = runtime
                .udp_rx
                .entries
                .iter()
                .any(|entry| entry.used && entry.pid == owner && entry.dst_port as Word == id);
            NET_READY_WRITE | if readable { NET_READY_READ } else { 0 }
        }
        NET_READINESS_ICMP => {
            let readable = runtime.icmp_rx.entries.iter().any(|entry| {
                entry.used && entry.owner_id == owner && (id == 0 || entry.identifier as Word == id)
            });
            NET_READY_WRITE | if readable { NET_READY_READ } else { 0 }
        }
        NET_READINESS_LISTENER => {
            let readable = runtime.tcp_connections.iter().any(|conn| {
                conn.active
                    && conn.owner_id == owner
                    && !conn.accepted
                    && conn.local_port as Word == id
                    && matches!(conn.state, TCP_STATE_ESTABLISHED | TCP_STATE_CLOSE_WAIT)
            });
            if readable {
                NET_READY_READ
            } else {
                0
            }
        }
        NET_READINESS_TCP => {
            if id == 0 {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            match active_tcp_connection_index(runtime, owner, id) {
                Some(index) => {
                    let conn = &runtime.tcp_connections[index];
                    let closed = conn.eof_pending || conn.state == TCP_STATE_CLOSE_WAIT;
                    (if runtime.tcp_rx.readable(index) || closed {
                        NET_READY_READ
                    } else {
                        0
                    }) | (if matches!(conn.state, TCP_STATE_ESTABLISHED | TCP_STATE_CLOSE_WAIT) {
                        NET_READY_WRITE
                    } else {
                        0
                    }) | (if closed { NET_READY_READ_CLOSED } else { 0 })
                }
                // A stale connection ID cannot reveal another owner's state.
                None => NET_READY_READ | NET_READY_WRITE | NET_READY_HANGUP | NET_READY_ERROR,
            }
        }
        _ => return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    (libnanami::OS_RESPONSE_OK, ready, 0)
}
