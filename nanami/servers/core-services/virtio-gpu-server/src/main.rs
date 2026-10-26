#![no_std]
#![no_main]
#[cfg(not(target_arch = "x86_64"))]
compile_error!("virtio-gpu currently uses the pc99 modern PCI transport");
mod gpu;
mod pci;
mod queue;
mod wire;
use nanami_client::{device, gfx};
use nanami_runtime::{
    ipc::{ServiceEvent, ServiceRequest},
    RequestError, Word,
};
use queue::Queue;
const PCI_SLOT: Word = 16;
const SERVICE_SLOT: Word = 20;
const TIMER_SLOT: Word = 24;
const MANAGER_SLOT: Word = 25;

fn retain_resources() -> ! {
    let notification =
        nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    loop {
        let _ = nanami_runtime::ipc::notification_wait(notification);
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[virtio-gpu] panic; retaining DMA and device ownership");
    retain_resources()
}

fn transport(port: Word) -> Result<pci::Pci, RequestError> {
    let platform = nanami_runtime::request_nanami_info_platform()?;
    if platform.architecture_name() != "x86_64" || platform.platform_name() != "pc99" {
        return Err(RequestError::Unsupported);
    }
    let mut ordinal = 0;
    while let Some(device) = nanami_client::pci::enumerate(port, ordinal)? {
        if pci::matches(device) {
            nanami_runtime::println!(
                "[virtio-gpu] PCI {:06x} modern GPU device={:04x}",
                device.address.packed(),
                device.device_id
            );
            return pci::Pci::new(port, device);
        }
        ordinal += 1;
    }
    Err(RequestError::Unsupported)
}
fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(status) => status,
        RequestError::Protocol | RequestError::Transport => nanami_runtime::OS_RESPONSE_FATAL,
    }
}
fn legacy(
    request: ServiceRequest,
    display: &mut gpu::Display<pci::Pci>,
) -> Result<(Word, Word), RequestError> {
    let mode = display.layout;
    match request.code {
        gfx::DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO => Ok((
            (32 << 16) | (mode.width << 32) | (mode.height << 48),
            mode.stride | (16 << 32) | (8 << 37) | (8 << 42) | (8 << 47) | (8 << 57),
        )),
        // Native virtio scanout is not a hardware vblank subscription or 3D GPU.
        gfx::DISPLAY_SERVICE_REQUEST_GET_CAPABILITIES => Ok((
            gfx::DISPLAY_CAP_NATIVE_SCANOUT | gfx::DISPLAY_CAP_GPU_INTERFACE,
            0,
        )),
        gfx::DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER => {
            display.legacy_buffer(request.identifier)
        }
        gfx::DISPLAY_SERVICE_REQUEST_PRESENT => {
            display.legacy_present(request.identifier, request.arg0, request.arg1)
        }
        _ => Err(RequestError::Unsupported),
    }
}
fn serve(display: &mut gpu::Display<pci::Pci>) -> Result<(), RequestError> {
    let port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_SLOT);
    let mut service = nanami_gpu::Service::default();
    let mut reply = None;
    loop {
        let event = if let Some((status, a, b)) = reply.take() {
            nanami_runtime::ipc::service_reply_receive_event(port, status, a, b)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        if let ServiceEvent::Request(message) = event {
            let result = if nanami_gpu::Service::handles(message.code) {
                service.request(
                    message.identifier,
                    message.code,
                    [message.arg0, message.arg1, message.arg2, message.arg3],
                    display,
                )
            } else {
                legacy(message, display)
            };
            reply = Some(match result {
                Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
                Err(error) => (status(error), 0, 0),
            });
        }
    }
}
fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::connect_service_by_name(device::DEVICE_MANAGER_SERVICE, MANAGER_SLOT)?;
    let manager = nanami_runtime::ipc::process_slot_descriptor(MANAGER_SLOT);
    let setup = (|| {
        nanami_client::registry::connect_boot_service("timer-service", TIMER_SLOT)?;
        let port = nanami_client::pci::connect(PCI_SLOT)?;
        transport(port)
    })();
    let transport = match setup {
        Ok(transport) => transport,
        Err(error) => {
            nanami_runtime::println!(
                "[virtio-gpu] unavailable: {}; requesting framebuffer fallback",
                error
            );
            device::report_display_probe(manager, false)?;
            return Ok(());
        }
    };
    let timer = nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT);
    let mut queue = Queue::new(transport, timer);
    let setup = queue.initialize().and_then(|_| {
        let reply = queue.execute(
            &wire::Command::display_info(),
            wire::OK_DISPLAY_INFO,
            wire::DISPLAY_INFO_BYTES,
        )?;
        wire::display_info(reply)
    });
    let (scanout, layout) = match setup {
        Ok(info) => info,
        Err(error) => {
            nanami_runtime::println!(
                "[virtio-gpu] initialization failed before scanout: {}",
                error
            );
            let stopped = queue.reset().is_ok();
            let report = device::report_display_probe(manager, false);
            if !stopped {
                retain_resources();
            }
            report?;
            return Ok(());
        }
    };
    let mut display = gpu::Display::new(queue, scanout, layout);
    if let Err(error) = nanami_client::registry::register_display_service() {
        let stopped = display.queue.reset().is_ok();
        let report = device::report_display_probe(manager, false);
        if !stopped {
            retain_resources();
        }
        report?;
        return Err(error.into());
    }
    nanami_runtime::println!(
        "[virtio-gpu] native 2D scanout={} {}x{} stride={} ready; fill/copy=CPU",
        scanout,
        layout.width,
        layout.height,
        layout.stride
    );
    if let Err(error) = device::report_display_probe(manager, true) {
        nanami_runtime::println!(
            "[virtio-gpu] readiness report failed: {}; keeping published display",
            error
        );
    }
    if let Err(error) = nanami_client::registry::register_service(nanami_client::gpu::GPU_SERVICE) {
        nanami_runtime::println!(
            "[virtio-gpu] GPU alias unavailable: {}; display endpoint remains usable",
            error
        );
    }
    if let Err(error) = serve(&mut display) {
        nanami_runtime::println!(
            "[virtio-gpu] service failed: {}; retaining resources",
            error
        );
        retain_resources();
    }
    Ok(())
}
nanami_runtime::nanami_entry!(nanami_main);
