use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub const DEVICE_MANAGER_SERVICE: &str = "device-manager";
pub const DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE: Word = 0xd001;
pub const DEVICE_MANAGER_REQUEST_USB_CONTROLLER: Word = 0xd002;

#[derive(Clone, Copy)]
pub struct UsbControllerResource {
    pub physical: Word,
    pub bytes: Word,
    pub irq: Option<Word>,
}

/// Controller resources are visible only to the boot-selected USB driver.
pub fn usb_controller(
    service_port: CapabilityDescriptor,
    index: Word,
) -> Result<Option<UsbControllerResource>, RequestError> {
    let (status, physical, details) = call_port(
        service_port,
        DEVICE_MANAGER_REQUEST_USB_CONTROLLER,
        index,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if physical == 0 {
        return Ok(None);
    }
    let bytes = details & !4095;
    let irq = details & 4095;
    if physical & 4095 != 0 || !bytes.is_power_of_two() || bytes > 0x100000 || irq >= 16 {
        return Err(RequestError::Protocol);
    }
    Ok(Some(UsbControllerResource {
        physical,
        bytes,
        irq: (irq != 0).then_some(irq),
    }))
}

pub fn hpet_mmio_base(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, base, _) = call_port(
        service_port,
        DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if base == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(base)
}
