use libnanami::{RequestError, Word};

use crate::{
    append_bytes, append_decimal, bytes_eq, copy_bytes, starts_with, COLS, SLOT_EXEC_SERVICE,
};

const EXEC_SHM_BYTES: Word = 0x4000;
const DEFAULT_PRIORITY: Word = 16;
const DEFAULT_PATH: &[u8] = b"/bin:/usr/bin";
const MAX_PATH_BYTES: usize = 256;
const MAX_ARGS: usize = 16;
const MAX_ENVS: usize = 8;
const LAUNCH_OFFSET: usize = 512;
const MAX_OUTPUT_LINES: usize = 4;
const DEFAULT_ENVS: [&[u8]; 4] = [
    b"PATH=/bin:/usr/bin",
    b"HOME=/",
    b"USER=root",
    b"TERM=nanami",
];

pub struct ExecShell {
    connected: bool,
    port: Word,
    shm: Word,
    shm_size: Word,
    path: [u8; MAX_PATH_BYTES],
    path_len: usize,
}

pub struct CommandOutput {
    lines: [[u8; COLS]; MAX_OUTPUT_LINES],
    len: usize,
}

impl CommandOutput {
    pub fn new() -> Self {
        Self {
            lines: [[0; COLS]; MAX_OUTPUT_LINES],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn line(&self, index: usize) -> [u8; COLS] {
        self.lines[index]
    }

    fn push_line(&mut self, line: [u8; COLS]) {
        if self.len >= MAX_OUTPUT_LINES {
            return;
        }
        self.lines[self.len] = line;
        self.len += 1;
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        let mut line = [0u8; COLS];
        copy_bytes(&mut line, bytes);
        self.push_line(line);
    }
}

impl ExecShell {
    pub const fn new() -> Self {
        let mut path = [0u8; MAX_PATH_BYTES];
        let mut i = 0usize;
        while i < DEFAULT_PATH.len() {
            path[i] = DEFAULT_PATH[i];
            i += 1;
        }
        Self {
            connected: false,
            port: 0,
            shm: 0,
            shm_size: 0,
            path,
            path_len: DEFAULT_PATH.len(),
        }
    }

    pub fn execute_builtin(&mut self, command: &[u8]) -> Option<CommandOutput> {
        let command = trim_spaces(command);
        if bytes_eq(command, b"path") {
            let mut out = CommandOutput::new();
            let mut line = [0u8; COLS];
            let pos = append_bytes(&mut line, 0, b"PATH=");
            let _ = append_bytes(&mut line, pos, &self.path[..self.path_len]);
            out.push_line(line);
            return Some(out);
        }
        if starts_with(command, b"path ") {
            return Some(self.set_path(trim_spaces(&command[5..])));
        }
        None
    }

    pub fn service_port(&self) -> Word {
        self.port
    }

    fn set_path(&mut self, value: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if value.is_empty() || value.len() > MAX_PATH_BYTES {
            out.push_bytes(b"path: invalid PATH");
            return out;
        }
        self.path = [0; MAX_PATH_BYTES];
        self.path[..value.len()].copy_from_slice(value);
        self.path_len = value.len();
        out.push_bytes(b"path: ok");
        out
    }

    pub fn spawn_with_terminal(
        &mut self,
        command: &[u8],
        terminal_id: Word,
        out: &mut CommandOutput,
    ) -> Option<(Word, [u8; MAX_PATH_BYTES], usize)> {
        if !self.ensure_connected(out) {
            return None;
        }

        let (name, _args) = split_command(command);
        if name.is_empty() {
            out.push_bytes(b"exec: empty command");
            return None;
        }
        let Some((launch_offset, launch_len)) = self.write_launch_block(command, terminal_id)
        else {
            out.push_bytes(b"exec: argument block too large");
            return None;
        };

        let mut candidates = CandidateIter::new(name, &self.path[..self.path_len]);
        while let Some((path, path_len)) = candidates.next() {
            if path_len > self.shm_size || path_len >= LAUNCH_OFFSET {
                continue;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(path.as_ptr(), self.shm as *mut u8, path_len);
            }
            match nanami_services::exec::exec_spawn_path_arguments(
                self.port,
                0,
                path_len as Word,
                launch_offset as Word,
                launch_len as Word,
            ) {
                Ok(pid) => return Some((pid, path, path_len)),
                Err(RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT)) => {}
                Err(error) => {
                    out.push_line(format_error_line(b"exec: spawn failed ", error));
                    return None;
                }
            }
        }

        let mut line = [0u8; COLS];
        let pos = append_bytes(&mut line, 0, b"command not found: ");
        let _ = append_bytes(&mut line, pos, name);
        out.push_line(line);
        None
    }

    fn ensure_connected(&mut self, out: &mut CommandOutput) -> bool {
        if self.connected {
            return true;
        }
        match nanami_services::registry::connect_exec_service(SLOT_EXEC_SERVICE) {
            Ok(()) => {}
            Err(error) => {
                out.push_line(format_error_line(b"exec: service unavailable ", error));
                return false;
            }
        }
        self.port = libnanami::ipc::process_slot_descriptor(SLOT_EXEC_SERVICE);
        match nanami_services::exec::exec_attach_shared_memory(self.port, EXEC_SHM_BYTES) {
            Ok((shm, shm_size)) => {
                if shm == 0 || shm_size == 0 {
                    out.push_bytes(b"exec: invalid shared memory");
                    return false;
                }
                self.shm = shm;
                self.shm_size = shm_size;
                self.connected = true;
                true
            }
            Err(error) => {
                out.push_line(format_error_line(b"exec: attach failed ", error));
                false
            }
        }
    }

    fn write_launch_block(&self, command: &[u8], terminal_id: Word) -> Option<(usize, usize)> {
        let mut spans = [(0usize, 0usize); MAX_ARGS];
        let argc = split_words(command, &mut spans)?;
        let mut terminal_env = [0u8; 40];
        let mut terminal_env_len = 0usize;
        if terminal_id != 0 {
            terminal_env_len = append_bytes(&mut terminal_env, 0, b"NANAMI_TERMINAL_ID=");
            terminal_env_len = append_decimal(&mut terminal_env, terminal_env_len, terminal_id);
        }
        let envc = DEFAULT_ENVS.len() + if terminal_id != 0 { 1 } else { 0 };
        if envc > MAX_ENVS {
            return None;
        }

        let mut cursor = LAUNCH_OFFSET + 24;
        if cursor > self.shm_size {
            return None;
        }
        write_shm_word(self.shm, LAUNCH_OFFSET, DEFAULT_PRIORITY);
        write_shm_word(self.shm, LAUNCH_OFFSET + 8, argc as Word);
        write_shm_word(self.shm, LAUNCH_OFFSET + 16, envc as Word);

        let mut i = 0usize;
        while i < argc {
            let (start, len) = spans[i];
            cursor = write_shm_string(
                self.shm,
                self.shm_size,
                cursor,
                &command[start..start + len],
            )?;
            i += 1;
        }
        i = 0;
        while i < DEFAULT_ENVS.len() {
            cursor = write_shm_string(self.shm, self.shm_size, cursor, DEFAULT_ENVS[i])?;
            i += 1;
        }
        if terminal_id != 0 {
            cursor = write_shm_string(
                self.shm,
                self.shm_size,
                cursor,
                &terminal_env[..terminal_env_len],
            )?;
        }
        Some((LAUNCH_OFFSET, cursor - LAUNCH_OFFSET))
    }
}

struct CandidateIter<'a> {
    name: &'a [u8],
    path: &'a [u8],
    offset: usize,
    direct_done: bool,
}

