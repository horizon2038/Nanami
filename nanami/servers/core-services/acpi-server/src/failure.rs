//! AML/host callbacks are infallible in parts of acpi's API. A panic disables
//! this service, but must not strand the caller or leave storage frozen.
use core::sync::atomic::{AtomicBool, Ordering};
use nanami_runtime::ipc::{process_slot_descriptor, ServiceEvent};

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
        if let Err(error) = nanami_client::vfs::vfs_thaw(process_slot_descriptor(super::VFS_SLOT))
        {
            nanami_runtime::println!("[acpi-server] unable to thaw storage: {}", error);
        }
    }
    if REQUEST_ACTIVE.swap(false, Ordering::Relaxed) {
        nanami_runtime::println!("[acpi-server] disabled after firmware/host failure; reboot before retrying power management");
        let port = process_slot_descriptor(super::SERVICE_SLOT);
        let mut reply = true;
        loop {
            let event = if reply {
                nanami_runtime::ipc::service_reply_receive_event(
                    port,
                    nanami_runtime::OS_RESPONSE_FATAL,
                    0,
                    0,
                )
            } else {
                nanami_runtime::ipc::service_receive_event(port)
            };
            match event {
                Ok(ServiceEvent::Request(_)) => reply = true,
                Ok(_) => reply = false,
                Err(_) => break,
            }
        }
    }
    let _ = nanami_runtime::request_exit();
    loop {
        nanami_runtime::yield_now();
    }
}
