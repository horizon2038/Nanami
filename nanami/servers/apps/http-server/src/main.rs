#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;

use core::cmp::min;
use core::ptr;
use nanami_runtime::Word;

mod content;
mod framing;
mod options;
mod reactor;
use reactor::run_http_reactor;

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
enum ResponseKind {
    Index,
    Style,
    NotFound,
    BadRequest,
}

struct ResponseCache {
    index: &'static [u8],
    style: &'static [u8],
    bad_request: [u8; HTTP_HEADER_CAP],
    bad_request_len: usize,
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
        index: b"",
        style: b"",
        bad_request: [0; HTTP_HEADER_CAP],
        bad_request_len: 0,
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
    nanami_runtime::print!("[http-server] panic\n");
    loop {}
}
#[alloc_error_handler]
fn oom(_: core::alloc::Layout) -> ! {
    panic!("out of memory");
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    nanami_runtime::heap::init_heap(8 * 1024 * 1024)?;
    if nanami_runtime::process_arg(1) == Some(b"--help") {
        nanami_runtime::println!(
            "http-server [--port 1..65535] [--file /www/index.html] [--css /www/style.css]"
        );
        return Ok(());
    }
    let options = options::Options::parse(|index| {
        nanami_runtime::process_arg(index).and_then(|bytes| core::str::from_utf8(bytes).ok())
    })
    .map_err(|message| {
        nanami_runtime::println!("[http-server] {}", message);
        nanami_runtime::NanamiError(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)
    })?;
    let (index, style) = content::load(options.file, options.css).map_err(|error| {
        nanami_runtime::println!("[http-server] content load failed: {}", error);
        error
    })?;
    let response_cache = build_response_cache(index, style);
    nanami_runtime::print!("[http-server] bootstrap\n");

    let notification =
        nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    nanami_runtime::ipc::bind_current_thread_notification(notification).map_err(|e| {
        log_request_error("[http-server] bind notification failed: ", e);
        e
    })?;

    let net_desc = nanami_runtime::ipc::process_slot_descriptor(NET_SLOT);
    let timer_desc = connect_timer_service();
    wait_network_service(timer_desc);

    let self_pid = match nanami_runtime::get_self_pid() {
        Ok(pid) => pid,
        Err(e) => {
            log_request_error("[http-server] get_self_pid failed: ", e);
            return Err(e.into());
        }
    };

    let (status, shm_vaddr, shm_size) = match nanami_client::net::net_service_control_ex(
        net_desc,
        nanami_client::net::NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY,
        self_pid,
        SHM_SIZE,
    ) {
        Ok(v) => v,
        Err(e) => {
            log_request_error("[http-server] attach shm failed: ", e);
            return Err(e.into());
        }
    };
    if status != nanami_runtime::OS_RESPONSE_OK {
        nanami_runtime::print!("[http-server] attach shm status=");
        nanami_runtime::print!("{:#x}", status);
        nanami_runtime::print!("\n");
        return Err(nanami_runtime::NanamiError(status));
    }

    let rx_notification = match nanami_client::net::net_service_attach_rx_notification(
        net_desc,
        nanami_runtime::PROCESS_SLOT_NOTIFICATION,
    ) {
        Ok(()) => Some(notification),
        Err(e) => {
            log_request_error("[http-server] attach rx notification failed: ", e);
            None
        }
    };

    let _ = nanami_client::net::net_service_control(
        net_desc,
        nanami_client::net::NET_SERVICE_CONTROL_LINK_UP,
        0,
        0,
    );
    nanami_client::net::net_service_control(
        net_desc,
        nanami_client::net::NET_SERVICE_CONTROL_TCP_BIND,
        options.port as Word,
        0,
    )
    .map_err(|e| {
        log_request_error("[http-server] tcp bind failed: ", e);
        e
    })?;
    nanami_runtime::println!(
        "[http-server] tcp listen port={} file={} bytes={}",
        options.port,
        options.file,
        index.len()
    );

    run_http_reactor(
        net_desc,
        timer_desc,
        rx_notification,
        shm_vaddr,
        shm_size,
        response_cache,
    )
}