impl<'a> CandidateIter<'a> {
    fn new(name: &'a [u8], path: &'a [u8]) -> Self {
        Self {
            name,
            path,
            offset: 0,
            direct_done: false,
        }
    }

    fn next(&mut self) -> Option<([u8; MAX_PATH_BYTES], usize)> {
        if contains_byte(self.name, b'/') {
            if self.direct_done {
                return None;
            }
            self.direct_done = true;
            return build_path_candidate(b"", self.name);
        }

        while self.offset <= self.path.len() {
            let start = self.offset;
            while self.offset < self.path.len() && self.path[self.offset] != b':' {
                self.offset += 1;
            }
            let entry = &self.path[start..self.offset];
            self.offset += 1;
            if entry.is_empty() {
                continue;
            }
            return build_path_candidate(entry, self.name);
        }
        None
    }
}

fn build_path_candidate(prefix: &[u8], name: &[u8]) -> Option<([u8; MAX_PATH_BYTES], usize)> {
    let mut out = [0u8; MAX_PATH_BYTES];
    let mut len = 0usize;
    if !prefix.is_empty() {
        len = append_raw(&mut out, len, prefix)?;
        if prefix[prefix.len() - 1] != b'/' {
            len = append_byte(&mut out, len, b'/')?;
        }
    }
    len = append_raw(&mut out, len, name)?;
    Some((out, len))
}

