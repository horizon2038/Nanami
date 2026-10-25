#![no_std]
#![no_main]
extern crate alloc;

#[cfg(not(target_arch = "x86_64"))]
compile_error!("pci-server's current configuration backend is x86_64/pc99");

mod backend;
mod config;
mod names;
use config::Access;
use nanami_client::pci::*;
use nanami_runtime::{
    ipc::{ServiceEvent, ServiceRequest},
    RequestError, Word,
};
const SERVICE_SLOT: Word = 20;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[pci-server] panic");
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn handle(
    request: ServiceRequest,
    backend: &mut backend::Backend,
    devices: &[Device],
    names: &[names::Names],
) -> Result<(Word, Word), RequestError> {
    if request.code == PCI_REQUEST_ENUMERATE {
        return Ok(devices
            .get(request.arg0)
            .map_or((PCI_ENUM_END, 0), |device| {
                (device.address.packed(), device.packed_info())
            }));
    }
    if request.code == PCI_REQUEST_NAME {
        return names
            .get(request.arg0)
            .ok_or(RequestError::InvalidArgument)?
            .chunk(request.arg1, request.arg2);
    }
    let address = Address::from_packed(request.arg0).ok_or(RequestError::InvalidArgument)?;
    match request.code {
        PCI_REQUEST_READ => backend
            .read(address, request.arg1, request.arg2)
            .map(|value| (value as Word, 0)),
        PCI_REQUEST_WRITE => {
            backend.write(
                address,
                request.arg1 & 0xffff,
                request.arg1 >> 16,
                request.arg2 as u32,
            )?;
            Ok((0, 0))
        }
        PCI_REQUEST_PROBE_BAR => {
            let device = devices
                .iter()
                .find(|device| device.address == address)
                .ok_or(RequestError::InvalidArgument)?;
            let bar = config::probe_bar(backend, *device, request.arg1)?;
            let flags = if bar.io { PCI_BAR_IO } else { 0 }
                | if bar.prefetchable {
                    PCI_BAR_PREFETCHABLE
                } else {
                    0
                };
            Ok((bar.physical, bar.bytes | flags))
        }
        _ => Err(RequestError::Unsupported),
    }
}

fn status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Status(status) => status,
        RequestError::Transport | RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    let platform = nanami_runtime::request_nanami_info_platform()?;
    if platform.architecture_name() != "x86_64" || platform.platform_name() != "pc99" {
        return Err(RequestError::Unsupported.into());
    }
    let rsdp = nanami_runtime::request_driver_platform_info(
        nanami_runtime::DRIVER_PLATFORM_INFO_RSDP_ADDRESS,
    )?
    .0;
    let mut backend = backend::Backend::new(rsdp)?;
    let devices = config::scan(&mut backend)?;
    let mut names = alloc::vec::Vec::new();
    names
        .try_reserve(devices.len())
        .map_err(|_| RequestError::Unsupported)?;
    for device in &devices {
        let identity = names::Names::identify(*device);
        nanami_runtime::println!(
            "[pci-server] {:04x}:{:02x}:{:02x}.{} id={:04x}:{:04x} class={:06x} {} / {} / {}",
            device.address.segment,
            device.address.bus,
            device.address.device,
            device.address.function,
            device.vendor_id,
            device.device_id,
            device.class,
            identity.get(PCI_NAME_VENDOR)?,
            identity.get(PCI_NAME_DEVICE)?,
            identity.get(PCI_NAME_SUBCLASS)?,
        );
        names.push(identity);
    }
    nanami_runtime::register_service_by_name(PCI_SERVICE, SERVICE_SLOT)?;
    nanami_runtime::println!("[pci-server] ready devices={}", devices.len());
    let port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_SLOT);
    let mut reply = None;
    loop {
        let event = if let Some((status, a, b)) = reply.take() {
            nanami_runtime::ipc::service_reply_receive_event(port, status, a, b)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        if let ServiceEvent::Request(request) = event {
            reply = Some(match handle(request, &mut backend, &devices, &names) {
                Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
                Err(error) => (status(error), 0, 0),
            });
        }
    }
}

nanami_runtime::nanami_entry!(nanami_main);
