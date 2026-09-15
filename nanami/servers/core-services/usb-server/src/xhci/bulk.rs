use super::*;
use crate::usb::mass_storage::Interface;

pub(super) struct BulkEndpoint {
    pub address: u8,
    pub ring: Producer,
    pub halted: bool,
}

impl BulkEndpoint {
    pub fn dci(&self) -> u8 {
        (self.address & 15) * 2 + (self.address >> 7)
    }
}

pub(super) struct Storage {
    pub interface: u8,
    pub endpoints: [BulkEndpoint; 2], // OUT, IN
    pub scratch: Dma,
    pub data: Dma,
    pub tag: u32,
    pub failed: bool,
}

impl Controller {
    pub(super) fn configure_storage(
        &self,
        device: &mut Device,
        interface: Interface,
        flags: &mut u32,
        highest: &mut u8,
    ) -> Result<(), RequestError> {
        let mut endpoints = Vec::new();
        for (index, endpoint) in [interface.output, interface.input].iter().enumerate() {
            let valid_packet = match device.speed_class {
                1 => endpoint.packet <= 64,
                3 => endpoint.packet == 512,
                4 => endpoint.packet == 1024,
                _ => false,
            };
            let dci = (endpoint.address & 15) * 2 + (endpoint.address >> 7);
            if !valid_packet || *flags & (1 << dci) != 0 {
                return Err(RequestError::Protocol);
            }
            *flags |= 1 << dci;
            *highest = (*highest).max(dci);
            let offset = BULK_BASE + index * 4096;
            let kind = if index == 0 { 2 } else { 6 };
            self.endpoint_context(
                device,
                dci as usize + 1,
                kind,
                endpoint.packet,
                0,
                device.dma.physical_at(offset),
            );
            self.set_context32(
                device,
                dci as usize + 1,
                1,
                (endpoint.packet as u32) << 16 | (endpoint.burst as u32) << 8 | kind << 3 | 3 << 1,
            );
            endpoints.push(BulkEndpoint {
                address: endpoint.address,
                ring: unsafe {
                    Producer::new(
                        device.dma.physical_at(offset),
                        device.dma.virtual_at(offset),
                    )
                },
                halted: false,
            });
        }
        // A 16-KiB-aligned buffer cannot cross the xHCI 64-KiB TRB boundary.
        // Reserve alignment slack per slot; the controller allocation need only
        // be page-aligned. One normal TRB suffices for each BOT stage.
        let data_physical = (device.dma.physical_at(BULK_BASE + 0x3000) + 0x3fff) & !0x3fff;
        let data_offset = (data_physical - device.dma.physical) as usize;
        debug_assert!(data_offset + 0x4000 <= device.dma.bytes);
        device.storage = Some(Storage {
            interface: interface.number,
            endpoints: endpoints.try_into().map_err(|_| RequestError::Protocol)?,
            scratch: Dma {
                physical: device.dma.physical_at(BULK_BASE + 0x2000),
                virtual_address: device.dma.virtual_at(BULK_BASE + 0x2000),
                bytes: 4096,
            },
            data: Dma {
                physical: data_physical,
                virtual_address: device.dma.virtual_at(data_offset),
                bytes: 0x4000,
            },
            tag: 0,
            failed: false,
        });
        Ok(())
    }

    pub(super) fn reset_endpoint(
        &mut self,
        slot: u8,
        dci: u8,
        ring: &mut Producer,
        halted: bool,
        input: &mut Input,
    ) -> Result<(), RequestError> {
        let result = (|| {
            self.command(
                Trb {
                    // Halted -> Reset Endpoint; Running -> Stop Endpoint.
                    control: ((if halted { 14 } else { 15 }) << 10)
                        | (dci as u32) << 16
                        | (slot as u32) << 24,
                    ..Trb::default()
                },
                input,
            )?;
            unsafe { ring.reset() };
            self.command(
                Trb {
                    parameter: ring.physical | 1,
                    control: (16 << 10) | (dci as u32) << 16 | (slot as u32) << 24,
                    ..Trb::default()
                },
                input,
            )?;
            Ok(())
        })();
        if result.is_err() {
            self.fail(input);
        }
        result
    }

    pub(super) fn bulk(
        &mut self,
        slot: u8,
        endpoint: &mut BulkEndpoint,
        buffer: Dma,
        len: usize,
        input: &mut Input,
    ) -> Result<usize, RequestError> {
        if !self.running || endpoint.halted {
            return Err(RequestError::Transport);
        }
        if len == 0 || len > buffer.bytes || (buffer.physical & 0xffff) + len as u64 > 0x10000 {
            return Err(RequestError::InvalidArgument);
        }
        let pointer = endpoint
            .ring
            .submit(&[Trb {
                parameter: buffer.physical,
                status: len as u32,
                control: (1 << 10) | IOC | (1 << 2),
            }])
            .ok_or(RequestError::Protocol)?;
        self.doorbell(slot, endpoint.dci());
        for _ in 0..5000 {
            for _ in 0..ring::ENTRIES {
                let Some(event) = self.event() else {
                    core::hint::spin_loop();
                    continue;
                };
                if event.kind() == 32 && event.slot() == slot && event.endpoint() == endpoint.dci()
                {
                    if event.parameter & !15 != pointer {
                        self.fail(input);
                        return Err(RequestError::Protocol);
                    }
                    if event.code() == 6 {
                        endpoint.halted = true;
                        return Err(RequestError::Unsupported); // Caller clears the halt before reuse.
                    }
                    let residual = (event.status & 0xffffff) as usize;
                    if matches!(event.code(), 1 | 13) && residual <= len {
                        endpoint.ring.complete();
                        return Ok(len - residual);
                    }
                    libnanami::println!(
                        "[usb-server] bulk slot={} ep={} code={}",
                        slot,
                        endpoint.dci(),
                        event.code()
                    );
                    self.fail(input);
                    return Err(RequestError::Transport);
                }
                self.dispatch(event, input);
                if !self.running {
                    return Err(RequestError::Transport);
                }
            }
            if let Err(error) = self.delay(1) {
                self.fail(input);
                return Err(error);
            }
        }
        // A timeout is not completion. Stop the controller and retain its DMA;
        // never copy from/reuse a buffer that hardware may still be accessing.
        self.fail(input);
        Err(RequestError::Transport)
    }
}
