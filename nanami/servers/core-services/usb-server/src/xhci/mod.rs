mod bulk;
mod capabilities;
mod dma;
mod enumerate;
mod events;
mod hid;
mod init;
mod poll;
mod protocol;
pub mod registers;
pub mod ring;
mod storage;
mod transfer;
mod wait;

pub use storage::Disk;

use crate::{
    input::Input,
    usb::{
        descriptors::BootInterface,
        hid::{Keyboard, Mouse},
    },
};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};
use dma::Dma;
use libnanami::{RequestError, Word};
use registers::*;
use ring::{Producer, Trb, IOC};

const MAX_SLOTS: usize = 16;
const MAX_INTERFACES: usize = 4;
const BULK_BASE: usize = (4 + 2 * MAX_INTERFACES) * 4096;
// Two bulk rings, BOT scratch, then one aligned data buffer with alignment slack.
const SLOT_BYTES: usize = BULK_BASE + 0x3000 + 2 * crate::usb::mass_storage::DATA_BYTES - 4096;
const SLOT_BASE: usize = 0x4000;
const _: () = assert!(
    crate::usb::mass_storage::DATA_BYTES == 0x10000
        && nanami_services::block::BLOCK_DEVICE_DEFAULT_SHM_BYTES
            <= crate::usb::mass_storage::DATA_BYTES
);

struct Endpoint {
    dci: u8,
    interface: BootInterface,
    ring: Producer,
    buffer: Dma,
    pending: u64,
    keyboard: Keyboard,
    mouse: Mouse,
    failed: bool,
}
struct Device {
    generation: u64,
    port: u8,
    speed: u8,
    speed_class: u8,
    dma: Dma,
    control: Producer,
    endpoints: Vec<Endpoint>,
    storage: Option<bulk::Storage>,
    vendor: u16,
    product: u16,
}

pub struct Controller {
    op: usize,
    runtime: usize,
    interrupter: usize,
    doorbells: usize,
    pub irq: Option<Word>,
    timer: Word,
    dma: Dma,
    _scratch: Option<Dma>,
    command_ring: Producer,
    events: events::Events,
    interrupts_enabled: bool,
    slots: Vec<Option<Device>>,
    context_bytes: usize,
    ports: usize,
    slot_type: [u8; 256],
    port_major: [u8; 256],
    port_speeds: [u32; 256],
    superspeeds: [u16; 256],
    generation: u64,
    borrowed_slot: Option<u8>,
    borrowed_hid: u32,
    held_events: Vec<Trb>,
    dirty: bool,
    storage_changed: bool,
    running: bool,
}

impl Controller {
    pub fn take_storage_change(&mut self) -> bool {
        core::mem::take(&mut self.storage_changed)
    }

    fn delay(&self, milliseconds: Word) -> Result<(), RequestError> {
        crate::delay(self.timer, milliseconds)
    }
    fn wait_register(
        &self,
        offset: usize,
        mask: u32,
        value: u32,
        limit_ms: usize,
    ) -> Result<(), RequestError> {
        let mut last = 0;
        for _ in 0..limit_ms {
            last = unsafe { read(self.op, offset) };
            if last & mask == value {
                return Ok(());
            }
            self.delay(1)?;
        }
        libnanami::println!(
            "[usb-server] xHCI timeout op+{:#x} value={:#010x} mask={:#010x} expected={:#010x}",
            offset,
            last,
            mask,
            value
        );
        Err(RequestError::Transport)
    }
    fn doorbell(&self, slot: u8, dci: u8) {
        fence(Ordering::SeqCst);
        unsafe { write(self.doorbells, slot as usize * 4, dci as u32) };
    }
    fn event(&mut self) -> Option<Trb> {
        self.events.pop(self.interrupter)
    }
    fn dispatch(&mut self, event: Trb, input: &mut Input) {
        // Synchronous storage/control temporarily borrows a composite device.
        // Preserve its HID completions until the device is back in the table.
        if event.kind() == 32 && self.borrowed_slot == Some(event.slot()) {
            if self.borrowed_hid & (1 << event.endpoint()) == 0 {
                return; // e.g. a bulk Stop Endpoint event during BOT recovery.
            }
            if self.held_events.len() < MAX_INTERFACES {
                self.held_events.push(event);
            } else {
                self.fail(input);
            }
            return;
        }
        match event.kind() {
            34 => self.dirty = true,
            32 => {
                let slot = event.slot() as usize;
                let Some(Some(device)) = self.slots.get_mut(slot) else {
                    return;
                };
                let Some(endpoint) = device
                    .endpoints
                    .iter_mut()
                    .find(|ep| ep.dci == event.endpoint())
                else {
                    return;
                };
                if endpoint.failed || event.parameter & !15 != endpoint.pending {
                    return;
                }
                endpoint.ring.complete();
                if !matches!(event.code(), 1 | 13) {
                    endpoint.failed = true;
                    endpoint.keyboard.release(|e| input.emit(e));
                    endpoint.mouse.release(|e| input.emit(e));
                    libnanami::println!(
                        "[usb-server] HID transfer failed slot={} ep={} code={}",
                        slot,
                        endpoint.dci,
                        event.code()
                    );
                    return;
                }
                let residual = (event.status & 0x00ff_ffff) as usize;
                let requested = endpoint.interface.packet_size as usize;
                if residual > requested {
                    endpoint.failed = true;
                    endpoint.keyboard.release(|e| input.emit(e));
                    endpoint.mouse.release(|e| input.emit(e));
                    return;
                }
                let report = unsafe {
                    core::slice::from_raw_parts(
                        endpoint.buffer.virtual_address as *const u8,
                        requested - residual,
                    )
                };
                if endpoint.interface.protocol == 1 {
                    endpoint.keyboard.report(report, |e| input.emit(e));
                } else {
                    endpoint.mouse.report(report, |e| input.emit(e));
                }
                if let Some(pointer) = endpoint.ring.submit(&[Trb {
                    parameter: endpoint.buffer.physical,
                    status: requested as u32,
                    control: (1 << 10) | IOC | (1 << 2),
                }]) {
                    endpoint.pending = pointer;
                    let dci = endpoint.dci;
                    self.doorbell(slot as u8, dci);
                }
            }
            37 => {
                libnanami::println!("[usb-server] host controller event code={}", event.code());
                self.fail(input);
            }
            _ => {}
        }
    }
    fn borrow_device(&mut self, slot: u8) -> Option<Device> {
        let device = self.slots.get_mut(slot as usize)?.take()?;
        self.borrowed_slot = Some(slot);
        self.borrowed_hid = device
            .endpoints
            .iter()
            .fold(0, |mask, endpoint| mask | (1 << endpoint.dci));
        Some(device)
    }

