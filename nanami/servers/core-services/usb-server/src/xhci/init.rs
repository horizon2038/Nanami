use super::*;
use crate::arch::ControllerResource;

impl Controller {
    pub fn initialize(
        resource: ControllerResource,
        timer: Word,
        irq_slot: Word,
    ) -> Result<Self, RequestError> {
        let base = resource.mmio;
        let cap = unsafe { read(base, 0) };
        let op_offset = (cap & 0xff) as usize;
        let hcs1 = unsafe { read(base, 4) };
        let hcs2 = unsafe { read(base, 8) };
        let hcc = unsafe { read(base, 0x10) };
        let ports = (hcs1 >> 24) as usize;
        let slots = (hcs1 as u8 as usize).min(MAX_SLOTS);
        let doorbells = unsafe { read(base, 0x14) } as usize & !3;
        let runtime = unsafe { read(base, 0x18) } as usize & !31;
        if cap >> 16 < 0x0096
            || op_offset < 0x20
            || ports == 0
            || slots == 0
            || op_offset + PORTSC + ports * 16 > resource.bytes
            || doorbells + (slots + 1) * 4 > resource.bytes
            || runtime + 0x40 > resource.bytes
        {
            return Err(RequestError::Unsupported);
        }
        let op = base + op_offset;
        let address64 = hcc & 1 != 0;
        if unsafe { read(op, 8) } & 1 == 0 {
            return Err(RequestError::Unsupported);
        }
        let mut slot_type = [0u8; 256];
        let mut port_major = [0u8; 256];
        let mut port_speeds = [0u32; 256];
        let mut superspeeds = [0u16; 256];
        let mut extended = ((hcc >> 16) as usize) * 4;
        // Extended capabilities are forward-relative; reject malformed chains
        // and do the BIOS/OS ownership handshake before resetting hardware.
        for _ in 0..256 {
            if extended == 0 {
                break;
            }
            if extended + 16 > resource.bytes {
                return Err(RequestError::Protocol);
            }
            let header = unsafe { read(base, extended) };
            if header as u8 == 1 {
                unsafe { write(base, extended, header | (1 << 24)) };
                let mut owned = false;
                for _ in 0..1000 {
                    if unsafe { read(base, extended) } & (1 << 16) == 0 {
                        owned = true;
                        break;
                    }
                    crate::delay(timer, 1)?;
                }
                if !owned {
                    return Err(RequestError::Transport);
                }
                // Disable legacy SMIs without acknowledging arbitrary RW1C bits.
                let legacy = unsafe { read(base, extended + 4) };
                unsafe { write(base, extended + 4, legacy & 0x000e_1fee) };
            } else if header as u8 == 2 {
                let compatible = unsafe { read(base, extended + 8) };
                let first = (compatible & 0xff) as usize;
                let count = ((compatible >> 8) & 0xff) as usize;
                let ty = unsafe { read(base, extended + 12) } as u8 & 31;
                // QEMU retains an empty USB3 capability with p3=0. It assigns
                // no registers; ignore that entry while validating real ranges.
                if count != 0 && (first == 0 || first + count > ports + 1) {
                    return Err(RequestError::Protocol);
                }
                let major = (header >> 24) as u8;
                let psi_count = (compatible >> 28) as usize;
                if extended + 16 + psi_count * 4 > resource.bytes {
                    return Err(RequestError::Protocol);
                }
                let mut speeds = 0;
                let mut super_ids = 0;
                if major == 2 && unsafe { read(base, extended + 4) } == 0x2042_5355 {
                    if psi_count == 0 {
                        speeds = protocol::DEFAULT_SPEEDS;
                    } else {
                        for index in 0..psi_count {
                            protocol::add_psi(&mut speeds, unsafe {
                                read(base, extended + 16 + index * 4)
                            })
                            .map_err(|_| RequestError::Protocol)?;
                        }
                    }
                }
                if major == 3 && unsafe { read(base, extended + 4) } == 0x2042_5355 {
                    if psi_count == 0 {
                        super_ids = 1 << 4;
                    } else {
                        for index in 0..psi_count {
                            protocol::add_superspeed(&mut super_ids, unsafe {
                                read(base, extended + 16 + index * 4)
                            })
                            .map_err(|_| RequestError::Protocol)?;
                        }
                    }
                }
                for port in first..first + count {
                    if port_major[port] != 0 {
                        return Err(RequestError::Protocol);
                    }
                    slot_type[port] = ty;
                    port_major[port] = major;
                    port_speeds[port] = speeds;
                    superspeeds[port] = super_ids;
                }
            }
            let next = ((header >> 8) & 0xff) as usize;
            extended = if next == 0 { 0 } else { extended + next * 4 };
        }
        if extended != 0 {
            return Err(RequestError::Protocol);
        }
        let dma = Dma::allocate(SLOT_BASE + slots * SLOT_BYTES, address64)?;
        let scratch_count = (((hcs2 >> 21) & 31) << 5 | (hcs2 >> 27)) as usize;
        let scratch = if scratch_count == 0 {
            None
        } else {
            let array_bytes = (scratch_count * 8 + 4095) & !4095;
            let pages = Dma::allocate(array_bytes + scratch_count * 4096, address64)?;
            unsafe {
                for index in 0..scratch_count {
                    core::ptr::write(
                        (pages.virtual_address as *mut u64).add(index),
                        pages.physical_at(array_bytes + index * 4096),
                    );
                }
                core::ptr::write(dma.virtual_address as *mut u64, pages.physical);
            }
            Some(pages)
        };
        let mut table = Vec::with_capacity(slots + 1);
        table.resize_with(slots + 1, || None);
        let mut controller = Self {
            op,
            interrupter: base + runtime + 0x20,
            doorbells: base + doorbells,
            irq: None,
            timer,
            dma,
            _scratch: scratch,
            command_ring: unsafe { Producer::new(dma.physical_at(0x1000), dma.virtual_at(0x1000)) },
            events: unsafe { Consumer::new(dma.physical_at(0x2000), dma.virtual_at(0x2000)) },
            slots: table,
            context_bytes: if hcc & 4 != 0 { 64 } else { 32 },
            ports,
            slot_type,
            port_major,
            port_speeds,
            superspeeds,
            generation: 0,
            borrowed_slot: None,
            borrowed_hid: 0,
            held_events: Vec::with_capacity(MAX_INTERFACES),
            dirty: true,
            running: false,
        };
        controller.wait_register(USBSTS, 1 << 11, 0, 1000)?;
        unsafe { write(op, USBCMD, 0) };
        controller.wait_register(USBSTS, 1, 1, 1000)?;
        unsafe { write(op, USBCMD, 2) };
        controller.wait_register(USBCMD, 2, 0, 1000)?;
        controller.wait_register(USBSTS, 1 << 11, 0, 1000)?;
        if let Some(line) = resource.irq {
            match libnanami::request_irq(line, libnanami::PROCESS_SLOT_NOTIFICATION, irq_slot) {
                Ok(()) => controller.irq = Some(libnanami::ipc::process_slot_descriptor(irq_slot)),
                Err(error) => libnanami::println!(
                    "[usb-server] IRQ {} unavailable: {}; timer polling fallback",
                    line,
                    error
                ),
            }
        }
        unsafe {
            write(op, 0x38, slots as u32);
            write64(op, 0x30, dma.physical);
            write64(op, 0x18, dma.physical_at(0x1000) | 1);
            core::ptr::write(dma.virtual_at(0x3000) as *mut u64, dma.physical_at(0x2000));
            core::ptr::write(dma.virtual_at(0x3008) as *mut u32, ring::ENTRIES as u32);
            fence(Ordering::SeqCst);
            write(controller.interrupter, 0x08, 1);
            write64(controller.interrupter, 0x18, dma.physical_at(0x2000));
            write64(controller.interrupter, 0x10, dma.physical_at(0x3000));
            write(controller.interrupter, 4, 4000); // 1-ms IRQ moderation.
            write(controller.interrupter, 0, 1); // Enable interrupts after initial enumeration.
            write(op, USBCMD, 1);
        }
        controller.wait_register(USBSTS, 1, 0, 1000)?;
        controller.running = true;
        for port in 1..=ports {
            let offset = PORTSC + (port - 1) * 16;
            let value = unsafe { read(op, offset) };
            unsafe { write(op, offset, port_controls(value) | PORT_POWER) };
        }
        libnanami::println!(
            "[usb-server] xHCI ports={} slots={} context={} scratch={} irq={}",
            ports,
            slots,
            controller.context_bytes,
            scratch_count,
            controller.irq.is_some()
        );
        Ok(controller)
    }
    pub fn enable_interrupts(&self) {
        if self.irq.is_some() {
            unsafe {
                write(self.interrupter, 0, 3);
                write(self.op, USBCMD, 5);
            }
        }
    }
}
