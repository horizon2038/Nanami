#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;
mod context_api;

use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{RequestError, Word};

#[path = "../../shared/src/common.rs"]
pub mod common;
#[path = "../../shared/src/personality.rs"]
pub mod personality;

pub use crate::common::{abi, arch, elf, fault, launch, loader, process, state};
pub use crate::personality::linux;

use abi::*;
use fault::*;
use loader::*;
use state::*;

const ALTER_LINUX_HEAP_BYTES: Word = 1024 * 1024;
const SERVICE_NAME: &str = "alter-linux";

fn launch_personality_from_flags(_flags: Word) -> state::OsPersonality {
    state::OsPersonality::Linux
}

#[panic_handler]
fn panic(_info: &::core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[alter] panic\n");
    let _ = nanami_runtime::request_exit();
    loop {}
}

#[alloc_error_handler]
fn alloc_error(layout: ::core::alloc::Layout) -> ! {
    let (used, remaining, total) = nanami_runtime::heap::heap_stats();
    nanami_runtime::println!(
        "[alter] allocation failed size={:#x} align={:#x} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        layout.size(),
        layout.align(),
        used,
        remaining,
        total
    );
    let _ = nanami_runtime::request_exit();
    loop {
        ::core::hint::spin_loop();
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::print!("[alter] bootstrap\n");
    nanami_runtime::ipc::init_ipc_tls()
        .map_err(|e| log_error("[alter] ipc tls init failed: ", e))?;
    let _ = nanami_runtime::heap::init_heap(ALTER_LINUX_HEAP_BYTES)
        .map_err(|e| log_error("[alter] heap init failed: ", e))?;
    let notification =
        nanami_runtime::ipc::process_slot_descriptor(nanami_runtime::PROCESS_SLOT_NOTIFICATION);
    nanami_runtime::ipc::bind_current_thread_notification(notification)
        .map_err(|e| log_error("[alter] bind notification failed: ", e))?;

    let posix_port = connect_posix_service();
    let timer_port = connect_timer_service();
    let (posix_shm, posix_shm_size) =
        nanami_client::posix::posix_attach_shared_memory(posix_port, ALTER_DEFAULT_SHM_BYTES)
            .map_err(|e| log_error("[alter] posix shm attach failed: ", e))?;
    let (posix_direct_shm, posix_direct_shm_size) =
        nanami_client::posix::posix_attach_direct_io(posix_port, ALTER_DEFAULT_SHM_BYTES)
            .unwrap_or((0, 0));

    nanami_client::registry::register_service(SERVICE_NAME)
        .map_err(|e| log_error("[alter] service register failed: ", e))?;
    nanami_runtime::print!("[alter] service registered: alter-linux\n");

    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut runtime = Runtime::new(
        posix_port,
        posix_shm,
        posix_shm_size,
        posix_direct_shm,
        posix_direct_shm_size,
        0,
        0,
        0,
    );
    runtime.timer_port = timer_port;
    let (scratch, scratch_bytes) = nanami_runtime::request_heap(ALTER_DEFAULT_SHM_BYTES)?;
    runtime.internal_shm = scratch;
    runtime.internal_shm_size = scratch_bytes;
    runtime.client_shm = scratch;
    runtime.client_shm_size = scratch_bytes;
    let mut pending = ReplyAction::DropReply;

    loop {
        let event = match pending {
            ReplyAction::Reply(status, detail0, detail1) => {
                nanami_runtime::ipc::service_reply_receive_event(
                    service_port,
                    status,
                    detail0,
                    detail1,
                )
            }
            ReplyAction::FaultContinue {
                hardware_context,
                hardware_context_count,
            } => nanami_runtime::ipc::service_fault_continue_receive_event(
                service_port,
                &hardware_context[..hardware_context_count],
            ),
            ReplyAction::DropReply => nanami_runtime::ipc::service_receive_event(service_port),
        };

        let event = match event {
            Ok(event) => event,
            Err(e) => return Err(log_error("[alter] ipc receive failed: ", e)),
        };

        let mut fault_pid = 0;
        pending = match event {
            ServiceEvent::Request(request) => {
                let reply = handle_request(&mut runtime, request);
                // Fork/exec scratch must never overwrite another client's
                // pending launch payload while it is being assembled.
                runtime.client_shm = runtime.internal_shm;
                runtime.client_shm_size = runtime.internal_shm_size;
                reply
            }
            ServiceEvent::Notification { identifier, .. } => {
                if identifier & nanami_client::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT != 0 {
                    linux::handle_timer_notification(&mut runtime, identifier);
                }
                if identifier & nanami_client::terminal::TERMINAL_NOTIFICATION_INPUT != 0 {
                    linux::handle_terminal_input(&mut runtime);
                }
                if identifier & nanami_client::net::NET_NOTIFICATION_RX != 0 {
                    linux::wake_network_waiters(&mut runtime);
                }
                if identifier & nanami_client::input::INPUT_NOTIFICATION_IDENTIFIER != 0 {
                    linux::wake_device_readers(&mut runtime);
                }
                linux::handle_readiness_notification(&mut runtime, identifier);
                ReplyAction::DropReply
            }
            ServiceEvent::Fault {
                identifier,
                reason,
                program_counter,
                fault_address,
                hardware_context,
                hardware_context_count,
                ..
            } => {
                fault_pid = identifier;
                if fault_pid == 0 {
                    // Legacy unbadged faults may resolve to the sole managed
                    // process. Exclude its PID, not the zero wire identifier.
                    fault_pid = runtime
                        .process_for_fault_identifier(identifier)
                        .map_or(0, |process| process.pid);
                }
                runtime.trapped_faults = runtime.trapped_faults.saturating_add(1);
                match fault::handle_fault(
                    &mut runtime,
                    FaultEvent {
                        identifier,
                        reason,
                        program_counter,
                        kernel_call_number: fault_address,
                        hardware_context,
                        hardware_context_count,
                    },
                ) {
                    FaultAction::Continue {
                        hardware_context,
                        hardware_context_count,
                    } => ReplyAction::FaultContinue {
                        hardware_context,
                        hardware_context_count,
                    },
                    FaultAction::Park => ReplyAction::DropReply,
                    FaultAction::Terminate { pid, status } => {
                        runtime.mark_process_exited(pid, status);
                        // Zombies retain exit status, not open pipe writers.
                        linux::close_process_files(&mut runtime, pid);
                        notify_terminal_process_exit(&mut runtime, pid, status);
                        let parent_pid = runtime
                            .managed_process(pid)
                            .map(|process| process.parent_pid)
                            .unwrap_or(0);
                        if parent_pid == 0 {
                            let _ = nanami_runtime::request_process_kill(pid, status);
                        } else {
                            let _ = nanami_runtime::request_process_kill(pid, status);
                            let signal = runtime
                                .managed_process(pid)
                                .and_then(|process| process.signals())
                                .map_or(0, |state| state.exit_signal);
                            let (code, detail) = if signal == 0 {
                                (1, status)
                            } else {
                                (2, signal)
                            };
                            linux::signals_runtime::notify_child(&mut runtime, pid, code, detail);
                        }
                        ReplyAction::DropReply
                    }
                }
            }
        };
        linux::handle_readiness_changes(&mut runtime);
        linux::signals_runtime::pump(&mut runtime, fault_pid);
    }
}

pub fn cleanup_process_tree(runtime: &mut Runtime, root_pid: Word, signal: Word) -> usize {
    let mut cleaned = 0usize;
    while let Some(process) = runtime.deepest_process_in_tree(root_pid) {
        let _ = nanami_runtime::request_process_kill(process.pid, signal);
        match nanami_runtime::request_process_reap(process.pid) {
            Ok(()) => {
                cleaned += 1;
            }
            Err(e) => {
                log_request_error("[alter] process cleanup reap failed: ", e);
            }
        }
        linux::close_process_files(runtime, process.pid);
        runtime.remove_process(process.pid);
    }
    cleaned
}

pub fn cleanup_exited_processes(runtime: &mut Runtime) -> usize {
    let mut cleaned = 0usize;
    while let Some(process) = runtime.exited_process() {
        match nanami_runtime::request_process_reap(process.pid) {
            Ok(()) => {
                cleaned += 1;
            }
            Err(e) => {
                log_request_error("[alter] exited cleanup reap failed: ", e);
            }
        }
        linux::close_process_files(runtime, process.pid);
        runtime.remove_process(process.pid);
    }
    cleaned
}

fn notify_terminal_process_exit(runtime: &mut Runtime, pid: Word, status: Word) {
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    if runtime.terminal_port == 0 || runtime.terminal_shm == 0 || process.terminal_id == 0 {
        return;
    }

    let mut message = [0u8; 40];
    let mut pos = 0usize;
    pos = append_bytes(&mut message, pos, b"\x1b]9001;");
    pos = append_decimal(&mut message, pos, pid);
    pos = append_bytes(&mut message, pos, b";");
    pos = append_decimal(&mut message, pos, status);
    pos = append_bytes(&mut message, pos, b"\x07");
    unsafe {
        ::core::ptr::copy_nonoverlapping(message.as_ptr(), runtime.terminal_shm as *mut u8, pos);
    }
    let _ = nanami_client::terminal::terminal_write_output(
        runtime.terminal_port,
        process.terminal_id,
        0,
        pos as Word,
    );
}

fn append_bytes(out: &mut [u8], mut pos: usize, bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() && pos < out.len() {
        out[pos] = bytes[i];
        pos += 1;
        i += 1;
    }
    pos
}

fn append_decimal(out: &mut [u8], pos: usize, value: Word) -> usize {
    if value == 0 {
        return append_bytes(out, pos, b"0");
    }
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    let mut n = value;
    while n != 0 && count < digits.len() {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    let mut out_pos = pos;
    while count != 0 {
        count -= 1;
        out_pos = append_bytes(out, out_pos, &digits[count..count + 1]);
    }
    out_pos
}

fn connect_posix_service() -> Word {
    let mut tries = 0usize;
    loop {
        match nanami_client::registry::connect_posix_service(SLOT_POSIX_SERVICE) {
            Ok(()) => return nanami_runtime::ipc::process_slot_descriptor(SLOT_POSIX_SERVICE),
            Err(e) => {
                if tries == 0 {
                    log_request_error("[alter] waiting posix-service: ", e);
                }
                tries += 1;
                nanami_runtime::yield_now();
            }
        }
    }
}

fn connect_timer_service() -> Word {
    // A domain may deliberately omit a timer provider. A high-priority retry
    // loop here can starve the lower-priority process that is still booting.
    match nanami_client::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
        Ok(()) => nanami_runtime::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE),
        Err(error) => {
            log_request_error("[alter] timer unavailable; clock calls will retry: ", error);
            0
        }
    }
}

