#![no_std]
#![no_main]

use core::cmp::min;
use core::ptr;
use libnanami::Word;

const NET_SLOT: usize = 23;
const TIMER_SLOT: usize = 22;
const SHM_SIZE: Word = 0x80000;
const META_OFFSET: Word = 0x1000;
const RX_OFFSET: Word = 0x2000;
const TX_OFFSET: Word = 0x8000;
const TX_SIZE: usize = 0x4000;
const TCP_FLAG_FIN: Word = 0x01;
const TCP_FLAG_PSH: Word = 0x08;
const TCP_FLAG_ACK: Word = 0x10;
const CONNECT_RETRY_SLEEP_MS: Word = 10;
const IDLE_RECV_SLEEP_MS: Word = 1;
const IDLE_SPIN_BEFORE_SLEEP: usize = 8;
const IDLE_RECV_SPIN_LOOPS: usize = 1_000;
const HTTP_CONNECTIONS: usize = 128;
const HTTP_HEADER_CAP: usize = 256;
const RX_BUDGET: usize = 128;
const TX_BUDGET: usize = 128;
const TCP_RECV_MAX: Word = 1400;

const INDEX_HTML: &str = include_str!("index.html");
const STYLE_CSS: &str = include_str!("style.css");
const HTTP_HDR_200: &[u8] = b"HTTP/1.1 200 OK\r\n";
const HTTP_HDR_404: &[u8] = b"HTTP/1.1 404 Not Found\r\n";
const HTTP_HDR_CONTENT_TYPE_HTML: &[u8] = b"Content-Type: text/html; charset=utf-8\r\n";
const HTTP_HDR_CONTENT_TYPE_CSS: &[u8] = b"Content-Type: text/css; charset=utf-8\r\n";
const HTTP_HDR_CONTENT_TYPE_TEXT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n";
const HTTP_HDR_CONTENT_LENGTH: &[u8] = b"Content-Length: ";
const HTTP_HDR_CONNECTION_CLOSE: &[u8] = b"Connection: close\r\n";
const HTTP_HDR_CONNECTION_KEEP_ALIVE: &[u8] = b"Connection: keep-alive\r\n";
const HTTP_HDR_END: &[u8] = b"\r\n";
const BODY_404: &[u8] = b"404 Not Found\n";

#[derive(Clone, Copy)]
struct HttpConnection {
    active: bool,
    connection_id: Word,
    kind: ResponseKind,
    body: &'static [u8],
    header_sent: usize,
    body_sent: usize,
    keep_alive: bool,
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
    };
}

#[derive(Clone, Copy)]
enum ResponseKind {
    Index,
    Style,
    NotFound,
}

struct ResponseCache {
    index_close: [u8; HTTP_HEADER_CAP],
    index_close_len: usize,
    index_keep_alive: [u8; HTTP_HEADER_CAP],
    index_keep_alive_len: usize,
    style_close: [u8; HTTP_HEADER_CAP],
    style_close_len: usize,
    style_keep_alive: [u8; HTTP_HEADER_CAP],
    style_keep_alive_len: usize,
    not_found_close: [u8; HTTP_HEADER_CAP],
    not_found_close_len: usize,
    not_found_keep_alive: [u8; HTTP_HEADER_CAP],
    not_found_keep_alive_len: usize,
}

impl ResponseCache {
    const EMPTY: Self = Self {
        index_close: [0; HTTP_HEADER_CAP],
        index_close_len: 0,
        index_keep_alive: [0; HTTP_HEADER_CAP],
        index_keep_alive_len: 0,
        style_close: [0; HTTP_HEADER_CAP],
        style_close_len: 0,
        style_keep_alive: [0; HTTP_HEADER_CAP],
        style_keep_alive_len: 0,
        not_found_close: [0; HTTP_HEADER_CAP],
        not_found_close_len: 0,
        not_found_keep_alive: [0; HTTP_HEADER_CAP],
        not_found_keep_alive_len: 0,
    };
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[http-server] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[http-server] bootstrap\n");

    let net_desc = libnanami::ipc::process_slot_descriptor(NET_SLOT);
    let timer_desc = connect_timer_service();
    wait_network_service(timer_desc);

    let self_pid = match libnanami::get_self_pid() {
        Ok(pid) => pid,
        Err(e) => {
            log_request_error("[http-server] get_self_pid failed: ", e);
            return Err(e.into());
        }
    };

