use crate::{input::Input, storage::Storage, xhci::Controller};
use libnanami::{ipc::ServiceEvent, RequestError, Word};

pub fn serve(
    controllers: &mut [Controller],
    input: &mut Input,
    storage: &mut Storage,
    timer: Word,
) -> Result<(), RequestError> {
    libnanami::register_service_by_name("usb-service", 20)?;
    // Polling-only hosts and hotplug must progress even without block requests.
    nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer,
        10,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )?;
    let port = libnanami::ipc::process_slot_descriptor(20);
    let mut pending = None;
    let mut trace_ticks = 0;
    let mut trace_requests = 0u64;
    let mut trace_irqs = 0u64;
    libnanami::println!("[usb-server] online controllers={}", controllers.len());
    maintain(controllers, input, storage, timer)?;
    loop {
        // No enumeration, HID polling or input-service RPC between completing
        // a block request and replying. Hotplug can wait on hardware for seconds.
        let event = if let Some((status, count, detail)) = pending.take() {
            if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                libnanami::println!(
                    "[usb.request] reply-receive status={:#x} count={:#x}",
                    status,
                    count
                );
            }
            io_trace!("reply-receive status={:#x} count={:#x}", status, count);
            libnanami::ipc::service_reply_receive_event(port, status, count, detail)?
        } else {
            libnanami::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                if option_env!("NANAMI_BOOT_PC_TRACE") == Some("1") {
                    trace_requests += 1;
                }
                if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                    libnanami::println!("[usb.request] request code={:#x}", request.code);
                }
                io_trace!("request code={:#x}", request.code);
                pending = Some(match request.code {
                    nanami_services::usb::USB_SERVICE_REQUEST_INFO => {
                        let interfaces = controllers.iter().map(|c| c.counts().1).sum();
                        (libnanami::OS_RESPONSE_OK, controllers.len(), interfaces)
                    }
                    _ => storage.request(request, controllers, input),
                });
                // Bulk I/O also dispatches HID completions. This notification
                // is non-blocking; only service connection belongs to maintain.
                input.flush();
            }
            ServiceEvent::Notification { identifier, .. } => {
                io_trace!("notification badge={:#x}", identifier);
                maintain(controllers, input, storage, timer)?;
                if option_env!("NANAMI_BOOT_PC_TRACE") == Some("1") {
                    let timer_bit = nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT;
                    trace_irqs += u64::from(identifier & !timer_bit != 0);
                    if identifier & timer_bit == 0 {
                        continue;
                    }
                    trace_ticks += 1;
                    if trace_ticks == 500 {
                        trace_ticks = 0;
                        // Sample only after replying and finishing maintenance.
                        // Separate from per-I/O tracing: serial output can dominate
                        // boot time. Count 10 ms events for a nominal 5 s interval
                        // without an extra clock RPC.
                        libnanami::println!(
                            "[usb.watch] sample begin requests={} irq-events={} timer-events=500",
                            trace_requests,
                            trace_irqs
                        );
                        let result = libnanami::request_nanami_control("boot.pc", "sample");
                        libnanami::println!("[usb.watch] sample end result={:?}", result);
                        trace_requests = 0;
                        trace_irqs = 0;
                    }
                }
            }
            ServiceEvent::Fault { .. } => {}
        }
    }
}

fn maintain(
    controllers: &mut [Controller],
    input: &mut Input,
    storage: &mut Storage,
    timer: Word,
) -> Result<(), RequestError> {
    io_trace!("maintenance begin");
    for controller in controllers.iter_mut() {
        controller.poll(input);
        if let Some(irq) = controller.irq {
            libnanami::ipc::interrupt_ack(irq)?;
            // Close the masked IRQ window. Poll-only controllers need no second pass.
            controller.poll(input);
        }
    }
    storage.poll(controllers, input, timer)?;
    if !input.connected() {
        // Retry on maintenance notifications, never once per disk transaction.
        io_trace!("input connect begin");
        let _ = input.connect();
        io_trace!("input connect end connected={}", input.connected());
    }
    input.flush();
    io_trace!("maintenance end");
    Ok(())
}
