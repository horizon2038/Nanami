use super::*;
use crate::arch::ControllerResource;

mod extended;

impl Controller {
    pub fn initialize(
        resource: ControllerResource,
        timer: Word,
        irq_slot: Word,
    ) -> Result<Self, RequestError> {
        if resource.bytes < 0x20 {
            libnanami::println!("[usb-server] xHCI BAR too short: {:#x}", resource.bytes);
            return Err(RequestError::Protocol);
        }
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
        libnanami::println!(
            "[usb-server] xHCI cap={:#010x} hcs1={:#010x} hcs2={:#010x} hcc={:#010x} dboff={:#x} rtsoff={:#x} bytes={:#x}",
            cap, hcs1, hcs2, hcc, doorbells, runtime, resource.bytes
        );
        if cap >> 16 < 0x0096
            || op_offset < 0x20
            || ports == 0
            || slots == 0
            || op_offset + PORTSC + ports * 16 > resource.bytes
            || doorbells + (slots + 1) * 4 > resource.bytes
            || runtime + 0x40 > resource.bytes
        {
            libnanami::println!("[usb-server] unsupported xHCI register layout/version");
            return Err(RequestError::Unsupported);
        }
        let op = base + op_offset;
        let address64 = hcc & 1 != 0;
        let page_size = unsafe { read(op, 8) };
        if page_size & 1 == 0 {
            libnanami::println!(
                "[usb-server] unsupported xHCI page-size mask={:#x}",
                page_size
            );
            return Err(RequestError::Unsupported);
        }
        let extended::Ports {
            slot_type,
            port_major,
            port_speeds,
            superspeeds,
        } = extended::configure(base, resource.bytes, hcc, ports, timer)?;
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
            storage_changed: true,
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
            if controller.port_major[port] == 0 {
                continue;
            }
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
