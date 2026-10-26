//! Production modern PCI, split queue and common GPU provider with checked DMA.
//! QEMU regression separately exercises the actual emulated device.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_gpu;
extern crate self as nanami_runtime;
pub use nanami_uapi::RequestError;
pub use std::println;
pub type Word = usize;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
#[path = "../../nanami/servers/libs/nanami-gpu/src/lib.rs"]
mod provider;
pub use provider::*;
#[path = "../../nanami/servers/core-services/virtio-gpu-server/src/gpu.rs"]
mod driver;
#[path = "../../nanami/servers/core-services/virtio-gpu-server/src/queue.rs"]
mod queue;
#[path = "../../nanami/servers/core-services/virtio-gpu-server/src/pci.rs"]
mod transport_pci;
#[path = "../../nanami/servers/core-services/virtio-gpu-server/src/wire.rs"]
mod wire;
use queue::Transport;
use std::{
    alloc::{alloc, dealloc, Layout as MemoryLayout},
    cell::RefCell,
    collections::BTreeMap,
};
const PAGE: usize = 4096;

struct Memory {
    physical: usize,
    local: usize,
    bytes: usize,
    refs: usize,
}
impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            dealloc(
                self.local as *mut u8,
                MemoryLayout::from_size_align(self.bytes, PAGE).unwrap(),
            );
        }
    }
}
struct Resource {
    width: usize,
    height: usize,
    backing: Option<(usize, usize)>,
    pixels: Vec<u8>,
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Fault {
    #[default]
    None,
    Stuck,
    Fence,
    Length,
    Descriptor,
    Reset,
}
struct Model {
    regs: [u32; 56],
    cfg: [u8; 256],
    memory: Vec<Memory>,
    next_physical: usize,
    resources: BTreeMap<u32, Resource>,
    active: u32,
    scanout_width: usize,
    scanout_height: usize,
    screen: Vec<u8>,
    used: u16,
    pending: bool,
    fault: Fault,
    high_features: u32,
    reject_features: bool,
    delay: usize,
    commands: Vec<u32>,
    transfers: Vec<Rect>,
    released: Vec<usize>,
    fail_share: bool,
    pci_command: u32,
}
impl Default for Model {
    fn default() -> Self {
        let mut cfg = [0; 256];
        cfg[6] = 1 << 4;
        cfg[0x34] = 0x40;
        for (offset, next, kind, length, region_offset, region_length) in [
            (0x40, 0x50, 1, 16, 0x80, 56),
            (0x50, 0x64, 2, 20, 0x3000, 256),
            (0x64, 0, 4, 16, 0x2000, 16),
        ] {
            cfg[offset..offset + 4].copy_from_slice(&[9, next, length, kind]);
            cfg[offset + 4] = 4;
            wire::put32(&mut cfg, offset + 8, region_offset);
            wire::put32(&mut cfg, offset + 12, region_length);
        }
        wire::put32(&mut cfg, 0x60, 8);
        Self {
            regs: [0; 56],
            cfg,
            memory: Vec::new(),
            next_physical: 0x100000,
            resources: BTreeMap::new(),
            active: 0,
            scanout_width: 0,
            scanout_height: 0,
            screen: Vec::new(),
            used: 0,
            pending: false,
            fault: Fault::None,
            high_features: 1,
            reject_features: false,
            delay: 0,
            commands: Vec::new(),
            transfers: Vec::new(),
            released: Vec::new(),
            fail_share: false,
            pci_command: 0,
        }
    }
}
thread_local! { static MODEL: RefCell<Model> = RefCell::new(Model::default()); }
fn model<T>(f: impl FnOnce(&mut Model) -> T) -> T {
    MODEL.with(|m| f(&mut m.borrow_mut()))
}
unsafe fn load16(address: usize) -> u16 {
    (address as *const u16).read()
}
unsafe fn load32(address: usize) -> u32 {
    (address as *const u32).read()
}
unsafe fn store16(address: usize, value: u16) {
    (address as *mut u16).write(value);
}
unsafe fn store32(address: usize, value: u32) {
    (address as *mut u32).write(value);
}
impl Model {
    fn local(&self, physical: usize, bytes: usize) -> usize {
        let memory = self
            .memory
            .iter()
            .find(|m| {
                physical >= m.physical
                    && physical
                        .checked_add(bytes)
                        .is_some_and(|end| end <= m.physical + m.bytes)
            })
            .expect("device DMA escaped owned allocation");
        memory.local + physical - memory.physical
    }
    fn address(&self, register: usize) -> usize {
        self.regs[register] as usize | ((self.regs[register + 4] as usize) << 32)
    }
    fn execute(&mut self) {
        let available = self.local(self.address(40), 32);
        let size = self.regs[24] as usize;
        assert_eq!(unsafe { load16(available + 2) }, self.used.wrapping_add(1));
        assert_eq!(
            unsafe { load16(available + 4 + self.used as usize % size * 2) },
            0
        );
        let descriptors = self.local(self.address(32), 32);
        let descriptor = unsafe { std::slice::from_raw_parts(descriptors as *const u8, 32) };
        assert_eq!(
            (
                wire::read32(descriptor, 12) & 0xffff,
                wire::read32(descriptor, 12) >> 16
            ),
            (1, 1)
        );
        assert_eq!(wire::read32(descriptor, 28), 2);
        let request_bytes = wire::read32(descriptor, 8) as usize;
        let response_bytes = wire::read32(descriptor, 24) as usize;
        let request_local = self.local(wire::read64(descriptor, 0) as usize, request_bytes);
        let request =
            unsafe { std::slice::from_raw_parts(request_local as *const u8, request_bytes) }
                .to_vec();
        assert_eq!(wire::read32(&request, 4), wire::FENCE);
        assert_eq!(wire::read32(&request, 16), 0);
        let kind = wire::read32(&request, 0);
        self.commands.push(kind);
        let mut response = vec![0; response_bytes];
        wire::put32(&mut response, 0, wire::OK);
        wire::put32(&mut response, 4, wire::FENCE);
        wire::put64(&mut response, 8, wire::read64(&request, 8));
        let rect = || Rect {
            x: wire::read32(&request, 24) as usize,
            y: wire::read32(&request, 28) as usize,
            width: wire::read32(&request, 32) as usize,
            height: wire::read32(&request, 36) as usize,
        };
        match kind {
            wire::GET_DISPLAY_INFO => {
                assert_eq!(request_bytes, 24);
                assert_eq!(response_bytes, wire::DISPLAY_INFO_BYTES);
                wire::put32(&mut response, 0, wire::OK_DISPLAY_INFO);
                let offset = 24 + 2 * 24; // Do not assume scanout zero.
                wire::put32(&mut response, offset + 8, 80);
                wire::put32(&mut response, offset + 12, 40);
                wire::put32(&mut response, offset + 16, 1);
            }
            wire::CREATE_2D => {
                assert_eq!(request_bytes, 40);
                let id = wire::read32(&request, 24);
                let width = wire::read32(&request, 32) as usize;
                let height = wire::read32(&request, 36) as usize;
                assert_eq!(wire::read32(&request, 28), 2);
                assert!(self
                    .resources
                    .insert(
                        id,
                        Resource {
                            width,
                            height,
                            backing: None,
                            pixels: vec![0xcc; width * height * 4]
                        }
                    )
                    .is_none());
            }
            wire::ATTACH => {
                assert_eq!(request_bytes, 48);
                assert_eq!(wire::read32(&request, 28), 1);
                let physical = wire::read64(&request, 32) as usize;
                let bytes = wire::read32(&request, 40) as usize;
                let local = self.local(physical, bytes);
                assert!(
                    unsafe { std::slice::from_raw_parts(local as *const u8, bytes) }
                        .iter()
                        .all(|b| *b == 0)
                );
                let resource = self.resources.get_mut(&wire::read32(&request, 24)).unwrap();
                assert!(bytes >= resource.width * resource.height * 4);
                resource.backing = Some((physical, bytes));
            }
            wire::TRANSFER => {
                assert_eq!(request_bytes, 56);
                let r = rect();
                let id = wire::read32(&request, 48);
                let resource = &self.resources[&id];
                assert!(r.x + r.width <= resource.width && r.y + r.height <= resource.height);
                assert_eq!(
                    wire::read64(&request, 40) as usize,
                    (r.y * resource.width + r.x) * 4
                );
                let local = self.local(resource.backing.unwrap().0, resource.backing.unwrap().1);
                let resource = self.resources.get_mut(&id).unwrap();
                for y in r.y..r.y + r.height {
                    let offset = (y * resource.width + r.x) * 4;
                    let source = unsafe {
                        std::slice::from_raw_parts((local + offset) as *const u8, r.width * 4)
                    };
                    resource.pixels[offset..offset + r.width * 4].copy_from_slice(source);
                }
                self.transfers.push(r);
            }
            wire::SET_SCANOUT => {
                assert_eq!(request_bytes, 48);
                assert_eq!(wire::read32(&request, 40), 2);
                let id = wire::read32(&request, 44);
                let r = rect();
                if id != 0 {
                    let resource = &self.resources[&id];
                    assert!(r.x + r.width <= resource.width && r.y + r.height <= resource.height);
                }
                self.active = id;
                self.scanout_width = r.width;
                self.scanout_height = r.height;
            }
            wire::FLUSH => {
                assert_eq!(request_bytes, 48);
                let id = wire::read32(&request, 40);
                let r = rect();
                let resource = &self.resources[&id];
                assert!(r.x + r.width <= resource.width && r.y + r.height <= resource.height);
                if self.active == id {
                    self.screen.clear();
                    for row in 0..self.scanout_height {
                        let start = row * resource.width * 4;
                        self.screen.extend_from_slice(
                            &resource.pixels[start..start + self.scanout_width * 4],
                        );
                    }
                }
            }
            wire::DETACH => {
                assert_eq!(request_bytes, 32);
                let id = wire::read32(&request, 24);
                assert_ne!(self.active, id);
                self.resources.get_mut(&id).unwrap().backing = None;
            }
            wire::UNREF => {
                assert_eq!(request_bytes, 32);
                let id = wire::read32(&request, 24);
                assert_ne!(self.active, id);
                assert!(self.resources.remove(&id).unwrap().backing.is_none());
            }
            _ => panic!("unsupported virtio command {kind:x}"),
        }
        if self.fault == Fault::Fence {
            wire::put64(&mut response, 8, 0);
        }
        let response_local = self.local(wire::read64(descriptor, 16) as usize, response_bytes);
        unsafe {
            std::ptr::copy_nonoverlapping(
                response.as_ptr(),
                response_local as *mut u8,
                response_bytes,
            );
        }
        let used = self.local(self.address(48), 128);
        let element = used + 4 + self.used as usize % size * 8;
        unsafe {
            store32(
                element,
                if self.fault == Fault::Descriptor {
                    1
                } else {
                    0
                },
            );
            store32(
                element + 4,
                if self.fault == Fault::Length {
                    u32::MAX
                } else {
                    response_bytes as u32
                },
            );
            self.used = self.used.wrapping_add(1);
            store16(used + 2, self.used);
        }
        self.pending = false;
    }
}
struct Hardware;
impl Transport for Hardware {
    fn read(&self, offset: Word, width: Word) -> u32 {
        assert!(matches!(width, 1 | 2 | 4) && offset % width == 0);
        model(|m| match offset {
            4 => m.high_features,
            18 => 2,
            24 => {
                if m.regs[24] == 0 {
                    256
                } else {
                    m.regs[24]
                }
            }
            30 => 3,
            _ => m.regs[offset],
        })
    }
    fn write(&mut self, offset: Word, width: Word, value: u32) {
        assert!(matches!(width, 1 | 2 | 4) && offset % width == 0);
        model(|m| {
            if offset == 20 && value == 0 {
                if m.fault == Fault::Reset {
                    m.regs[20] = 15;
                    return;
                }
                m.pending = false;
                m.regs = [0; 56];
                m.resources.clear();
                m.active = 0;
                m.used = 0;
            } else if offset == 20 && value == 11 && m.reject_features {
                m.regs[20] = 3;
            } else {
                m.regs[offset] = value;
            }
        });
    }
    fn notify_offset(&self, queue_offset: u32) -> Result<Word, RequestError> {
        assert_eq!(queue_offset, 3);
        Ok(24)
    }
    fn notify(&mut self, offset: Word) -> Result<(), RequestError> {
        assert_eq!(offset, 24);
        model(|m| {
            assert!(!m.pending);
            m.pending = true;
            if m.delay == 0 && m.fault != Fault::Stuck {
                m.execute();
            }
        });
        Ok(())
    }
}
pub fn request_dma(bytes: Word) -> Result<(Word, Word), RequestError> {
    model(|m| {
        let local = unsafe { alloc(MemoryLayout::from_size_align(bytes, PAGE).unwrap()) } as usize;
        assert_ne!(local, 0);
        unsafe {
            std::ptr::write_bytes(local as *mut u8, 0xa5, bytes);
        }
        let physical = m.next_physical;
        m.next_physical += bytes + PAGE;
        m.memory.push(Memory {
            physical,
            local,
            bytes,
            refs: 1,
        });
        Ok((physical, local))
    })
}
pub fn request_shared_memory(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    panic!("DMA backing must be shared without a copy");
}
pub fn request_mapping_share(_: Word, local: Word, bytes: Word) -> Result<Word, RequestError> {
    model(|m| {
        if m.fail_share {
            return Err(RequestError::Transport);
        }
        let memory = m
            .memory
            .iter_mut()
            .find(|memory| memory.local == local)
            .unwrap();
        assert_eq!(memory.bytes, bytes);
        memory.refs += 1;
        Ok(local)
    })
}
pub fn request_mapping_release(local: Word, bytes: Word) -> Result<(), RequestError> {
    model(|m| {
        assert!(!m.pending, "released backing while DMA remains pending");
        let index = m
            .memory
            .iter()
            .position(|memory| memory.local == local)
            .unwrap();
        let physical = m.memory[index].physical;
        assert!(!m.resources.values().any(|resource| resource
            .backing
            .is_some_and(|backing| backing.0 == physical)));
        assert_eq!(m.memory[index].bytes, bytes);
        m.memory[index].refs -= 1;
        if m.memory[index].refs == 0 {
            m.memory.remove(index);
        }
        m.released.push(local);
        Ok(())
    })
}
pub fn request_process_alive(_: Word) -> Result<bool, RequestError> {
    Ok(true)
}
pub fn request_mmio(physical: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    assert_eq!(physical & 4095, 0);
    model(|m| Ok((physical, m.local(physical, bytes))))
}
pub mod timer {
    pub fn timer_service_sleep_milliseconds(
        _: usize,
        ms: usize,
    ) -> Result<(), super::RequestError> {
        assert_eq!(ms, 1);
        super::model(|m| {
            if m.pending && m.fault != super::Fault::Stuck && m.fault != super::Fault::Reset {
                m.delay = m.delay.saturating_sub(1);
                if m.delay == 0 {
                    m.execute();
                }
            }
        });
        Ok(())
    }
}
pub mod pci {
    use super::*;
    pub use nanami_uapi::pci::*;
    pub fn read(_: Word, _: Address, offset: Word, width: Word) -> Result<u32, RequestError> {
        model(|m| {
            if offset == 4 && width == 2 {
                return Ok(m.pci_command);
            }
            assert!(offset + width <= 256);
            let mut word = [0; 4];
            word[..width].copy_from_slice(&m.cfg[offset..offset + width]);
            Ok(u32::from_le_bytes(word))
        })
    }
    pub fn write(
        _: Word,
        _: Address,
        offset: Word,
        width: Word,
        value: u32,
    ) -> Result<(), RequestError> {
        assert_eq!((offset, width), (4, 2));
        model(|m| m.pci_command = value);
        Ok(())
    }
    pub fn probe_bar(_: Word, _: Address, index: Word) -> Result<Bar, RequestError> {
        assert_eq!(index, 4);
        Ok(Bar {
            physical: 0xd0000000,
            bytes: 65536,
            io: false,
            prefetchable: true,
        })
    }
    pub fn disable_message_interrupts(_: Word, _: Address) -> Result<(), RequestError> {
        Ok(())
    }
}

fn reset() {
    model(|m| *m = Model::default());
}
fn queue() -> queue::Queue<Hardware> {
    let mut queue = queue::Queue::new(Hardware, 24);
    queue.initialize().unwrap();
    queue
}
fn display() -> driver::Display<Hardware> {
    reset();
    let mut queue = queue();
    let reply = queue
        .execute(
            &wire::Command::display_info(),
            wire::OK_DISPLAY_INFO,
            wire::DISPLAY_INFO_BYTES,
        )
        .unwrap();
    let (scanout, layout) = wire::display_info(reply).unwrap();
    assert_eq!(scanout, 2);
    assert_eq!(layout.stride, 320);
    driver::Display::new(queue, scanout, layout)
}
fn full(layout: Layout) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: layout.width,
        height: layout.height,
    }
}

