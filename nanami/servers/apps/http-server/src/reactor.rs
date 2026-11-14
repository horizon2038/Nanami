use super::*;
use alloc::vec::Vec;

const TX_RETRY_MS: Word = 1;

#[derive(Clone)]
struct HttpConnection {
    active: bool,
    connection_id: Word,
    kind: ResponseKind,
    body: &'static [u8],
    header_sent: usize,
    body_sent: usize,
    keep_alive: bool,
    close_only: bool,
    responding: bool,
    head: bool,
    peer_closed: bool,
    request: Vec<u8>,
}

impl HttpConnection {
    const EMPTY: Self = Self {
        active: false,
        connection_id: 0,
        kind: ResponseKind::NotFound,
        body: b"",
        header_sent: 0,
        body_sent: 0,
        keep_alive: false,
        close_only: false,
        responding: false,
        head: false,
        peer_closed: false,
        request: Vec::new(),
    };
}

pub(super) fn run_http_reactor(
    net_desc: Word,
    timer_desc: Option<Word>,
    rx_notification: Option<Word>,
    shm_vaddr: Word,
    shm_size: Word,
    response_cache: ResponseCache,
) -> nanami_runtime::NanamiResult {
    let mut connections: Vec<_> = (0..HTTP_CONNECTIONS)
        .map(|_| HttpConnection::EMPTY)
        .collect();
    let tx_capacity = tx_capacity(shm_size);
    if tx_capacity == 0 {
        nanami_runtime::print!("[http-server] invalid tx shm capacity=0\n");
        return Err(nanami_runtime::NanamiError(
            nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        ));
    }

    let mut idle_streak = 0usize;
    let mut rx_notification = rx_notification;
    loop {
        let received_any = drain_tcp_rx(net_desc, shm_vaddr, &response_cache, &mut connections);
        let (sent_any, pending_tx) = flush_http_tx(
            net_desc,
            shm_vaddr,
            tx_capacity,
            &response_cache,
            &mut connections,
        );

        if !received_any && !sent_any {
            idle_recv_backoff(
                net_desc,
                timer_desc,
                &mut rx_notification,
                &mut idle_streak,
                pending_tx,
            );
        } else {
            idle_streak = 0;
        }
    }
}

fn drain_tcp_rx(
    net_desc: Word,
    shm_vaddr: Word,
    response_cache: &ResponseCache,
    connections: &mut [HttpConnection],
) -> bool {
    let mut did_work = false;
    let mut budget = 0usize;
    // A deferred response owns its slot until it is sent. Do not dequeue a
    // request that we cannot retain while all response slots are occupied.
    let mut free_slots = connections.iter().filter(|conn| !conn.active).count();
    let mut partial = 0;
    while budget < RX_BUDGET {
        let filter = if free_slots == 0 {
            // All response slots can be occupied by fragmented headers. Read
            // only their already-owned streams, not a new unretainable stream.
            let Some(index) = (partial..connections.len())
                .find(|i| connections[*i].active && !connections[*i].responding)
            else {
                break;
            };
            partial = index + 1;
            connections[index].connection_id
        } else {
            0
        };
        let result = if filter == 0 {
            nanami_client::net::net_service_tcp_recv_ex(
                net_desc,
                META_OFFSET,
                RX_OFFSET,
                TCP_RECV_MAX,
            )
        } else {
            nanami_client::net::net_service_tcp_recv_on_connection(
                net_desc,
                filter,
                META_OFFSET,
                RX_OFFSET,
                TCP_RECV_MAX,
            )
        };
        let (received, connection_id) = match result {
            Ok(v) => v,
            Err(e) => {
                log_request_error("[http-server] tcp recv failed: ", e);
                break;
            }
        };
        if connection_id == 0 {
            if received == 0 {
                if filter == 0 {
                    break;
                } else {
                    continue;
                }
            }
            budget += 1;
            continue;
        }
        let req = unsafe {
            let src = (shm_vaddr + RX_OFFSET) as *const u8;
            core::slice::from_raw_parts(src, received as usize)
        };
        match acquire_connection(connections, connection_id) {
            Some(index) => {
                let conn = &mut connections[index];
                if !conn.active {
                    free_slots -= 1;
                }
                if received == 0 {
                    finish_peer_close(conn, connection_id);
                } else {
                    if !conn.active {
                        conn.request.clear();
                        conn.connection_id = connection_id;
                        conn.active = true;
                        conn.responding = false;
                        conn.peer_closed = false;
                    }
                    if conn.request.len().saturating_add(req.len()) > framing::MAX_PENDING {
                        // Do not overwrite an in-flight response. Finish it
                        // and close; otherwise reply with a bounded error.
                        conn.peer_closed = true;
                        if !conn.responding {
                            bad_request(conn);
                        }
                    } else {
                        conn.request.extend_from_slice(req);
                        if !conn.responding {
                            prepare_buffered(conn, response_cache);
                        }
                    }
                }
            }
            None => nanami_runtime::print!("[http-server] no free http connection slot\n"),
        }
        did_work = true;
        budget += 1;
    }
    did_work
}