    let (status, shm_vaddr, shm_size) = match nanami_services::net::net_service_control_ex(
        net_desc,
        nanami_services::net::NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY,
        self_pid,
        SHM_SIZE,
    ) {
        Ok(v) => v,
        Err(e) => {
            log_request_error("[http-server] attach shm failed: ", e);
            return Err(e.into());
        }
    };
    if status != libnanami::OS_RESPONSE_OK {
        libnanami::print!("[http-server] attach shm status=");
        libnanami::print!("{:#x}", status);
        libnanami::print!("\n");
        return Err(libnanami::NanamiError(status));
    }

    let _ = nanami_services::net::net_service_control(
        net_desc,
        nanami_services::net::NET_SERVICE_CONTROL_LINK_UP,
        0,
        0,
    );
    let _ = nanami_services::net::net_service_control(
        net_desc,
        nanami_services::net::NET_SERVICE_CONTROL_TCP_BIND,
        80,
        0,
    );
    libnanami::print!("[http-server] tcp listen port=80\n");

    run_http_reactor(net_desc, timer_desc, shm_vaddr, shm_size)
}

fn run_http_reactor(
    net_desc: Word,
    timer_desc: Option<Word>,
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
    let response_cache = build_response_cache();
    loop {
        let received_any = drain_tcp_rx(net_desc, shm_vaddr, &response_cache, &mut connections);
        let sent_any = flush_http_tx(
            net_desc,
            shm_vaddr,
            tx_capacity,
            &response_cache,
            &mut connections,
        );

        if !received_any && !sent_any {
            idle_recv_backoff(timer_desc, &mut idle_streak);
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
    while budget < RX_BUDGET {
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
        if received == 0 {
            break;
        }
        if connection_id == 0 {
            budget += 1;
            continue;
        }

        let req = unsafe {
            let src = (shm_vaddr + RX_OFFSET) as *const u8;
            core::slice::from_raw_parts(src, received as usize)
        };
        match acquire_connection(connections, connection_id) {
            Some(index) => {
                prepare_response(&mut connections[index], connection_id, req, response_cache)
            }
            None => libnanami::print!("[http-server] no free http connection slot\n"),
        }
        did_work = true;
        budget += 1;
    }
    did_work
}

fn flush_http_tx(
    net_desc: Word,
    shm_vaddr: Word,
    tx_capacity: usize,
    response_cache: &ResponseCache,
    connections: &mut [HttpConnection],
) -> bool {
    let mut did_work = false;
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
                Err(e) => {
                    if !is_client_abort(e) {
                        log_request_error("[http-server] tcp send failed: ", e);
                    }
                    connections[index] = HttpConnection::EMPTY;
                    did_work = true;
                    sent += 1;
                }
            }
        }
        index += 1;
    }
    did_work
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

fn wait_network_service(timer_desc: Option<Word>) {
    let mut retry = 0usize;
    loop {
        match nanami_services::registry::connect_network_service(NET_SLOT) {
            Ok(()) => {
                libnanami::print!("[http-server] connected network-service\n");
                return;
            }
            Err(e) => {
                if retry % 1024 == 0 {
                    log_request_error("[http-server] waiting network-service: ", e);
                }
                retry = retry.wrapping_add(1);
                if timer_desc.is_some() {
                    sleep_ms(timer_desc, CONNECT_RETRY_SLEEP_MS);
                } else {
                    let mut spin = 0usize;
                    while spin < 200_000 {
                        core::hint::spin_loop();
                        spin += 1;
                    }
                }
            }
        }
    }
}

fn log_request_error(prefix: &str, err: libnanami::RequestError) {
    libnanami::print!(prefix);
    match err {
        libnanami::RequestError::InvalidArgument => libnanami::print!("invalid-arg\n"),
        libnanami::RequestError::Unsupported => libnanami::print!("unsupported\n"),
        libnanami::RequestError::Transport => libnanami::print!("transport\n"),
        libnanami::RequestError::Protocol => libnanami::print!("protocol\n"),
        libnanami::RequestError::Status(code) => {
            libnanami::print!("status=");
            libnanami::print!("{:#x}", code);
            libnanami::print!("\n");
        }
    }
}