#[test]
fn common_gpu_protocol_draws_transfers_and_destroys_with_fenced_dma_lifetime() {
    let mut display = display();
    // Exercise padded rows using a non-multiple-of-16 visible width.
    display.layout = Layout::new(79, 40, 0).unwrap();
    let mut service = Service::default();
    let (id, local) = service
        .request(42, CREATE, [79 | (40 << 32), 320, 1, 0], &mut display)
        .unwrap();
    assert_eq!(
        service.request(42, GET_INFO, [0; 4], &mut display).unwrap(),
        Info {
            supported: CAP_ALL,
            accelerated: 0
        }
        .words()
    );
    let r = Rect {
        x: 3,
        y: 5,
        width: 7,
        height: 9,
    };
    service
        .request(
            42,
            FILL,
            [id, r.position(), r.size(), 0x123456],
            &mut display,
        )
        .unwrap();
    service
        .request(42, PRESENT, [id, r.position(), r.size(), 0], &mut display)
        .unwrap();
    model(|m| {
        assert_eq!(m.active, 1);
        assert_eq!(m.transfers[0], full(display.layout));
        assert_eq!(m.screen.len(), 79 * 40 * 4);
        for y in 0..40 {
            for x in 0..79 {
                let pixel = wire::read32(&m.screen, (y * 79 + x) * 4);
                let expected = if (3..10).contains(&x) && (5..14).contains(&y) {
                    0x123456
                } else {
                    0
                };
                assert_eq!(pixel, expected);
            }
        }
    });
    let r = Rect {
        x: 10,
        y: 12,
        width: 4,
        height: 3,
    };
    service
        .request(42, PRESENT, [id, r.position(), r.size(), 0], &mut display)
        .unwrap();
    assert_eq!(model(|m| *m.transfers.last().unwrap()), r);
    service
        .request(42, DESTROY, [id, 0, 0, 0], &mut display)
        .unwrap();
    model(|m| {
        assert!(m.resources.is_empty());
        assert_eq!(m.active, 0);
    });
    request_mapping_release(local, display.layout.bytes).unwrap();
    display.queue.reset().unwrap();
    assert_eq!(model(|m| m.memory.len()), 0);
}

