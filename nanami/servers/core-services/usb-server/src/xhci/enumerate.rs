use super::*;
use crate::usb::descriptors::boot_interfaces;

impl Controller {
    pub(super) fn scan_ports(&mut self, input: &mut Input) {
        for port in 1..=self.ports {
            if !self.running {
                return;
            }
            if self.port_major[port] == 0 {
                continue; // Unassigned/unsupported protocol: do not touch PORTSC.
            }
            let offset = PORTSC + (port - 1) * 16;
            let status = unsafe { read(self.op, offset) };
            let existing = self.slots.iter().position(|slot| {
                slot.as_ref()
                    .is_some_and(|device| device.port as usize == port)
            });
            if let Some(slot) = existing {
                // A disconnect/reconnect may have coalesced into one change.
                if status & 1 == 0 || status & (1 << 17) != 0 {
                    self.detach(slot as u8, input);
                } else {
                    continue;
                }
            }
            unsafe {
                write(
                    self.op,
                    offset,
                    port_controls(status) | (status & PORT_CHANGE),
                )
            };
            if status & 1 == 0 {
                continue;
            }
            if let Err(error) = self.attach(port as u8, input) {
                libnanami::println!("[usb-server] port {} skipped: {}", port, error);
            }
        }
    }

    fn detach(&mut self, slot: u8, input: &mut Input) {
        if let Some(mut device) = self.slots[slot as usize].take() {
            self.storage_changed |= device.storage.is_some();
            for endpoint in &mut device.endpoints {
                endpoint.keyboard.release(|event| input.emit(event));
                endpoint.mouse.release(|event| input.emit(event));
            }
            if self
                .command(
                    Trb {
                        control: (10 << 10) | (slot as u32) << 24,
                        ..Trb::default()
                    },
                    input,
                )
                .is_err()
            {
                self.fail(input); // Do not reuse a slot whose DMA ownership is uncertain.
            }
            libnanami::println!("[usb-server] detached port={} slot={}", device.port, slot);
        }
    }

    fn attach(&mut self, port: u8, input: &mut Input) -> Result<(), RequestError> {
        let offset = PORTSC + (port as usize - 1) * 16;
        self.delay(100)?; // USB connection debounce.
        let status = unsafe { read(self.op, offset) };
        if status & 1 == 0 {
            return Ok(());
        }
        match self.port_major[port as usize] {
            2 => {
                unsafe { write(self.op, offset, port_controls(status) | PORT_RESET) };
                self.wait_register(offset, PORT_RESET, 0, 1000)?;
            }
            3 => {
                // SuperSpeed performs link training automatically. Warm-reset
                // only if the connected port has not reached Enabled/U0.
                if status & 2 == 0 || status & (15 << 5) != 0 {
                    unsafe { write(self.op, offset, port_controls(status) | (1 << 31)) };
                    self.wait_register(offset, 1 << 31, 0, 1000)?;
                }
            }
            _ => return Err(RequestError::Unsupported),
        }
        let status = unsafe { read(self.op, offset) };
        if status & 3 != 3 {
            return Err(RequestError::Transport);
        }
        unsafe {
            write(
                self.op,
                offset,
                port_controls(status) | (status & PORT_CHANGE),
            )
        };
        self.delay(10)?; // Reset recovery before the first control transaction.
        let speed = ((status >> 10) & 15) as u8;
        let speed_class = if self.superspeeds[port as usize] & (1 << speed) != 0 {
            4
        } else {
            protocol::classify(self.port_speeds[port as usize], speed)
        };
        let packet = match speed_class {
            1 | 2 => 8,
            3 => 64,
            4 => 512,
            _ => {
                libnanami::println!(
                    "[usb-server] port {} unsupported speed-id={} PORTSC={:#010x}",
                    port,
                    speed,
                    status
                );
                return Err(RequestError::Unsupported);
            }
        };
        let event = self.command(
            Trb {
                control: (9 << 10) | ((self.slot_type[port as usize] as u32) << 16),
                ..Trb::default()
            },
            input,
        )?;
        let slot = event.slot();
        if slot == 0 || slot as usize >= self.slots.len() || self.slots[slot as usize].is_some() {
            self.fail(input);
            return Err(RequestError::Protocol);
        }
        let offset = SLOT_BASE + (slot as usize - 1) * SLOT_BYTES;
        let dma = Dma {
            physical: self.dma.physical_at(offset),
            virtual_address: self.dma.virtual_at(offset),
            bytes: SLOT_BYTES,
        };
        dma.clear(); // Enable Slot/previous Disable Slot has returned ownership.
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(RequestError::Protocol)?;
        let mut device = Device {
            generation: self.generation,
            port,
            speed,
            speed_class,
            dma,
            control: unsafe { Producer::new(dma.physical_at(0x2000), dma.virtual_at(0x2000)) },
            endpoints: Vec::new(),
            storage: None,
            vendor: 0,
            product: 0,
        };
        let result = self.configure_device(slot, &mut device, packet, input);
        if let Err(error) = result {
            if self
                .command(
                    Trb {
                        control: (10 << 10) | (slot as u32) << 24,
                        ..Trb::default()
                    },
                    input,
                )
                .is_err()
            {
                self.fail(input);
            }
            return Err(error);
        }
        libnanami::println!(
            "[usb-server] device port={} slot={} vid={:04x} pid={:04x} boot-hid={} storage={}",
            port,
            slot,
            device.vendor,
            device.product,
            device.endpoints.len(),
            device.storage.is_some()
        );
        for endpoint in &mut device.endpoints {
            endpoint.pending = endpoint
                .ring
                .submit(&[Trb {
                    parameter: endpoint.buffer.physical,
                    status: endpoint.interface.packet_size as u32,
                    control: (1 << 10) | IOC | (1 << 2),
                }])
                .ok_or(RequestError::Protocol)?;
        }
        self.storage_changed |= device.storage.is_some();
        self.slots[slot as usize] = Some(device);
        for endpoint in &self.slots[slot as usize].as_ref().unwrap().endpoints {
            self.doorbell(slot, endpoint.dci);
        }
        Ok(())
    }

