use libnanami::{RequestError, Word};

use crate::{append_bytes, append_decimal, bytes_eq, copy_bytes, starts_with, COLS, SLOT_VFS_SERVICE};

const PATH_MAX: usize = 128;
const MAX_COMPONENTS: usize = 16;
const MAX_OUTPUT_LINES: usize = 18;
const VFS_SHM_BYTES: Word = 0x4000;
const PATH_OFFSET: usize = 0;
const PATH2_OFFSET: usize = 256;
const IO_OFFSET: usize = 512;
const DIR_ENTRIES_PER_READ: Word = 8;
const CAT_CHUNK_BYTES: Word = 384;
const CAT_MAX_BYTES: Word = 2048;

pub struct FileShell {
    cwd: [u8; PATH_MAX],
    cwd_len: usize,
    connected: bool,
    vfs_port: Word,
    shm: Word,
    shm_size: Word,
}

pub struct CommandOutput {
    lines: [[u8; COLS]; MAX_OUTPUT_LINES],
    len: usize,
}

impl CommandOutput {
    pub const fn new() -> Self {
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

impl FileShell {
    pub const fn new() -> Self {
        let mut cwd = [0u8; PATH_MAX];
        cwd[0] = b'/';
        Self {
            cwd,
            cwd_len: 1,
            connected: false,
            vfs_port: 0,
            shm: 0,
            shm_size: 0,
        }
    }

    pub fn cwd_line(&self) -> [u8; COLS] {
        let mut line = [0u8; COLS];
        let pos = append_bytes(&mut line, 0, b"cwd: ");
        let _ = append_bytes(&mut line, pos, &self.cwd[..self.cwd_len]);
        line
    }

    pub fn invalidate_vfs_session(&mut self) {
        self.connected = false;
        self.vfs_port = 0;
        self.shm = 0;
        self.shm_size = 0;
    }

    pub fn execute(&mut self, command: &[u8]) -> Option<CommandOutput> {
        if bytes_eq(command, b"ls") {
            return Some(self.ls(b""));
        }
        if starts_with(command, b"ls ") {
            return Some(self.ls(trim_spaces(&command[3..])));
        }
        if starts_with(command, b"cat ") {
            return Some(self.cat(trim_spaces(&command[4..])));
        }
        if starts_with(command, b"rm ") {
            return Some(self.rm(trim_spaces(&command[3..])));
        }
        if starts_with(command, b"mkdir ") {
            return Some(self.mkdir(trim_spaces(&command[6..])));
        }
        if bytes_eq(command, b"cd") {
            return Some(self.cd(b"/"));
        }
        if starts_with(command, b"cd ") {
            return Some(self.cd(trim_spaces(&command[3..])));
        }
        None
    }

    fn ensure_connected(&mut self, out: &mut CommandOutput) -> bool {
        if self.connected {
            return true;
        }
        let _ = nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE);
        self.vfs_port = libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE);
        match nanami_services::vfs::vfs_attach_shared_memory(self.vfs_port, VFS_SHM_BYTES) {
            Ok((shm, shm_size)) => {
                if shm_size < 0x1000 {
                    out.push_bytes(b"vfs: shared memory too small");
                    return false;
                }
                self.shm = shm;
                self.shm_size = shm_size;
                self.connected = true;
                true
            }
            Err(_) => {
                out.push_bytes(b"vfs: attach shared memory failed");
                false
            }
        }
    }

    fn ls(&mut self, arg: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if !self.ensure_connected(&mut out) {
            return out;
        }
        let Some((path, path_len)) = self.resolve_or_report(arg, &mut out) else {
            return out;
        };
        write_shm_bytes(self.shm, PATH_OFFSET, &path[..path_len]);
        let handle = match nanami_services::vfs::vfs_open(self.vfs_port, PATH_OFFSET as Word, path_len as Word) {
            Ok(handle) => handle,
            Err(_) => {
                out.push_bytes(b"ls: open failed");
                return out;
            }
        };

        let mut index = 0 as Word;
        loop {
            let result = nanami_services::vfs::vfs_read_dir(
                self.vfs_port,
                handle,
                index,
                DIR_ENTRIES_PER_READ,
                IO_OFFSET as Word,
            );
            let (entries, _) = match result {
                Ok(v) => v,
                Err(_) => {
                    out.push_bytes(b"ls: readdir failed");
                    break;
                }
            };
            if entries == 0 {
                break;
            }
            let mut i = 0usize;
            while i < entries as usize {
                out.push_line(format_dirent_name(
                    self.shm,
                    IO_OFFSET + i * nanami_services::vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES,
                ));
                i += 1;
            }
            index += entries;
            if out.len() >= MAX_OUTPUT_LINES || entries < DIR_ENTRIES_PER_READ {
                break;
            }
        }
        let _ = nanami_services::vfs::vfs_close(self.vfs_port, handle);
        out
    }

