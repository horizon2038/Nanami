#![no_std]
#![no_main]

use nanami_runtime::{self as runtime, service::*, RequestError, Word};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("BUFFER-MANAGER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn spawn(fault_handler: bool) -> Result<(Word, ProviderId), RequestError> {
    // The spawn wire carries at most 24 bytes; ./ is optional for exact paths.
    let pid = if fault_handler {
        runtime::request_process_spawn_fault_handler("modules/buffer-provider", 24)?
    } else {
        runtime::request_process_spawn("modules/buffer-provider")?
    };
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(26))?;
    Ok((pid, provider_lookup(pid)?.0))
}
fn call(connection: Connection, operation: Word) -> Result<(Word, Word), RequestError> {
    let (status, owner, detail) =
        runtime::call_service_port(connection.endpoint, operation, 0, 0, 0, 0, 1)?;
    assert_eq!(status, runtime::OS_RESPONSE_OK);
    Ok((owner, detail))
}
fn quiet(state: BindingState, next: ProviderId) -> Result<(), RequestError> {
    prepare(state, next)?;
    quiesce(state, 30)?;
    runtime::ipc::notification_wait(runtime::ipc::process_slot_descriptor(30))?;
    assert_eq!(binding_state(state.service)?.phase, BindingPhase::Quiesced);
    Ok(())
}

fn main() -> Result<(), RequestError> {
    runtime::ipc::init_ipc_tls()?;
    assert_eq!(runtime::get_self_pid()?, 1);
    runtime::request_notification_port_create(26, 1)?;
    let (first_pid, first) = spawn(true)?;
    let (second_pid, second) = spawn(false)?;
    let (_, third) = spawn(false)?;
    let service = publish("opaque-buffer-7", first)?;
    let connection = connect("opaque-buffer-7", 31)?;
    assert!(create_buffer(connection.session, SESSION_BUFFER_MAX_BYTES + 1).is_err());
    let buffer = create_buffer(connection.session, 65536)?;
    let free = runtime::request_nanami_info_memory()?.free_bytes;
    assert_eq!(
        create_buffer(connection.session, 65536)?.address,
        buffer.address
    );
    assert_eq!(runtime::request_nanami_info_memory()?.free_bytes, free);
    assert_eq!(unsafe { (buffer.address as *const u64).read_volatile() }, 0);
    assert_eq!(
        unsafe { ((buffer.address + buffer.bytes - 8) as *const u64).read_volatile() },
        0
    );
    let words = buffer.address as *mut u64;
    unsafe {
        words.write_volatile(0x5142554646455251);
        words.add(1).write_volatile(10);
    }
    assert!(
        session_buffer(connection.session).is_err(),
        "client acquired Provider view authority"
    );
    assert!(
        close(connection.session).is_err(),
        "active Provider buffer closed without quiescence"
    );
    assert_eq!(call(connection, 42)?, (first_pid, 11));
    let old_va = call(connection, 43)?.1;
    let (scratch, _) = runtime::request_heap(4096)?;
    runtime::request_process_memory_read(first_pid, old_va, scratch, 16)?;
    let state = binding_state(service)?;
    quiet(state, second)?;
    assert!(
        runtime::request_process_memory_read(first_pid, old_va, scratch, 16).is_err(),
        "old Provider/copy-window mapping survived quiescence"
    );
    abort(state)?;
    assert_eq!(
        call(connection, 43)?.1,
        old_va,
        "abort changed the old Provider VA"
    );
    assert_eq!(call(connection, 42)?, (first_pid, 12));
    quiet(state, second)?;
    commit(state)?;
    assert_eq!(call(connection, 42)?, (second_pid, 13));
    assert_eq!(unsafe { words.add(1).read_volatile() }, 13);
    // Receive the actual kernel fault through the existing native resolver ABI,
    // not through serial-log parsing or an Alpha metadata lookup. The old task
    // is now blocked awaiting this resolver's reply; never reply/resume it.
    match runtime::ipc::service_receive_event(runtime::ipc::process_slot_descriptor(20))? {
        runtime::ipc::ServiceEvent::Fault {
            identifier,
            fault_address,
            reason,
            ..
        } => {
            assert_eq!(identifier, first_pid);
            assert_eq!(fault_address, old_va);
            assert_ne!(reason, 0);
        }
        _ => panic!("stale mapping did not generate a kernel fault"),
    }
    runtime::request_process_kill(first_pid, 9)?;
    assert!(runtime::request_process_status(first_pid)?.0);
    assert!(
        runtime::request_process_reap(first_pid).is_err(),
        "uncleaned Provider Cspace was recycled"
    );
    let state = binding_state(service)?;
    assert_eq!(state.generation, 2);
    quiet(state, third)?;
    close(connection.session)?;
    assert!(runtime::request_process_memory_read(1, buffer.address, scratch, 16).is_err());
    abort(state)?;
    assert!(call(connection, 42).is_err());
    // Keep the terminal marker short enough to survive busy SMP boot output.
    runtime::println!("BUFFER-PROBE-PASS persisted=13 fault=true");
    Ok(())
}
fn entry() -> runtime::NanamiResult {
    main().expect("buffer manager operation failed");
    Ok(())
}
runtime::nanami_entry!(entry);
