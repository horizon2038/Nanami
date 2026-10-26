//! Common GPU service backend. Standard virtio 2D has no fill/copy opcodes;
//! CPU rasterization and fenced virtio scanout deliberately have separate caps.
use crate::{
    queue::{Queue, Transport, PAGE},
    wire::{self, Command},
};
use core::ptr;
use nanami_gpu::{Allocation, Backend, Buffer, Layout, Rect};
use nanami_runtime::{RequestError, Word};

pub struct Display<T> {
    pub queue: Queue<T>,
    pub layout: Layout,
    pub scanout: u32,
    owner: Word,
    active: u32,
    next_resource: u32,
    legacy: Option<Buffer>,
}
impl<T: Transport> Display<T> {
    pub fn new(queue: Queue<T>, scanout: u32, layout: Layout) -> Self {
        Self {
            queue,
            scanout,
            layout,
            owner: 0,
            active: 0,
            next_resource: 1,
            legacy: None,
        }
    }
    fn authorize(&self, owner: Word) -> Result<(), RequestError> {
        if owner == 0 || (self.owner != 0 && self.owner != owner) {
            return Err(RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
            ));
        }
        Ok(())
    }
    fn full(layout: Layout) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: layout.width,
            height: layout.height,
        }
    }
    pub fn legacy_buffer(&mut self, owner: Word) -> Result<(Word, Word), RequestError> {
        self.authorize(owner)?;
        if self.legacy.is_none() {
            let allocation = self.allocate(owner, self.layout)?;
            self.legacy = Some(Buffer {
                owner,
                id: 0,
                layout: self.layout,
                allocation,
            });
            self.owner = owner;
        }
        Ok((self.legacy.unwrap().allocation.peer, self.layout.bytes))
    }
    pub fn legacy_present(
        &mut self,
        owner: Word,
        position: Word,
        size: Word,
    ) -> Result<(Word, Word), RequestError> {
        self.authorize(owner)?;
        let buffer = self.legacy.ok_or(RequestError::InvalidArgument)?;
        let rect =
            Rect::decode(self.layout, position, size).ok_or(RequestError::InvalidArgument)?;
        self.present(buffer, rect)?;
        Ok((rect.width * rect.height, 0))
    }
    fn destroy_resource(&mut self, id: u32) -> Result<(), RequestError> {
        self.queue.ensure_healthy()?;
        if id == self.active {
            self.queue.command(Command::scanout(
                self.scanout,
                0,
                Rect {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                },
            ))?;
            self.active = 0;
        }
        self.queue.command(Command::resource(wire::DETACH, id))?;
        self.queue.command(Command::resource(wire::UNREF, id))
    }
}
impl<T: Transport> Backend for Display<T> {
    fn owner_alive(&mut self, owner: Word) -> Result<bool, RequestError> {
        nanami_runtime::request_process_alive(owner)
    }
    fn allocate(&mut self, owner: Word, layout: Layout) -> Result<Allocation, RequestError> {
        self.queue.ensure_healthy()?;
        let id = self.next_resource;
        self.next_resource = id.checked_add(1).ok_or(RequestError::Unsupported)?;
        let (physical, local) = nanami_runtime::request_dma(layout.bytes)?;
        if local == 0
            || local & (PAGE - 1) != 0
            || physical & (PAGE - 1) != 0
            || physical.checked_add(layout.bytes).is_none()
        {
            return Err(RequestError::Protocol);
        }
        unsafe {
            ptr::write_bytes(local as *mut u8, 0, layout.bytes);
        }
        // Do not release this backing if a command's completion is uncertain.
        self.queue.command(Command::create(id, layout))?;
        self.queue
            .command(Command::attach(id, physical, layout.bytes))?;
        let peer = match nanami_runtime::request_mapping_share(owner, local, layout.bytes) {
            Ok(peer) => peer,
            Err(error) => {
                if self.destroy_resource(id).is_ok() {
                    let _ = nanami_runtime::request_mapping_release(local, layout.bytes);
                }
                return Err(error);
            }
        };
        Ok(Allocation {
            local,
            peer,
            gpu: id as Word,
        })
    }
    fn release(&mut self, buffer: Buffer) -> Result<(), RequestError> {
        self.destroy_resource(buffer.allocation.gpu as u32)?;
        nanami_runtime::request_mapping_release(buffer.allocation.local, buffer.layout.bytes)
    }
    fn fill(&mut self, buffer: Buffer, rect: Rect, color: u32) -> Result<(), RequestError> {
        self.queue.ensure_healthy()?;
        nanami_gpu::cpu_fill(buffer, rect, color);
        Ok(())
    }
    fn copy(
        &mut self,
        source: Buffer,
        destination: Buffer,
        rect: Rect,
        x: Word,
        y: Word,
    ) -> Result<(), RequestError> {
        self.queue.ensure_healthy()?;
        nanami_gpu::cpu_copy(source, destination, rect, x, y);
        Ok(())
    }
    fn present(&mut self, buffer: Buffer, mut damage: Rect) -> Result<(), RequestError> {
        self.authorize(buffer.owner)?;
        if buffer.layout != self.layout {
            return Err(RequestError::InvalidArgument);
        }
        self.owner = buffer.owner;
        let id = buffer.allocation.gpu as u32;
        let changed = self.active != id;
        if changed {
            // Initialize every visible pixel before a new resource is shown.
            damage = Self::full(buffer.layout);
        }
        self.queue
            .command(Command::transfer(id, buffer.layout, damage))?;
        if changed {
            self.queue.command(Command::scanout(
                self.scanout,
                id,
                Self::full(buffer.layout),
            ))?;
            self.active = id;
        }
        self.queue.command(Command::flush(id, damage))
    }
}
