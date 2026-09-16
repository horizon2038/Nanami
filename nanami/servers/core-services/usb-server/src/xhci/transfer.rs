use super::*;

impl Controller {
    pub(super) fn control(
        &mut self,
        slot: u8,
        device: &mut Device,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        len: usize,
        input: &mut Input,
    ) -> Result<usize, RequestError> {
        if len > 4096 || !self.running {
            return Err(RequestError::InvalidArgument);
        }
        let receive = request_type & 0x80 != 0;
        let setup = request_type as u64
            | (request as u64) << 8
            | (value as u64) << 16
            | (index as u64) << 32
            | (len as u64) << 48;
        let mut stages = [Trb::default(); 3];
        stages[0] = Trb {
            parameter: setup,
            status: 8,
            control: (2 << 10)
                | (1 << 6)
                | (if len == 0 {
                    0
                } else if receive {
                    3
                } else {
                    2
                }) << 16,
        };
        let mut last = 1;
        if len != 0 {
            stages[1] = Trb {
                parameter: device.dma.physical_at(0x3000),
                status: len as u32,
                control: (3 << 10) | (1 << 2) | ((receive as u32) << 16),
            };
            last = 2;
        }
        stages[last] = Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) | IOC | (((len == 0 || !receive) as u32) << 16),
        };
        let pointer = device
            .control
            .submit(&stages[..=last])
            .ok_or(RequestError::Protocol)?;
        self.doorbell(slot, 1);
        let mut received = len;
        for _ in 0..1000 {
            for _ in 0..ring::ENTRIES {
                let Some(event) = self.event() else {
                    break;
                };
                if event.kind() == 32 && event.slot() == slot && event.endpoint() == 1 {
                    if event.code() == 13 {
                        let residual = (event.status & 0xffffff) as usize;
                        if residual > len {
                            return Err(RequestError::Protocol);
                        }
                        received = len - residual;
                    } else if event.code() == 6 {
                        // Optional requests (e.g. GET_MAX_LUN) may stall. Restore
                        // EP0's dequeue before a fallback or subsequent request.
                        self.reset_endpoint(slot, 1, &mut device.control, true, input)?;
                        return Err(RequestError::Unsupported);
                    } else if event.code() != 1 {
                        libnanami::println!(
                            "[usb-server] control req={:#x} slot={} code={}",
                            request,
                            slot,
                            event.code()
                        );
                        return Err(RequestError::Protocol);
                    }
                    if event.parameter & !15 == pointer {
                        device.control.complete();
                        return Ok(received);
                    }
                } else {
                    self.dispatch(event, input);
                }
            }
            self.delay(1)?;
        }
        // No further DMA-buffer access is safe after an uncompleted transfer.
        self.fail(input);
        Err(RequestError::Transport)
    }

    pub(super) fn set_context32(&self, device: &Device, context: usize, word: usize, value: u32) {
        unsafe {
            core::ptr::write(
                (device.dma.virtual_address + context * self.context_bytes + word * 4) as *mut u32,
                value,
            )
        };
    }
    pub(super) fn copy_output_slot(&self, device: &Device) {
        unsafe {
            core::ptr::copy_nonoverlapping(
                device.dma.virtual_at(0x1000) as *const u8,
                (device.dma.virtual_address + self.context_bytes) as *mut u8,
                self.context_bytes,
            );
        }
    }
    pub(super) fn endpoint_context(
        &self,
        device: &Device,
        context: usize,
        kind: u32,
        packet: u16,
        interval: u8,
        ring: u64,
    ) {
        self.set_context32(device, context, 0, (interval as u32) << 16);
        self.set_context32(
            device,
            context,
            1,
            (packet as u32) << 16 | (kind << 3) | (3 << 1),
        );
        self.set_context32(device, context, 2, ring as u32 | 1);
        self.set_context32(device, context, 3, (ring >> 32) as u32);
        self.set_context32(
            device,
            context,
            4,
            if kind == 4 {
                8
            } else if matches!(kind, 2 | 6) {
                packet as u32 // Bulk has no periodic Max ESIT Payload.
            } else {
                (packet as u32) << 16 | packet as u32
            },
        );
    }
}
