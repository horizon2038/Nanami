pub use nanami_uapi::device::*;

use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn report_display_probe(
    service: CapabilityDescriptor,
    ready: bool,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service,
        DEVICE_MANAGER_REQUEST_DISPLAY_PROBE,
        ready as Word,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

/// None means ACPI resolution is still in progress; Some(None) means no route.
pub fn usb_irq(
    service: CapabilityDescriptor,
    index: Word,
) -> Result<Option<Option<Word>>, RequestError> {
    let (status, ready, irq) =
        call_port(service, DEVICE_MANAGER_REQUEST_USB_IRQ, index, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if ready > 1 || irq >= 24 {
        return Err(RequestError::Protocol);
    }
    Ok((ready != 0).then_some((irq != 0).then_some(irq)))
}

/// Only acpi-server can inspect unresolved boot-selected USB PCI functions.
pub fn usb_pci_route(
    service: CapabilityDescriptor,
    index: Word,
) -> Result<Option<(Word, u8)>, RequestError> {
    let (status, address, pin) = call_port(
        service,
        DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE,
        index,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if pin == 0 {
        return Ok(None);
    }
    if address & !0x00ff_1f07 != 0 || pin > 4 {
        return Err(RequestError::Protocol);
    }
    Ok(Some((address, pin as u8)))
}

pub fn report_usb_irq(
    service: CapabilityDescriptor,
    index: Word,
    irq: Option<Word>,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service,
        DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT,
        index,
        irq.unwrap_or(0),
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

/// RSDP is delegated only to the boot-selected ACPI server.
pub fn acpi_rsdp(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, rsdp, _) = call_port(
        service_port,
        DEVICE_MANAGER_REQUEST_ACPI_RSDP,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if rsdp == 0 {
        return Err(RequestError::Unsupported);
    }
    Ok(rsdp)
}

/// Report 0/1 roots, or 2 for ambiguity/failed discovery. A zero-root driver
/// does not wait and may report again when its storage inventory changes. A
/// candidate waits for every boot-selected driver before publishing block-device.
/// Selection (including ambiguity) is final; late media never replaces a root.
/// Slots 28/29 are reserved by participating drivers for manager/timer IPC.
/// For repeated reports, use `select_storage_root_on_port` with an existing
/// connection instead of trying to populate slot 28 again.
pub fn select_storage_root(roots: Word) -> Result<bool, RequestError> {
    nanami_runtime::connect_service_by_name(DEVICE_MANAGER_SERVICE, 28)?;
    let manager = nanami_runtime::ipc::process_slot_descriptor(28);
    select_storage_root_on_port(manager, roots)
}

/// Reuse an existing manager connection for change-driven storage discovery.
/// Like `select_storage_root`, reserves slot 29 for a candidate's timer waits.
pub fn select_storage_root_on_port(
    manager: CapabilityDescriptor,
    roots: Word,
) -> Result<bool, RequestError> {
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
            timer = Some(nanami_runtime::ipc::process_slot_descriptor(29));
        }
        if let Some(timer) = timer {
            crate::timer::timer_service_sleep_milliseconds(timer, 100)?;
            waits += 1;
        } else {
            nanami_runtime::yield_now();
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
    if physical & 4095 != 0 || !bytes.is_power_of_two() || bytes > 0x100000 || irq >= 24 {
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

/// Only the boot-selected USB driver may request this counter view. It must
/// never write HPET registers: hpet-server remains the sole timer owner.
pub fn hpet_counter_base(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, base, _) = call_port(
        service_port,
        DEVICE_MANAGER_REQUEST_HPET_COUNTER_BASE,
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
        return Err(RequestError::Unsupported);
    }
    Ok(base)
}
