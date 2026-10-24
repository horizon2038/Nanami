#![no_std]
#![no_main]

use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{RequestError, Word};
use nanami_client::terminal::*;

mod ring;
use ring::ByteRing;

const SLOT_SERVICE_PORT: Word = 20;
const MAX_TERMINALS: usize = 8;
const MAX_CLIENTS: usize = 24;
const RING_BYTES: usize = 4096;
const SLOT_OUTPUT_NOTIFICATION_BASE: Word = 32;
const SLOT_INPUT_NOTIFICATION_BASE: Word = SLOT_OUTPUT_NOTIFICATION_BASE + MAX_TERMINALS as Word;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[terminal-service] panic\n");
    loop {}
}

#[derive(Clone, Copy)]
struct ClientSession {
    active: bool,
    pid: Word,
    shm: Word,
    shm_size: Word,
}

impl ClientSession {
    const EMPTY: Self = Self {
        active: false,
        pid: 0,
        shm: 0,
        shm_size: 0,
    };
}

#[derive(Clone, Copy)]
struct TerminalSession {
    active: bool,
    id: Word,
    columns: Word,
    rows: Word,
    input_notification: Word,
    output_notification: Word,
    echo_enabled: bool,
    output_crlf: bool,
    input_edit_len: usize,
    input_filter: Word,
    input: ByteRing,
    output: ByteRing,
}

impl TerminalSession {
    const EMPTY: Self = Self {
        active: false,
        id: 0,
        columns: TERMINAL_DEFAULT_COLUMNS,
        rows: TERMINAL_DEFAULT_ROWS,
        input_notification: 0,
        output_notification: 0,
        echo_enabled: true,
        output_crlf: true,
        input_edit_len: 0,
        input_filter: 0,
        input: ByteRing::EMPTY,
        output: ByteRing::EMPTY,
    };
}

struct Runtime {
    clients: [ClientSession; MAX_CLIENTS],
    terminals: [TerminalSession; MAX_TERMINALS],
    next_terminal_id: Word,
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::print!("[terminal-service] bootstrap\n");
    nanami_runtime::ipc::init_ipc_tls()
        .map_err(|e| log_error("[terminal-service] ipc tls failed: ", e))?;
    nanami_client::registry::register_terminal_service()
        .map_err(|e| log_error("[terminal-service] register failed: ", e))?;
    nanami_runtime::print!("[terminal-service] service registered: terminal-service\n");

    let mut runtime = Runtime {
        clients: [ClientSession::EMPTY; MAX_CLIENTS],
        terminals: [TerminalSession::EMPTY; MAX_TERMINALS],
        next_terminal_id: 1,
    };
    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = Reply::Drop;

