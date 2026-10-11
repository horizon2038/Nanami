#![no_std]
#![no_main]
extern crate alloc;

// Opt-in only: serial tracing changes timing and is unsuitable for benchmarks.
macro_rules! io_trace {
    ($($arg:tt)*) => {
        if option_env!("NANAMI_USB_IO_TRACE") == Some("1") {
            libnanami::println!("[usb.trace] {}", format_args!($($arg)*));
        }
    };
}

mod arch;
mod input;
mod service;
mod storage;
mod usb;
mod xhci;

use alloc::vec::Vec;
use libnanami::{RequestError, Word};

const SLOT_DELAY_NOTIFICATION: Word = 26;

fn delay(timer: Word, milliseconds: Word) -> Result<(), RequestError> {
    // The bound notification also receives periodic polling and IRQ events.
    // Neither may satisfy a debounce/reset delay early. Transfer completion
    // waits instead use IRQ-or-deadline on the bound notification.
    io_trace!("timer wait begin ms={}", milliseconds);
    let result = nanami_services::timer::timer_service_sleep_on_notification_milliseconds(
        timer,
        milliseconds,
        SLOT_DELAY_NOTIFICATION,
    );
    io_trace!("timer wait end ok={}", result.is_ok());
    result
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // Do not exit and release DMA mappings while hardware could still own them.
    libnanami::print!("[usb-server] panic; DMA mappings retained\n");
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    loop {
        let _ = libnanami::ipc::notification_wait(notification);
    }
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    libnanami::ipc::bind_current_thread_notification(notification)?;
    libnanami::request_notification_port_create(
        SLOT_DELAY_NOTIFICATION,
        nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )?;
    loop {
        if nanami_services::registry::connect_timer_service(24).is_ok() {
            break;
        }
        libnanami::yield_now();
    }
    let timer = libnanami::ipc::process_slot_descriptor(24);
    libnanami::connect_service_by_name(nanami_services::device::DEVICE_MANAGER_SERVICE, 25)?;
    let manager = libnanami::ipc::process_slot_descriptor(25);
    let mut resources = Vec::new();
    for index in 0..4 {
        let Some(mut resource) = nanami_services::device::usb_controller(manager, index)? else {
            break;
        };
        if option_env!("NANAMI_USB_POLL_ONLY") != Some("1") {
            for attempt in 0..3000 {
                match nanami_services::device::usb_irq(manager, index) {
                    Ok(Some(irq)) => {
                        resource.irq = irq;
                        break;
                    }
                    Ok(None) if attempt != 2999 => delay(timer, 10)?,
                    result => {
                        libnanami::println!("[usb-server] controller {} ACPI IRQ unavailable ({:?}); polling fallback", index, result);
                        break;
                    }
                }
            }
        }
        resources.push(resource);
    }
    let mut input = input::Input::new();
    // Storage must work before input-service, which itself lives in rootfs.
    let mut controllers = Vec::new();
    let mut discovery_failed = false;
    for (index, resource) in resources.into_iter().enumerate() {
        let controller = arch::prepare(resource)
            .and_then(|resource| xhci::Controller::initialize(resource, timer, 32 + index));
        match controller {
            Ok(mut controller) => {
                controller.poll(&mut input);
                controller.enable_interrupts();
                controllers.push(controller);
            }
            Err(error) => {
                discovery_failed = true;
                libnanami::println!("[usb-server] controller {} unavailable: {}", index, error)
            }
        }
    }
    // Once controllers run, errors must not exit and release their DMA memory.
    let mut storage = storage::Storage::new(manager, discovery_failed);
    if let Err(error) = service::serve(&mut controllers, &mut input, &mut storage, timer) {
        libnanami::println!("[usb-server] service failed: {}; retaining DMA", error);
        loop {
            let _ = libnanami::ipc::notification_wait(notification);
        }
    }
    Ok(())
}

libnanami::nanami_entry!(nanami_main);
