//! Software provider of the same GPU protocol. No hardware acceleration claims.
use super::*;
use nanami_gpu::{Backend, Buffer, Rect};
pub struct Adapter<'a>(pub &'a mut DisplayState);
impl Backend for Adapter<'_> {
    fn owner_alive(&mut self, owner: Word) -> Result<bool, RequestError> {
        nanami_runtime::request_process_alive(owner)
    }
    fn supported(&self) -> Word {
        let screen = self.0.screen;
        let mut caps = nanami_gpu::CAP_FILL | nanami_gpu::CAP_COPY;
        if (
            screen.red_position,
            screen.red_size,
            screen.green_position,
            screen.green_size,
            screen.blue_position,
            screen.blue_size,
        ) == (16, 8, 8, 8, 0, 8)
        {
            caps |= nanami_gpu::CAP_PRESENT;
        }
        caps
    }
    fn present(&mut self, source: Buffer, rect: Rect) -> Result<(), RequestError> {
        let display = &mut *self.0;
        if source.layout.width != display.screen.width
            || source.layout.height != display.screen.height
            || source.layout.stride != display.screen.stride
        {
            return Err(RequestError::InvalidArgument);
        }
        let previous = display.shared;
        if previous.owner_pid != 0 && previous.owner_pid != source.owner {
            return Err(RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
            ));
        }
        display.shared = SharedFramebuffer {
            owner_pid: source.owner,
            local_vaddr: source.allocation.local,
            peer_vaddr: source.allocation.peer,
            bytes: source.layout.bytes,
        };
        let result =
            present_shared_framebuffer(display, source.owner, rect.position(), rect.size());
        display.shared = SharedFramebuffer {
            owner_pid: source.owner,
            ..previous
        };
        result.map(|_| ())
    }
}
