//! Buffered foreground-terminal output, with serial output for non-terminal launches.
use core::fmt;
use nanami_runtime::{RequestError, Word};
const TERMINAL_SLOT: Word = 22;

pub struct Output {
    terminal: Option<(Word, Word, Word, Word)>, // port, ID, shared address, capacity
    bytes: [u8; 512],
    len: usize,
}
impl Output {
    pub fn new() -> Result<Self, RequestError> {
        let mut terminal = None;
        if let Some(value) = nanami_runtime::process_env_value(b"NANAMI_TERMINAL_ID") {
            let mut id = 0usize;
            if value.is_empty() {
                return Err(RequestError::InvalidArgument);
            }
            for byte in value {
                if !byte.is_ascii_digit() {
                    return Err(RequestError::InvalidArgument);
                }
                id = id
                    .checked_mul(10)
                    .and_then(|id| id.checked_add((byte - b'0') as usize))
                    .ok_or(RequestError::InvalidArgument)?;
            }
            nanami_client::registry::connect_terminal_service(TERMINAL_SLOT)?;
            let port = nanami_runtime::ipc::process_slot_descriptor(TERMINAL_SLOT);
            let (shared, size) =
                nanami_client::terminal::terminal_attach_shared_memory(port, 4096)?;
            if shared == 0 || size == 0 {
                return Err(RequestError::Protocol);
            }
            terminal = Some((port, id, shared, size));
        }
        Ok(Self {
            terminal,
            bytes: [0; 512],
            len: 0,
        })
    }

    pub fn flush(&mut self) -> Result<(), RequestError> {
        if let Some((port, id, shared, size)) = self.terminal {
            let mut offset = 0;
            let mut stalled = 0;
            while offset < self.len {
                let count = (self.len - offset).min(size);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.bytes[offset..].as_ptr(),
                        shared as *mut u8,
                        count,
                    );
                }
                let written = nanami_client::terminal::terminal_write_output(port, id, 0, count)?;
                if written > count {
                    return Err(RequestError::Protocol);
                }
                if written == 0 {
                    stalled += 1;
                    if stalled == 4096 {
                        return Err(RequestError::Transport);
                    }
                    nanami_runtime::yield_now();
                } else {
                    offset += written;
                    stalled = 0;
                }
            }
        } else {
            for &byte in &self.bytes[..self.len] {
                nanami_runtime::debug::write_char(byte as char);
            }
        }
        self.len = 0;
        Ok(())
    }
}
impl fmt::Write for Output {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut remaining = text.as_bytes();
        while !remaining.is_empty() {
            if self.len == self.bytes.len() {
                self.flush().map_err(|_| fmt::Error)?;
            }
            let count = remaining.len().min(self.bytes.len() - self.len);
            self.bytes[self.len..self.len + count].copy_from_slice(&remaining[..count]);
            self.len += count;
            remaining = &remaining[count..];
        }
        Ok(())
    }
}