    loop {
        let event = match pending {
            Reply::Send(status, detail0, detail1) => {
                pending = Reply::Drop;
                nanami_runtime::ipc::service_reply_receive_event(service_port, status, detail0, detail1)
            }
            Reply::Drop => nanami_runtime::ipc::service_receive_event(service_port),
        };
        let event = match event {
            Ok(event) => event,
            Err(e) => return Err(log_error("[terminal-service] ipc failed: ", e)),
        };
        pending = match event {
            ServiceEvent::Request(request) => handle_request(&mut runtime, request),
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => Reply::Drop,
        };
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Send(Word, Word, Word),
    Drop,
}

fn handle_request(runtime: &mut Runtime, request: ServiceRequest) -> Reply {
    let (status, detail0, detail1) = match request.code {
        TERMINAL_REQUEST_CONTROL => handle_control(runtime, request),
        TERMINAL_REQUEST_CREATE => handle_create(runtime, request),
        TERMINAL_REQUEST_WRITE_INPUT => handle_write(runtime, request, true),
        TERMINAL_REQUEST_READ_INPUT => handle_read(runtime, request, true),
        TERMINAL_REQUEST_WRITE_OUTPUT => handle_write(runtime, request, false),
        TERMINAL_REQUEST_READ_OUTPUT => handle_read(runtime, request, false),
        TERMINAL_REQUEST_GET_SIZE => handle_get_size(runtime, request),
        TERMINAL_REQUEST_ATTACH_OUTPUT_NOTIFICATION => {
            handle_attach_output_notification(runtime, request)
        }
        TERMINAL_REQUEST_ATTACH_INPUT_NOTIFICATION => {
            handle_attach_input_notification(runtime, request)
        }
        TERMINAL_REQUEST_CLEAR => handle_clear(runtime, request),
        TERMINAL_REQUEST_SET_ECHO => handle_set_echo(runtime, request),
        TERMINAL_REQUEST_SET_SIZE => handle_set_size(runtime, request),
        TERMINAL_REQUEST_SET_OUTPUT_CRLF => match find_terminal(runtime, request.arg0) {
            Some(index) => {
                runtime.terminals[index].output_crlf = request.arg1 != 0;
                (nanami_runtime::OS_RESPONSE_OK, 0, 0)
            }
            None => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    Reply::Send(status, detail0, detail1)
}

fn handle_control(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.arg0 {
        TERMINAL_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                TERMINAL_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match nanami_runtime::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match client_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.clients[index] = ClientSession {
                            active: true,
                            pid: request.identifier,
                            shm: local,
                            shm_size: size,
                        };
                        (nanami_runtime::OS_RESPONSE_OK, peer, size)
                    }
                    None => (nanami_runtime::OS_RESPONSE_FATAL, 0, 0),
                },
                Err(e) => (map_request_error(e), 0, 0),
            }
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_create(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = free_terminal(runtime) else {
        return (nanami_runtime::OS_RESPONSE_FATAL, 0, 0);
    };
    let columns = if request.arg0 == 0 {
        TERMINAL_DEFAULT_COLUMNS
    } else {
        request.arg0
    };
    let rows = if request.arg1 == 0 {
        TERMINAL_DEFAULT_ROWS
    } else {
        request.arg1
    };
    let id = runtime.next_terminal_id;
    runtime.next_terminal_id = runtime.next_terminal_id.saturating_add(1);
    runtime.terminals[index] = TerminalSession {
        active: true,
        id,
        columns,
        rows,
        input_notification: 0,
        output_notification: 0,
        echo_enabled: true,
        output_crlf: true,
        input_edit_len: 0,
        input_filter: 0,
        input: ByteRing::EMPTY,
        output: ByteRing::EMPTY,
    };
    nanami_runtime::println!(
        "[terminal-service] created id={} owner={} size={}x{}",
        id,
        request.identifier,
        columns,
        rows
    );
    (nanami_runtime::OS_RESPONSE_OK, id, 0)
}

fn handle_write(runtime: &mut Runtime, request: ServiceRequest, input: bool) -> (Word, Word, Word) {
    let Some(client) = find_client(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1.saturating_add(request.arg2) > client.shm_size {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    // Output has no per-byte editing. With echo disabled, input does not either.
    // Keep the editing/echo path below unchanged.
    let terminal = &mut runtime.terminals[index];
    if input {
        terminal.input_filter = 0;
    }
    if !input && terminal.output_crlf {
        let done = unsafe {
            terminal
                .output
                .write_crlf((client.shm + request.arg1) as *const u8, request.arg2)
        };
        if done != 0 {
            notify_terminal(terminal.output_notification);
        }
        return (nanami_runtime::OS_RESPONSE_OK, done, 0);
    }
    if !input || !terminal.echo_enabled {
        let (ring, notification) = if input {
            (&mut terminal.input, terminal.input_notification)
        } else {
            (&mut terminal.output, terminal.output_notification)
        };
        let done = unsafe {
            ring.write_from(
                (client.shm + request.arg1) as *const u8,
                request.arg2 as usize,
            )
        };
        if done != 0 {
            notify_terminal(notification);
        }
        return (nanami_runtime::OS_RESPONSE_OK, done as Word, 0);
    }
    let mut done = 0usize;
    let mut echoed_any = false;
    while done < request.arg2 as usize {
        let byte =
            unsafe { core::ptr::read((client.shm + request.arg1 + done as Word) as *const u8) };
        let (ok, echoed) = push_input_byte(&mut runtime.terminals[index], byte);
        if !ok {
            break;
        }
        echoed_any |= echoed;
        done += 1;
    }
    if done != 0 {
        notify_terminal(runtime.terminals[index].input_notification);
        if echoed_any {
            notify_terminal(runtime.terminals[index].output_notification);
        }
    }
    (nanami_runtime::OS_RESPONSE_OK, done as Word, 0)
}

fn handle_set_echo(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    runtime.terminals[index].echo_enabled = request.arg1 != 0;
    if !runtime.terminals[index].echo_enabled {
        runtime.terminals[index].input_edit_len = 0;
    }
    (nanami_runtime::OS_RESPONSE_OK, 0, 0)
}

fn push_input_byte(terminal: &mut TerminalSession, byte: u8) -> (bool, bool) {
    if !terminal.echo_enabled {
        return (terminal.input.push(byte), false);
    }
    match byte {
        0x7f | 0x08 if terminal.input_edit_len == 0 => (true, false),
        0x7f | 0x08 => {
            let ok = terminal.input.push(byte);
            if ok {
                terminal.input_edit_len -= 1;
                echo_input_byte(&mut terminal.output, byte);
            }
            (ok, ok)
        }
        b'\n' | b'\r' => {
            let ok = terminal.input.push(byte);
            if ok {
                terminal.input_edit_len = 0;
                echo_input_byte(&mut terminal.output, byte);
            }
            (ok, ok)
        }
        0x20..=0x7e | b'\t' => {
            let ok = terminal.input.push(byte);
            if ok {
                terminal.input_edit_len = terminal.input_edit_len.saturating_add(1);
                echo_input_byte(&mut terminal.output, byte);
            }
            (ok, ok)
        }
        _ => (terminal.input.push(byte), false),
    }
}

fn echo_input_byte(output: &mut ByteRing, byte: u8) {
    match byte {
        b'\n' | b'\r' => {
            let _ = output.push(b'\r');
            let _ = output.push(b'\n');
        }
        0x7f | 0x08 => {
            let _ = output.push(0x08);
            let _ = output.push(b' ');
            let _ = output.push(0x08);
        }
        0x20..=0x7e => {
            let _ = output.push(byte);
        }
        _ => {}
    }
}

fn handle_read(runtime: &mut Runtime, request: ServiceRequest, input: bool) -> (Word, Word, Word) {
    let Some(client) = find_client(runtime, request.identifier) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1.saturating_add(request.arg2) > client.shm_size {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let terminal = &mut runtime.terminals[index];
    let mut events = 0;
    // A canonical reader consumes one byte per call. Do not rescan the same
    // batch until another input write or a different control configuration.
    if input && request.arg3 & TERMINAL_INPUT_CONTROLS != 0
        && terminal.input_filter != request.arg3
    {
        let controls = [request.arg3 as u8, (request.arg3 >> 8) as u8, (request.arg3 >> 16) as u8];
        let flush = request.arg3 & TERMINAL_INPUT_FLUSH != 0;
        events = terminal.input.take_controls(controls, flush);
        terminal.input_filter = request.arg3;
        if events != 0 {
            if flush {
                terminal.output.clear();
                terminal.input_edit_len = 0;
            }
            if terminal.echo_enabled && request.arg3 & TERMINAL_INPUT_ECHO != 0 {
                for (index, byte) in controls.iter().copied().enumerate() {
                    if events & (1 << index) != 0 {
                        if byte < 0x20 {
                            terminal.output.push(b'^');
                            terminal.output.push(byte + b'@');
                        } else {
                            terminal.output.push(byte);
                        }
                        terminal.output.push(b'\r');
                        terminal.output.push(b'\n');
                    }
                }
                notify_terminal(terminal.output_notification);
            }
        }
    }
    if request.arg2 == 0 {
        let len = if input {
            terminal.input.len
        } else {
            terminal.output.len
        };
        return (nanami_runtime::OS_RESPONSE_OK, len as Word, events);
    }
    let ring = if input {
        &mut terminal.input
    } else {
        &mut terminal.output
    };
    let done = unsafe {
        ring.read_into(
            (client.shm + request.arg1) as *mut u8,
            request.arg2 as usize,
        )
    };
    (nanami_runtime::OS_RESPONSE_OK, done as Word, events)
}

fn handle_get_size(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    (
        nanami_runtime::OS_RESPONSE_OK,
        runtime.terminals[index].columns,
        runtime.terminals[index].rows,
    )
}

fn handle_set_size(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1 == 0
        || request.arg2 == 0
        || request.arg1 > u16::MAX as Word
        || request.arg2 > u16::MAX as Word
    {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let terminal = &mut runtime.terminals[index];
    terminal.columns = request.arg1;
    terminal.rows = request.arg2;
    (nanami_runtime::OS_RESPONSE_OK, 0, 0)
}

fn handle_clear(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1 & TERMINAL_CLEAR_INPUT != 0 {
        runtime.terminals[index].input.clear();
        runtime.terminals[index].input_edit_len = 0;
    }
    if request.arg1 & TERMINAL_CLEAR_OUTPUT != 0 {
        runtime.terminals[index].output.clear();
    }
    (nanami_runtime::OS_RESPONSE_OK, 0, 0)
}

fn handle_attach_output_notification(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    handle_attach_notification(runtime, request, false)
}

fn handle_attach_input_notification(
    runtime: &mut Runtime,
    request: ServiceRequest,
) -> (Word, Word, Word) {
    handle_attach_notification(runtime, request, true)
}

fn handle_attach_notification(
    runtime: &mut Runtime,
    request: ServiceRequest,
    input: bool,
) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let source_slot = if request.arg1 == 0 {
        nanami_runtime::PROCESS_SLOT_NOTIFICATION
    } else {
        request.arg1
    };
    let destination_slot = if input {
        SLOT_INPUT_NOTIFICATION_BASE + index as Word
    } else {
        SLOT_OUTPUT_NOTIFICATION_BASE + index as Word
    };
    let identifier = if input {
        TERMINAL_NOTIFICATION_INPUT | (request.arg0 & 0xffff_ffff)
    } else {
        TERMINAL_NOTIFICATION_OUTPUT | (request.arg0 & 0xffff_ffff)
    };
    match nanami_runtime::request_notification_port_copy(
        request.identifier,
        source_slot,
        destination_slot,
        identifier,
    ) {
        Ok(()) => {
            let descriptor = nanami_runtime::ipc::process_slot_descriptor(destination_slot);
            if input {
                runtime.terminals[index].input_notification = descriptor;
                if runtime.terminals[index].input.len != 0 {
                    notify_terminal(descriptor);
                }
            } else {
                runtime.terminals[index].output_notification = descriptor;
                if runtime.terminals[index].output.len != 0 {
                    notify_terminal(descriptor);
                }
            }
            (nanami_runtime::OS_RESPONSE_OK, 0, 0)
        }
        Err(e) => (map_request_error(e), 0, 0),
    }
}

fn notify_terminal(notification: Word) {
    if notification != 0 {
        let _ = nanami_runtime::ipc::notification_notify(notification);
    }
}

fn client_for_pid(runtime: &mut Runtime, pid: Word) -> Option<usize> {
    let mut empty = None;
    let mut i = 0usize;
    while i < runtime.clients.len() {
        if runtime.clients[i].active && runtime.clients[i].pid == pid {
            return Some(i);
        }
        if !runtime.clients[i].active && empty.is_none() {
            empty = Some(i);
        }
        i += 1;
    }
    empty
}

fn find_client(runtime: &Runtime, pid: Word) -> Option<ClientSession> {
    let mut i = 0usize;
    while i < runtime.clients.len() {
        let client = runtime.clients[i];
        if client.active && client.pid == pid {
            return Some(client);
        }
        i += 1;
    }
    None
}

fn free_terminal(runtime: &Runtime) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.terminals.len() {
        if !runtime.terminals[i].active {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_terminal(runtime: &Runtime, id: Word) -> Option<usize> {
    let mut i = 0usize;
    while i < runtime.terminals.len() {
        if runtime.terminals[i].active && runtime.terminals[i].id == id {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn map_request_error(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(status) => status,
        RequestError::Transport | RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
    }
}

fn log_error(prefix: &str, error: RequestError) -> nanami_runtime::NanamiError {
    nanami_runtime::print!("{}", prefix);
    match error {
        RequestError::InvalidArgument => nanami_runtime::print!("invalid-arg\n"),
        RequestError::Unsupported => nanami_runtime::print!("unsupported\n"),
        RequestError::Transport => nanami_runtime::print!("transport\n"),
        RequestError::Protocol => nanami_runtime::print!("protocol\n"),
        RequestError::Status(status) => nanami_runtime::println!("status={:#x}", status),
    }
    error.into()
}

nanami_runtime::nanami_entry!(nanami_main);
