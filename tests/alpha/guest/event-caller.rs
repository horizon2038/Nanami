#![no_std]
#![no_main]
use nanami_runtime::{self as runtime, service::*, RequestError};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("EVENT-CALLER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}
fn run() -> Result<(), RequestError> {
    runtime::ipc::init_ipc_tls()?;
    let connection = connect("event-queue-8", 23)?;
    let (status, _, calls) = runtime::call_service_port(connection.endpoint, 44, 0, 0, 0, 0, 1)?;
    assert_eq!(status, runtime::OS_RESPONSE_OK);
    assert_eq!(calls, 1, "accepted non-idempotent CALL was replayed");
    close(connection.session)?;
    runtime::request_notification_port_copy(1, 36, 25, 1)?;
    runtime::ipc::notification_notify(runtime::ipc::process_slot_descriptor(25))?;
    Ok(())
}
fn main() -> runtime::NanamiResult {
    run().expect("accepted CALL lost its reply during quiesce");
    Ok(())
}
runtime::nanami_entry!(main);
