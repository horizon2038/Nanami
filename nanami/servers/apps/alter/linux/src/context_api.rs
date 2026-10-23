//! Owner-scoped execution contexts. OCI names/configuration remain in Negoto.
use crate::state::{ReplyAction, Runtime};
use alloc::vec;
use nanami_client::uapi::alter::*;
use nanami_runtime::{ipc::ServiceRequest, Word};

pub fn handle(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if request.code == ALTER_REQUEST_PREPARE_CONTEXT {
        return prepare(runtime, request);
    }
    let Some(process) = runtime.managed_process(request.arg0).copied() else {
        return reply(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0);
    };
    if process.owner_pid != request.identifier || process.rootfs.root_len == 0 {
        return reply(nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0);
    }
    match request.code {
        ALTER_REQUEST_START_CONTEXT => {
            if process.exited || !process.rootfs.prepared {
                return reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0);
            }
            if a9n_abi::arch::process_control_block::resume(process.pcb).is_err() {
                return reply(nanami_runtime::OS_RESPONSE_FATAL, 0);
            }
            runtime
                .managed_process_mut(process.pid)
                .unwrap()
                .rootfs
                .prepared = false;
            reply(nanami_runtime::OS_RESPONSE_OK, process.pid)
        }
        ALTER_REQUEST_SIGNAL_CONTEXT => {
            if process.exited || !matches!(request.arg1, 0 | 9 | 15) {
                return reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0);
            }
            if request.arg1 == 0 {
                return reply(nanami_runtime::OS_RESPONSE_OK, 0);
            }
            match nanami_runtime::request_process_kill(process.pid, request.arg1) {
                Ok(()) => {
                    runtime.mark_process_exited(process.pid, 128 + request.arg1);
                    crate::linux::close_process_files(runtime, process.pid);
                    reply(nanami_runtime::OS_RESPONSE_OK, 0)
                }
                Err(error) => reply(crate::loader::map_request_error_to_status(error), 0),
            }
        }
        ALTER_REQUEST_DELETE_CONTEXT => {
            if !process.exited {
                return reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0);
            }
            // Keep every record on failure. A retry must still own its backing.
            while let Some(child) = runtime.deepest_process_in_tree(process.pid) {
                if !child.exited {
                    if let Err(error) = nanami_runtime::request_process_kill(child.pid, 9) {
                        return reply(crate::loader::map_request_error_to_status(error), 0);
                    }
                    runtime.mark_process_exited(child.pid, 137);
                }
                if crate::linux::close_process_files_checked(runtime, child.pid).is_err() {
                    return reply(nanami_runtime::OS_RESPONSE_FATAL, 0);
                }
                if let Err(error) = nanami_runtime::request_process_reap(child.pid) {
                    return reply(crate::loader::map_request_error_to_status(error), 0);
                }
                runtime.remove_process(child.pid);
            }
            reply(nanami_runtime::OS_RESPONSE_OK, 0)
        }
        _ => reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0),
    }
}

fn prepare(runtime: &mut Runtime, mut request: ServiceRequest) -> ReplyAction {
    let Some((address, bytes)) = runtime.client_memory(request.identifier) else {
        return reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0);
    };
    if request.arg1 > 32 * 1024
        || request
            .arg0
            .checked_add(request.arg1)
            .is_none_or(|end| end > bytes)
    {
        return reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0);
    }
    // Validate only a private snapshot, never mutable client-controlled offsets.
    let mut snapshot = vec![0; request.arg1 + crate::abi::ALTER_PATH_MAX];
    for offset in 0..request.arg1 {
        snapshot[offset] =
            unsafe { core::ptr::read_volatile((address + request.arg0 + offset) as *const u8) };
    }
    let saved = (runtime.client_shm, runtime.client_shm_size);
    runtime.client_shm = snapshot.as_mut_ptr() as Word;
    runtime.client_shm_size = snapshot.len();
    request.arg0 = 0;
    request.arg3 = 0;
    let result = crate::launch::handle_prepare_context(runtime, request);
    runtime.client_shm = saved.0;
    runtime.client_shm_size = saved.1;
    result
}

fn reply(status: Word, detail: Word) -> ReplyAction {
    ReplyAction::Reply(status, detail, 0)
}
