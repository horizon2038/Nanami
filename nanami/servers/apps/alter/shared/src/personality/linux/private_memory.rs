//! Cold private-backing mutations. Never stop/replay an already handled wait.
use super::{map_request_error, Runtime, Word, EIO};
use alloc::vec::Vec;
use nanami_runtime::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY};

pub(super) fn mark_shared(runtime: &mut Runtime, pid: Word) {
    let group = runtime.vm_group(pid);
    for process in &mut runtime.managed {
        if process.pid != 0 && process.vm_group == group {
            process.vm_private_shared = true;
        }
    }
}

pub(super) fn with_private_write<T>(
    runtime: &Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    if bytes == 0
        || !runtime
            .managed_process(pid)
            .is_some_and(|process| process.vm_private_shared)
    {
        return operation();
    }
    if !nanami_runtime::request_process_private_write(pid, address, bytes, OS_PRIVATE_WRITE_QUERY)
        .map_err(map_request_error)?
    {
        return operation();
    }
    with_vm_quiescence(runtime, pid, || {
        super::guest_views::invalidate(runtime, pid);
        nanami_runtime::request_process_private_write(pid, address, bytes, OS_PRIVATE_WRITE_DETACH)
            .map_err(map_request_error)?;
        operation()
    })
}

/// Fork also needs this barrier: an earlier permission downgrade may still
/// have a writable TLB entry on a running peer even when the PTE is read-only.
/// With no active peer, the current delivered fault needs no additional IPC.
pub(super) fn with_vm_quiescence<T>(
    runtime: &Runtime,
    pid: Word,
    operation: impl FnOnce() -> Result<T, i32>,
) -> Result<T, i32> {
    let group = runtime.vm_group(pid);
    let mut stopped = Vec::new();
    for process in &runtime.managed {
        if process.pid == 0
            || process.exited
            || process.vm_group != group
            || process.pid == runtime.current_fault_pid
            || process.rootfs.prepared
            || process.parked_context().is_some()
            || process
                .signals()
                .is_some_and(|state| state.stopped || state.stopped_context.is_some())
        {
            continue;
        }
        // An unaccepted fault may be cancelled/replayed: emulation has not run.
        // Do not resume a failed stop whose remote state is not acknowledged.
        if a9n_abi::arch::process_control_block::suspend(process.pcb).is_err() {
            resume(&stopped);
            return Err(EIO);
        }
        stopped.push((process.pid, process.pcb));
    }
    let result = operation();
    resume(&stopped);
    result
}

fn resume(stopped: &[(Word, Word)]) {
    for &(pid, pcb) in stopped.iter().rev() {
        if a9n_abi::arch::process_control_block::resume(pcb).is_err() {
            nanami_runtime::println!("[alter/linux] private VM resume failed pid={}", pid);
            // Match the signal pump's failed-resume policy; exit processing
            // updates managed state through the existing Alpha event path.
            let _ = nanami_runtime::request_process_kill(pid, 11);
        }
    }
}
