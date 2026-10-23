use nanami_runtime::{self, service::*, RequestError, Word};

fn spawn_provider(image: &str) -> Result<(Word, ProviderId), RequestError> {
    let pid = nanami_runtime::request_process_spawn(image)?;
    nanami_runtime::ipc::notification_wait(nanami_runtime::ipc::process_slot_descriptor(26))?;
    Ok((pid, provider_lookup(pid)?.0))
}

fn call(connection: Connection) -> Result<(Word, Word), RequestError> {
    let (status, owner, count) =
        nanami_runtime::call_service_port(connection.endpoint, 42, 0, 0, 0, 0, 1)?;
    assert_eq!(status, nanami_runtime::OS_RESPONSE_OK);
    Ok((owner, count))
}

pub fn run() -> Result<(), RequestError> {
    let (first_pid, first) = spawn_provider("./modules/handover-node")?;
    let (bad_pid, bad) = spawn_provider("./modules/handover-bad")?;
    let (second_pid, second) = spawn_provider("./modules/handover-node")?;
    assert_ne!(bad_pid, second_pid);
    let service = publish("laboratory-endpoint-7", first)?;
    let connection = connect("laboratory-endpoint-7", 31)?;
    assert_eq!(connection.service, service);
    assert_eq!(call(connection)?, (first_pid, 1));
    let state = binding_state(service)?;
    assert!(
        prepare(state, bad).is_err(),
        "incompatible Provider was prepared"
    );
    assert_eq!(binding_state(service)?.phase, BindingPhase::Active);
    assert_eq!(call(connection)?, (first_pid, 2));
    prepare(state, second)?;
    assert!(
        commit(state).is_err(),
        "commit before quiescence was accepted"
    );
    quiesce(state, 30)?;
    let notification = nanami_runtime::ipc::process_slot_descriptor(30);
    assert_eq!(
        nanami_runtime::ipc::notification_wait(notification)?,
        BINDING_NOTIFICATION
    );
    assert_eq!(binding_state(service)?.phase, BindingPhase::Quiesced);
    commit(state)?;
    // The old provider reports retirement, then blocks in its native exit RPC.
    // Alpha marks it exited before issuing this second completion notification.
    assert_eq!(
        nanami_runtime::ipc::notification_wait(notification)?,
        BINDING_NOTIFICATION
    );
    assert!(nanami_runtime::request_process_status(first_pid)?.0);
    let new = binding_state(service)?;
    assert_eq!(new.generation, state.generation + 1);
    assert_eq!(new.phase, BindingPhase::Active);
    assert_eq!(call(connection)?, (second_pid, 1));
    assert_eq!(call(connection)?, (second_pid, 2));
    assert!(matches!(
        a9n_abi::arch::ipc_port::identify(connection.endpoint, 0),
        Err(a9n_abi::CapabilityError::PermissionDenied)
    ));
    let mut info = a9n_abi::MessageInfo::normal(false, 0, 0);
    let mut badge = 0;
    assert!(
        matches!(
            a9n_abi::arch::ipc_port::receive(connection.endpoint, &mut info, &mut badge),
            Err(a9n_abi::CapabilityError::PermissionDenied)
        ),
        "managed client can receive from public endpoint"
    );
    close(connection.session)?;
    assert!(
        call(connection).is_err(),
        "closed client handle remains usable"
    );
    nanami_runtime::println!("HANDOVER-PROBE-PASS stable-handle=true old-stopped=true calls=4 no-replay=true wo-client=true closed=true");
    Ok(())
}