fn push_bytes(buf: &mut [u8], pos: &mut usize, data: &[u8]) -> bool {
    if *pos + data.len() > buf.len() {
        return false;
    }
    let end = *pos + data.len();
    buf[*pos..end].copy_from_slice(data);
    *pos = end;
    true
}

fn push_usize_ascii(buf: &mut [u8], pos: &mut usize, mut value: usize) -> bool {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    if value == 0 {
        return push_bytes(buf, pos, b"0");
    }
    while value > 0 {
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        if !push_bytes(buf, pos, &tmp[n..n + 1]) {
            return false;
        }
    }
    true
}

fn build_http_header(
    dst: &mut [u8],
    status_line: &[u8],
    content_type: &[u8],
    body_len: usize,
    keep_alive: bool,
) -> Option<usize> {
    let mut pos = 0usize;
    if !push_bytes(dst, &mut pos, status_line) {
        return None;
    }
    if !push_bytes(dst, &mut pos, content_type) {
        return None;
    }
    if !push_bytes(dst, &mut pos, HTTP_HDR_CONTENT_LENGTH) {
        return None;
    }
    if !push_usize_ascii(dst, &mut pos, body_len) {
        return None;
    }
    if !push_bytes(dst, &mut pos, b"\r\n") {
        return None;
    }
    let connection_header = if keep_alive {
        HTTP_HDR_CONNECTION_KEEP_ALIVE
    } else {
        HTTP_HDR_CONNECTION_CLOSE
    };
    if !push_bytes(dst, &mut pos, connection_header) {
        return None;
    }
    if !push_bytes(dst, &mut pos, HTTP_HDR_END) {
        return None;
    }
    Some(pos)
}

fn request_path<'a>(req: &'a [u8]) -> Option<&'a [u8]> {
    if req.len() < 6 || !req.starts_with(b"GET ") {
        return None;
    }
    let mut i = 4usize;
    while i < req.len() {
        if req[i] == b' ' {
            return Some(&req[4..i]);
        }
        if req[i] == b'\r' || req[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

fn ascii_eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < a.len() {
        let ac = if a[i].is_ascii_uppercase() {
            a[i] + 32
        } else {
            a[i]
        };
        let bc = if b[i].is_ascii_uppercase() {
            b[i] + 32
        } else {
            b[i]
        };
        if ac != bc {
            return false;
        }
        i += 1;
    }
    true
}

fn contains_token_ignore_case(haystack: &[u8], token: &[u8]) -> bool {
    if token.is_empty() || haystack.len() < token.len() {
        return false;
    }
    let mut i = 0usize;
    while i + token.len() <= haystack.len() {
        if ascii_eq_ignore_case(&haystack[i..i + token.len()], token) {
            return true;
        }
        i += 1;
    }
    false
}

fn header_value<'a>(req: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut line_start = 0usize;
    while line_start < req.len() {
        let mut line_end = line_start;
        while line_end < req.len() && req[line_end] != b'\n' {
            line_end += 1;
        }
        let mut end = line_end;
        if end > line_start && req[end - 1] == b'\r' {
            end -= 1;
        }
        let line = &req[line_start..end];
        if line.len() > name.len()
            && ascii_eq_ignore_case(&line[..name.len()], name)
            && line[name.len()] == b':'
        {
            let mut value_start = name.len() + 1;
            while value_start < line.len()
                && (line[value_start] == b' ' || line[value_start] == b'\t')
            {
                value_start += 1;
            }
            return Some(&line[value_start..]);
        }
        line_start = line_end.saturating_add(1);
    }
    None
}

fn request_is_http11(req: &[u8]) -> bool {
    let mut i = 0usize;
    while i < req.len() {
        if req[i] == b'\r' || req[i] == b'\n' {
            return false;
        }
        if i + 8 <= req.len() && &req[i..i + 8] == b"HTTP/1.1" {
            return true;
        }
        i += 1;
    }
    false
}

fn should_keep_alive(req: &[u8]) -> bool {
    if let Some(value) = header_value(req, b"Connection") {
        if contains_token_ignore_case(value, b"close") {
            return false;
        }
        if contains_token_ignore_case(value, b"keep-alive") {
            return true;
        }
    }
    request_is_http11(req)
}

fn select_response_kind(req: &[u8]) -> ResponseKind {
    let path = request_path(req).unwrap_or(b"/");
    if path == b"/" || path == b"/index.html" {
        return ResponseKind::Index;
    }
    if path == b"/style.css" {
        return ResponseKind::Style;
    }
    ResponseKind::NotFound
}