fn finish_peer_close(conn: &mut HttpConnection, connection_id: Word) {
    if !conn.active {
        *conn = HttpConnection {
            active: true,
            connection_id,
            close_only: true,
            responding: true,
            ..HttpConnection::EMPTY
        };
    }
    conn.peer_closed = true;
    if !conn.responding {
        conn.close_only = true;
        conn.responding = true;
    }
}

fn flush_http_tx(
    net_desc: Word,
    shm_vaddr: Word,
    tx_capacity: usize,
    response_cache: &ResponseCache,
    connections: &mut [HttpConnection],
) -> (bool, bool) {
    let mut did_work = false;
    let mut pending_tx = false;
    let mut sent = 0usize;
    let mut index = 0usize;
    while index < connections.len() && sent < TX_BUDGET {
        if connections[index].active {
            match flush_one_connection(
                net_desc,
                shm_vaddr,
                tx_capacity,
                response_cache,
                &mut connections[index],
            ) {
                Ok(true) => {
                    did_work = true;
                    sent += 1;
                }
                Ok(false) => {}
                Err(nanami_runtime::RequestError::Status(
                    nanami_client::net::NET_SERVICE_RESPONSE_WOULD_BLOCK,
                )) => {}
                Err(e) => {
                    if !is_client_abort(e) {
                        log_request_error("[http-server] tcp send failed: ", e);
                    }
                    connections[index] = HttpConnection::EMPTY;
                    did_work = true;
                    sent += 1;
                }
            }
            pending_tx |= connections[index].active && connections[index].responding;
        }
        index += 1;
    }
    (did_work, pending_tx || index < connections.len())
}

fn is_client_abort(err: nanami_runtime::RequestError) -> bool {
    matches!(
        err,
        nanami_runtime::RequestError::Status(code) if code == nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION
    )
}

fn flush_one_connection(
    net_desc: Word,
    shm_vaddr: Word,
    tx_capacity: usize,
    response_cache: &ResponseCache,
    conn: &mut HttpConnection,
) -> Result<bool, nanami_runtime::RequestError> {
    if !conn.active {
        return Ok(false);
    }
    if !conn.responding {
        return Ok(false);
    }
    if conn.close_only {
        nanami_client::net::net_service_tcp_send_on_connection(
            net_desc,
            conn.connection_id,
            0,
            0,
            TCP_FLAG_ACK | TCP_FLAG_FIN,
        )?;
        conn.active = false;
        return Ok(true);
    }

    let mut written = 0usize;
    let mut header_advance = 0usize;
    let mut body_advance = 0usize;
    let (header, header_len) = response_header(response_cache, conn.kind, conn.keep_alive);
    unsafe {
        let dst = (shm_vaddr + TX_OFFSET) as *mut u8;
        if conn.header_sent < header_len {
            let remain = header_len - conn.header_sent;
            let n = min(remain, tx_capacity);
            ptr::copy_nonoverlapping(header.as_ptr().add(conn.header_sent), dst, n);
            written += n;
            header_advance = n;
        }
        if written < tx_capacity && conn.header_sent + header_advance == header_len {
            let body_len = if conn.head { 0 } else { conn.body.len() };
            let remain = body_len - conn.body_sent;
            let n = min(remain, tx_capacity - written);
            if n > 0 {
                ptr::copy_nonoverlapping(
                    conn.body.as_ptr().add(conn.body_sent),
                    dst.add(written),
                    n,
                );
                written += n;
                body_advance = n;
            }
        }
    }

    if written == 0 {
        conn.active = false;
        return Ok(false);
    }

    let will_finish = conn.header_sent + header_advance == header_len
        && conn.body_sent + body_advance == if conn.head { 0 } else { conn.body.len() };
    let pipelined = conn.keep_alive && matches!(framing::request(&conn.request), Ok(Some(_)));
    let flags = if will_finish {
        if conn.keep_alive && (!conn.peer_closed || pipelined) {
            TCP_FLAG_ACK | TCP_FLAG_PSH
        } else {
            TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN
        }
    } else {
        TCP_FLAG_ACK | TCP_FLAG_PSH
    };

    let accepted = nanami_client::net::net_service_tcp_send_on_connection(
        net_desc,
        conn.connection_id,
        TX_OFFSET,
        written as Word,
        flags,
    )?;
    if accepted > written {
        return Err(nanami_runtime::RequestError::Protocol);
    }
    if accepted == 0 {
        return Err(nanami_runtime::RequestError::Status(
            nanami_client::net::NET_SERVICE_RESPONSE_WOULD_BLOCK,
        ));
    }

    // The transport may accept only a prefix under NIC backpressure. Advance
    // by that prefix, never resend bytes whose sequence space was consumed.
    let header_remaining = header_len - conn.header_sent;
    header_advance = accepted.min(header_remaining);
    body_advance = accepted - header_advance;

    conn.header_sent += header_advance;
    conn.body_sent += body_advance;
    if will_finish && accepted == written {
        conn.responding = false;
        if conn.keep_alive && !conn.request.is_empty() && (!conn.peer_closed || pipelined) {
            prepare_buffered(conn, response_cache);
        } else {
            conn.active = false;
            conn.request.clear();
        }
    }
    Ok(true)
}