#[test]
fn cpu_copy_and_surface_switching_preserve_pixels_and_display_owner() {
    let mut display = display();
    let mut service = Service::default();
    let geometry = display.layout.width | (display.layout.height << 32);
    let (source, source_local) = service
        .request(
            42,
            CREATE,
            [geometry, display.layout.stride, 1, 0],
            &mut display,
        )
        .unwrap();
    let (target, target_local) = service
        .request(
            42,
            CREATE,
            [geometry, display.layout.stride, 1, 0],
            &mut display,
        )
        .unwrap();
    let rect = Rect {
        x: 1,
        y: 2,
        width: 4,
        height: 3,
    };
    let before = model(|m| m.commands.len());
    service
        .request(
            42,
            FILL,
            [source, rect.position(), rect.size(), 0xabcdef],
            &mut display,
        )
        .unwrap();
    service
        .request(
            42,
            COPY,
            [
                source,
                target,
                1 | (2 << 16) | (10 << 32) | (12 << 48),
                rect.size(),
            ],
            &mut display,
        )
        .unwrap();
    assert_eq!(model(|m| m.commands.len()), before); // No invented 2D GPU blit command.
    let full = full(display.layout);
    service
        .request(
            42,
            PRESENT,
            [target, full.position(), full.size(), 0],
            &mut display,
        )
        .unwrap();
    assert_eq!(
        model(|m| wire::read32(&m.screen, (12 * 80 + 10) * 4)),
        0xabcdef
    );
    service
        .request(
            42,
            PRESENT,
            [source, rect.position(), rect.size(), 0],
            &mut display,
        )
        .unwrap();
    model(|m| {
        assert_eq!(*m.transfers.last().unwrap(), full);
        assert_eq!(wire::read32(&m.screen, (2 * 80 + 1) * 4), 0xabcdef);
        assert_eq!(wire::read32(&m.screen, (12 * 80 + 10) * 4), 0);
    });
    assert_eq!(
        service.request(
            43,
            PRESENT,
            [source, full.position(), full.size(), 0],
            &mut display
        ),
        Err(RequestError::Status(2))
    );
    assert!(display.legacy_buffer(43).is_err());
    for (id, local) in [(source, source_local), (target, target_local)] {
        service
            .request(42, DESTROY, [id, 0, 0, 0], &mut display)
            .unwrap();
        request_mapping_release(local, display.layout.bytes).unwrap();
    }
    display.queue.reset().unwrap();
}

