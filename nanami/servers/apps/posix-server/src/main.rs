#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};
use nanami_services::posix::*;

mod control;
use control::*;

mod filesystem;
use filesystem::*;

mod io;
use io::*;

mod environment;
mod fd;
mod path;
pub(crate) mod process;
mod state;

use environment::*;
use fd::*;
use process::*;
use state::*;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[posix-server] panic\n");
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[posix-server] bootstrap\n");
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[posix-server] ipc tls init failed: ", e))?;

    let vfs_port = connect_vfs_service();
    let (vfs_shm, vfs_shm_size) =
        nanami_services::vfs::vfs_attach_shared_memory(vfs_port, VFS_SHM_BYTES)
            .map_err(|e| log_error("[posix-server] vfs shm attach failed: ", e))?;

    nanami_services::registry::register_posix_service()
        .map_err(|e| log_error("[posix-server] service register failed: ", e))?;
    libnanami::print!("[posix-server] service registered: posix-service\n");

    let mut runtime = Runtime {
        vfs_port,
        vfs_shm,
        vfs_shm_size,
        sessions: [Session::EMPTY; MAX_SESSIONS],
        open_files: [OpenFile::EMPTY; MAX_OPEN_FILES],
        next_posix_pid: 100,
        cached_owner_pid: 0,
        cached_session_index: INVALID_SESSION_INDEX,
    };

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending = ReplyAction::DropReply;

    loop {
        let event = match pending {
            ReplyAction::Reply(status, detail0, detail1) => {
                pending = ReplyAction::DropReply;
                match libnanami::ipc::service_reply_receive_event(
                    service_port,
                    status,
                    detail0,
                    detail1,
                ) {
                    Ok(e) => e,
                    Err(e) => return Err(log_error("[posix-server] reply_receive failed: ", e)),
                }
            }
            ReplyAction::DropReply => match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[posix-server] receive failed: ", e)),
            },
        };

        match event {
            ServiceEvent::Request(request) => {
                pending = handle_request(&mut runtime, request);
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::println!(
                    "[posix-server] fault id={} reason={:#x}",
                    identifier,
                    reason
                );
            }
        }
    }
}

fn connect_vfs_service() -> Word {
    let mut tries = 0usize;
    loop {
        match nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE),
            Err(e) => {
                if tries == 0 {
                    log_request_error("[posix-server] waiting vfs-service: ", e);
                }
                tries += 1;
                libnanami::yield_now();
            }
        }
    }
}

fn handle_request(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    let reply = match request.code {
        POSIX_REQUEST_CONTROL => handle_control(runtime, request),
        POSIX_REQUEST_GETPID => match session_for_pid(runtime, request.identifier) {
            Some(index) => (
                libnanami::OS_RESPONSE_OK,
                runtime.sessions[index].posix_pid,
                0,
            ),
            None => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
        },
        POSIX_REQUEST_GETCWD => handle_getcwd(runtime, request),
        POSIX_REQUEST_CHDIR => handle_chdir(runtime, request),
        POSIX_REQUEST_OPEN => handle_open(runtime, request),
        POSIX_REQUEST_CLOSE => handle_close(runtime, request),
        POSIX_REQUEST_DUP => handle_dup(runtime, request),
        POSIX_REQUEST_DUP2 => handle_dup2(runtime, request),
        POSIX_REQUEST_FCNTL => handle_fcntl(runtime, request),
        POSIX_REQUEST_GETENV => handle_getenv(runtime, request),
        POSIX_REQUEST_SETENV => handle_setenv(runtime, request),
        POSIX_REQUEST_UNSETENV => handle_unsetenv(runtime, request),
        POSIX_REQUEST_ENV_COUNT => handle_env_count(runtime, request),
        POSIX_REQUEST_ENV_AT => handle_env_at(runtime, request),
        POSIX_REQUEST_READ | POSIX_REQUEST_PREAD => handle_read(runtime, request),
        POSIX_REQUEST_READ_DIRECT | POSIX_REQUEST_PREAD_DIRECT => {
            handle_read_direct(runtime, request)
        }
        POSIX_REQUEST_WRITE | POSIX_REQUEST_PWRITE => handle_write(runtime, request),
        POSIX_REQUEST_WRITE_DIRECT | POSIX_REQUEST_PWRITE_DIRECT => {
            handle_write_direct(runtime, request)
        }
        POSIX_REQUEST_STAT => handle_stat(runtime, request),
        POSIX_REQUEST_MKDIR => handle_mkdir(runtime, request),
        POSIX_REQUEST_UNLINK => handle_unlink(runtime, request),
        POSIX_REQUEST_LINK => handle_link(runtime, request),
        POSIX_REQUEST_RENAME => handle_rename(runtime, request),
        POSIX_REQUEST_FSTAT => handle_fstat(runtime, request),
        POSIX_REQUEST_READ_DIR => handle_read_dir(runtime, request),
        POSIX_REQUEST_SEEK => handle_seek(runtime, request),
        POSIX_REQUEST_RMDIR => handle_rmdir(runtime, request),
        POSIX_REQUEST_GETPPID => handle_getppid(runtime, request),
        POSIX_REQUEST_GET_NATIVE_PID => handle_get_native_pid(runtime, request),
        POSIX_REQUEST_GETUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETEUID => handle_getuid(runtime, request),
        POSIX_REQUEST_GETGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETEGID => handle_getgid(runtime, request),
        POSIX_REQUEST_GETPGID => handle_getpgid(runtime, request),
        POSIX_REQUEST_GETSID => handle_getsid(runtime, request),
        POSIX_REQUEST_SETPGID => handle_setpgid(runtime, request),
        POSIX_REQUEST_SETSID => handle_setsid(runtime, request),
        POSIX_REQUEST_SPAWN => handle_spawn(runtime, request),
        POSIX_REQUEST_WAITPID => handle_waitpid(runtime, request),
        POSIX_REQUEST_FORK => handle_fork(runtime, request),
        POSIX_REQUEST_EXEC => return handle_exec(runtime, request),
        POSIX_REQUEST_KILL => return handle_kill(runtime, request),
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    };
    ReplyAction::Reply(reply.0, reply.1, reply.2)
}

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, error);
    libnanami::NanamiError::from(error)
}

pub(crate) fn log_request_error(prefix: &str, error: RequestError) {
    libnanami::print!("{}", prefix);
    libnanami::println!("{:?}", error);
}

libnanami::nanami_entry!(nanami_main);
