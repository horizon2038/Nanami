//! Existing net-device SHM/batch protocol, bound to one owning network server.
use crate::{registers::*, ring::Rings, ServiceRequest, CLIENT_NOTIFICATION};
use core::ptr;
use nanami_client::net::*;
use nanami_runtime::{RequestError, Word};

pub struct Service {
    physical: u64,
    mac: [u8; 6],
    owner: Word,
    shared: Word,
    bytes: Word,
    notification: Word,
    enabled: bool,
}

impl Service {
    pub fn authorized(&self, caller: Word) -> bool {
        caller != 0 && self.owner == caller
    }

    pub fn select(&mut self, physical: u64, mac: [u8; 6]) {
        self.physical = physical;
        self.mac = mac;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn new(physical: u64, mac: [u8; 6]) -> Self {
        Self {
            physical,
            mac,
            owner: 0,
            shared: 0,
            bytes: 0,
            notification: 0,
            enabled: true,
        }
    }

    fn range(&self, offset: usize, bytes: usize) -> Result<usize, RequestError> {
        if self.shared == 0 || offset.checked_add(bytes).is_none_or(|end| end > self.bytes) {
            return Err(RequestError::InvalidArgument);
        }
        self.shared
            .checked_add(offset)
            .ok_or(RequestError::InvalidArgument)
    }

    pub fn notify_rx(&self, rings: &Rings) {
        if self.enabled && self.notification != 0 && rings.pending() {
            let _ = nanami_runtime::ipc::notification_notify(self.notification);
        }
    }

    pub fn request(
        &mut self,
        request: ServiceRequest,
        regs: &mut impl Registers,
        rings: &mut Rings,
    ) -> (Word, Word, Word) {
        // GET_MAC/features are read-only. All state and DMA operations belong
        // to the first nonzero client attaching a shared memory mapping.
        if request.identifier == 0 || (self.owner != 0 && request.identifier != self.owner) {
            return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
        }
        let result = self.handle(request, regs, rings);
        match result {
            Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
            Err(RequestError::InvalidArgument) => {
                (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0)
            }
            Err(RequestError::Status(s)) => (s, 0, 0),
            Err(_) => (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0),
        }
    }

    fn handle(
        &mut self,
        request: ServiceRequest,
        regs: &mut impl Registers,
        rings: &mut Rings,
    ) -> Result<(Word, Word), RequestError> {
        match request.code {
            NET_DEVICE_REQUEST_SEND => {
                if !self.enabled {
                    return Err(RequestError::Unsupported);
                }
                let source = self.range(request.arg0, request.arg1)?;
                let bytes = unsafe { rings.send(regs, self.physical, source, request.arg1)? };
                Ok((bytes, 0))
            }
            NET_DEVICE_REQUEST_RECV => {
                if !self.enabled || request.arg1 == 0 {
                    return Err(RequestError::InvalidArgument);
                }
                let bytes = request.arg1.min(NET_DEVICE_RX_FRAME_MAX);
                let destination = self.range(request.arg0, bytes)?;
                Ok((unsafe { rings.receive(regs, destination, bytes) }, 0))
            }
            NET_DEVICE_REQUEST_RECV_BATCH => {
                if !self.enabled || !(1..=NET_DEVICE_RX_BATCH_MAX).contains(&request.arg1) {
                    return Err(RequestError::InvalidArgument);
                }
                let destination =
                    self.range(request.arg0, request.arg1 * NET_DEVICE_RX_SLOT_STRIDE)?;
                let mut count = 0;
                while count < request.arg1 {
                    let record = destination + count * NET_DEVICE_RX_SLOT_STRIDE;
                    let bytes = unsafe { rings.receive(regs, record + 4, NET_DEVICE_RX_FRAME_MAX) };
                    if bytes == 0 {
                        break;
                    }
                    unsafe {
                        ptr::write_unaligned(record as *mut u32, (bytes as u32).to_le());
                    }
                    count += 1;
                }
                Ok((count, 0))
            }
            NET_DEVICE_REQUEST_CONTROL => match request.arg0 {
                NET_DEVICE_CONTROL_REPORT_STATUS => {
                    if self.owner == 0 {
                        return Err(RequestError::InvalidArgument);
                    }
                    Ok(rings.report_status(regs))
                }
                NET_DEVICE_CONTROL_GET_FEATURES => Ok((NET_DEVICE_FEATURE_RECV_BATCH, 0)),
                NET_DEVICE_CONTROL_GET_MAC => {
                    let mut value = 0;
                    for (index, byte) in self.mac.iter().enumerate() {
                        value |= (*byte as Word) << (index * 8);
                    }
                    Ok((value, 0))
                }
                NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY => {
                    if request.arg1 == 0
                        || request.arg2 == 0
                        || request.arg1.checked_add(request.arg2).is_none()
                    {
                        return Err(RequestError::InvalidArgument);
                    }
                    self.owner = request.identifier;
                    self.shared = request.arg1;
                    self.bytes = request.arg2;
                    Ok((0, 0))
                }
                NET_DEVICE_CONTROL_ATTACH_RX_NOTIFICATION => {
                    if self.owner == 0 {
                        return Err(RequestError::InvalidArgument);
                    }
                    if self.notification != 0 {
                        return Ok((1, 0));
                    }
                    let slot = if request.arg1 == 0 {
                        nanami_runtime::PROCESS_SLOT_NOTIFICATION
                    } else {
                        request.arg1
                    };
                    nanami_runtime::request_notification_port_copy(
                        request.identifier,
                        slot,
                        CLIENT_NOTIFICATION,
                        NET_NOTIFICATION_RX,
                    )?;
                    self.notification =
                        nanami_runtime::ipc::process_slot_descriptor(CLIENT_NOTIFICATION);
                    self.notify_rx(rings);
                    // True means asynchronous RX wakeups, including timer fallback.
                    Ok((1, 0))
                }
                NET_DEVICE_CONTROL_LINK_UP | NET_DEVICE_CONTROL_LINK_DOWN => {
                    if self.owner == 0 {
                        return Err(RequestError::InvalidArgument);
                    }
                    self.enabled = request.arg0 == NET_DEVICE_CONTROL_LINK_UP;
                    regs.write(RCTL, if self.enabled { RX_CONTROL } else { 0 });
                    regs.write(TCTL, if self.enabled { TX_CONTROL } else { 0 });
                    regs.flush();
                    Ok((0, 0))
                }
                _ => Err(RequestError::Unsupported),
            },
            _ => Err(RequestError::Unsupported),
        }
    }
}