#[test]
fn legacy_display_protocol_shares_one_resource_and_presents_without_a_new_client() {
    let mut display = display();
    let (local, bytes) = display.legacy_buffer(42).unwrap();
    assert_eq!(display.legacy_buffer(42).unwrap(), (local, bytes));
    assert_eq!(model(|m| m.resources.len()), 1);
    unsafe {
        (local as *mut u32).write(0x123456);
    }
    let rect = full(display.layout);
    display
        .legacy_present(42, rect.position(), rect.size())
        .unwrap();
    assert_eq!(model(|m| wire::read32(&m.screen, 0)), 0x123456);
    assert!(display
        .legacy_present(43, rect.position(), rect.size())
        .is_err());
    assert!(display.legacy_present(42, rect.position(), 0).is_err());
}

#[test]
fn queue_wrap_and_delayed_responses_do_not_reuse_an_in_flight_descriptor() {
    reset();
    let mut queue = queue();
    for _ in 0..65540 {
        // Both split-ring indices cross u16::MAX.
        model(|m| m.delay = 1);
        let reply = queue
            .execute(
                &wire::Command::display_info(),
                wire::OK_DISPLAY_INFO,
                wire::DISPLAY_INFO_BYTES,
            )
            .unwrap();
        assert!(wire::display_info(reply).is_ok());
    }
    queue.reset().unwrap();
}

