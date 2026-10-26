#![no_std]
//! Validated provider-side GPU protocol, shared by hardware and software backends.
use core::sync::atomic::{fence, Ordering};
use nanami_runtime::{RequestError, Word};
pub use nanami_uapi::gpu::*;
const MAX_BUFFERS: usize = 16;
const MAX_TOTAL_BYTES: Word = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Allocation {
    pub local: Word,
    pub peer: Word,
    pub gpu: Word,
}
#[derive(Clone, Copy, Debug)]
pub struct Buffer {
    pub owner: Word,
    pub id: Word,
    pub layout: Layout,
    pub allocation: Allocation,
}

pub trait Backend {
    fn owner_alive(&mut self, owner: Word) -> Result<bool, RequestError>;
    fn supported(&self) -> Word {
        CAP_ALL
    }
    fn accelerated(&self) -> Word {
        0
    }
    fn allocate(&mut self, owner: Word, layout: Layout) -> Result<Allocation, RequestError> {
        let (local, peer) = nanami_runtime::request_shared_memory(owner, layout.bytes)?;
        Ok(Allocation {
            local,
            peer,
            gpu: 0,
        })
    }
    fn release(&mut self, buffer: Buffer) -> Result<(), RequestError> {
        nanami_runtime::request_mapping_release(buffer.allocation.local, buffer.layout.bytes)
    }
    fn fill(&mut self, destination: Buffer, rect: Rect, color: u32) -> Result<(), RequestError> {
        cpu_fill(destination, rect, color);
        Ok(())
    }
    fn copy(
        &mut self,
        source: Buffer,
        destination: Buffer,
        from: Rect,
        x: Word,
        y: Word,
    ) -> Result<(), RequestError> {
        cpu_copy(source, destination, from, x, y);
        Ok(())
    }
    fn present(&mut self, source: Buffer, damage: Rect) -> Result<(), RequestError>;
}

pub fn cpu_fill(destination: Buffer, rect: Rect, color: u32) {
    fence(Ordering::Acquire);
    for y in rect.y..rect.y + rect.height {
        let address = destination.allocation.local + y * destination.layout.stride + rect.x * 4;
        unsafe {
            core::slice::from_raw_parts_mut(address as *mut u32, rect.width).fill(color);
        }
    }
    fence(Ordering::Release);
}
pub fn cpu_copy(source: Buffer, destination: Buffer, from: Rect, x: Word, y: Word) {
    fence(Ordering::Acquire);
    for row in 0..from.height {
        let source_address =
            source.allocation.local + (from.y + row) * source.layout.stride + from.x * 4;
        let target_address =
            destination.allocation.local + (y + row) * destination.layout.stride + x * 4;
        unsafe {
            core::ptr::copy_nonoverlapping(
                source_address as *const u8,
                target_address as *mut u8,
                from.width * 4,
            );
        }
    }
    fence(Ordering::Release);
}

