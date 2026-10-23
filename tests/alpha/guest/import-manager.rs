#![no_std]
#![no_main]

use nanami_runtime::{self as runtime, service::*, RequestError, Word};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("IMPORT-MANAGER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn spawn() -> Result<(Word, ProviderId), RequestError> {
    let pid = runtime::request_process_spawn("./modules/handover-node")?;
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(26))?;
    Ok((pid, provider_lookup(pid)?.0))
}

fn main() -> runtime::NanamiResult {
    runtime::ipc::init_ipc_tls()?;
    assert_eq!(runtime::get_self_pid()?, 1);
    runtime::request_notification_port_create(26, 1)?;
    let (_, hidden) = spawn()?;
    let (old_pid, first) = spawn()?;
    let (_, second) = spawn()?;
    publish("unshared-parent-9", hidden)?;
    // Give the upstream import an identity different from the child's first
    // local Session. Neither domain may interpret that identity as a PID.
    let secret = connect("unshared-parent-9", 31)?;
    assert_eq!(secret.session.0, SESSION_IDENTIFIER_BIT | 1);
    let service = publish("upstream-only-4", first)?;
    // The configured child starts after publication. Its first real request
    // asks the Provider to notify us, without an Alpha data-path proxy.
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(26))?;
    let state = binding_state(service)?;
    runtime::println!("IMPORT-MANAGER-QUIESCE");
    prepare(state, second)?;
    quiesce(state, 30)?;
    let notification = runtime::ipc::process_slot_descriptor(30);
    runtime::ipc::notification_wait(notification)?;
    assert_eq!(binding_state(service)?.phase, BindingPhase::Quiesced);
    commit(state)?;
    runtime::ipc::notification_wait(notification)?;
    assert!(runtime::request_process_status(old_pid)?.0);
    runtime::println!("IMPORT-MANAGER-PASS upstream-switched=true old-stopped=true");
    for _ in 0..3 {
        runtime::println!("I-M-OK");
    }
    Ok(())
}

runtime::nanami_entry!(main);
