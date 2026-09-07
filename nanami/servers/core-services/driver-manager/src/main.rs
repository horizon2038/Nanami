#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{RequestError, Word};

#[path = "arch.rs"]
mod arch;

const SERVICE_PORT_SLOT: Word = 20;

pub struct TimerSelection {
    pub image: Option<&'static str>,
    pub hpet_mmio_base: Word,
}

struct DriverState {
    hpet_pid: Word,
    hpet_mmio_base: Word,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[driver-manager] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn spawn(image: &str) -> Result<Word, RequestError> {
    let pid = libnanami::request_process_spawn(image)?;
    libnanami::println!("[driver-manager] spawned {} pid={}", image, pid);
    Ok(pid)
}

fn handle_request(request: ServiceRequest, state: &DriverState) -> (Word, Word, Word) {
    match request.code {
        nanami_services::device::DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE
            if request.identifier == state.hpet_pid
                && state.hpet_pid != 0
                && state.hpet_mmio_base != 0 =>
        {
            (libnanami::OS_RESPONSE_OK, state.hpet_mmio_base, 0)
        }
        nanami_services::device::DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE => {
            (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0)
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()?;
    let platform = libnanami::request_nanami_info_platform()?;
    libnanami::println!(
        "[driver-manager] architecture={} platform={} cores={}",
        platform.architecture_name(),
        platform.platform_name(),
        platform.core_count
    );
    if let Err(error) = arch::validate_platform(platform.architecture_name(), platform.platform_name()) {
        libnanami::print!("[driver-manager] no drivers for this architecture/platform\n");
        return Err(error.into());
    }
    libnanami::register_service_by_name(
        nanami_services::device::DEVICE_MANAGER_SERVICE,
        SERVICE_PORT_SLOT,
    )?;

    if let Some(image) = arch::select_storage_driver()? {
        let _ = spawn(image)?;
    }

    let selected_timer = arch::select_timer_driver()?;
    let mut state = DriverState {
        hpet_pid: 0,
        hpet_mmio_base: selected_timer.hpet_mmio_base,
    };

    if let Some(image) = selected_timer.image {
        let pid = spawn(image)?;
        if image == "./bin/hpet-server" {
            state.hpet_pid = pid;
        }
    }

    libnanami::print!("[driver-manager] ready\n");
    let port = libnanami::ipc::process_slot_descriptor(SERVICE_PORT_SLOT);
    let mut reply = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_reply = false;
    loop {
        let event = if has_reply {
            has_reply = false;
            libnanami::ipc::service_reply_receive_event(port, reply.0, reply.1, reply.2)?
        } else {
            libnanami::ipc::service_receive_event(port)?
        };
        match event {
            ServiceEvent::Request(request) => {
                reply = handle_request(request, &state);
                has_reply = true;
            }
            ServiceEvent::Notification { .. } | ServiceEvent::Fault { .. } => {}
        }
    }
}

libnanami::nanami_entry!(nanami_main);