fn acquire_connection(connections: &mut [HttpConnection], connection_id: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < connections.len() {
        if connections[i].active && connections[i].connection_id == connection_id {
            return Some(i);
        }
        i += 1;
    }
    i = 0;
    while i < connections.len() {
        if !connections[i].active {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn prepare_response(
    conn: &mut HttpConnection,
    connection_id: Word,
    req: &[u8],
    response_cache: &ResponseCache,
) {
    let kind = select_response_kind(req);
    let body = response_body(response_cache, kind);
    let keep_alive = should_keep_alive(req);
    conn.header_sent = 0;
    conn.body_sent = 0;
    conn.close_only = false;
    conn.head = false;
    conn.responding = true;
    conn.active = true;
    conn.connection_id = connection_id;
    conn.kind = kind;
    conn.body = body;
    conn.keep_alive = keep_alive;
    let (_, header_len) = response_header(response_cache, kind, keep_alive);
    if header_len == 0 || header_len > HTTP_HEADER_CAP {
        conn.active = false;
        nanami_runtime::print!("[http-server] cached header invalid\n");
        return;
    }
}

fn bad_request(conn: &mut HttpConnection) {
    conn.kind = ResponseKind::BadRequest;
    conn.body = b"400 Bad Request\n";
    conn.header_sent = 0;
    conn.body_sent = 0;
    conn.head = false;
    conn.keep_alive = false;
    conn.responding = true;
    conn.close_only = false;
    conn.request.clear();
}
fn prepare_buffered(conn: &mut HttpConnection, cache: &ResponseCache) {
    match framing::request(&conn.request) {
        Ok(Some(request)) => {
            // Own the completed header while preparing so the connection's
            // queued pipeline remains intact and the response is immutable.
            let mut bytes = core::mem::take(&mut conn.request);
            prepare_response(conn, conn.connection_id, &bytes[..request.bytes], cache);
            bytes.drain(..request.bytes);
            conn.request = bytes;
            conn.head = request.head;
        }
        Ok(None) => {}
        Err(()) => bad_request(conn),
    }
}

fn idle_backoff_spin() {
    let mut i = 0usize;
    while i < IDLE_RECV_SPIN_LOOPS {
        core::hint::spin_loop();
        i += 1;
    }
}

fn idle_recv_backoff(
    net_desc: Word,
    timer_desc: Option<Word>,
    rx_notification: &mut Option<Word>,
    idle_streak: &mut usize,
    pending_tx: bool,
) {
    if pending_tx {
        // ARP replies/notifications can be lost. Only the deferred-send path
        // needs a bounded wait and a backend poll, including when RX is paused.
        if timer_desc.is_some() {
            sleep_ms(timer_desc, TX_RETRY_MS);
        } else {
            idle_backoff_spin();
        }
        let _ = nanami_client::net::net_service_control(
            net_desc,
            nanami_client::net::NET_SERVICE_CONTROL_POLL,
            0,
            0,
        );
        *idle_streak = 0;
        return;
    }
    if let Some(notification) = *rx_notification {
        match nanami_runtime::ipc::notification_wait(notification) {
            Ok(_) => {
                *idle_streak = 0;
                return;
            }
            Err(e) => {
                log_request_error("[http-server] rx notification wait failed: ", e);
                *rx_notification = None;
            }
        }
    }

    *idle_streak = idle_streak.saturating_add(1);
    if timer_desc.is_some() && *idle_streak > IDLE_SPIN_BEFORE_SLEEP {
        sleep_ms(timer_desc, IDLE_RECV_SLEEP_MS);
    } else {
        idle_backoff_spin();
    }
}
