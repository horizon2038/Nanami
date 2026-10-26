//! CLONE_VM uses a real shared A9N AddressSpace; no page copying or mirrored PTEs.
use super::*;

const CLONE_FS: Word = 0x200;
const CLONE_FILES: Word = 0x400;
const CLONE_SIGHAND: Word = 0x800;
const CLONE_THREAD: Word = 0x10000;
const CLONE_SYSVSEM: Word = 0x40000;
const CLONE_CHILD_CLEARTID: Word = 0x200000;
const CLONE_DETACHED: Word = 0x400000;

pub(super) fn clone_vm(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let flags = context.args[0];
    let allowed = 0xff
        | LINUX_CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED
        | LINUX_CLONE_SETTLS
        | LINUX_CLONE_PARENT_SETTID
        | LINUX_CLONE_CHILD_SETTID;
    if flags & !allowed != 0 {
        return Err(ENOSYS);
    }
    let thread = flags & CLONE_THREAD != 0;
    if (thread && (flags & CLONE_SIGHAND == 0 || flags & 0xff != 0)) || context.args[1] == 0 {
        return Err(EINVAL);
    }
    if !thread && flags & 0xff != LINUX_SIGCHLD {
        return Err(EINVAL);
    }
    let options = clone_options(context);
    // Allocate shared dispositions even if the parent has not used a signal syscall.
    let parent_signals = runtime
        .managed_process_mut(parent_pid)
        .ok_or(ESRCH)?
        .signals_mut()
        .clone_task(flags & CLONE_SIGHAND != 0, thread);
    let parent = runtime.managed_process(parent_pid).copied().ok_or(ESRCH)?;
    let slot = runtime.next_pcb_slot().ok_or(ENOMEM)?;
    let pid = nanami_runtime::request_process_thread_create(parent_pid, slot, 20)
        .map_err(map_request_error)?;
    let pcb = nanami_runtime::ipc::process_slot_descriptor(slot);
    let result = (|| {
        let fs_base = if options.set_tls {
            options.tls
        } else {
            read_register_value(parent.pcb, REG_FS_BASE).unwrap_or(parent.fs_base)
        };
        clone_registers_for_fork(parent.pcb, pcb, context, options.child_stack, fs_base)
            .map_err(|_| EIO)?;
        if !runtime.install_managed_child_process(
            pid,
            parent_pid,
            parent.owner_pid,
            pcb,
            parent.terminal_id,
            &parent.image_name[..parent.image_name_len],
        ) {
            return Err(ENOMEM);
        }
        if flags & CLONE_FILES == 0 {
            inherit_linux_files(runtime, parent_pid, pid)?;
        }
        if !runtime.share_task_resources(
            parent_pid,
            pid,
            flags & CLONE_FILES != 0,
            flags & CLONE_FS != 0,
            thread,
        ) {
            return Err(ENOMEM);
        }
        let _ = runtime.set_cwd(pid, &parent.cwd[..parent.cwd_len]);
        let child = runtime.managed_process_mut(pid).ok_or(ESRCH)?;
        *child.signals_mut() = parent_signals;
        child.fs_base = fs_base;
        child.trace_enabled = parent.trace_enabled;
        child.diagnostics_enabled = parent.diagnostics_enabled;
        child.personality = parent.personality;
        child.graphics_enabled = parent.graphics_enabled;
        child.graphics_session = parent.graphics_session;
        child.framebuffer_size = parent.framebuffer_size;
        write_clone_tid_pointers(
            runtime,
            parent_pid,
            pid,
            options.parent_tid,
            options.child_tid,
            options.write_parent_tid,
            options.write_child_tid,
        )?;
        if flags & CLONE_CHILD_CLEARTID != 0 {
            runtime.tasks.ensure(pid).ok_or(ENOMEM)?.clear_tid = options.child_tid;
        }
        a9n_abi::arch::process_control_block::resume(pcb).map_err(|_| EIO)?;
        Ok(pid)
    })();
    if result.is_err() {
        // Suspend before touching shared robust/clear-tid state or releasing resources.
        let _ = a9n_abi::arch::process_control_block::suspend(pcb);
        close_process_files(runtime, pid);
        discard_spawned_child(pid);
        runtime.remove_process(pid);
    }
    result
}

pub fn exit_peers(runtime: &mut Runtime, pid: Word, status: Word) {
    let group = runtime.thread_group(pid);
    let peers: alloc::vec::Vec<_> = runtime
        .managed
        .iter()
        .filter(|process| {
            process.pid != 0 && process.pid != pid && !process.exited && process.tgid == group
        })
        .map(|process| (process.pid, process.pcb))
        .collect();
    for (tid, pcb) in peers {
        if a9n_abi::arch::process_control_block::suspend(pcb).is_err() {
            continue;
        }
        runtime.mark_process_exited(tid, status);
        close_process_files(runtime, tid);
        let _ = nanami_runtime::request_process_kill(tid, status);
        if tid != group && nanami_runtime::request_process_reap(tid).is_ok() {
            runtime.remove_process(tid);
        }
    }
}
