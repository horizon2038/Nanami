//! BOT command sequencing and recovery. HID events continue to be dispatched
//! while synchronous block I/O waits for its command/data/status stages.
use super::*;
use crate::usb::mass_storage;
use bulk::Storage;

#[derive(Clone, Copy, Debug)]
pub struct Disk {
    pub slot: u8,
    pub lun: u8,
    generation: u64,
}

impl Controller {
    pub fn storage_disks(&mut self, input: &mut Input) -> Result<Vec<Disk>, RequestError> {
        let mut disks = Vec::new();
        for slot in 1..self.slots.len() {
            let Some(mut device) = self.borrow_device(slot as u8) else {
                continue;
            };
            let result = if let Some(storage) = &device.storage {
                let interface = storage.interface;
                match self.control(
                    slot as u8,
                    &mut device,
                    0xa1,
                    0xfe,
                    0,
                    interface as u16,
                    1,
                    input,
                ) {
                    Ok(1) => {
                        let max = unsafe { *(device.dma.virtual_at(0x3000) as *const u8) };
                        if max <= 15 {
                            Ok(Some(max))
                        } else {
                            Err(RequestError::Protocol)
                        }
                    }
                    Err(RequestError::Unsupported) => Ok(Some(0)), // GET_MAX_LUN STALL means LUN zero.
                    Ok(_) => Err(RequestError::Protocol),
                    Err(error) => Err(error),
                }
            } else {
                Ok(None)
            };
            if let Ok(Some(max)) = result {
                for lun in 0..=max {
                    disks.push(Disk {
                        slot: slot as u8,
                        lun,
                        generation: device.generation,
                    });
                }
            }
            self.return_device(slot as u8, device, input);
            result?;
        }
        Ok(disks)
    }

    pub fn disk_present(&self, disk: Disk) -> bool {
        self.running
            && self
                .slots
                .get(disk.slot as usize)
                .and_then(Option::as_ref)
                .is_some_and(|device| {
                    device.generation == disk.generation
                        && device
                            .storage
                            .as_ref()
                            .is_some_and(|storage| !storage.failed)
                        && unsafe { read(self.op, PORTSC + (device.port as usize - 1) * 16) }
                            & (1 | (1 << 17))
                            == 1
                })
    }

    pub fn scsi(
        &mut self,
        disk: Disk,
        cdb: &[u8],
        data: &mut [u8],
        receive: bool,
        input: &mut Input,
    ) -> Result<(bool, usize), RequestError> {
        if !self.disk_present(disk) {
            return Err(RequestError::Transport);
        }
        let mut device = self
            .borrow_device(disk.slot)
            .ok_or(RequestError::Transport)?;
        // disk_present already checked that this generation has storage.
        let mut storage = device.storage.take().unwrap();
        let result = if data.len() > storage.data.bytes {
            Err(RequestError::InvalidArgument)
        } else {
            self.bot(disk, &mut device, &mut storage, cdb, data, receive, input)
        };
        if result.is_err() {
            storage.failed = true;
        }
        device.storage = Some(storage);
        self.return_device(disk.slot, device, input);
        result
    }

    fn clear_halt(
        &mut self,
        slot: u8,
        device: &mut Device,
        endpoint: &mut bulk::BulkEndpoint,
        input: &mut Input,
    ) -> Result<(), RequestError> {
        // USB endpoint toggle state and xHCI endpoint dequeue are independent.
        self.control(slot, device, 2, 1, 0, endpoint.address as u16, 0, input)?;
        self.reset_endpoint(
            slot,
            endpoint.dci(),
            &mut endpoint.ring,
            endpoint.halted,
            input,
        )?;
        endpoint.halted = false;
        Ok(())
    }

    fn bot(
        &mut self,
        disk: Disk,
        device: &mut Device,
        storage: &mut Storage,
        cdb: &[u8],
        data: &mut [u8],
        receive: bool,
        input: &mut Input,
    ) -> Result<(bool, usize), RequestError> {
        storage.tag = storage.tag.wrapping_add(1);
        let cbw = mass_storage::cbw(storage.tag, disk.lun, cdb, data.len(), receive)
            .map_err(|_| RequestError::InvalidArgument)?;
        let result = (|| {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    cbw.as_ptr(),
                    storage.scratch.virtual_address as *mut u8,
                    cbw.len(),
                )
            };
            if self.bulk(
                disk.slot,
                &mut storage.endpoints[0],
                storage.scratch,
                cbw.len(),
                input,
            )? != cbw.len()
            {
                return Err(RequestError::Protocol);
            }
            let mut transferred = 0;
            if !data.is_empty() {
                if !receive {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data.as_ptr(),
                            storage.data.virtual_address as *mut u8,
                            data.len(),
                        )
                    };
                }
                let endpoint = &mut storage.endpoints[receive as usize];
                transferred = match self.bulk(disk.slot, endpoint, storage.data, data.len(), input)
                {
                    Ok(bytes) => bytes,
                    Err(RequestError::Unsupported) => {
                        self.clear_halt(disk.slot, device, endpoint, input)?;
                        0 // Fetch CSW; the failed SCSI command can then request sense.
                    }
                    Err(error) => return Err(error),
                };
            }
            let csw_len = match self.bulk(
                disk.slot,
                &mut storage.endpoints[1],
                storage.scratch,
                13,
                input,
            ) {
                Err(RequestError::Unsupported) => {
                    self.clear_halt(disk.slot, device, &mut storage.endpoints[1], input)?;
                    self.bulk(
                        disk.slot,
                        &mut storage.endpoints[1],
                        storage.scratch,
                        13,
                        input,
                    )?
                }
                other => other?,
            };
            let csw = unsafe {
                core::slice::from_raw_parts(storage.scratch.virtual_address as *const u8, csw_len)
            };
            let (passed, processed) = mass_storage::csw(csw, storage.tag, data.len(), transferred)
                .map_err(|_| RequestError::Protocol)?;
            if passed && receive && processed != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        storage.data.virtual_address as *const u8,
                        data.as_mut_ptr(),
                        processed,
                    )
                };
            }
            Ok((passed, processed))
        })();
        if result.is_err() && self.running {
            // BOT reset recovery: class reset, clear IN, clear OUT. Do not retry
            // the command: a failed write may already have reached the medium.
            let recovered = self
                .control(
                    disk.slot,
                    device,
                    0x21,
                    0xff,
                    0,
                    storage.interface as u16,
                    0,
                    input,
                )
                .and_then(|_| self.clear_halt(disk.slot, device, &mut storage.endpoints[1], input))
                .and_then(|_| self.clear_halt(disk.slot, device, &mut storage.endpoints[0], input));
            if recovered.is_err() {
                self.fail(input);
            }
        }
        result
    }
}
