use libnanami::{RequestError, Word};

use crate::ansi_escape::{AnsiAction, AnsiTerminal};
use crate::{append_bytes, append_decimal, copy_bytes, COLS, DEFAULT_TEXT_COLOR};

const SLOT_TERMINAL_SERVICE: Word = 29;
const SLOT_ALTER_SERVICE: Word = 28;
const SLOT_EXEC_SERVICE: Word = 31;
const TERMINAL_SHM_BYTES: Word = 0x4000;
const ALTER_REQUEST_KILL_TERMINAL: Word = 0xb107;
const TERMINAL_READ_BYTES: usize = 256;
const OUTPUT_LINES: usize = 4;

pub struct ForegroundApp {
    connected: bool,
    terminal_port: Word,
    terminal_shm: Word,
    terminal_id: Word,
    lifecycle_port: Word,
    active_pid: Word,
    terminal: AnsiTerminal,
    pending: [u8; TERMINAL_READ_BYTES],
    pending_pos: usize,
    pending_len: usize,
    output_error_reported: bool,
}

pub struct CommandOutput {
    lines: [[u8; COLS]; OUTPUT_LINES],
    colors: [[u32; COLS]; OUTPUT_LINES],
    partial: [bool; OUTPUT_LINES],
    len: usize,
    clear_screen: bool,
}

