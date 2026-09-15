use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub const DEVICE_MANAGER_SERVICE: &str = "device-manager";
pub const DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE: Word = 0xd001;
pub const DEVICE_MANAGER_REQUEST_USB_CONTROLLER: Word = 0xd002;
pub const DEVICE_MANAGER_REQUEST_STORAGE_PROBE: Word = 0xd003;
pub const STORAGE_PENDING: Word = 0;
pub const STORAGE_SELECTED: Word = 1;
pub const STORAGE_NOT_SELECTED: Word = 2;
pub const STORAGE_AMBIGUOUS: Word = 3;

/// Report 0/1 roots, or 2 for ambiguity/failed discovery. A zero-root driver
/// reports once and may continue serving another class (USB HID). A candidate
/// waits for every boot-selected driver before publishing block-device.
/// Slots 28/29 are reserved by participating drivers for manager/timer IPC.
pub fn select_storage_root(roots: Word) -> Result<bool, RequestError> {
    libnanami::connect_service_by_name(DEVICE_MANAGER_SERVICE, 28)?;
    let manager = libnanami::ipc::process_slot_descriptor(28);
    let mut timer = None;
    let mut waits = 0;
    loop {
        let (status, decision, _) = call_port(
            manager,
            DEVICE_MANAGER_REQUEST_STORAGE_PROBE,
            roots,
            0,
            0,
            0,
            2,
        )?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        match decision {
            STORAGE_SELECTED => return Ok(true),
            STORAGE_NOT_SELECTED => return Ok(false),
            STORAGE_AMBIGUOUS => return Err(RequestError::Protocol),
            STORAGE_PENDING if roots != 1 => return Ok(false),
            STORAGE_PENDING => {}
            _ => return Err(RequestError::Protocol),
        }
        if waits >= 300 {
            return Err(RequestError::Transport);
        }
        if timer.is_none() && crate::registry::connect_timer_service(29).is_ok() {
            timer = Some(libnanami::ipc::process_slot_descriptor(29));
        }
        if let Some(timer) = timer {
            crate::timer::timer_service_sleep_milliseconds(timer, 100)?;
            waits += 1;
        } else {
            libnanami::yield_now();
        }
    }
}

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
