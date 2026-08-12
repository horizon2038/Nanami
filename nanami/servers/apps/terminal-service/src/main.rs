#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{RequestError, Word};
use nanami_services::terminal::*;

const SLOT_SERVICE_PORT: Word = 20;
const MAX_TERMINALS: usize = 8;
const MAX_CLIENTS: usize = 24;
const RING_BYTES: usize = 4096;
const SLOT_OUTPUT_NOTIFICATION_BASE: Word = 32;
const SLOT_INPUT_NOTIFICATION_BASE: Word = SLOT_OUTPUT_NOTIFICATION_BASE + MAX_TERMINALS as Word;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[terminal-service] panic\n");
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
    input_edit_len: usize,
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
        input_edit_len: 0,
        input: ByteRing::EMPTY,
        output: ByteRing::EMPTY,
    };
}

#[derive(Clone, Copy)]
struct ByteRing {
    buffer: [u8; RING_BYTES],
    read: usize,
    write: usize,
    len: usize,
}

impl ByteRing {
    const EMPTY: Self = Self {
        buffer: [0; RING_BYTES],
        read: 0,
        write: 0,
        len: 0,
    };

    fn push(&mut self, byte: u8) -> bool {
        if self.len >= self.buffer.len() {
            return false;
        }
        self.buffer[self.write] = byte;
        self.write = (self.write + 1) % self.buffer.len();
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buffer[self.read];
        self.read = (self.read + 1) % self.buffer.len();
        self.len -= 1;
        Some(byte)
    }

    fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.len = 0;
    }
}

struct Runtime {
    clients: [ClientSession; MAX_CLIENTS],
    terminals: [TerminalSession; MAX_TERMINALS],
    next_terminal_id: Word,
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[terminal-service] bootstrap\n");
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[terminal-service] ipc tls failed: ", e))?;
    nanami_services::registry::register_terminal_service()
        .map_err(|e| log_error("[terminal-service] register failed: ", e))?;
    libnanami::print!("[terminal-service] service registered: terminal-service\n");

    let mut runtime = Runtime {
        clients: [ClientSession::EMPTY; MAX_CLIENTS],
        terminals: [TerminalSession::EMPTY; MAX_TERMINALS],
        next_terminal_id: 1,
    };
    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = Reply::Drop;

    loop {
        let event = match pending {
            Reply::Send(status, detail0, detail1) => {
                pending = Reply::Drop;
                libnanami::ipc::service_reply_receive_event(
                    service_port,
                    status,
                    detail0,
                    detail1,
                )
            }
            Reply::Drop => libnanami::ipc::service_receive_event(service_port),
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
        TERMINAL_REQUEST_ATTACH_INPUT_NOTIFICATION => handle_attach_input_notification(runtime, request),
        TERMINAL_REQUEST_CLEAR => handle_clear(runtime, request),
        TERMINAL_REQUEST_SET_ECHO => handle_set_echo(runtime, request),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
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
            match libnanami::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match client_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.clients[index] = ClientSession {
                            active: true,
                            pid: request.identifier,
                            shm: local,
                            shm_size: size,
                        };
                        (libnanami::OS_RESPONSE_OK, peer, size)
                    }
                    None => (libnanami::OS_RESPONSE_FATAL, 0, 0),
                },
                Err(e) => (map_request_error(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_create(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = free_terminal(runtime) else {
        return (libnanami::OS_RESPONSE_FATAL, 0, 0);
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
        input_edit_len: 0,
        input: ByteRing::EMPTY,
        output: ByteRing::EMPTY,
    };
    libnanami::println!(
        "[terminal-service] created id={} owner={} size={}x{}",
        id,
        request.identifier,
        columns,
        rows
    );
    (libnanami::OS_RESPONSE_OK, id, 0)
}

fn handle_write(runtime: &mut Runtime, request: ServiceRequest, input: bool) -> (Word, Word, Word) {
    let Some(client) = find_client(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1.saturating_add(request.arg2) > client.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let mut done = 0usize;
    let mut echoed_any = false;
    while done < request.arg2 as usize {
        let byte = unsafe {
            core::ptr::read((client.shm + request.arg1 + done as Word) as *const u8)
        };
        let (ok, echoed) = if input {
            push_input_byte(&mut runtime.terminals[index], byte)
        } else {
            (runtime.terminals[index].output.push(byte), true)
        };
        if !ok {
            break;
        }
        echoed_any |= echoed;
        done += 1;
    }
    if done != 0 {
        if input {
            notify_terminal(runtime.terminals[index].input_notification);
            if echoed_any {
                notify_terminal(runtime.terminals[index].output_notification);
            }
        } else {
            notify_terminal(runtime.terminals[index].output_notification);
        }
    }
    (libnanami::OS_RESPONSE_OK, done as Word, 0)
}

fn handle_set_echo(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    runtime.terminals[index].echo_enabled = request.arg1 != 0;
    if !runtime.terminals[index].echo_enabled {
        runtime.terminals[index].input_edit_len = 0;
    }
    (libnanami::OS_RESPONSE_OK, 0, 0)
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
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1.saturating_add(request.arg2) > client.shm_size {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg2 == 0 {
        let len = if input {
            runtime.terminals[index].input.len
        } else {
            runtime.terminals[index].output.len
        };
        return (libnanami::OS_RESPONSE_OK, len as Word, 0);
    }
    let mut done = 0usize;
    while done < request.arg2 as usize {
        let byte = if input {
            runtime.terminals[index].input.pop()
        } else {
            runtime.terminals[index].output.pop()
        };
        let Some(byte) = byte else {
            break;
        };
        unsafe {
            core::ptr::write((client.shm + request.arg1 + done as Word) as *mut u8, byte);
        }
        done += 1;
    }
    (libnanami::OS_RESPONSE_OK, done as Word, 0)
}

fn handle_get_size(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    (
        libnanami::OS_RESPONSE_OK,
        runtime.terminals[index].columns,
        runtime.terminals[index].rows,
    )
}

fn handle_clear(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = find_terminal(runtime, request.arg0) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if request.arg1 & TERMINAL_CLEAR_INPUT != 0 {
        runtime.terminals[index].input.clear();
        runtime.terminals[index].input_edit_len = 0;
    }
    if request.arg1 & TERMINAL_CLEAR_OUTPUT != 0 {
        runtime.terminals[index].output.clear();
    }
    (libnanami::OS_RESPONSE_OK, 0, 0)
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
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let source_slot = if request.arg1 == 0 {
        libnanami::PROCESS_SLOT_NOTIFICATION
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
    match libnanami::request_notification_port_copy(
        request.identifier,
        source_slot,
        destination_slot,
        identifier,
    ) {
        Ok(()) => {
            let descriptor = libnanami::ipc::process_slot_descriptor(destination_slot);
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
            (libnanami::OS_RESPONSE_OK, 0, 0)
        }
        Err(e) => (map_request_error(e), 0, 0),
    }
}

fn notify_terminal(notification: Word) {
    if notification != 0 {
        let _ = libnanami::ipc::notification_notify(notification);
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
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(status) => status,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    libnanami::print!("{}", prefix);
    match error {
        RequestError::InvalidArgument => libnanami::print!("invalid-arg\n"),
        RequestError::Unsupported => libnanami::print!("unsupported\n"),
        RequestError::Transport => libnanami::print!("transport\n"),
        RequestError::Protocol => libnanami::print!("protocol\n"),
        RequestError::Status(status) => libnanami::println!("status={:#x}", status),
    }
    error.into()
}

libnanami::nanami_entry!(nanami_main);