    fn configure_device(
        &mut self,
        slot: u8,
        device: &mut Device,
        mut packet: u16,
        input: &mut Input,
    ) -> Result<(), RequestError> {
        self.set_context32(device, 0, 1, 3); // Slot and EP0 add-context flags.
        self.set_context32(device, 1, 0, (1 << 27) | (device.speed as u32) << 20);
        self.set_context32(device, 1, 1, (device.port as u32) << 16);
        self.endpoint_context(device, 2, 4, packet, 0, device.control.physical);
        unsafe {
            core::ptr::write(
                (self.dma.virtual_address as *mut u64).add(slot as usize),
                device.dma.physical_at(0x1000),
            )
        };
        self.command(
            Trb {
                parameter: device.dma.physical,
                control: (11 << 10) | (slot as u32) << 24,
                status: 0,
            },
            input,
        )?;
        self.delay(2)?; // SET_ADDRESS recovery.
        if device.speed_class == 1 {
            if self.control(slot, device, 0x80, 6, 0x100, 0, 8, input)? != 8 {
                return Err(RequestError::Protocol);
            }
            packet = unsafe { *((device.dma.virtual_at(0x3000) + 7) as *const u8) } as u16;
            if !matches!(packet, 8 | 16 | 32 | 64) {
                return Err(RequestError::Protocol);
            }
            unsafe {
                core::ptr::write_bytes(device.dma.virtual_address as *mut u8, 0, 4096);
                core::ptr::copy_nonoverlapping(
                    (device.dma.virtual_at(0x1000) + self.context_bytes) as *const u8,
                    (device.dma.virtual_address + 2 * self.context_bytes) as *mut u8,
                    self.context_bytes,
                );
            }
            self.set_context32(device, 0, 1, 2);
            self.set_context32(device, 2, 1, (packet as u32) << 16 | (4 << 3) | (3 << 1));
            self.command(
                Trb {
                    parameter: device.dma.physical,
                    control: (13 << 10) | (slot as u32) << 24,
                    status: 0,
                },
                input,
            )?;
        }
        if self.control(slot, device, 0x80, 6, 0x100, 0, 18, input)? != 18 {
            return Err(RequestError::Protocol);
        }
        let descriptor =
            unsafe { core::slice::from_raw_parts(device.dma.virtual_at(0x3000) as *const u8, 18) };
        if descriptor[0] != 18 || descriptor[1] != 1 || descriptor[4] == 9 {
            return Err(RequestError::Unsupported);
        }
        if device.speed_class == 4 && descriptor[7] != 9 {
            return Err(RequestError::Protocol);
        }
        device.vendor = u16::from_le_bytes([descriptor[8], descriptor[9]]);
        device.product = u16::from_le_bytes([descriptor[10], descriptor[11]]);
        if self.control(slot, device, 0x80, 6, 0x200, 0, 9, input)? != 9 {
            return Err(RequestError::Protocol);
        }
        let total =
            unsafe { core::ptr::read_unaligned((device.dma.virtual_at(0x3000) + 2) as *const u16) }
                as usize;
        if total < 9 || total > 4096 {
            return Err(RequestError::Unsupported);
        }
        if self.control(slot, device, 0x80, 6, 0x200, 0, total, input)? != total {
            return Err(RequestError::Protocol);
        }
        let configuration = unsafe {
            core::slice::from_raw_parts(device.dma.virtual_at(0x3000) as *const u8, total)
        };
        let mut interfaces = Vec::new();
        let configuration_value = boot_interfaces(configuration, |interface| {
            if device.speed_class != 4 && interfaces.len() < MAX_INTERFACES {
                interfaces.push(interface);
            }
        })
        .map_err(|_| RequestError::Protocol)?;
        let storage = crate::usb::mass_storage::interface(configuration, device.speed_class == 4)
            .map_err(|_| RequestError::Protocol)?;
        if interfaces.is_empty() && storage.is_none() {
            return Err(RequestError::Unsupported);
        }
        unsafe { core::ptr::write_bytes(device.dma.virtual_address as *mut u8, 0, 4096) };
        self.copy_output_slot(device);
        let mut flags = 1u32;
        let mut highest = 1u8;
        for (index, interface) in interfaces.iter().enumerate() {
            let dci = (interface.endpoint & 15) * 2 + 1;
            if flags & (1 << dci) != 0 {
                return Err(RequestError::Protocol);
            }
            flags |= 1 << dci;
            highest = highest.max(dci);
            let interval = if device.speed_class == 3 {
                if interface.interval > 16 {
                    return Err(RequestError::Protocol);
                }
                interface.interval - 1
            } else {
                (7 - interface.interval.leading_zeros() as u8) + 3
            };
            if device.speed_class == 2 && interface.packet_size > 8
                || device.speed_class == 1 && interface.packet_size > 64
            {
                return Err(RequestError::Protocol);
            }
            let offset = 0x4000 + index * 0x2000;
            self.endpoint_context(
                device,
                dci as usize + 1,
                7,
                interface.packet_size,
                interval,
                device.dma.physical_at(offset),
            );
            device.endpoints.push(Endpoint {
                dci,
                interface: *interface,
                ring: unsafe {
                    Producer::new(
                        device.dma.physical_at(offset),
                        device.dma.virtual_at(offset),
                    )
                },
                buffer: Dma {
                    physical: device.dma.physical_at(offset + 4096),
                    virtual_address: device.dma.virtual_at(offset + 4096),
                    bytes: 4096,
                },
                pending: 0,
                keyboard: Keyboard::new(),
                mouse: Mouse::new(),
                failed: false,
            });
        }
        if let Some(interface) = storage {
            self.configure_storage(device, interface, &mut flags, &mut highest)?;
        }
        self.set_context32(device, 0, 1, flags);
        self.set_context32(
            device,
            1,
            0,
            (highest as u32) << 27 | (device.speed as u32) << 20,
        );
        self.command(
            Trb {
                parameter: device.dma.physical,
                control: (12 << 10) | (slot as u32) << 24,
                status: 0,
            },
            input,
        )?;
        self.control(slot, device, 0, 9, configuration_value as u16, 0, 0, input)?;
        for interface in interfaces {
            self.control(slot, device, 0x21, 11, 0, interface.number as u16, 0, input)?; // Boot Protocol
            if interface.protocol == 1 {
                self.control(slot, device, 0x21, 10, 0, interface.number as u16, 0, input)?;
            }
        }
        Ok(())
    }
}
