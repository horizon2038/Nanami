//! Boot-time display selection. Declining a probe must leave firmware scanout
//! intact; an already-active display is never replaced by a second writer.
use nanami_runtime::{RequestError, Word};

#[derive(Default, PartialEq, Eq)]
enum Backend {
    #[default]
    Unavailable,
    Probing,
    Native,
    Framebuffer,
}

#[derive(Default)]
pub struct Selection {
    probe_pid: Word,
    backend: Backend,
}

impl Selection {
    pub fn start(candidate: Option<&str>) -> Self {
        let mut selection = Self::default();
        if let Some(image) = candidate {
            match crate::spawn(image) {
                Ok(pid) => {
                    selection.probe_pid = pid;
                    selection.backend = Backend::Probing;
                    return selection;
                }
                Err(error) => nanami_runtime::println!(
                    "[driver-manager] display driver {} spawn failed: {}; selecting framebuffer",
                    image,
                    error
                ),
            }
        }
        if let Err(error) = selection.fallback() {
            nanami_runtime::println!("[driver-manager] framebuffer unavailable: {}", error);
        }
        selection
    }

    fn fallback(&mut self) -> Result<(), RequestError> {
        crate::start_framebuffer()?;
        self.backend = Backend::Framebuffer;
        Ok(())
    }

    pub fn report(&mut self, caller: Word, ready: Word) -> Result<(), RequestError> {
        if self.probe_pid == 0 || caller != self.probe_pid {
            return Err(RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
            ));
        }
        if ready > 1 || self.backend != Backend::Probing {
            return Err(RequestError::InvalidArgument);
        }
        if ready == 0 {
            self.fallback()?;
        } else {
            self.backend = Backend::Native;
            nanami_runtime::println!("[driver-manager] native display ready pid={}", caller);
        }
        Ok(())
    }
}
