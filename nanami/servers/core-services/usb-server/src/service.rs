use crate::{input::Input, storage::Storage, xhci::Controller};
use nanami_runtime::{ipc::ServiceEvent, RequestError, Word};

// Exactly four opt-in records on successful startup, outside the controller's
// enumeration budget. Never called from maintenance or the request loop.
macro_rules! startup_trace {
    ($($arg:tt)*) => {
        if option_env!("NANAMI_USB_INIT_TRACE") == Some("1") {
            nanami_runtime::println!("[usb.init] {}", format_args!($($arg)*));
        }
    };
}

pub fn serve(
    controllers: &mut [Controller],
    input: &mut Input,
    storage: &mut Storage,
    timer: Word,
) -> Result<(), RequestError> {
    startup_trace!("stage=service-register.begin service=usb-service slot=20");
    let registered = nanami_runtime::register_service_by_name("usb-service", 20);
    startup_trace!("stage=service-register.end ok={}", registered.is_ok());
    registered?;
    // Polling-only hosts and hotplug must progress even without block requests.
    startup_trace!(
        "stage=timer-interval.begin ms=10 notification={}",
        nanami_runtime::PROCESS_SLOT_NOTIFICATION
    );
    let interval = nanami_client::timer::timer_service_interval_on_notification_milliseconds(
        timer,
        10,
        nanami_runtime::PROCESS_SLOT_NOTIFICATION,
    );
    startup_trace!("stage=timer-interval.end ok={}", interval.is_ok());
    interval?;
    for controller in controllers.iter_mut() {
        controller.start_maintenance();
    }
    let port = nanami_runtime::ipc::process_slot_descriptor(20);
    let mut pending = None;
    let mut trace_ticks = 0;
    let mut trace_requests = 0u64;
    let mut trace_irqs = 0u64;
    let profile = option_env!("NANAMI_USB_WAIT_TRACE") == Some("1");
    let sample_clock = |controllers: &[Controller]| {
        profile
            .then(|| controllers.first().and_then(Controller::profile_clock))
            .flatten()
    };
    let mut trace_started = sample_clock(controllers);
    nanami_runtime::diagnostics::clock_info("usb");
    let mut trace_bytes = 0u64;
    let mut trace_service_ns = 0u64;
    let mut trace_service_max_ns = 0u64;
    nanami_runtime::println!("[usb-server] online controllers={}", controllers.len());
    maintain(controllers, input, storage, timer)?;
    loop {
        // No enumeration, HID polling or input-service RPC between completing
        // a block request and replying. Hotplug can wait on hardware for seconds.
        let event = if let Some((status, count, detail)) = pending.take() {
            if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                nanami_runtime::println!(
                    "[usb.request] reply-receive status={:#x} count={:#x}",
                    status,
                    count
                );
            }
            io_trace!("reply-receive status={:#x} count={:#x}", status, count);
            nanami_runtime::ipc::service_reply_receive_event(port, status, count, detail)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                if option_env!("NANAMI_BOOT_PC_TRACE") == Some("1")
                    || option_env!("NANAMI_USB_WAIT_TRACE") == Some("1")
                {
                    trace_requests += 1;
                }
                if option_env!("NANAMI_BOOT_IO_TRACE") == Some("1") {
                    nanami_runtime::println!("[usb.request] request code={:#x}", request.code);
                }
                io_trace!("request code={:#x}", request.code);
                let is_io = matches!(
                    request.code,
                    nanami_client::block::BLOCK_DEVICE_REQUEST_READ
                        | nanami_client::block::BLOCK_DEVICE_REQUEST_WRITE
                );
                let before = sample_clock(controllers);
                let reply = match request.code {
                    nanami_client::usb::USB_SERVICE_REQUEST_INFO => {
                        let interfaces = controllers.iter().map(|c| c.counts().1).sum();
                        (
                            nanami_runtime::OS_RESPONSE_OK,
                            controllers.len(),
                            interfaces,
                        )
                    }
                    _ => storage.request(request, controllers, input),
                };
                if profile && is_io && reply.0 == nanami_runtime::OS_RESPONSE_OK {
                    trace_bytes += reply.1 as u64;
                    if let (Some(before), Some(after)) = (before, sample_clock(controllers)) {
                        let elapsed = after.saturating_sub(before) as u64;
                        trace_service_ns += elapsed;
                        trace_service_max_ns = trace_service_max_ns.max(elapsed);
                    }
                }
                pending = Some(reply);
                // Bulk I/O also dispatches HID completions. This notification
                // is non-blocking; only service connection belongs to maintain.
                input.flush();
            }
            ServiceEvent::Notification { identifier, .. } => {
                // Flush previously captured key records BEFORE this tick's HID
                // polling, never between publishing keys and notifying input.
                if identifier & nanami_client::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT != 0 {
                    nanami_runtime::diagnostics::flush_keys(
                        nanami_runtime::diagnostics::InputStage::Usb,
                    );
                }
                io_trace!("notification badge={:#x}", identifier);
                maintain(controllers, input, storage, timer)?;
                if option_env!("NANAMI_BOOT_PC_TRACE") == Some("1")
                    || option_env!("NANAMI_USB_WAIT_TRACE") == Some("1")
                {
                    let timer_bit = nanami_client::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT;
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
                        nanami_runtime::println!(
                            "[usb.watch] sample begin requests={} irq-events={} timer-events=500",
                            trace_requests,
                            trace_irqs
                        );
                        if nanami_runtime::diagnostics::ENABLED {
                            nanami_runtime::println!(
                                "[pipe.reference] tick={} hpet-ns={}",
                                nanami_runtime::diagnostics::stamp().unwrap_or(0),
                                controllers
                                    .first()
                                    .and_then(Controller::profile_clock)
                                    .unwrap_or(0)
                            );
                        }
                        if option_env!("NANAMI_USB_WAIT_TRACE") == Some("1") {
                            let elapsed = trace_started
                                .zip(sample_clock(controllers))
                                .map_or(0, |(before, after)| after.saturating_sub(before));
                            nanami_runtime::println!(
                                "[usb.io] bytes={} service-us={} max-service-us={} sample-us={}",
                                trace_bytes,
                                trace_service_ns / 1000,
                                trace_service_max_ns / 1000,
                                elapsed / 1000
                            );
                            for controller in controllers.iter_mut() {
                                controller.print_wait_profile();
                            }
                        }
                        if option_env!("NANAMI_BOOT_PC_TRACE") == Some("1") {
                            let result =
                                nanami_runtime::request_nanami_control("boot.pc", "sample");
                            nanami_runtime::println!("[usb.watch] sample end result={:?}", result);
                        }
                        trace_requests = 0;
                        trace_irqs = 0;
                        trace_bytes = 0;
                        trace_service_ns = 0;
                        trace_service_max_ns = 0;
                        trace_started = sample_clock(controllers);
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
            nanami_runtime::ipc::interrupt_ack(irq)?;
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
