use libnanami::RequestError;

use super::super::TimerSelection;

pub fn usb_controller(_index: usize) -> Result<Option<libnanami::Word>, RequestError> {
    Ok(None) // PCI host resource discovery for this platform is deferred.
}

pub fn prepare_usb_controller(
    _: usize,
) -> Result<nanami_services::device::UsbControllerResource, RequestError> {
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
