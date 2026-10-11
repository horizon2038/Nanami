#![no_std]
#![no_main]

use nanami_runtime::{self, Word};

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
#[path = "app/wait.rs"]
mod wait;

use crate::constants::{
    HEARTBEAT_IRQ_INTERVAL, PS2_DATA_PORT, PS2_STATUS_PORT, SLOT_INPUT_NOTIFICATION,
    SLOT_INPUT_SERVICE, SLOT_INTERRUPT_KBD, SLOT_INTERRUPT_MOUSE, SLOT_IO_PORT, SLOT_NOTIFICATION,
};
use crate::controller::drain_controller;
use crate::init::initialize_mouse;
use crate::irq::{ack_waited_irqs, update_irq_counters};
use crate::logging::{log_error, log_request_error};
use crate::publish::publish_pending_events;
use crate::state::{DrainState, Ps2Server};

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[ps2-server] panic\n");
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    initialize_ipc_and_hardware()?;

    let notif_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    let irq1_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_INTERRUPT_KBD);
    let irq12_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_INTERRUPT_MOUSE);
    let io_desc = nanami_runtime::ipc::process_slot_descriptor(SLOT_IO_PORT);

    controller::read_status(io_desc)
        .map_err(|e| log_error("[ps2-server] controller unavailable: ", e))?;
    wait::initialize().map_err(|e| log_error("[ps2-server] timer init failed: ", e))?;
    bind_notification(notif_desc)?;
    let (input_port, input_pid) = connect_input_service()?;
    attach_input_drivers(input_port)?;
    let input_queue_vaddr = attach_input_driver_queue(input_port)?;
    let input_notification = attach_input_notification(input_pid)?;

    let mut server = Ps2Server::new(io_desc, input_queue_vaddr, input_notification);
    nanami_runtime::print!("[ps2-server] initializing controller\n");
    initialize_mouse(&mut server).map_err(|e| log_error("[ps2-server] init failed: ", e))?;
    arm_irqs(irq1_desc, irq12_desc)?;

    nanami_runtime::print!("[ps2-server] online\n");
    service_loop(&mut server, notif_desc, irq1_desc, irq12_desc)
}

fn initialize_ipc_and_hardware() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()
        .map_err(|e| log_error("[ps2-server] ipc tls init failed: ", e))?;

    nanami_runtime::request_io_port(PS2_DATA_PORT, PS2_STATUS_PORT, SLOT_IO_PORT)
        .map_err(|e| log_error("[ps2-server] request io port failed: ", e))?;

    nanami_runtime::request_irq(1, SLOT_NOTIFICATION, SLOT_INTERRUPT_KBD)
        .map_err(|e| log_error("[ps2-server] request irq1 failed: ", e))?;
    nanami_runtime::print!("[ps2-server] irq1 granted\n");

    nanami_runtime::request_irq(12, SLOT_NOTIFICATION, SLOT_INTERRUPT_MOUSE)
        .map_err(|e| log_error("[ps2-server] request irq12 failed: ", e))?;
    nanami_runtime::print!("[ps2-server] irq12 granted\n");

    Ok(())
}

fn bind_notification(notif_desc: Word) -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::bind_current_thread_notification(notif_desc)
        .map_err(|e| log_error("[ps2-server] bind notification failed: ", e))
}

fn connect_input_service() -> Result<(Word, Word), nanami_runtime::NanamiError> {
    loop {
        match nanami_client::registry::connect_input_service_with_pid(SLOT_INPUT_SERVICE) {
            Ok(pid) => {
                return Ok((
                    nanami_runtime::ipc::process_slot_descriptor(SLOT_INPUT_SERVICE),
                    pid,
                ));
            }
            Err(e) => {
                log_request_error("[ps2-server] waiting input-service: ", e);
                wait::sleep(10).map_err(|e| log_error("[ps2-server] retry wait failed: ", e))?;
            }
        }
    }
}

fn attach_input_drivers(input_port: Word) -> nanami_runtime::NanamiResult {
    nanami_client::input::input_service_attach_driver(
        input_port,
        nanami_client::input::INPUT_DRIVER_KEYBOARD,
    )
    .map_err(|e| log_error("[ps2-server] input keyboard attach failed: ", e))?;
    nanami_runtime::print!("[ps2-server] keyboard attached\n");

    nanami_client::input::input_service_attach_driver(
        input_port,
        nanami_client::input::INPUT_DRIVER_MOUSE,
    )
    .map_err(|e| log_error("[ps2-server] input mouse attach failed: ", e))?;
    nanami_runtime::print!("[ps2-server] mouse attached\n");

    Ok(())
}

fn attach_input_driver_queue(input_port: Word) -> Result<Word, nanami_runtime::NanamiError> {
    match nanami_client::input::input_service_attach_driver_shared(
        input_port,
        nanami_client::input::INPUT_DRIVER_KEYBOARD,
    ) {
        Ok((queue_vaddr, _queue_bytes)) => Ok(queue_vaddr),
        Err(e) => Err(log_error("[ps2-server] input queue attach failed: ", e)),
    }
}

fn attach_input_notification(input_pid: Word) -> Result<Word, nanami_runtime::NanamiError> {
    nanami_runtime::request_notification_port_copy(
        input_pid,
        nanami_runtime::PROCESS_SLOT_NOTIFICATION,
        SLOT_INPUT_NOTIFICATION,
        nanami_client::input::INPUT_DRIVER_NOTIFICATION_IDENTIFIER,
    )
    .map_err(|e| log_error("[ps2-server] input notification attach failed: ", e))?;
    Ok(nanami_runtime::ipc::process_slot_descriptor(
        SLOT_INPUT_NOTIFICATION,
    ))
}

fn arm_irqs(irq1_desc: Word, irq12_desc: Word) -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::interrupt_ack(irq1_desc)
        .map_err(|e| log_error("[ps2-server] irq1 arm failed: ", e))?;
    nanami_runtime::ipc::interrupt_ack(irq12_desc)
        .map_err(|e| log_error("[ps2-server] irq12 arm failed: ", e))?;
    Ok(())
}

fn service_loop(
    server: &mut Ps2Server,
    notif_desc: Word,
    irq1_desc: Word,
    irq12_desc: Word,
) -> nanami_runtime::NanamiResult {
    loop {
        let state = drain_and_publish(server);

        if matches!(state, DrainState::Empty) && !server.has_pending_events() {
            let waited = nanami_runtime::ipc::notification_wait(notif_desc)
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
            nanami_runtime::print!("[ps2-server] drain budget reached\n");
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

    nanami_runtime::print!("[ps2-server] alive irq1=");
    nanami_runtime::print!("{}", server.irq1_count);
    nanami_runtime::print!(" irq12=");
    nanami_runtime::print!("{}", server.irq12_count);
    nanami_runtime::print!(" key=");
    nanami_runtime::print!("{}", server.key_count);
    nanami_runtime::print!(" mouse_packet=");
    nanami_runtime::print!("{}", server.mouse_packet_count);
    nanami_runtime::print!(" published=");
    nanami_runtime::print!("{}", server.published_count);
    nanami_runtime::print!("\n");
}

nanami_runtime::nanami_entry!(nanami_main);
