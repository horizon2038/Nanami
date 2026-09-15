#![no_std]
#![no_main]
extern crate alloc;

mod arch;
mod input;
mod storage;
mod usb;
mod xhci;

use alloc::vec::Vec;
use libnanami::ipc::ServiceEvent;
use libnanami::{RequestError, Word};

const SLOT_DELAY_NOTIFICATION: Word = 26;

fn delay(timer: Word, milliseconds: Word) -> Result<(), RequestError> {
    // The bound notification also receives periodic polling and IRQ events.
    // Neither may satisfy a debounce/reset/transfer wait early.
    nanami_services::timer::timer_service_sleep_on_notification_milliseconds(
        timer,
        milliseconds,
        SLOT_DELAY_NOTIFICATION,
    )
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
        let Some(resource) = nanami_services::device::usb_controller(manager, index)? else {
            break;
        };
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
    let mut storage =
        storage::Storage::initialize(&mut controllers, &mut input, timer, discovery_failed);
    if let Err(error) = serve(&mut controllers, &mut input, &mut storage, timer) {
        libnanami::println!("[usb-server] service failed: {}; retaining DMA", error);
        loop {
            let _ = libnanami::ipc::notification_wait(notification);
        }
    }
    Ok(())
}

fn serve(
    controllers: &mut [xhci::Controller],
    input: &mut input::Input,
    storage: &mut storage::Storage,
    timer: Word,
) -> Result<(), RequestError> {
    libnanami::register_service_by_name("usb-service", 20)?;
    storage.register()?;
    // Maintenance also recovers missed/shared INTx events. No hardware counter
    // is exposed to users; this uses the selected platform timer service.
    nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer,
        10,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )?;
    let port = libnanami::ipc::process_slot_descriptor(20);
    let mut pending = None;
    let mut input_retry = 0u8;
    libnanami::println!("[usb-server] online controllers={}", controllers.len());
    loop {
        for controller in controllers.iter_mut() {
            controller.poll(input);
            if let Some(irq) = controller.irq {
                let _ = libnanami::ipc::interrupt_ack(irq);
            }
            controller.poll(input);
        }
        input.flush();
        if !input.connected() {
            // Also retry under sustained block traffic, without a rootfs boot
            // dependency or a high-priority busy loop. Failed lookups are cheap.
            input_retry = input_retry.wrapping_add(1);
            if input_retry == 1 {
                let _ = input.connect();
            }
        }
        let event = if let Some((status, count, detail)) = pending.take() {
            libnanami::ipc::service_reply_receive_event(port, status, count, detail)?
        } else {
            libnanami::ipc::service_receive_event(port)?
        };
        if let ServiceEvent::Request(request) = event {
            pending = Some(match request.code {
                nanami_services::usb::USB_SERVICE_REQUEST_INFO => {
                    let interfaces: usize = controllers
                        .iter()
                        .map(|controller| controller.counts().1)
                        .sum();
                    (libnanami::OS_RESPONSE_OK, controllers.len(), interfaces)
                }
                _ => storage.request(request, controllers, input),
            });
        }
    }
}

libnanami::nanami_entry!(nanami_main);