pub struct Service {
    buffers: [Option<Buffer>; MAX_BUFFERS],
    next_id: Word,
    bytes: Word,
}
impl Default for Service {
    fn default() -> Self {
        Self {
            buffers: [None; MAX_BUFFERS],
            next_id: 1,
            bytes: 0,
        }
    }
}
impl Service {
    fn buffer(&self, owner: Word, id: Word) -> Result<Buffer, RequestError> {
        let buffer = self
            .buffers
            .iter()
            .flatten()
            .find(|buffer| buffer.id == id)
            .ok_or(RequestError::InvalidArgument)?;
        if buffer.owner != owner {
            return Err(RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
            ));
        }
        Ok(*buffer)
    }
    pub fn handles(code: Word) -> bool {
        (GET_INFO..=PRESENT).contains(&code)
    }
    pub fn request(
        &mut self,
        owner: Word,
        code: Word,
        args: [Word; 4],
        backend: &mut impl Backend,
    ) -> Result<(Word, Word), RequestError> {
        if owner == 0 {
            return Err(RequestError::InvalidArgument);
        }
        match code {
            GET_INFO => {
                if args != [0; 4] {
                    return Err(RequestError::InvalidArgument);
                }
                Ok(Info {
                    supported: backend.supported() & CAP_ALL,
                    accelerated: backend.accelerated() & backend.supported() & CAP_ALL,
                }
                .words())
            }
            CREATE => {
                if args[2] != FORMAT_XRGB8888 || args[3] != 0 {
                    return Err(RequestError::Unsupported);
                }
                let layout = Layout::new(args[0] & 0xffff_ffff, args[0] >> 32, args[1])
                    .ok_or(RequestError::InvalidArgument)?;
                if self.buffers.iter().all(Option::is_some)
                    || layout.bytes > MAX_TOTAL_BYTES - self.bytes
                {
                    // Only allocation pressure queries Alpha, never drawing.
                    // Failed liveness/cleanup checks retain backing and quota.
                    for entry in &mut self.buffers {
                        if let Some(buffer) = *entry {
                            if matches!(backend.owner_alive(buffer.owner), Ok(false)) {
                                *entry = None;
                                if backend.release(buffer).is_ok() {
                                    self.bytes -= buffer.layout.bytes;
                                }
                            }
                        }
                    }
                }
                let index = self
                    .buffers
                    .iter()
                    .position(Option::is_none)
                    .ok_or(RequestError::Unsupported)?;
                if layout.bytes > MAX_TOTAL_BYTES - self.bytes || self.next_id == Word::MAX {
                    return Err(RequestError::Unsupported);
                }
                let allocation = backend.allocate(owner, layout)?;
                let id = self.next_id;
                self.next_id += 1;
                self.buffers[index] = Some(Buffer {
                    owner,
                    id,
                    layout,
                    allocation,
                });
                self.bytes += layout.bytes;
                Ok((id, allocation.peer))
            }
            DESTROY => {
                if args[1..] != [0; 3] {
                    return Err(RequestError::InvalidArgument);
                }
                let buffer = self.buffer(owner, args[0])?;
                let index = self
                    .buffers
                    .iter()
                    .position(|entry| entry.is_some_and(|value| value.id == buffer.id))
                    .unwrap();
                self.buffers[index] = None;
                // Retire the handle even if cleanup fails part-way through.
                // Keep its quota charged and backing pinned on uncertainty.
                backend.release(buffer)?;
                self.bytes -= buffer.layout.bytes;
                Ok((0, 0))
            }
            FILL | PRESENT => {
                let capability = if code == FILL { CAP_FILL } else { CAP_PRESENT };
                if backend.supported() & capability == 0 {
                    return Err(RequestError::Unsupported);
                }
                let buffer = self.buffer(owner, args[0])?;
                let rect = Rect::decode(buffer.layout, args[1], args[2])
                    .ok_or(RequestError::InvalidArgument)?;
                if code == FILL {
                    if args[3] > u32::MAX as Word {
                        return Err(RequestError::InvalidArgument);
                    }
                    backend.fill(buffer, rect, args[3] as u32)?;
                } else {
                    if args[3] != 0 {
                        return Err(RequestError::InvalidArgument);
                    }
                    backend.present(buffer, rect)?;
                }
                Ok((0, 0))
            }
            COPY => {
                if backend.supported() & CAP_COPY == 0 {
                    return Err(RequestError::Unsupported);
                }
                let source = self.buffer(owner, args[0])?;
                let destination = self.buffer(owner, args[1])?;
                let coordinates = args[2];
                let from = Rect {
                    x: coordinates & 0xffff,
                    y: (coordinates >> 16) & 0xffff,
                    width: args[3] & 0xffff_ffff,
                    height: args[3] >> 32,
                };
                let x = (coordinates >> 32) & 0xffff;
                let y = coordinates >> 48;
                let to = Rect {
                    x,
                    y,
                    width: from.width,
                    height: from.height,
                };
                if !from.inside(source.layout)
                    || !to.inside(destination.layout)
                    || (source.id == destination.id && from.overlaps(to))
                {
                    return Err(RequestError::InvalidArgument);
                }
                backend.copy(source, destination, from, x, y)?;
                Ok((0, 0))
            }
            _ => Err(RequestError::Unsupported),
        }
    }
}