fn split_command(command: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0usize;
    while i < command.len() && command[i] != b' ' {
        i += 1;
    }
    let name = &command[..i];
    let args = if i < command.len() {
        trim_spaces(&command[i + 1..])
    } else {
        b""
    };
    (name, args)
}

fn split_words(input: &[u8], out: &mut [(usize, usize); MAX_ARGS]) -> Option<usize> {
    let mut count = 0usize;
    let mut cursor = 0usize;
    while cursor < input.len() && count < out.len() {
        while cursor < input.len() && input[cursor] == b' ' {
            cursor += 1;
        }
        let start = cursor;
        while cursor < input.len() && input[cursor] != b' ' {
            cursor += 1;
        }
        if cursor > start {
            out[count] = (start, cursor - start);
            count += 1;
        }
    }
    while cursor < input.len() && input[cursor] == b' ' {
        cursor += 1;
    }
    if count == 0 || cursor < input.len() {
        None
    } else {
        Some(count)
    }
}

fn trim_spaces(mut bytes: &[u8]) -> &[u8] {
    while !bytes.is_empty() && bytes[0] == b' ' {
        bytes = &bytes[1..];
    }
    while !bytes.is_empty() && bytes[bytes.len() - 1] == b' ' {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn append_raw(dst: &mut [u8; MAX_PATH_BYTES], mut len: usize, src: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < src.len() {
        len = append_byte(dst, len, src[i])?;
        i += 1;
    }
    Some(len)
}

fn append_byte(dst: &mut [u8; MAX_PATH_BYTES], len: usize, byte: u8) -> Option<usize> {
    if len >= MAX_PATH_BYTES {
        return None;
    }
    dst[len] = byte;
    Some(len + 1)
}

fn contains_byte(bytes: &[u8], needle: u8) -> bool {
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == needle {
            return true;
        }
        i += 1;
    }
    false
}

fn format_error_line(prefix: &[u8], error: RequestError) -> [u8; COLS] {
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

fn write_shm_string(base: Word, size: Word, offset: usize, bytes: &[u8]) -> Option<usize> {
    let end = offset.checked_add(bytes.len())?.checked_add(1)?;
    if end > size {
        return None;
    }
    unsafe {
        let mut i = 0usize;
        while i < bytes.len() {
            core::ptr::write_volatile((base + offset + i) as *mut u8, bytes[i]);
            i += 1;
        }
        core::ptr::write_volatile((base + offset + bytes.len()) as *mut u8, 0);
    }
    Some(end)
}

fn write_shm_word(base: Word, offset: usize, value: Word) {
    let bytes = value.to_ne_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        unsafe {
            core::ptr::write_volatile((base + offset + i) as *mut u8, bytes[i]);
        }
        i += 1;
    }
}
