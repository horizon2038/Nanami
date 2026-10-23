#![no_std]
#![no_main]
use core::sync::atomic::Ordering;
use nanami_runtime::{self as runtime, service::*, RequestError, Word};
#[path = "event-queue.rs"]
mod queue;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("EVENT-MANAGER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}
fn spawn() -> Result<(Word, ProviderId), RequestError> {
    let pid = runtime::request_process_spawn("modules/event-provider")?;
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(26))?;
    Ok((pid, provider_lookup(pid)?.0))
}
fn quiet(state: BindingState, next: ProviderId) -> Result<(), RequestError> {
    prepare(state, next).expect("prepare event successor");
    quiesce(state, 30).expect("quiesce event receiver");
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(30))?;
    assert_eq!(binding_state(state.service)?.phase, BindingPhase::Quiesced);
    Ok(())
}
fn owner(connection: Connection) -> Result<Word, RequestError> {
    let (status, pid, _) = runtime::call_service_port(connection.endpoint, 43, 0, 0, 0, 0, 1)?;
    assert_eq!(status, runtime::OS_RESPONSE_OK);
    Ok(pid)
}
fn drained(data: &queue::Queue, head: Word, total: Word, owner: Word) {
    for _ in 0..100000 {
        if data.head.load(Ordering::Acquire) == head {
            assert_eq!(data.total.load(Ordering::Acquire), total);
            assert_eq!(data.owner.load(Ordering::Acquire), owner);
            return;
        }
        runtime::yield_now();
    }
    panic!("direct work notification did not drain the data queue");
}
fn run() -> Result<(), RequestError> {
    runtime::ipc::init_ipc_tls()?;
    assert_eq!(runtime::get_self_pid()?, 1);
    assert!(matches!(
        runtime::request_io_port(0, 0xffff, 37),
        Err(RequestError::Status(runtime::OS_RESPONSE_PERMISSION_DENIED))
    ));
    assert!(matches!(
        runtime::request_irq(usize::MAX, 38, 39),
        Err(RequestError::Status(runtime::OS_RESPONSE_PERMISSION_DENIED))
    ));
    // Native self-PCB authority permits priority reconfiguration. Manifest
    // priority is initial policy, not an enforced ceiling. Restore the policy.
    let pcb = runtime::ipc::process_slot_descriptor(1);
    for priority in [30, 4] {
        let config = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
            false, false, false, false, false, false, false, false, true, false,
        );
        a9n_abi::arch::process_control_block::configure(
            pcb, config, 0, 0, 0, 0, 0, 0, 0, 0, priority, 0,
        )
        .expect("native self-PCB priority reconfiguration");
    }
    let cores = runtime::request_nanami_info_smp()?.online_cores;
    runtime::request_notification_port_create(26, 1)?;
    runtime::request_notification_port_create(36, 1)?;
    // The legacy copy API must not export Alpha's administrative OS endpoint,
    // dispatch Notification opcodes against IPC/PCB slots, or destroy a valid
    // destination when the source is rejected.
    runtime::request_notification_port_create(34, 8)?;
    assert!(runtime::register_service_by_name("not-a-notification", 34).is_err());
    assert!(runtime::request_notification_port_copy(0, 64, 35, 1).is_err());
    assert!(runtime::request_notification_port_copy(1, 20, 34, 1).is_err());
    runtime::request_notification_port_copy(1, 34, 35, 16)?;
    runtime::request_notification_port_copy(1, 34, 35, 32)?;
    runtime::ipc::notification_notify(runtime::ipc::process_slot_descriptor(35))?;
    assert_eq!(
        runtime::ipc::notification_poll(runtime::ipc::process_slot_descriptor(34))?,
        32
    );
    let unused =
        runtime::request_process_spawn_fault_handler_suspended("modules/event-provider", 40)?;
    // A9N rejects migrating a running self-PCB, but a delegated, stopped child
    // PCB can be assigned any online core, not merely a manifest CPU partition.
    let config = a9n_abi::capability_call::process_control_block::ConfigurationInfo::new(
        false, false, false, false, false, false, false, false, false, true,
    );
    a9n_abi::arch::process_control_block::configure(
        runtime::ipc::process_slot_descriptor(40),
        config,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        cores - 1,
    )
    .expect("stopped child PCB accepts any online affinity");
    assert!(runtime::request_notification_port_copy(1, 40, 34, 1).is_err());
    runtime::request_process_kill(unused, 9)?;
    let (first_pid, first) = spawn()?;
    let (second_pid, second) = spawn()?;
    let (_, third) = spawn()?;
    let service = publish("event-queue-8", first).expect("publish event Service");
    let connection = connect("event-queue-8", 31).expect("connect event Service");
    assert!(connect_event(connection.session, 31).is_err());
    let event = connect_event(connection.session, 32).expect("grant client work Notification");
    assert!(runtime::request_notification_port_copy(1, 34, 32, 1).is_err());
    assert!(runtime::request_notification_port_copy(first_pid, 29, 34, 1).is_err());
    assert!(runtime::request_notification_port_copy(1, 32, 34, 1).is_err());
    runtime::ipc::notification_notify(runtime::ipc::process_slot_descriptor(34))?;
    assert_eq!(
        runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(35))?,
        8
    );
    assert_eq!(connect_event(connection.session, 32)?, event);
    assert!(connect_event(connection.session, 33).is_err());
    assert!(a9n_abi::arch::notification_port::poll(event).is_err());
    assert!(a9n_abi::arch::notification_port::identify(event, 2).is_err());
    let buffer = create_buffer(connection.session, 4096).expect("create event data buffer");
    let data = unsafe { &*(buffer.address as *const queue::Queue) };
    assert_eq!(data.head.load(Ordering::Acquire), 0);
    assert_eq!(owner(connection)?, first_pid);
    data.push(10);
    runtime::ipc::notification_notify(event)?;
    drained(data, 1, 10, first_pid);

    let state = binding_state(service)?;
    runtime::request_process_spawn("modules/event-caller")?;
    for _ in 0..100000 {
        if data.accepted_calls.load(Ordering::Acquire) == 1 {
            break;
        }
        runtime::yield_now();
    }
    assert_eq!(data.accepted_calls.load(Ordering::Acquire), 1);
    prepare(state, second)?;
    quiesce(state, 30)?;
    assert_eq!(binding_state(service)?.phase, BindingPhase::Quiescing);
    assert_eq!(data.accepted_calls.load(Ordering::Acquire), 1);
    data.reply_release.store(1, Ordering::Release);
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(30))?;
    assert_eq!(binding_state(service)?.phase, BindingPhase::Quiesced);
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(36))?;
    data.push(20);
    runtime::ipc::notification_notify(event)?;
    assert_eq!(data.head.load(Ordering::Acquire), 1);
    abort(state).expect("abort event receiver exchange");
    drained(data, 2, 30, first_pid);
    assert_eq!(owner(connection)?, first_pid);

    quiet(state, second)?;
    data.push(30);
    runtime::ipc::notification_notify(event)?;
    assert_eq!(data.head.load(Ordering::Acquire), 2);
    commit(state).expect("commit event receiver exchange");
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(30))?;
    assert!(runtime::request_process_status(first_pid)?.0);
    assert!(runtime::request_process_reap(first_pid).is_err());
    drained(data, 3, 60, second_pid);
    assert_eq!(data.accepted_calls.load(Ordering::Acquire), 1);
    assert_eq!(owner(connection)?, second_pid);
    // No new notification handle, no reconnect, no repeated queue operation.
    data.push(40);
    runtime::ipc::notification_notify(event)?;
    drained(data, 4, 100, second_pid);
    let state = binding_state(service)?;
    quiet(state, third)?;
    close(connection.session).expect("close event Session");
    assert!(runtime::ipc::notification_notify(event).is_err());
    assert!(connect_event(connection.session, 33).is_err());
    abort(state).expect("restore receiver without closed resources");
    assert!(
        owner(connection).is_err(),
        "closed control handle was restored"
    );
    for _ in 0..3 {
        runtime::println!("EVENT-PROBE-PASS");
    }
    Ok(())
}
fn main() -> runtime::NanamiResult {
    run().expect("event manager operation failed");
    Ok(())
}
runtime::nanami_entry!(main);
