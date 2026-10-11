use nanami_runtime::{ipc::ServiceRequest, Word};
use nanami_client::posix::*;

use super::{io::map_request_error_to_status, process::session_for_pid, state::Runtime};

pub(crate) fn handle_control(runtime: &mut Runtime, request: ServiceRequest) -> (Word, Word, Word) {
    match request.arg0 {
        POSIX_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                POSIX_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            match nanami_runtime::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => match session_for_pid(runtime, request.identifier) {
                    Some(index) => {
                        runtime.sessions[index].shm_local = local;
                        runtime.sessions[index].shm_size = size;
                        (nanami_runtime::OS_RESPONSE_OK, peer, size)
                    }
                    None => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
                },
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        POSIX_CONTROL_ATTACH_DIRECT_IO => {
            let size = if request.arg1 == 0 {
                POSIX_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            let Some(index) = session_for_pid(runtime, request.identifier) else {
                return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
            };
            match nanami_client::vfs::vfs_attach_delegated_shared_memory(
                runtime.vfs_port,
                request.identifier,
                size,
            ) {
                Ok((peer_vaddr, mapped_size, delegate_id)) => {
                    runtime.sessions[index].direct_vfs_delegate = delegate_id;
                    runtime.sessions[index].direct_io_size = mapped_size;
                    (nanami_runtime::OS_RESPONSE_OK, peer_vaddr, mapped_size)
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}
