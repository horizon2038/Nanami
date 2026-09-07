use libnanami::RequestError;

use super::super::TimerSelection;

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
