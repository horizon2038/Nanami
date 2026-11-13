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
    // QEMU virt's platform inventory exposes 32 virtio-mmio transports. Do not
    // launch a block backend just because the architecture is AArch64. DTB
    // handoff is not currently part of Alpha's platform-info protocol.
    let (_, base) = nanami_runtime::request_mmio(0x0a00_0000, 0x4000)?;
    for index in 0..32 {
        let transport = base + index * 0x200;
        let read = |offset| unsafe { core::ptr::read_volatile((transport + offset) as *const u32) };
        if read(0) == 0x7472_6976 && read(4) == 2 && read(8) == 2 {
            return Ok(Some("./bin/virtio-blk-server"));
        }
    }
    Ok(None)
}

pub fn select_timer_driver() -> Result<TimerSelection, RequestError> {
    // The boot platform was checked as QEMU virt. Timer support is intentionally
    // deferred: in particular, EL0 access to CNTVCT/CNTV is not enabled.
    Ok(TimerSelection {
        image: None,
        hpet_mmio_base: 0,
    })
}
