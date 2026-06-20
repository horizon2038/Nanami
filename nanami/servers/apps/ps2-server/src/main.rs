#![no_std]
#![no_main]

use libnanami::{self, Word};

#[path = "app/constants.rs"]
pub mod constants;
#[path = "app/controller.rs"]
pub mod controller;
#[path = "app/init.rs"]
pub mod init;
#[path = "app/irq.rs"]
pub mod irq;
#[path = "app/logging.rs"]
pub mod logging;
#[path = "app/publish.rs"]
pub mod publish;
#[path = "app/state.rs"]
pub mod state;

use crate::constants::{
    HEARTBEAT_IRQ_INTERVAL, PS2_DATA_PORT, PS2_STATUS_PORT, SLOT_INPUT_NOTIFICATION,
    SLOT_INPUT_SERVICE, SLOT_INTERRUPT_KBD, SLOT_INTERRUPT_MOUSE, SLOT_IO_PORT, SLOT_NOTIFICATION,
};
use crate::controller::drain_controller;
use crate::init::initialize_mouse;
use crate::irq::{ack_waited_irqs, update_irq_counters};
use crate::logging::{busy_delay, log_error, log_request_error};
use crate::publish::publish_pending_events;
use crate::state::{DrainState, Ps2Server};

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[ps2-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    initialize_ipc_and_hardware()?;

    let notif_desc = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq1_desc = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT_KBD);
    let irq12_desc = libnanami::ipc::process_slot_descriptor(SLOT_INTERRUPT_MOUSE);
    let io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_PORT);

    bind_notification(notif_desc)?;
    let (input_port, input_pid) = connect_input_service()?;
    attach_input_drivers(input_port)?;
    let input_queue_vaddr = attach_input_driver_queue(input_port)?;
    let input_notification = attach_input_notification(input_pid)?;

    let mut server = Ps2Server::new(io_desc, input_queue_vaddr, input_notification);
    initialize_mouse(&mut server).map_err(|e| log_error("[ps2-server] init failed: ", e))?;
    arm_irqs(irq1_desc, irq12_desc)?;

    libnanami::print!("[ps2-server] online\n");
    service_loop(&mut server, notif_desc, irq1_desc, irq12_desc)
}

fn initialize_ipc_and_hardware() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[ps2-server] ipc tls init failed: ", e))?;

    libnanami::request_io_port(PS2_DATA_PORT, PS2_STATUS_PORT, SLOT_IO_PORT)
        .map_err(|e| log_error("[ps2-server] request io port failed: ", e))?;

    libnanami::request_irq(1, SLOT_NOTIFICATION, SLOT_INTERRUPT_KBD)
        .map_err(|e| log_error("[ps2-server] request irq1 failed: ", e))?;
    libnanami::print!("[ps2-server] irq1 granted\n");

    libnanami::request_irq(12, SLOT_NOTIFICATION, SLOT_INTERRUPT_MOUSE)
        .map_err(|e| log_error("[ps2-server] request irq12 failed: ", e))?;
    libnanami::print!("[ps2-server] irq12 granted\n");

    Ok(())
}

fn bind_notification(notif_desc: Word) -> libnanami::NanamiResult {
    libnanami::ipc::bind_current_thread_notification(notif_desc)
        .map_err(|e| log_error("[ps2-server] bind notification failed: ", e))
}

fn connect_input_service() -> Result<(Word, Word), libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_input_service_with_pid(SLOT_INPUT_SERVICE) {
            Ok(pid) => {
                return Ok((
                    libnanami::ipc::process_slot_descriptor(SLOT_INPUT_SERVICE),
                    pid,
                ));
            }
            Err(e) => {
                log_request_error("[ps2-server] waiting input-service: ", e);
                busy_delay();
            }
        }
    }
}