    fn return_device(&mut self, slot: u8, mut device: Device, input: &mut Input) {
        if !self.running {
            for endpoint in &mut device.endpoints {
                endpoint.keyboard.release(|event| input.emit(event));
                endpoint.mouse.release(|event| input.emit(event));
            }
        }
        self.slots[slot as usize] = Some(device);
        self.borrowed_slot = None;
        while !self.held_events.is_empty() {
            let event = self.held_events.remove(0);
            if self.running {
                self.dispatch(event, input);
            }
        }
    }
    fn fail(&mut self, input: &mut Input) {
        if !self.running {
            return;
        }
        self.storage_changed = true;
        self.running = false;
        let (command, status, iman) = unsafe {
            (
                read(self.op, USBCMD),
                read(self.op, USBSTS),
                read(self.interrupter, 0),
            )
        };
        let erdp = unsafe {
            u64::from(read(self.interrupter, 0x18)) | u64::from(read(self.interrupter, 0x1c)) << 32
        };
        // Do not free or reuse DMA memory even if this controller cannot halt.
        unsafe { write(self.op, USBCMD, 0) };
        for device in self.slots.iter_mut().flatten() {
            for endpoint in &mut device.endpoints {
                endpoint.keyboard.release(|event| input.emit(event));
                endpoint.mouse.release(|event| input.emit(event));
            }
        }
        libnanami::println!(
            "[usb-server] controller stopped: USBCMD={:#x} USBSTS={:#x} IMAN={:#x} irq-enabled={}",
            command,
            status,
            iman,
            self.interrupts_enabled
        );
        libnanami::println!(
            "[usb-server] event ring ERDP={:#x} next={:#x}",
            erdp,
            self.events.dequeue()
        );
        self.interrupts_enabled = false;
    }
    fn command(&mut self, trb: Trb, input: &mut Input) -> Result<Trb, RequestError> {
        if !self.running {
            return Err(RequestError::Transport);
        }
        let pointer = self
            .command_ring
            .submit(&[trb])
            .ok_or(RequestError::Protocol)?;
        self.doorbell(0, 0);
        self.wait_completion(1000, input, |this, input, event| {
            if event.kind() == 33 && event.parameter & !15 == pointer {
                this.command_ring.complete();
                if event.code() != 1 {
                    libnanami::println!(
                        "[usb-server] command {} failed code={}",
                        trb.kind(),
                        event.code()
                    );
                    return Err(RequestError::Protocol);
                }
                return Ok(Some(event));
            }
            this.dispatch(event, input);
            Ok(None)
        })
        .inspect_err(|error| {
            libnanami::println!(
                "[usb-server] command wait failed kind={} slot={} trb={:#x}: {}",
                trb.kind(),
                trb.slot(),
                pointer,
                error
            );
        })
    }
    pub fn poll(&mut self, input: &mut Input) {
        if !self.running {
            return;
        }
        self.events.begin(self.op, self.interrupter);
        for _ in 0..ring::ENTRIES {
            let Some(event) = self.event() else {
                break;
            };
            self.dispatch(event, input);
        }
        self.events.finish(self.interrupter);
        if self.dirty {
            self.dirty = false;
            self.scan_ports(input);
        }
    }
    pub fn counts(&self) -> (usize, usize) {
        let devices = self.slots.iter().flatten().count();
        let interfaces = self
            .slots
            .iter()
            .flatten()
            .map(|device| device.endpoints.len())
            .sum();
        (devices, interfaces)
    }
}
