use super::*;

const TX_RETRY_MS: Word = 10;

#[derive(Clone, Copy)]
struct HttpConnection {
    active: bool,
    connection_id: Word,
    kind: ResponseKind,
    body: &'static [u8],
    header_sent: usize,
    body_sent: usize,
    keep_alive: bool,
    close_only: bool,
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
    };
}

pub(super) fn run_http_reactor(
    net_desc: Word,
    timer_desc: Option<Word>,
    rx_notification: Option<Word>,
    shm_vaddr: Word,
    shm_size: Word,
) -> libnanami::NanamiResult {
    let mut connections = [HttpConnection::EMPTY; HTTP_CONNECTIONS];
    let tx_capacity = tx_capacity(shm_size);
    if tx_capacity == 0 {
        libnanami::print!("[http-server] invalid tx shm capacity=0\n");
        return Err(libnanami::NanamiError(
            libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        ));
    }

    let mut idle_streak = 0usize;
    let mut rx_notification = rx_notification;
    let response_cache = build_response_cache();
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
    while budget < RX_BUDGET && free_slots != 0 {
        let (received, connection_id) = match nanami_services::net::net_service_tcp_recv_ex(
            net_desc,
            META_OFFSET,
            RX_OFFSET,
            TCP_RECV_MAX,
        ) {
            Ok(v) => v,
            Err(e) => {
                log_request_error("[http-server] tcp recv failed: ", e);
                break;
            }
        };
        if connection_id == 0 {
            if received == 0 {
                break;
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
                    prepare_response(conn, connection_id, req, response_cache);
                }
            }
            None => libnanami::print!("[http-server] no free http connection slot\n"),
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
            ..HttpConnection::EMPTY
        };
    }
    conn.keep_alive = false;
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
                Err(libnanami::RequestError::Status(
                    nanami_services::net::NET_SERVICE_RESPONSE_WOULD_BLOCK,
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
            pending_tx |= connections[index].active;
        }
        index += 1;
    }
    (did_work, pending_tx || index < connections.len())
}

fn is_client_abort(err: libnanami::RequestError) -> bool {
    matches!(
        err,
        libnanami::RequestError::Status(code) if code == libnanami::OS_RESPONSE_ILLEGAL_OPERATION
    )
}

fn flush_one_connection(
    net_desc: Word,
    shm_vaddr: Word,
    tx_capacity: usize,
    response_cache: &ResponseCache,
    conn: &mut HttpConnection,
) -> Result<bool, libnanami::RequestError> {
    if !conn.active {
        return Ok(false);
    }
    if conn.close_only {
        nanami_services::net::net_service_tcp_send_on_connection(
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
            let remain = conn.body.len() - conn.body_sent;
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
        && conn.body_sent + body_advance == conn.body.len();
    let flags = if will_finish {
        if conn.keep_alive {
            TCP_FLAG_ACK | TCP_FLAG_PSH
        } else {
            TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN
        }
    } else {
        TCP_FLAG_ACK | TCP_FLAG_PSH
    };

    nanami_services::net::net_service_tcp_send_on_connection(
        net_desc,
        conn.connection_id,
        TX_OFFSET,
        written as Word,
        flags,
    )?;

    conn.header_sent += header_advance;
    conn.body_sent += body_advance;
    if will_finish {
        conn.active = false;
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
    let body = response_body(kind);
    let keep_alive = should_keep_alive(req);
    *conn = HttpConnection::EMPTY;
    conn.active = true;
    conn.connection_id = connection_id;
    conn.kind = kind;
    conn.body = body;
    conn.keep_alive = keep_alive;
    let (_, header_len) = response_header(response_cache, kind, keep_alive);
    if header_len == 0 || header_len > HTTP_HEADER_CAP {
        conn.active = false;
        libnanami::print!("[http-server] cached header invalid\n");
        return;
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
        let _ = nanami_services::net::net_service_control(
            net_desc,
            nanami_services::net::NET_SERVICE_CONTROL_POLL,
            0,
            0,
        );
        *idle_streak = 0;
        return;
    }
    if let Some(notification) = *rx_notification {
        match libnanami::ipc::notification_wait(notification) {
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
