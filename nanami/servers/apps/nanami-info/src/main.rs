#![no_std]
#![no_main]

use core::fmt::{self, Write};
use libnanami::Word;

const SLOT_TERMINAL_SERVICE: Word = 22;
const OUTPUT_SHM_BYTES: Word = 0x1000;
const OUTPUT_BYTES: usize = 512;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[nanami-info] panic: {}", info);
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;

    let mut text = FixedText::new();
    match libnanami::process_arg(1) {
        Some(b"memory") => write_memory_info(&mut text),
        Some(b"process") => write_process_info(&mut text),
        _ => {
            let _ = text.write_str("usage: nanami-info memory|process\n");
        }
    }
    write_output(text.as_bytes());
    Ok(())
}

fn write_memory_info(text: &mut FixedText) {
    match libnanami::request_nanami_info_memory() {
        Ok(info) => {
            let used = info.used_bytes();
            let usage = percent(used, info.total_bytes);
            let _ = writeln!(text, "Memory");
            let _ = writeln!(text, "  total: {} MiB", to_mib(info.total_bytes));
            let _ = writeln!(text, "  used:  {} MiB ({}%)", to_mib(used), usage);
            let _ = writeln!(text, "  free:  {} MiB", to_mib(info.free_bytes));
        }
        Err(error) => {
            let _ = writeln!(text, "nanami-info: memory request failed: {}", error);
        }
    }
}

fn write_process_info(text: &mut FixedText) {
    match libnanami::request_nanami_info_process() {
        Ok(info) => {
            let _ = writeln!(text, "Processes");
            let _ = writeln!(text, "  running: {}", info.running);
            let _ = writeln!(text, "  exited:  {}", info.exited);
            let _ = writeln!(text, "  total:   {}", info.total());
        }
        Err(error) => {
            let _ = writeln!(text, "nanami-info: process request failed: {}", error);
        }
    }
}

fn write_output(bytes: &[u8]) {
    let Some(terminal_id) = parse_terminal_id() else {
        write_debug(bytes);
        return;
    };
    if nanami_services::registry::connect_terminal_service(SLOT_TERMINAL_SERVICE).is_err() {
        write_debug(bytes);
        return;
    }
    let port = libnanami::ipc::process_slot_descriptor(SLOT_TERMINAL_SERVICE);
    let Ok((shm, size)) =
        nanami_services::terminal::terminal_attach_shared_memory(port, OUTPUT_SHM_BYTES)
    else {
        write_debug(bytes);
        return;
    };
    if shm == 0 || size == 0 {
        write_debug(bytes);
        return;
    }

    let mut offset = 0usize;
    while offset < bytes.len() {
        let count = (bytes.len() - offset).min(size as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes[offset..].as_ptr(), shm as *mut u8, count);
        }
        match nanami_services::terminal::terminal_write_output(port, terminal_id, 0, count as Word)
        {
            Ok(written) if written != 0 => offset += written as usize,
            _ => {
                write_debug(&bytes[offset..]);
                return;
            }
        }
    }
}

fn parse_terminal_id() -> Option<Word> {
    let bytes = libnanami::process_env_value(b"NANAMI_TERMINAL_ID")?;
    if bytes.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for byte in bytes.iter().copied() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as usize)?;
    }
    Some(value as Word)
}

fn write_debug(bytes: &[u8]) {
    for byte in bytes.iter().copied() {
        libnanami::debug::write_char(byte as char);
    }
}

fn to_mib(bytes: Word) -> Word {
    bytes / (1024 * 1024)
}

fn percent(value: Word, total: Word) -> Word {
    if total == 0 {
        return 0;
    }
    value.saturating_mul(100) / total
}

struct FixedText {
    bytes: [u8; OUTPUT_BYTES],
    len: usize,
}

impl FixedText {
    const fn new() -> Self {
        Self {
            bytes: [0; OUTPUT_BYTES],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl Write for FixedText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.bytes.len().saturating_sub(self.len);
        let count = value.len().min(available);
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        if count == value.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

libnanami::nanami_entry!(nanami_main);