fn response_body(kind: ResponseKind) -> &'static [u8] {
    match kind {
        ResponseKind::Index => INDEX_HTML.as_bytes(),
        ResponseKind::Style => STYLE_CSS.as_bytes(),
        ResponseKind::NotFound => BODY_404,
    }
}

fn response_header(cache: &ResponseCache, kind: ResponseKind, keep_alive: bool) -> (&[u8], usize) {
    match (kind, keep_alive) {
        (ResponseKind::Index, false) => (
            &cache.index_close[..cache.index_close_len],
            cache.index_close_len,
        ),
        (ResponseKind::Index, true) => (
            &cache.index_keep_alive[..cache.index_keep_alive_len],
            cache.index_keep_alive_len,
        ),
        (ResponseKind::Style, false) => (
            &cache.style_close[..cache.style_close_len],
            cache.style_close_len,
        ),
        (ResponseKind::Style, true) => (
            &cache.style_keep_alive[..cache.style_keep_alive_len],
            cache.style_keep_alive_len,
        ),
        (ResponseKind::NotFound, false) => (
            &cache.not_found_close[..cache.not_found_close_len],
            cache.not_found_close_len,
        ),
        (ResponseKind::NotFound, true) => (
            &cache.not_found_keep_alive[..cache.not_found_keep_alive_len],
            cache.not_found_keep_alive_len,
        ),
    }
}

fn build_response_cache() -> ResponseCache {
    let mut cache = ResponseCache::EMPTY;
    cache.index_close_len = build_http_header(
        &mut cache.index_close,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_HTML,
        INDEX_HTML.len(),
        false,
    )
    .unwrap_or(0);
    cache.index_keep_alive_len = build_http_header(
        &mut cache.index_keep_alive,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_HTML,
        INDEX_HTML.len(),
        true,
    )
    .unwrap_or(0);
    cache.style_close_len = build_http_header(
        &mut cache.style_close,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_CSS,
        STYLE_CSS.len(),
        false,
    )
    .unwrap_or(0);
    cache.style_keep_alive_len = build_http_header(
        &mut cache.style_keep_alive,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_CSS,
        STYLE_CSS.len(),
        true,
    )
    .unwrap_or(0);
    cache.not_found_close_len = build_http_header(
        &mut cache.not_found_close,
        HTTP_HDR_404,
        HTTP_HDR_CONTENT_TYPE_TEXT,
        BODY_404.len(),
        false,
    )
    .unwrap_or(0);
    cache.not_found_keep_alive_len = build_http_header(
        &mut cache.not_found_keep_alive,
        HTTP_HDR_404,
        HTTP_HDR_CONTENT_TYPE_TEXT,
        BODY_404.len(),
        true,
    )
    .unwrap_or(0);
    cache
}

fn connect_timer_service() -> Option<Word> {
    match nanami_services::registry::connect_timer_service(TIMER_SLOT) {
        Ok(()) => Some(libnanami::ipc::process_slot_descriptor(TIMER_SLOT)),
        Err(e) => {
            log_request_error("[http-server] timer connect failed: ", e);
            None
        }
    }
}

fn sleep_ms(timer_desc: Option<Word>, ms: Word) {
    if let Some(desc) = timer_desc {
        let _ = nanami_services::timer::timer_service_sleep_milliseconds(desc, ms);
    }
}

fn idle_backoff_spin() {
    let mut i = 0usize;
    while i < IDLE_RECV_SPIN_LOOPS {
        core::hint::spin_loop();
        i += 1;
    }
}

fn idle_recv_backoff(timer_desc: Option<Word>, idle_streak: &mut usize) {
    *idle_streak = idle_streak.saturating_add(1);
    if timer_desc.is_some() && *idle_streak > IDLE_SPIN_BEFORE_SLEEP {
        sleep_ms(timer_desc, IDLE_RECV_SLEEP_MS);
    } else {
        idle_backoff_spin();
    }
}

fn tx_capacity(shm_size: Word) -> usize {
    let tx_end = TX_OFFSET + TX_SIZE as Word;

    if tx_end > shm_size {
        return shm_size.saturating_sub(TX_OFFSET) as usize;
    }

    TX_SIZE
}

libnanami::nanami_entry!(nanami_main);
