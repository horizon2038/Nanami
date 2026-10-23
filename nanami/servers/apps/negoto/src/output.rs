use nanami_runtime::{ipc, Word};

pub struct Output {
    terminal: Option<(Word, Word, Word, Word)>,
}
impl Output {
    pub fn new() -> Self {
        let terminal = (|| {
            let id =
                core::str::from_utf8(nanami_runtime::process_env_value(b"NANAMI_TERMINAL_ID")?)
                    .ok()?
                    .parse::<Word>()
                    .ok()?;
            nanami_client::registry::connect_terminal_service(27).ok()?;
            let port = ipc::process_slot_descriptor(27);
            let (shm, bytes) =
                nanami_client::terminal::terminal_attach_shared_memory(port, 4096).ok()?;
            Some((id, port, shm, bytes))
        })();
        Self { terminal }
    }
    pub fn terminal_id(&self) -> Word {
        self.terminal.map_or(0, |terminal| terminal.0)
    }
    pub fn write(&mut self, bytes: &[u8]) {
        if let Some((id, port, shm, capacity)) = self.terminal {
            let mut done = 0;
            while done < bytes.len() {
                let count = (bytes.len() - done).min(capacity);
                if count == 0 {
                    break;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes[done..].as_ptr(), shm as *mut u8, count);
                }
                match nanami_client::terminal::terminal_write_output(port, id, 0, count) {
                    Ok(written) if written != 0 && written <= count => done += written,
                    _ => break,
                }
            }
            if done == bytes.len() {
                return;
            }
        }
        if let Ok(text) = core::str::from_utf8(bytes) {
            nanami_runtime::print!("{}", text);
        }
    }
}