#[test]
fn invalid_completions_and_timeouts_pin_dma_until_reset_acknowledgement() {
    for fault in [Fault::Stuck, Fault::Fence, Fault::Length, Fault::Descriptor] {
        reset();
        let mut queue = queue();
        model(|m| m.fault = fault);
        assert!(queue
            .execute(
                &wire::Command::display_info(),
                wire::OK_DISPLAY_INFO,
                wire::DISPLAY_INFO_BYTES
            )
            .is_err());
        assert!(queue.failed);
        assert!(model(|m| m.released.is_empty()));
        assert!(queue
            .execute(
                &wire::Command::display_info(),
                wire::OK_DISPLAY_INFO,
                wire::DISPLAY_INFO_BYTES
            )
            .is_err());
        model(|m| m.fault = Fault::Reset);
        assert!(queue.reset().is_err());
        assert!(model(|m| m.released.is_empty()));
        model(|m| m.fault = Fault::None);
        queue.reset().unwrap();
        assert_eq!(model(|m| m.memory.len()), 0);
    }
}

#[test]
fn failed_transfer_prevents_backing_release_and_new_dma_allocation() {
    let mut display = display();
    let allocation = display.allocate(42, display.layout).unwrap();
    let buffer = Buffer {
        owner: 42,
        id: 1,
        layout: display.layout,
        allocation,
    };
    model(|m| m.fault = Fault::Stuck);
    assert!(display.present(buffer, full(buffer.layout)).is_err());
    assert!(display.release(buffer).is_err());
    let allocated = model(|m| m.memory.len());
    assert!(display.allocate(42, buffer.layout).is_err());
    assert_eq!(model(|m| m.memory.len()), allocated);
    assert!(model(|m| m.released.is_empty()));
}