impl CommandOutput {
    pub const fn new() -> Self {
        Self {
            lines: [[0; COLS]; OUTPUT_LINES],
            colors: [[DEFAULT_TEXT_COLOR; COLS]; OUTPUT_LINES],
            partial: [false; OUTPUT_LINES],
            len: 0,
            clear_screen: false,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn line(&self, index: usize) -> [u8; COLS] {
        self.lines[index]
    }

    pub fn colors(&self, index: usize) -> [u32; COLS] {
        self.colors[index]
    }

    pub fn is_partial(&self, index: usize) -> bool {
        self.partial[index]
    }

    pub fn clear_screen(&self) -> bool {
        self.clear_screen
    }

    fn remaining(&self) -> usize {
        self.lines.len().saturating_sub(self.len)
    }

    fn request_clear_screen(&mut self) {
        self.clear_screen = true;
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        if self.len >= self.lines.len() {
            return;
        }
        copy_bytes(&mut self.lines[self.len], bytes);
        self.colors[self.len] = [DEFAULT_TEXT_COLOR; COLS];
        self.len += 1;
    }

    fn push_line(&mut self, line: [u8; COLS]) {
        self.push_colored_line(line, [DEFAULT_TEXT_COLOR; COLS]);
    }

    fn push_colored_line(&mut self, line: [u8; COLS], colors: [u32; COLS]) {
        self.push_colored(line, colors, false);
    }

    fn push_partial_colored_line(&mut self, line: [u8; COLS], colors: [u32; COLS]) {
        self.push_colored(line, colors, true);
    }

    fn push_colored(&mut self, line: [u8; COLS], colors: [u32; COLS], partial: bool) {
        if self.len >= self.lines.len() {
            return;
        }
        self.lines[self.len] = line;
        self.colors[self.len] = colors;
        self.partial[self.len] = partial;
        self.len += 1;
    }
}

impl ForegroundApp {
    pub const fn new() -> Self {
        Self {
            connected: false,
            terminal_port: 0,
            terminal_shm: 0,
            terminal_id: 0,
            lifecycle_port: 0,
            active_pid: 0,
            terminal: AnsiTerminal::new(),
            pending: [0; TERMINAL_READ_BYTES],
            pending_pos: 0,
            pending_len: 0,
            output_error_reported: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_pid != 0
    }

    pub fn terminal_id(&self) -> Word {
        self.terminal_id
    }

    pub fn start(&mut self, pid: Word, lifecycle_port: Word) {
        self.active_pid = pid;
        self.lifecycle_port = lifecycle_port;
    }

    pub fn prepare_start(&mut self) {
        self.reset_terminal_session_state();
    }

    pub fn ensure_terminal(&mut self, out: &mut CommandOutput) -> bool {
        if self.connected {
            return true;
        }
        if nanami_services::registry::connect_terminal_service(SLOT_TERMINAL_SERVICE).is_err() {
            out.push_bytes(b"terminal-service unavailable");
            return false;
        }
        self.terminal_port = libnanami::ipc::process_slot_descriptor(SLOT_TERMINAL_SERVICE);
        match nanami_services::terminal::terminal_attach_shared_memory(
            self.terminal_port,
            TERMINAL_SHM_BYTES,
        ) {
            Ok((shm, size)) if shm != 0 && size >= TERMINAL_READ_BYTES as Word => {
                self.terminal_shm = shm;
            }
            Ok(_) => {
                out.push_bytes(b"terminal attach returned invalid shared memory");
                return false;
            }
            Err(error) => {
                out.push_line(format_request_error_line(b"terminal attach failed ", error));
                return false;
            }
        }
        match nanami_services::terminal::terminal_create(self.terminal_port, 80, 24) {
            Ok(id) => {
                self.terminal_id = id;
                if let Err(error) = nanami_services::terminal::terminal_attach_output_notification(
                    self.terminal_port,
                    self.terminal_id,
                    libnanami::PROCESS_SLOT_NOTIFICATION,
                ) {
                    out.push_line(format_request_error_line(
                        b"terminal notification attach failed ",
                        error,
                    ));
                    self.terminal_id = 0;
                    return false;
                }
                self.connected = true;
                true
            }
            Err(error) => {
                out.push_line(format_request_error_line(b"terminal create failed ", error));
                false
            }
        }
    }

    pub fn terminate_active(&mut self) -> CommandOutput {
        let mut out = CommandOutput::new();
        if self.active_pid == 0 {
            out.push_bytes(b"foreground: no active process");
            return out;
        }
        let pid = self.active_pid;
        self.wake_reader();
        self.cleanup_alter_terminal_processes();
        match self.lifecycle_port().and_then(|port| {
            nanami_services::exec::exec_process_kill(port, pid, 1)?;
            nanami_services::exec::exec_process_reap(port, pid)?;
            Ok(())
        }) {
            Ok(()) => {
                self.active_pid = 0;
                self.reset_terminal_session_state();
                let mut line = [0u8; COLS];
                let pos = append_bytes(&mut line, 0, b"terminated pid=");
                let _ = append_decimal(&mut line, pos, pid);
                out.push_line(line);
            }
            Err(error) => out.push_line(format_request_error_line(b"kill failed ", error)),
        }
        out
    }

    pub fn shutdown(&mut self) {
        if self.active_pid == 0 {
            return;
        }
        let pid = self.active_pid;
        self.wake_reader();
        self.cleanup_alter_terminal_processes();
        if let Ok(port) = self.lifecycle_port() {
            let _ = nanami_services::exec::exec_process_kill(port, pid, 1);
            let _ = nanami_services::exec::exec_process_reap(port, pid);
        } else {
            let _ = libnanami::request_process_kill(pid, 1);
            let _ = libnanami::request_process_reap(pid);
        }
        self.active_pid = 0;
        self.reset_terminal_session_state();
    }

    fn cleanup_alter_terminal_processes(&self) {
        if self.terminal_id == 0 {
            return;
        }
        self.cleanup_alter_terminal_processes_for_service("alter-linux");
        self.cleanup_alter_terminal_processes_for_service("alter-freebsd");
    }

    fn cleanup_alter_terminal_processes_for_service(&self, service_name: &str) {
        if libnanami::connect_service_by_name(service_name, SLOT_ALTER_SERVICE).is_err() {
            return;
        }
        let port = libnanami::ipc::process_slot_descriptor(SLOT_ALTER_SERVICE);
        let _ = libnanami::call_service_port(
            port,
            ALTER_REQUEST_KILL_TERMINAL,
            self.terminal_id,
            1,
            0,
            0,
            3,
        );
    }

    pub fn send_input_byte(&mut self, byte: u8) -> bool {
        if !self.connected || self.terminal_id == 0 {
            return false;
        }
        unsafe {
            core::ptr::write(self.terminal_shm as *mut u8, byte);
        }
        matches!(
            nanami_services::terminal::terminal_write_input(
                self.terminal_port,
                self.terminal_id,
                0,
                1,
            ),
            Ok(1)
        )
    }

    pub fn drain_output(&mut self) -> Option<CommandOutput> {
        if !self.connected || self.terminal_id == 0 {
            return None;
        }
        let mut out = CommandOutput::new();
        self.drain_terminal_output_into(&mut out);
        if out.len() == 0 && !out.clear_screen() {
            None
        } else {
            Some(out)
        }
    }

    pub fn poll_status(&mut self) -> Option<CommandOutput> {
        let mut out = CommandOutput::new();
        self.poll_status_into(&mut out);
        if out.len() == 0 && !out.clear_screen() {
            None
        } else {
            Some(out)
        }
    }

    fn drain_terminal_output_into(&mut self, out: &mut CommandOutput) -> bool {
        let mut dirty_line = false;
        let mut changed = false;
        let mut exhausted = false;
        while out.remaining() > 1 {
            if self.pending_pos >= self.pending_len {
                match self.refill_pending() {
                    Ok(true) => {}
                    Ok(false) => {
                        exhausted = true;
                        break;
                    }
                    Err(()) => {
                        if !self.output_error_reported && out.remaining() != 0 {
                            out.push_bytes(b"[terminal output read failed]");
                            self.output_error_reported = true;
                        }
                        exhausted = true;
                        break;
                    }
                }
            }
            let byte = self.pending[self.pending_pos];
            self.pending_pos += 1;
            changed = true;
            match self.terminal.process_byte(byte) {
                AnsiAction::None => {}
                AnsiAction::DirtyLine => {
                    dirty_line = true;
                }
                AnsiAction::FlushLine => {
                    self.flush_terminal_line(out);
                    dirty_line = false;
                }
                AnsiAction::FlushLineAndRetry => {
                    self.flush_terminal_line(out);
                    dirty_line = matches!(self.terminal.process_byte(byte), AnsiAction::DirtyLine);
                }
                AnsiAction::ClearScreen => {
                    out.request_clear_screen();
                    self.terminal.clear_line();
                    dirty_line = true;
                }
            }
        }
        if changed && (dirty_line || self.terminal.col() != 0) && out.remaining() != 0 {
            out.push_partial_colored_line(self.terminal.line(), self.terminal.colors());
        }
        exhausted
    }

    fn refill_pending(&mut self) -> Result<bool, ()> {
        let bytes = nanami_services::terminal::terminal_read_output(
            self.terminal_port,
            self.terminal_id,
            0,
            TERMINAL_READ_BYTES as Word,
        )
        .map_err(|_| ())? as usize;
        if bytes == 0 {
            self.pending_pos = 0;
            self.pending_len = 0;
            self.output_error_reported = false;
            return Ok(false);
        }
        if bytes > self.pending.len() {
            return Err(());
        }
        let mut i = 0usize;
        while i < bytes {
            self.pending[i] = read_byte(self.terminal_shm, i);
            i += 1;
        }
        self.pending_pos = 0;
        self.pending_len = bytes;
        self.output_error_reported = false;
        Ok(true)
    }

    fn flush_terminal_line(&mut self, out: &mut CommandOutput) {
        out.push_colored_line(self.terminal.line(), self.terminal.colors());
        self.terminal.clear_line();
    }

    fn poll_status_into(&mut self, out: &mut CommandOutput) {
        if self.active_pid == 0 || out.remaining() == 0 {
            return;
        }
        let pid = self.active_pid;
        let lifecycle_port = match self.lifecycle_port() {
            Ok(port) => port,
            Err(_) => return,
        };
        let (exited, exit_status) =
            match nanami_services::exec::exec_process_status(lifecycle_port, pid) {
                Ok(status) => status,
                Err(_) => return,
            };
        if !exited {
            return;
        }
        if !self.drain_terminal_output_into(out) || out.remaining() == 0 {
            return;
        }
        self.active_pid = 0;
        self.reset_output_state();
        if nanami_services::exec::exec_process_reap(lifecycle_port, pid).is_err() {
            let _ = libnanami::request_process_reap(pid);
        }
        let mut line = [0u8; COLS];
        let mut pos = append_bytes(&mut line, 0, b"exited status=");
        pos = append_decimal(&mut line, pos, exit_status);
        let _ = pos;
        out.push_line(line);
    }

    fn lifecycle_port(&self) -> Result<Word, RequestError> {
        if self.lifecycle_port != 0 {
            return Ok(self.lifecycle_port);
        }
        nanami_services::registry::connect_exec_service(SLOT_EXEC_SERVICE)?;
        Ok(libnanami::ipc::process_slot_descriptor(SLOT_EXEC_SERVICE))
    }

    fn reset_terminal_session_state(&mut self) {
        self.reset_output_state();
        if !self.connected || self.terminal_id == 0 {
            return;
        }
        let _ = nanami_services::terminal::terminal_clear(
            self.terminal_port,
            self.terminal_id,
            nanami_services::terminal::TERMINAL_CLEAR_INPUT
                | nanami_services::terminal::TERMINAL_CLEAR_OUTPUT,
        );
        let _ = nanami_services::terminal::terminal_set_echo(
            self.terminal_port,
            self.terminal_id,
            true,
        );
        self.drain_terminal_ring(true);
        self.drain_terminal_ring(false);
    }

    fn reset_output_state(&mut self) {
        self.terminal.reset();
        self.pending_pos = 0;
        self.pending_len = 0;
        self.output_error_reported = false;
    }

    fn drain_terminal_ring(&mut self, input: bool) {
        loop {
            let bytes = if input {
                nanami_services::terminal::terminal_read_input(
                    self.terminal_port,
                    self.terminal_id,
                    0,
                    256,
                )
            } else {
                nanami_services::terminal::terminal_read_output(
                    self.terminal_port,
                    self.terminal_id,
                    0,
                    256,
                )
            };
            match bytes {
                Ok(0) | Err(_) => return,
                Ok(bytes) if bytes < 256 => return,
                Ok(_) => {}
            }
        }
    }

    fn wake_reader(&mut self) {
        if !self.connected || self.terminal_id == 0 {
            return;
        }
        unsafe {
            core::ptr::write(self.terminal_shm as *mut u8, b'\n');
        }
        let _ = nanami_services::terminal::terminal_write_input(
            self.terminal_port,
            self.terminal_id,
            0,
            1,
        );
    }
}

fn read_byte(base: Word, offset: usize) -> u8 {
    unsafe { core::ptr::read((base + offset) as *const u8) }
}

fn format_request_error_line(prefix: &[u8], error: RequestError) -> [u8; COLS] {
    let mut line = [0u8; COLS];
    let mut pos = append_bytes(&mut line, 0, prefix);
    match error {
        RequestError::Status(status) => {
            pos = append_bytes(&mut line, pos, b"status=");
            let _ = append_decimal(&mut line, pos, status);
        }
        RequestError::InvalidArgument => {
            let _ = append_bytes(&mut line, pos, b"invalid-arg");
        }
        RequestError::Unsupported => {
            let _ = append_bytes(&mut line, pos, b"unsupported");
        }
        RequestError::Transport => {
            let _ = append_bytes(&mut line, pos, b"transport");
        }
        RequestError::Protocol => {
            let _ = append_bytes(&mut line, pos, b"protocol");
        }
    }
    line
}