fn wait_network_service(timer_desc: Option<Word>) {
    let mut retry = 0usize;
    loop {
        match nanami_client::registry::connect_network_service(NET_SLOT) {
            Ok(()) => {
                nanami_runtime::print!("[http-server] connected network-service\n");
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

fn log_request_error(prefix: &str, err: nanami_runtime::RequestError) {
    nanami_runtime::print!(prefix);
    match err {
        nanami_runtime::RequestError::InvalidArgument => nanami_runtime::print!("invalid-arg\n"),
        nanami_runtime::RequestError::Unsupported => nanami_runtime::print!("unsupported\n"),
        nanami_runtime::RequestError::Transport => nanami_runtime::print!("transport\n"),
        nanami_runtime::RequestError::Protocol => nanami_runtime::print!("protocol\n"),
        nanami_runtime::RequestError::Status(code) => {
            nanami_runtime::print!("status=");
            nanami_runtime::print!("{:#x}", code);
            nanami_runtime::print!("\n");
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
    let start = if req.starts_with(b"GET ") {
        4
    } else if req.starts_with(b"HEAD ") {
        5
    } else {
        return None;
    };
    if req.len() < start + 2 {
        return None;
    }
    let mut i = start;
    while i < req.len() {
        if req[i] == b' ' {
            return Some(&req[start..i]);
        }
        if req[i] == b'\r' || req[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

fn should_keep_alive(req: &[u8]) -> bool {
    framing::request(req)
        .ok()
        .flatten()
        .is_some_and(|request| request.keep_alive)
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

fn response_body(cache: &ResponseCache, kind: ResponseKind) -> &'static [u8] {
    match kind {
        ResponseKind::Index => cache.index,
        ResponseKind::Style => cache.style,
        ResponseKind::NotFound => BODY_404,
        ResponseKind::BadRequest => b"400 Bad Request\n",
    }
}

fn response_header(cache: &ResponseCache, kind: ResponseKind, keep_alive: bool) -> (&[u8], usize) {
    match (kind, keep_alive) {
        (ResponseKind::BadRequest, _) => (
            &cache.bad_request[..cache.bad_request_len],
            cache.bad_request_len,
        ),
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

fn build_response_cache(index: &'static [u8], style: &'static [u8]) -> ResponseCache {
    let mut cache = ResponseCache::EMPTY;
    cache.index = index;
    cache.style = style;
    cache.bad_request_len = build_http_header(
        &mut cache.bad_request,
        b"HTTP/1.1 400 Bad Request\r\n",
        HTTP_HDR_CONTENT_TYPE_TEXT,
        b"400 Bad Request\n".len(),
        false,
    )
    .unwrap_or(0);
    cache.index_close_len = build_http_header(
        &mut cache.index_close,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_HTML,
        index.len(),
        false,
    )
    .unwrap_or(0);
    cache.index_keep_alive_len = build_http_header(
        &mut cache.index_keep_alive,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_HTML,
        index.len(),
        true,
    )
    .unwrap_or(0);
    cache.style_close_len = build_http_header(
        &mut cache.style_close,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_CSS,
        style.len(),
        false,
    )
    .unwrap_or(0);
    cache.style_keep_alive_len = build_http_header(
        &mut cache.style_keep_alive,
        HTTP_HDR_200,
        HTTP_HDR_CONTENT_TYPE_CSS,
        style.len(),
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
    match nanami_client::registry::connect_timer_service(TIMER_SLOT) {
        Ok(()) => Some(nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT)),
        Err(e) => {
            log_request_error("[http-server] timer connect failed: ", e);
            None
        }
    }
}

fn sleep_ms(timer_desc: Option<Word>, ms: Word) {
    if let Some(desc) = timer_desc {
        let _ = nanami_client::timer::timer_service_sleep_milliseconds(desc, ms);
    }
}

fn tx_capacity(shm_size: Word) -> usize {
    let tx_end = TX_OFFSET + TX_SIZE as Word;

    if tx_end > shm_size {
        return shm_size.saturating_sub(TX_OFFSET) as usize;
    }

    TX_SIZE
}

nanami_runtime::nanami_entry!(nanami_main);