#[test]
fn rejected_features_and_iommu_requirement_are_not_acknowledged_as_ready() {
    for high in [0, 3] {
        reset();
        model(|m| m.high_features = high);
        let mut queue = queue::Queue::new(Hardware, 24);
        assert!(queue.initialize().is_err());
        assert!(model(|m| m.memory.is_empty()));
    }
    reset();
    model(|m| m.reject_features = true);
    let mut queue = queue::Queue::new(Hardware, 24);
    assert!(queue.initialize().is_err());
    assert!(model(|m| m.memory.is_empty()));
}

#[test]
fn declining_an_incompatible_firmware_owned_device_does_not_reset_its_scanout() {
    reset();
    model(|m| {
        m.regs[20] = 15;
        m.regs[0] = 0;
        m.high_features = 3;
        m.active = 0x1234;
    });
    let mut queue = queue::Queue::new(Hardware, 24);
    assert!(queue.initialize().is_err());
    queue.reset().unwrap(); // Fallback cleanup must leave unclaimed hardware alone.
    model(|m| {
        assert_eq!(m.regs[20], 15);
        assert_eq!(m.regs[0], 0);
        assert_eq!(m.active, 0x1234);
        assert!(m.memory.is_empty());
    });
}

#[test]
fn failed_peer_mapping_detaches_and_unrefs_before_freeing_dma() {
    let mut display = display();
    model(|m| m.fail_share = true);
    assert!(display.allocate(42, display.layout).is_err());
    model(|m| {
        assert!(m.resources.is_empty());
        assert_eq!(
            &m.commands[m.commands.len() - 2..],
            &[wire::DETACH, wire::UNREF]
        );
        assert_eq!(m.released.len(), 1);
    });
}

