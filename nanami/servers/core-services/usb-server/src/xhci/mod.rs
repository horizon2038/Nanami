mod dma;
mod enumerate;
mod init;
mod protocol;
pub mod registers;
pub mod ring;
mod transfer;

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
use ring::{Consumer, Producer, Trb, IOC};

const MAX_SLOTS: usize = 16;
const MAX_INTERFACES: usize = 4;
const SLOT_BYTES: usize = (4 + 2 * MAX_INTERFACES) * 4096;
const SLOT_BASE: usize = 0x4000;

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
    port: u8,
    speed: u8,
    speed_class: u8,
    dma: Dma,
    control: Producer,
    endpoints: Vec<Endpoint>,
    vendor: u16,
    product: u16,
}

pub struct Controller {
    op: usize,
    interrupter: usize,
    doorbells: usize,
    pub irq: Option<Word>,
    timer: Word,
    dma: Dma,
    _scratch: Option<Dma>,
    command_ring: Producer,
    events: Consumer,
    slots: Vec<Option<Device>>,
    context_bytes: usize,
    ports: usize,
    slot_type: [u8; 256],
    port_major: [u8; 256],
    port_speeds: [u32; 256],
    dirty: bool,
    running: bool,
}

impl Controller {
    fn delay(&self, milliseconds: Word) -> Result<(), RequestError> {
        nanami_services::timer::timer_service_sleep_milliseconds(self.timer, milliseconds)
    }
    fn wait_register(
        &self,
        offset: usize,
        mask: u32,
        value: u32,
        limit_ms: usize,
    ) -> Result<(), RequestError> {
        for _ in 0..limit_ms {
            if unsafe { read(self.op, offset) } & mask == value {
                return Ok(());
            }
            self.delay(1)?;
        }
        Err(RequestError::Transport)
    }
    fn doorbell(&self, slot: u8, dci: u8) {
        fence(Ordering::SeqCst);
        unsafe { write(self.doorbells, slot as usize * 4, dci as u32) };
    }
    fn event(&mut self) -> Option<Trb> {
        let event = self.events.pop()?;
        // Advance ERDP including EHB. Device ownership of consumed event entries
        // resumes only after the event fields have been acquired.
        fence(Ordering::SeqCst);
        unsafe { write64(self.interrupter, 0x18, self.events.next_physical() | 8) };
        Some(event)
    }
    fn dispatch(&mut self, event: Trb, input: &mut Input) {
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
            37 => self.fail(input), // Host Controller Event (e.g. event-ring overrun).
            _ => {}
        }
    }
    fn fail(&mut self, input: &mut Input) {
        self.running = false;
        // Do not free or reuse DMA memory even if this controller cannot halt.
        unsafe { write(self.op, USBCMD, 0) };
        for device in self.slots.iter_mut().flatten() {
            for endpoint in &mut device.endpoints {
                endpoint.keyboard.release(|event| input.emit(event));
                endpoint.mouse.release(|event| input.emit(event));
            }
        }
        libnanami::print!("[usb-server] controller stopped after fatal error\n");
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
        for _ in 0..1000 {
            for _ in 0..ring::ENTRIES {
                let Some(event) = self.event() else {
                    break;
                };
                if event.kind() == 33 && event.parameter & !15 == pointer {
                    self.command_ring.complete();
                    if event.code() != 1 {
                        libnanami::println!(
                            "[usb-server] command {} failed code={}",
                            trb.kind(),
                            event.code()
                        );
                        return Err(RequestError::Protocol);
                    }
                    return Ok(event);
                }
                self.dispatch(event, input);
            }
            self.delay(1)?;
        }
        self.fail(input);
        Err(RequestError::Transport)
    }
    pub fn poll(&mut self, input: &mut Input) {
        if !self.running {
            return;
        }
        // Acknowledge the interrupt source before rearming the kernel IRQ, then
        // recheck the event ring in the server loop to close the masked window.
        unsafe {
            write(self.op, USBSTS, 1 << 3);
            write(self.interrupter, 0, if self.irq.is_some() { 3 } else { 1 });
        }
        for _ in 0..ring::ENTRIES {
            let Some(event) = self.event() else {
                break;
            };
            self.dispatch(event, input);
        }
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
