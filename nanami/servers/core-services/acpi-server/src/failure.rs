//! AML/host callbacks are infallible in parts of acpi's API. A panic disables
//! this service, but must not strand the caller or leave storage frozen.
use core::sync::atomic::{AtomicBool, Ordering};
use libnanami::ipc::{process_slot_descriptor, ServiceEvent};

static REQUEST_ACTIVE: AtomicBool = AtomicBool::new(false);
static STORAGE_FROZEN: AtomicBool = AtomicBool::new(false);

pub fn request_active(active: bool) {
    REQUEST_ACTIVE.store(active, Ordering::Relaxed);
}
pub fn storage_frozen(frozen: bool) {
    STORAGE_FROZEN.store(frozen, Ordering::Relaxed);
}

pub fn stop() -> ! {
    if STORAGE_FROZEN.swap(false, Ordering::Relaxed) {
        if let Err(error) = nanami_services::vfs::vfs_thaw(process_slot_descriptor(super::VFS_SLOT))
        {
            libnanami::println!("[acpi-server] unable to thaw storage: {}", error);
        }
    }
    if REQUEST_ACTIVE.swap(false, Ordering::Relaxed) {
        libnanami::println!("[acpi-server] disabled after firmware/host failure; reboot before retrying power management");
        let port = process_slot_descriptor(super::SERVICE_SLOT);
        let mut reply = true;
        loop {
            let event = if reply {
                libnanami::ipc::service_reply_receive_event(
                    port,
                    libnanami::OS_RESPONSE_FATAL,
                    0,
                    0,
                )
            } else {
                libnanami::ipc::service_receive_event(port)
            };
            match event {
                Ok(ServiceEvent::Request(_)) => reply = true,
                Ok(_) => reply = false,
                Err(_) => break,
            }
        }
    }
    let _ = libnanami::request_exit();
    loop {
        libnanami::yield_now();
    }
}
