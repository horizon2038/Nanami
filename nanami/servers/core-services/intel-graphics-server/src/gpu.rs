use crate::{hardware::Scanout, Shared};
use nanami_gpu::{Allocation, Backend, Buffer, Layout, Rect};
use nanami_runtime::{RequestError, Word};
pub struct Adapter<'a> {
    pub display: &'a mut Scanout,
    pub shared: &'a mut Shared,
}
impl Backend for Adapter<'_> {
    fn owner_alive(&mut self, owner: Word) -> Result<bool, RequestError> {
        nanami_runtime::request_process_alive(owner)
    }
    fn accelerated(&self) -> Word {
        self.display.accelerated()
    }
    fn allocate(&mut self, owner: Word, layout: Layout) -> Result<Allocation, RequestError> {
        self.display.allocate_surface(owner, layout)
    }
    fn release(&mut self, buffer: Buffer) -> Result<(), RequestError> {
        self.display.release_surface(buffer)
    }
    fn fill(&mut self, destination: Buffer, rect: Rect, color: u32) -> Result<(), RequestError> {
        self.display.fill_surface(destination, rect, color)
    }
    fn copy(
        &mut self,
        source: Buffer,
        destination: Buffer,
        from: Rect,
        x: Word,
        y: Word,
    ) -> Result<(), RequestError> {
        self.display.copy_surface(source, destination, from, x, y)
    }
    fn present(&mut self, source: Buffer, rect: Rect) -> Result<(), RequestError> {
        if self.shared.owner != 0 && self.shared.owner != source.owner {
            return Err(RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
            ));
        }
        self.shared.owner = source.owner;
        self.display.present_surface(source, rect)
    }
}
