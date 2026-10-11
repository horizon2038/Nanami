pub use nanami_uapi::exec::*;

use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn exec_attach_shared_memory(
    service_port: CapabilityDescriptor,
    size_bytes: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, local_vaddr, mapped_size) = call_port(
        service_port,
        EXEC_REQUEST_CONTROL,
        EXEC_CONTROL_ATTACH_SHARED_MEMORY,
        size_bytes,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((local_vaddr, mapped_size))
}

pub fn exec_spawn_path(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
    priority: Word,
) -> Result<Word, RequestError> {
    let (status, pid, _) = call_port(
        service_port,
        EXEC_REQUEST_SPAWN_PATH,
        path_offset,
        path_len,
        priority,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn exec_spawn_path_arguments(
    service_port: CapabilityDescriptor,
    path_offset: Word,
    path_len: Word,
    launch_offset: Word,
    launch_len: Word,
) -> Result<Word, RequestError> {
    let (status, pid, _) = call_port(
        service_port,
        EXEC_REQUEST_SPAWN_PATH_ARGUMENTS,
        path_offset,
        path_len,
        launch_offset,
        launch_len,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(pid)
}

pub fn exec_process_status(
    service_port: CapabilityDescriptor,
    pid: Word,
) -> Result<(bool, Word), RequestError> {
    let (status, exited, exit_status) =
        call_port(service_port, EXEC_REQUEST_PROCESS_STATUS, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((exited != 0, exit_status))
}

pub fn exec_process_reap(
    service_port: CapabilityDescriptor,
    pid: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(service_port, EXEC_REQUEST_PROCESS_REAP, pid, 0, 0, 0, 2)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn exec_process_kill(
    service_port: CapabilityDescriptor,
    pid: Word,
    signal: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = call_port(
        service_port,
        EXEC_REQUEST_PROCESS_KILL,
        pid,
        signal,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}