fn attach_input_drivers(input_port: Word) -> libnanami::NanamiResult {
    nanami_services::input::input_service_attach_driver(
        input_port,
        nanami_services::input::INPUT_DRIVER_KEYBOARD,
    )
    .map_err(|e| log_error("[ps2-server] input keyboard attach failed: ", e))?;
    libnanami::print!("[ps2-server] keyboard attached\n");

    nanami_services::input::input_service_attach_driver(
        input_port,
        nanami_services::input::INPUT_DRIVER_MOUSE,
    )
    .map_err(|e| log_error("[ps2-server] input mouse attach failed: ", e))?;
    libnanami::print!("[ps2-server] mouse attached\n");

    Ok(())
}

fn attach_input_driver_queue(input_port: Word) -> Result<Word, libnanami::NanamiError> {
    match nanami_services::input::input_service_attach_driver_shared(
        input_port,
        nanami_services::input::INPUT_DRIVER_KEYBOARD,
    ) {
        Ok((queue_vaddr, _queue_bytes)) => Ok(queue_vaddr),
        Err(e) => Err(log_error("[ps2-server] input queue attach failed: ", e)),
    }
}

fn attach_input_notification(input_pid: Word) -> Result<Word, libnanami::NanamiError> {
    libnanami::request_notification_port_copy(
        input_pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        SLOT_INPUT_NOTIFICATION,
        nanami_services::input::INPUT_DRIVER_NOTIFICATION_IDENTIFIER,
    )
    .map_err(|e| log_error("[ps2-server] input notification attach failed: ", e))?;
    Ok(libnanami::ipc::process_slot_descriptor(
        SLOT_INPUT_NOTIFICATION,
    ))
}

fn arm_irqs(irq1_desc: Word, irq12_desc: Word) -> libnanami::NanamiResult {
    libnanami::ipc::interrupt_ack(irq1_desc)
        .map_err(|e| log_error("[ps2-server] irq1 arm failed: ", e))?;
    libnanami::ipc::interrupt_ack(irq12_desc)
        .map_err(|e| log_error("[ps2-server] irq12 arm failed: ", e))?;
    Ok(())
}

fn service_loop(
    server: &mut Ps2Server,
    notif_desc: Word,
    irq1_desc: Word,
    irq12_desc: Word,
) -> libnanami::NanamiResult {
    loop {
        let state = drain_and_publish(server);

        if matches!(state, DrainState::Empty) && !server.has_pending_events() {
            let waited = libnanami::ipc::notification_wait(notif_desc)
                .map_err(|e| log_error("[ps2-server] notification wait failed: ", e))?;

            update_irq_counters(waited, &mut server.irq1_count, &mut server.irq12_count);
            ack_waited_irqs(waited, irq1_desc, irq12_desc)
                .map_err(|e| log_error("[ps2-server] irq ack failed: ", e))?;
        }

        print_heartbeat(server);
    }
}

fn drain_and_publish(server: &mut Ps2Server) -> DrainState {
    let mut state = drain_controller(server);
    publish_pending_events(server);

    while matches!(state, DrainState::ReachedBudget) {
        server.drain_budget_hits = server.drain_budget_hits.wrapping_add(1);
        if (server.drain_budget_hits & 0xff) == 0 {
            libnanami::print!("[ps2-server] drain budget reached\n");
        }

        state = drain_controller(server);
        publish_pending_events(server);
    }

    state
}

fn print_heartbeat(server: &Ps2Server) {
    let irq_total = server.irq1_count.wrapping_add(server.irq12_count);
    if (irq_total % HEARTBEAT_IRQ_INTERVAL) != 0 || irq_total == 0 {
        return;
    }

    libnanami::print!("[ps2-server] alive irq1=");
    libnanami::print!("{}", server.irq1_count);
    libnanami::print!(" irq12=");
    libnanami::print!("{}", server.irq12_count);
    libnanami::print!(" key=");
    libnanami::print!("{}", server.key_count);
    libnanami::print!(" mouse_packet=");
    libnanami::print!("{}", server.mouse_packet_count);
    libnanami::print!(" published=");
    libnanami::print!("{}", server.published_count);
    libnanami::print!("\n");
}

libnanami::nanami_entry!(nanami_main);
