#![no_std]
#![no_main]

use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{RequestError, Word};

#[path = "arch.rs"]
mod arch;
mod storage;
mod usb_irq;

const SERVICE_PORT_SLOT: Word = 20;

pub struct TimerSelection {
    pub image: Option<&'static str>,
    pub hpet_mmio_base: Word,
}

struct DriverState {
    storage: storage::Selection,
    hpet_pid: Word,
    hpet_mmio_base: Word,
    acpi_pid: Word,
    acpi_rsdp: Word,
    usb_pid: Word,
    usb_controllers: [Option<nanami_client::device::UsbControllerResource>; 4],
    usb_count: usize,
    usb_routes: usb_irq::Routes,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[driver-manager] panic\n");
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn spawn(image: &str) -> Result<Word, RequestError> {
    let pid = nanami_runtime::request_process_spawn(image)?;
    nanami_runtime::println!("[driver-manager] spawned {} pid={}", image, pid);
    Ok(pid)
}

fn handle_request(request: ServiceRequest, state: &mut DriverState) -> (Word, Word, Word) {
    match request.code {
        nanami_client::device::DEVICE_MANAGER_REQUEST_USB_PCI_ROUTE
        | nanami_client::device::DEVICE_MANAGER_REQUEST_USB_IRQ_REPORT => state
            .usb_routes
            .handle(request, state.acpi_pid, &mut state.usb_controllers),
        nanami_client::device::DEVICE_MANAGER_REQUEST_USB_IRQ => {
            if state.usb_pid == 0 || request.identifier != state.usb_pid {
                return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            if request.arg0 >= state.usb_count {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            match state.usb_routes.query(request.arg0, state.acpi_pid) {
                Ok(ready) => (
                    nanami_runtime::OS_RESPONSE_OK,
                    ready as Word,
                    state.usb_controllers[request.arg0]
                        .unwrap()
                        .irq
                        .unwrap_or(0),
                ),
                Err(error) => {
                    nanami_runtime::println!("[driver-manager] ACPI IRQ dispatch failed: {}", error);
                    (nanami_runtime::OS_RESPONSE_FATAL, 0, 0)
                }
            }
        }
        nanami_client::device::DEVICE_MANAGER_REQUEST_ACPI_RSDP => {
            if state.acpi_pid != 0 && request.identifier == state.acpi_pid {
                (nanami_runtime::OS_RESPONSE_OK, state.acpi_rsdp, 0)
            } else {
                (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0)
            }
        }
        nanami_client::device::DEVICE_MANAGER_REQUEST_STORAGE_PROBE => {
            match state.storage.report(request.identifier, request.arg0) {
                Ok(decision) => (nanami_runtime::OS_RESPONSE_OK, decision, 0),
                Err(()) => (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0),
            }
        }
        nanami_client::device::DEVICE_MANAGER_REQUEST_USB_CONTROLLER => {
            if state.usb_pid == 0 || request.identifier != state.usb_pid {
                (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0)
            } else if request.arg0 < state.usb_count {
                let resource = state.usb_controllers[request.arg0].unwrap();
                (
                    nanami_runtime::OS_RESPONSE_OK,
                    resource.physical,
                    resource.bytes | resource.irq.unwrap_or(0),
                )
            } else {
                (nanami_runtime::OS_RESPONSE_OK, 0, 0)
            }
        }
        nanami_client::device::DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE
            if request.identifier == state.hpet_pid
                && state.hpet_pid != 0
                && state.hpet_mmio_base != 0 =>
        {
            (nanami_runtime::OS_RESPONSE_OK, state.hpet_mmio_base, 0)
        }
        nanami_client::device::DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE => {
            (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0)
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let platform = nanami_runtime::request_nanami_info_platform()?;
    nanami_runtime::println!(
        "[driver-manager] architecture={} platform={} cores={}",
        platform.architecture_name(),
        platform.platform_name(),
        platform.core_count
    );
    if let Err(error) =
        arch::validate_platform(platform.architecture_name(), platform.platform_name())
    {
        nanami_runtime::print!("[driver-manager] no drivers for this architecture/platform\n");
        return Err(error.into());
    }
    nanami_runtime::register_service_by_name(
        nanami_client::device::DEVICE_MANAGER_SERVICE,
        SERVICE_PORT_SLOT,
    )?;

    // Finish PCI discovery before launching any driver that configures PCI.
    let storage_image = arch::select_storage_driver()?;
    let selected_timer = arch::select_timer_driver()?;
    let mut state = DriverState {
        storage: storage::Selection::new(),
        hpet_pid: 0,
        hpet_mmio_base: selected_timer.hpet_mmio_base,
        acpi_pid: 0,
        acpi_rsdp: 0,
        usb_pid: 0,
        usb_controllers: [None; 4],
        usb_count: 0,
        usb_routes: usb_irq::Routes::default(),
    };
    for index in 0..state.usb_controllers.len() {
        let Some(address) = arch::usb_controller(index)? else {
            break;
        };
        match arch::prepare_usb_controller(address) {
            Ok((resource, pin)) => {
                state
                    .usb_routes
                    .add(state.usb_count, address, pin, resource.irq);
                state.usb_controllers[state.usb_count] = Some(resource);
                state.usb_count += 1;
            }
            Err(error) => nanami_runtime::println!(
                "[driver-manager] USB PCI {:06x} skipped: {}",
                address,
                error
            ),
        }
    }
    if let Some(image) = storage_image {
        state.storage.pids[0] = spawn(image)?;
    }

    if let Some(image) = selected_timer.image {
        let pid = spawn(image)?;
        if image == "./bin/hpet-server" {
            state.hpet_pid = pid;
        }
    }

    if state.usb_count != 0 {
        match spawn("./bin/usb-server") {
            Ok(pid) => {
                state.usb_pid = pid;
                state.storage.pids[1] = pid;
            }
            Err(error) => nanami_runtime::println!("[driver-manager] USB spawn failed: {}", error),
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        state.acpi_rsdp =
            nanami_runtime::request_driver_platform_info(nanami_runtime::DRIVER_PLATFORM_INFO_RSDP_ADDRESS)?
                .0;
        if state.acpi_rsdp != 0 {
            match spawn("./bin/acpi-server") {
                Ok(pid) => state.acpi_pid = pid,
                Err(error) => nanami_runtime::println!("[driver-manager] ACPI spawn failed: {}", error),
            }
        }
    }
    nanami_runtime::print!("[driver-manager] ready\n");
    let port = nanami_runtime::ipc::process_slot_descriptor(SERVICE_PORT_SLOT);
    let mut reply = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            nanami_runtime::ipc::service_reply_receive_event(port, reply.0, reply.1, reply.2)?
        } else {
            nanami_runtime::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                reply = handle_request(request, &mut state);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => {}
        }
    }
}

nanami_runtime::nanami_entry!(nanami_main);