#[test]
fn pci_capability_validation_and_notification_bounds_use_advertised_offsets() {
    reset();
    let address = pci::Address::new(0, 2, 0);
    let caps = transport_pci::capabilities(1, address).unwrap();
    assert_eq!(
        (caps.common.offset, caps.notify.offset, caps.multiplier),
        (0x80, 0x3000, 8)
    );
    // Page-aligned MMIO request plus an intra-page common-config offset.
    let (_, mmio) = request_dma(65536).unwrap();
    model(|m| m.memory.last_mut().unwrap().physical = 0xd0000000);
    unsafe {
        store32(mmio + 0x2008, 1);
    }
    let device = pci::Device {
        address,
        vendor_id: 0x1af4,
        device_id: 0x1050,
        class: 0x030000,
        header: 0,
    };
    let transport = transport_pci::Pci::new(1, device).unwrap();
    assert_eq!(transport.notify_offset(3).unwrap(), 24);
    assert!(transport.notify_offset(32).is_err());
    assert!(model(|m| m.pci_command & (6 | (1 << 10)) == 6 | (1 << 10)));
    assert!(!transport_pci::matches(pci::Device {
        device_id: 0x1000,
        ..device
    }));
    for invalid in [0x41, 0x3c, 0xff] {
        model(|m| m.cfg[0x34] = invalid);
        assert!(transport_pci::capabilities(1, address).is_err());
    }
    model(|m| {
        m.cfg[0x34] = 0x40;
        m.cfg[0x65] = 0x40;
    });
    assert!(transport_pci::capabilities(1, address).is_err());
}
