use core::ptr;

use libnanami::ipc::ServiceRequest;
use libnanami::Word;
use nanami_services::posix::*;

use crate::state::{EnvironmentVariable, Runtime, Session, ENV_NAME_MAX, ENV_VALUE_MAX};

pub(crate) fn init_default_environment(session: &mut Session) {
    let _ = set_session_env(session, b"PATH", b"/bin:/usr/bin");
    let _ = set_session_env(session, b"HOME", b"/");
    let _ = set_session_env(session, b"USER", b"root");
    let _ = set_session_env(session, b"SHELL", b"/bin/shell");
}

pub(crate) fn handle_getenv(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((name, name_len)) =
        read_client_name(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(env_index) = find_env_index(&runtime.sessions[index], &name[..name_len]) else {
        return (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
    };
    let env = runtime.sessions[index].env[env_index];
    if env.value_len > request.arg3 as usize {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if !write_client_bytes(runtime, index, request.arg2 as usize, &env.value[..env.value_len]) {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    (libnanami::OS_RESPONSE_OK, env.value_len as Word, 0)
}

pub(crate) fn handle_setenv(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((name, name_len)) =
        read_client_name(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((value, value_len)) =
        read_client_value(runtime, index, request.arg2 as usize, request.arg3 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    match set_session_env(
        &mut runtime.sessions[index],
        &name[..name_len],
        &value[..value_len],
    ) {
        Ok(()) => (libnanami::OS_RESPONSE_OK, 0, 0),
        Err(()) => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

pub(crate) fn handle_unsetenv(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some((name, name_len)) =
        read_client_name(runtime, index, request.arg0 as usize, request.arg1 as usize)
    else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    if let Some(env_index) = find_env_index(&runtime.sessions[index], &name[..name_len]) {
        runtime.sessions[index].env[env_index] = EnvironmentVariable::EMPTY;
    }
    (libnanami::OS_RESPONSE_OK, 0, 0)
}

pub(crate) fn handle_env_count(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let mut count = 0usize;
    for env in runtime.sessions[index].env.iter() {
        if env.active {
            count += 1;
        }
    }
    (libnanami::OS_RESPONSE_OK, count as Word, 0)
}

pub(crate) fn handle_env_at(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    let Some(index) = crate::find_session(runtime, request.identifier) else {
        return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let target = request.arg0 as usize;
    let out_offset = request.arg1 as usize;
    let max_len = request.arg2 as usize;
    let mut seen = 0usize;
    for env in runtime.sessions[index].env.iter() {
        if !env.active {
            continue;
        }
        if seen == target {
            let needed = env.name_len + 1 + env.value_len;
            let session = runtime.sessions[index];
            if needed > max_len || out_offset.saturating_add(needed) > session.shm_size as usize {
                return (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            }
            unsafe {
                let dst = (session.shm_local as usize + out_offset) as *mut u8;
                ptr::copy_nonoverlapping(env.name.as_ptr(), dst, env.name_len);
                *dst.add(env.name_len) = b'=';
                ptr::copy_nonoverlapping(
                    env.value.as_ptr(),
                    dst.add(env.name_len + 1),
                    env.value_len,
                );
            }
            return (
                libnanami::OS_RESPONSE_OK,
                env.name_len as Word,
                env.value_len as Word,
            );
        }
        seen += 1;
    }
    (libnanami::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0)
}

fn read_client_name(
    runtime: &Runtime,
    session_index: usize,
    offset: usize,
    len: usize,
) -> Option<([u8; ENV_NAME_MAX], usize)> {
    let session = runtime.sessions[session_index];
    if len == 0 || len > ENV_NAME_MAX || offset.checked_add(len)? > session.shm_size as usize {
        return None;
    }
    let input = unsafe {
        core::slice::from_raw_parts((session.shm_local as usize + offset) as *const u8, len)
    };
    if !valid_env_name(input) {
        return None;
    }
    let mut out = [0u8; ENV_NAME_MAX];
    out[..len].copy_from_slice(input);
    Some((out, len))
}

fn read_client_value(
    runtime: &Runtime,
    session_index: usize,
    offset: usize,
    len: usize,
) -> Option<([u8; ENV_VALUE_MAX], usize)> {
    let session = runtime.sessions[session_index];
    if len > ENV_VALUE_MAX || offset.checked_add(len)? > session.shm_size as usize {
        return None;
    }
    let input = unsafe {
        core::slice::from_raw_parts((session.shm_local as usize + offset) as *const u8, len)
    };
    let mut out = [0u8; ENV_VALUE_MAX];
    out[..len].copy_from_slice(input);
    Some((out, len))
}

fn write_client_bytes(
    runtime: &Runtime,
    session_index: usize,
    offset: usize,
    bytes: &[u8],
) -> bool {
    let session = runtime.sessions[session_index];
    if offset.saturating_add(bytes.len()) > session.shm_size as usize {
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (session.shm_local as usize + offset) as *mut u8,
            bytes.len(),
        );
    }
    true
}

fn valid_env_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for byte in name.iter() {
        if *byte == b'=' || *byte == 0 {
            return false;
        }
    }
    true
}

fn find_env_index(session: &Session, name: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i < session.env.len() {
        let env = session.env[i];
        if env.active && env.name_len == name.len() && env.name[..env.name_len] == name[..] {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn set_session_env(session: &mut Session, name: &[u8], value: &[u8]) -> Result<(), ()> {
    if !valid_env_name(name) || name.len() > ENV_NAME_MAX || value.len() > ENV_VALUE_MAX {
        return Err(());
    }
    let mut target = find_env_index(session, name);
    if target.is_none() {
        let mut i = 0usize;
        while i < session.env.len() {
            if !session.env[i].active {
                target = Some(i);
                break;
            }
            i += 1;
        }
    }
    let Some(index) = target else {
        return Err(());
    };
    let mut env = EnvironmentVariable::EMPTY;
    env.active = true;
    env.name_len = name.len();
    env.value_len = value.len();
    env.name[..name.len()].copy_from_slice(name);
    env.value[..value.len()].copy_from_slice(value);
    session.env[index] = env;
    Ok(())
}