    fn cat(&mut self, arg: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if arg.is_empty() {
            out.push_bytes(b"usage: cat <path>");
            return out;
        }
        if !self.ensure_connected(&mut out) {
            return out;
        }
        let Some((path, path_len)) = self.resolve_or_report(arg, &mut out) else {
            return out;
        };
        write_shm_bytes(self.shm, PATH_OFFSET, &path[..path_len]);
        let handle = match nanami_services::vfs::vfs_open(self.vfs_port, PATH_OFFSET as Word, path_len as Word) {
            Ok(handle) => handle,
            Err(e) => {
                out.push_line(format_error_line(b"cat: open failed ", e));
                return out;
            }
        };
        let (_, size, kind) = match nanami_services::vfs::vfs_fstat(self.vfs_port, handle) {
            Ok(v) => v,
            Err(e) => {
                out.push_line(format_error_line(b"cat: fstat failed ", e));
                let _ = nanami_services::vfs::vfs_close(self.vfs_port, handle);
                return out;
            }
        };
        if kind != nanami_services::vfs::VFS_FILE_TYPE_REGULAR {
            out.push_bytes(b"cat: not a regular file");
            let _ = nanami_services::vfs::vfs_close(self.vfs_port, handle);
            return out;
        }

        let mut file_offset = 0 as Word;
        let read_limit = size.min(CAT_MAX_BYTES);
        while file_offset < read_limit && out.len() < MAX_OUTPUT_LINES {
            let limit = (read_limit - file_offset).min(CAT_CHUNK_BYTES);
            let bytes = match nanami_services::vfs::vfs_read(
                self.vfs_port,
                handle,
                file_offset,
                limit,
                IO_OFFSET as Word,
            ) {
                Ok(bytes) => bytes,
                Err(e) => {
                    out.push_line(format_error_line(b"cat: read failed ", e));
                    break;
                }
            };
            if bytes == 0 {
                break;
            }
            push_text_lines(&mut out, self.shm, IO_OFFSET, bytes as usize);
            file_offset += bytes;
            if bytes < limit {
                break;
            }
        }
        let _ = nanami_services::vfs::vfs_close(self.vfs_port, handle);
        out
    }

    fn rm(&mut self, arg: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if arg.is_empty() {
            out.push_bytes(b"usage: rm <path>");
            return out;
        }
        if !self.ensure_connected(&mut out) {
            return out;
        }
        let Some((path, path_len)) = self.resolve_or_report(arg, &mut out) else {
            return out;
        };
        write_shm_bytes(self.shm, PATH_OFFSET, &path[..path_len]);
        match nanami_services::vfs::vfs_remove(self.vfs_port, PATH_OFFSET as Word, path_len as Word) {
            Ok(()) => out.push_bytes(b"rm: ok"),
            Err(e) => out.push_line(format_error_line(b"rm: failed ", e)),
        }
        out
    }

    fn mkdir(&mut self, arg: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if arg.is_empty() {
            out.push_bytes(b"usage: mkdir <path>");
            return out;
        }
        if !self.ensure_connected(&mut out) {
            return out;
        }
        let Some((path, path_len)) = self.resolve_or_report(arg, &mut out) else {
            return out;
        };
        write_shm_bytes(self.shm, PATH_OFFSET, &path[..path_len]);
        match nanami_services::vfs::vfs_mkdir(self.vfs_port, PATH_OFFSET as Word, path_len as Word) {
            Ok(_) => out.push_bytes(b"mkdir: ok"),
            Err(_) => out.push_bytes(b"mkdir: failed"),
        }
        out
    }

    fn cd(&mut self, arg: &[u8]) -> CommandOutput {
        let mut out = CommandOutput::new();
        if !self.ensure_connected(&mut out) {
            return out;
        }
        let Some((path, path_len)) = self.resolve_or_report(arg, &mut out) else {
            return out;
        };
        write_shm_bytes(self.shm, PATH_OFFSET, &path[..path_len]);
        match nanami_services::vfs::vfs_stat(self.vfs_port, PATH_OFFSET as Word, path_len as Word) {
            Ok((_, _, kind)) if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY => {
                self.cwd[..path_len].copy_from_slice(&path[..path_len]);
                self.cwd_len = path_len;
                out.push_line(self.cwd_line());
            }
            Ok(_) => out.push_bytes(b"cd: not a directory"),
            Err(_) => out.push_bytes(b"cd: no such directory"),
        }
        out
    }

