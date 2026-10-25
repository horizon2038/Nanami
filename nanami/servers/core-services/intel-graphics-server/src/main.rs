#![no_std]
#![no_main]
#[cfg(not(target_arch = "x86_64"))]
compile_error!("Intel Gen9 graphics requires the x86_64/pc99 backend");
#[path = "x86_64/cache.rs"]
mod cache;
mod connector;
mod ddc;
mod edid;
mod gen9;
mod hardware;
mod vbt;
use hardware::Scanout;
use nanami_client::{device, gfx, pci};
use nanami_runtime::{
    ipc::{ServiceEvent, ServiceRequest},
    RequestError, Word,
};

const PCI_SLOT: Word = 16;
const SERVICE_SLOT: Word = 20;
const TIMER_SLOT: Word = 24;
const MANAGER_SLOT: Word = 25;

#[derive(Default)]
struct Shared {
    owner: Word,
    local: Word,
    peer: Word,
}

fn retain_scanout() -> ! {
    let notification =
        nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    loop {
        let _ = nanami_runtime::ipc::notification_wait(notification);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[intel-graphics] panic; retaining hardware resources");
    retain_scanout()
}

fn initialize(port: Word) -> Result<Scanout, RequestError> {
    let platform = nanami_runtime::request_nanami_info_platform()?;
    if platform.architecture_name() != "x86_64" || platform.platform_name() != "pc99" {
        return Err(RequestError::Unsupported);
    }
    nanami_client::registry::connect_boot_service("timer-service", TIMER_SLOT)?;
    let timer = nanami_runtime::ipc::process_slot_descriptor(TIMER_SLOT);
    let mut index = 0;
    while let Some(device) = pci::enumerate(port, index)? {
        if let Some(generation) = nanami_client::uapi::intel_graphics::generation(
            device.vendor_id,
            device.device_id,
            device.class,
        ) {
            nanami_runtime::println!(
                "[intel-graphics] PCI {:06x} device={:04x} generation={:?}",
                device.address.packed(),
                device.device_id,
                generation
            );
            let mut scanout = Scanout::new(port, device, timer)?;
            scanout.inspect_output(port, device);
            return Ok(scanout);
        }
        index += 1;
    }
    Err(RequestError::Unsupported)
}

fn request(
    request: ServiceRequest,
    display: &mut Scanout,
    shared: &mut Shared,
) -> Result<(Word, Word), RequestError> {
    let mode = display.mode;
    match request.code {
        gfx::DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO => {
            let geometry = (32 << 16) | (mode.width << 32) | (mode.height << 48);
            let format = mode.stride | (16 << 32) | (8 << 37) | (8 << 42) | (8 << 47) | (8 << 57);
            Ok((geometry, format))
        }
        gfx::DISPLAY_SERVICE_REQUEST_GET_CAPABILITIES => Ok((display.capabilities(), 0)),
        gfx::DISPLAY_SERVICE_REQUEST_CONNECTOR_INFO => {
            if request.arg0 != 0 {
                return Ok((0, 0));
            }
            Ok(display
                .output
                .as_ref()
                .map_or((0, 0), |connector| connector.info().words()))
        }
        gfx::DISPLAY_SERVICE_REQUEST_READ_EDID => {
            if request.arg0 != 0 {
                return Err(RequestError::InvalidArgument);
            }
            display
                .output
                .as_ref()
                .ok_or(RequestError::Unsupported)?
                .read_edid(request.arg1)
        }
        gfx::DISPLAY_SERVICE_REQUEST_MONITOR_MODE => {
            if request.arg0 != 0 {
                return Err(RequestError::InvalidArgument);
            }
            display
                .output
                .as_ref()
                .ok_or(RequestError::Unsupported)?
                .monitor_mode(request.arg1, request.arg2)
        }
        gfx::DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER => {
            if request.identifier == 0 || (shared.owner != 0 && shared.owner != request.identifier)
            {
                return Err(RequestError::Status(
                    nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
                ));
            }
            if shared.owner == 0 {
                let (local, peer) =
                    nanami_runtime::request_shared_memory(request.identifier, mode.bytes)?;
                display.copy_current(local);
                *shared = Shared {
                    owner: request.identifier,
                    local,
                    peer,
                };
            }
            Ok((shared.peer, mode.bytes))
        }
        gfx::DISPLAY_SERVICE_REQUEST_PRESENT => {
            if shared.owner == 0 || shared.owner != request.identifier {
                return Err(RequestError::Status(
                    nanami_runtime::OS_RESPONSE_PERMISSION_DENIED,
                ));
            }
            let rect = gen9::Rect::checked(mode, request.arg0, request.arg1)?;
            let frame = display.present(shared.local, rect)?;
            Ok((rect.width * rect.height, frame))
        }
        _ => Err(RequestError::Unsupported),
    }
}

fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(value) => value,
        RequestError::Protocol | RequestError::Transport => nanami_runtime::OS_RESPONSE_FATAL,
    }
}

fn serve(display: &mut Scanout) -> Result<(), RequestError> {
    let port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_SLOT);
    let mut shared = Shared::default();
    let mut reply = None;
    loop {
        let event = if let Some((status, a, b)) = reply.take() {
            nanami_runtime::ipc::service_reply_receive_event(port, status, a, b)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        if let ServiceEvent::Request(message) = event {
            reply = Some(match request(message, display, &mut shared) {
                Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
                Err(error) => (status(error), 0, 0),
            });
        }
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::connect_service_by_name(device::DEVICE_MANAGER_SERVICE, MANAGER_SLOT)?;
    let manager = nanami_runtime::ipc::process_slot_descriptor(MANAGER_SLOT);
    let result = pci::connect(PCI_SLOT).and_then(initialize);
    let mut display = match result {
        Ok(display) => display,
        Err(error) => {
            nanami_runtime::println!("[intel-graphics] native initialization unavailable: {}; requesting framebuffer fallback", error);
            device::report_display_probe(manager, false)?;
            return Ok(());
        }
    };
    if let Err(error) = nanami_client::registry::register_display_service() {
        display.detach_before_flip();
        device::report_display_probe(manager, false)?;
        return Err(error.into());
    }
    if let Err(error) = device::report_display_probe(manager, true) {
        // The service is already published and callers may hold its port.
        // Keep serving instead of exiting and leaving them blocked. A failed
        // readiness reply is not permission to start another display owner.
        nanami_runtime::println!(
            "[intel-graphics] readiness report failed: {}; keeping published display",
            error
        );
    }
    if let Err(error) = serve(&mut display) {
        nanami_runtime::println!(
            "[intel-graphics] service failed: {}; retaining scanout resources",
            error
        );
        retain_scanout();
    }
    Ok(())
}

nanami_runtime::nanami_entry!(nanami_main);
