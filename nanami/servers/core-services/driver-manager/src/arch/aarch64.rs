use nanami_runtime::RequestError;

use super::super::TimerSelection;

pub fn initialize() -> Result<(), RequestError> {
    Ok(())
}

pub fn start_peripherals() -> Result<Option<&'static str>, RequestError> {
    Ok(None)
}

pub fn usb_controller(_index: usize) -> Result<Option<nanami_runtime::Word>, RequestError> {
    Ok(None) // PCI host resource discovery for this platform is deferred.
}

pub fn prepare_usb_controller(
    _: usize,
) -> Result<(nanami_client::device::UsbControllerResource, u8), RequestError> {
    Err(RequestError::Unsupported)
}

pub fn select_storage_driver() -> Result<Option<&'static str>, RequestError> {
    Ok(Some("./bin/virtio-blk-server"))
}

pub fn select_timer_driver() -> Result<TimerSelection, RequestError> {
    // The boot platform was checked as QEMU virt. Timer support is intentionally
    // deferred: in particular, EL0 access to CNTVCT/CNTV is not enabled.
    Ok(TimerSelection {
        image: None,
        hpet_mmio_base: 0,
    })
}