    fn resolve_or_report(&self, arg: &[u8], out: &mut CommandOutput) -> Option<([u8; PATH_MAX], usize)> {
        match resolve_path(&self.cwd[..self.cwd_len], arg) {
            Some(v) => Some(v),
            None => {
                out.push_bytes(b"path: too long");
                None
            }
        }
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

fn resolve_path(cwd: &[u8], arg: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut raw = [0u8; PATH_MAX];
    let mut raw_len = 0usize;
    let input = if arg.is_empty() { cwd } else { arg };
    if input.first() == Some(&b'/') {
        raw_len = copy_path_part(&mut raw, raw_len, input)?;
    } else {
        raw_len = copy_path_part(&mut raw, raw_len, cwd)?;
        if raw_len != 1 {
            raw_len = push_path_byte(&mut raw, raw_len, b'/')?;
        }
        raw_len = copy_path_part(&mut raw, raw_len, input)?;
    }
    normalize_path(&raw[..raw_len])
}

fn copy_path_part(dst: &mut [u8; PATH_MAX], mut len: usize, src: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < src.len() {
        len = push_path_byte(dst, len, src[i])?;
        i += 1;
    }
    Some(len)
}

fn push_path_byte(dst: &mut [u8; PATH_MAX], len: usize, byte: u8) -> Option<usize> {
    if len >= PATH_MAX {
        return None;
    }
    dst[len] = byte;
    Some(len + 1)
}

fn normalize_path(raw: &[u8]) -> Option<([u8; PATH_MAX], usize)> {
    let mut starts = [0usize; MAX_COMPONENTS];
    let mut lens = [0usize; MAX_COMPONENTS];
    let mut count = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        while i < raw.len() && raw[i] == b'/' {
            i += 1;
        }
        let start = i;
        while i < raw.len() && raw[i] != b'/' {
            i += 1;
        }
        let len = i.saturating_sub(start);
        if len == 0 || (len == 1 && raw[start] == b'.') {
            continue;
        }
        if len == 2 && raw[start] == b'.' && raw[start + 1] == b'.' {
            count = count.saturating_sub(1);
            continue;
        }
        if count >= MAX_COMPONENTS {
            return None;
        }
        starts[count] = start;
        lens[count] = len;
        count += 1;
    }

    let mut out = [0u8; PATH_MAX];
    let mut out_len = 1usize;
    out[0] = b'/';
    let mut component = 0usize;
    while component < count {
        if out_len != 1 {
            out_len = push_path_byte(&mut out, out_len, b'/')?;
        }
        let start = starts[component];
        let len = lens[component];
        let mut j = 0usize;
        while j < len {
            out_len = push_path_byte(&mut out, out_len, raw[start + j])?;
            j += 1;
        }
        component += 1;
    }
    Some((out, out_len))
}

fn push_text_lines(out: &mut CommandOutput, base: Word, offset: usize, len: usize) {
    let mut line = [0u8; COLS];
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < len && out.len() < MAX_OUTPUT_LINES {
        let byte = read_shm_byte(base, offset + i);
        if byte == b'\n' {
            out.push_line(line);
            line = [0u8; COLS];
            pos = 0;
        } else {
            if pos >= COLS {
                out.push_line(line);
                line = [0u8; COLS];
                pos = 0;
            }
            line[pos] = printable(byte);
            pos += 1;
        }
        i += 1;
    }
    if pos != 0 && out.len() < MAX_OUTPUT_LINES {
        out.push_line(line);
    }
}

fn format_dirent_name(base: Word, offset: usize) -> [u8; COLS] {
    let inode = read_shm_word(base, offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET);
    let kind = read_shm_word(base, offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET);
    let name_len = read_shm_word(
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET,
    ) as usize;
    let name_len = name_len.min(nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES);
    let mut line = [0u8; COLS];
    let mut pos = 0usize;
    if kind == nanami_services::vfs::VFS_FILE_TYPE_DIRECTORY {
        pos = append_bytes(&mut line, pos, b"[d] ");
    } else {
        pos = append_bytes(&mut line, pos, b"[f] ");
    }
    pos = append_shm_text(
        &mut line,
        pos,
        base,
        offset + nanami_services::vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET,
        name_len,
    );
    pos = append_bytes(&mut line, pos, b"  #");
    let _ = append_decimal(&mut line, pos, inode);
    line
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

fn read_shm_word(base: Word, offset: usize) -> Word {
    unsafe { core::ptr::read_unaligned((base as usize + offset) as *const Word) }
}

fn read_shm_byte(base: Word, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}

fn write_shm_bytes(base: Word, offset: usize, bytes: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (base as usize + offset) as *mut u8,
            bytes.len(),
        );
    }
}

fn append_shm_text(dst: &mut [u8], mut pos: usize, base: Word, offset: usize, len: usize) -> usize {
    let mut i = 0usize;
    while pos < dst.len() && i < len {
        dst[pos] = printable(read_shm_byte(base, offset + i));
        pos += 1;
        i += 1;
    }
    pos
}

fn printable(byte: u8) -> u8 {
    match byte {
        b'\r' | b'\t' => b' ',
        0x20..=0x7e => byte,
        _ => b'.',
    }
}