fn handle_request(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if let Some((address, bytes)) = runtime.client_memory(request.identifier) {
        runtime.client_shm = address;
        runtime.client_shm_size = bytes;
    }
    use nanami_client::uapi::alter::*;
    match request.code {
        ALTER_REQUEST_PREPARE_CONTEXT
        | ALTER_REQUEST_START_CONTEXT
        | ALTER_REQUEST_DELETE_CONTEXT
        | ALTER_REQUEST_SIGNAL_CONTEXT => context_api::handle(runtime, request),
        ALTER_REQUEST_CONTROL => handle_control(runtime, request),
        ALTER_REQUEST_LOAD_ELF => handle_load_elf(runtime, request),
        ALTER_REQUEST_SPAWN_INITRAMFS => handle_spawn_initramfs(runtime, request),
        ALTER_REQUEST_SPAWN_LINUX => launch::handle_spawn_linux(runtime, request),
        ALTER_REQUEST_STATUS => handle_status(runtime, request),
        ALTER_REQUEST_KILL => handle_kill(runtime, request),
        ALTER_REQUEST_KILL_TERMINAL => handle_kill_terminal(runtime, request),
        _ => ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_status(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if request.arg0 == 0 {
        return ReplyAction::Reply(
            nanami_runtime::OS_RESPONSE_OK,
            runtime.trapped_faults,
            runtime.loaded_entry,
        );
    }
    let Some(mut process) = runtime.managed_process(request.arg0).copied() else {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_OK, 1, 1);
    };
    if process.rootfs.leader != 0 && process.owner_pid != request.identifier {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    // Also observe native faults/external termination. Ordinary Alter status
    // queries retain their existing local-only path.
    if process.rootfs.leader != 0 && !process.exited {
        match nanami_runtime::request_process_status(process.pid) {
            Ok((true, status)) => {
                runtime.mark_process_exited(process.pid, status);
                linux::close_process_files(runtime, process.pid);
                process.exited = true;
                process.exit_status = status;
            }
            Ok((false, _)) => {}
            Err(error) => return ReplyAction::Reply(map_request_error_to_status(error), 0, 0),
        }
    }
    ReplyAction::Reply(
        nanami_runtime::OS_RESPONSE_OK,
        if process.exited { 1 } else { 0 },
        process.exit_status,
    )
}

fn handle_kill(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    let pid = request.arg0;
    if pid == 0 || runtime.managed_process(pid).is_none() {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if runtime
        .managed_process(pid)
        .is_some_and(|process| process.owner_pid != request.identifier)
    {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
    }
    let signal = if request.arg1 == 0 { 1 } else { request.arg1 };
    let cleaned = cleanup_process_tree(runtime, pid, signal);
    if cleaned != 0 {
        ReplyAction::Reply(nanami_runtime::OS_RESPONSE_OK, cleaned as Word, 0)
    } else {
        ReplyAction::Reply(nanami_runtime::OS_RESPONSE_FATAL, 0, 0)
    }
}

fn handle_kill_terminal(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    let terminal_id = request.arg0;
    if terminal_id == 0 {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    let signal = if request.arg1 == 0 { 1 } else { request.arg1 };
    let mut cleaned = 0usize;
    while let Some(process) = runtime.root_process_for_terminal(terminal_id) {
        cleaned += cleanup_process_tree(runtime, process.pid, signal);
    }
    ReplyAction::Reply(nanami_runtime::OS_RESPONSE_OK, cleaned as Word, 0)
}

fn handle_control(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    match request.arg0 {
        ALTER_CONTROL_ATTACH_SHARED_MEMORY => {
            let size = if request.arg1 == 0 {
                ALTER_DEFAULT_SHM_BYTES
            } else {
                request.arg1
            };
            if let Some(client) = runtime.attached_client(request.identifier) {
                if size > client.bytes {
                    return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
                }
                return ReplyAction::Reply(
                    nanami_runtime::OS_RESPONSE_OK,
                    client.peer,
                    client.bytes,
                );
            }
            if !runtime.has_client_slot() {
                return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            match nanami_runtime::request_shared_memory(request.identifier, size) {
                Ok((local, peer)) => {
                    if !runtime.remember_client(request.identifier, local, peer, size) {
                        return ReplyAction::Reply(
                            nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
                            0,
                            0,
                        );
                    }
                    runtime.client_shm = local;
                    runtime.client_shm_size = size;
                    ReplyAction::Reply(nanami_runtime::OS_RESPONSE_OK, peer, size)
                }
                Err(e) => ReplyAction::Reply(map_request_error_to_status(e), 0, 0),
            }
        }
        _ => ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn handle_spawn_initramfs(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if runtime.client_shm == 0 || request.arg1 == 0 || request.arg1 > 24 {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }
    if request
        .arg0
        .checked_add(request.arg1)
        .filter(|end| *end <= runtime.client_shm_size)
        .is_none()
    {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    let name = unsafe {
        ::core::slice::from_raw_parts(
            (runtime.client_shm + request.arg0) as *const u8,
            request.arg1 as usize,
        )
    };
    let Ok(image_name) = ::core::str::from_utf8(name) else {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    };
    let Some(pcb_slot) = runtime.next_pcb_slot() else {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
    };

    match nanami_runtime::request_process_spawn_fault_handler(image_name, pcb_slot) {
        Ok(pid) => {
            let pcb = nanami_runtime::ipc::process_slot_descriptor(pcb_slot);
            if !runtime.install_managed_process(
                pid,
                request.identifier,
                pcb,
                0,
                image_name.as_bytes(),
            ) {
                let _ = nanami_runtime::request_process_kill(pid, 1);
                let _ = nanami_runtime::request_process_reap(pid);
                return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            nanami_runtime::println!(
                "[alter] managed process image={} pid={} pcb={:#x}",
                image_name,
                pid,
                pcb
            );
            ReplyAction::Reply(nanami_runtime::OS_RESPONSE_OK, pid, pcb)
        }
        Err(e) => ReplyAction::Reply(map_request_error_to_status(e), 0, 0),
    }
}

fn handle_load_elf(runtime: &mut Runtime, request: ServiceRequest) -> ReplyAction {
    if runtime.client_shm == 0 {
        return ReplyAction::Reply(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
    }

    match validate_linux_elf(runtime, request.arg0, request.arg1) {
        Ok(metadata) => {
            runtime.loaded_entry = metadata.entry_point;
            runtime.loaded_segment_count = metadata.load_segment_count;
            nanami_runtime::println!(
                "[alter] ELF ok entry={:#x} phoff={:#x} phnum={} load={}",
                metadata.entry_point,
                metadata.program_header_offset,
                metadata.program_header_count,
                metadata.load_segment_count
            );
            nanami_runtime::println!(
                "[alter] first load va={:#x} off={:#x} file={:#x} mem={:#x} flags={:#x}",
                metadata.first_load.virtual_address,
                metadata.first_load.offset,
                metadata.first_load.file_size,
                metadata.first_load.memory_size,
                metadata.first_load.flags
            );
            ReplyAction::Reply(
                nanami_runtime::OS_RESPONSE_OK,
                metadata.entry_point,
                metadata.load_segment_count,
            )
        }
        Err(e) => ReplyAction::Reply(map_load_error_to_status(e), 0, 0),
    }
}

fn log_error(prefix: &str, error: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, error);
    error.into()
}

fn log_request_error(prefix: &str, error: RequestError) {
    nanami_runtime::print!("{}", prefix);
    match error {
        RequestError::InvalidArgument => nanami_runtime::print!("invalid-arg\n"),
        RequestError::Unsupported => nanami_runtime::print!("unsupported\n"),
        RequestError::Transport => nanami_runtime::print!("transport\n"),
        RequestError::Protocol => nanami_runtime::print!("protocol\n"),
        RequestError::Status(status) => nanami_runtime::println!("status={:#x}", status),
    }
}

nanami_runtime::nanami_entry!(nanami_main);
