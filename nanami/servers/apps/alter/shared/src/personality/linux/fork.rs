use super::{
    arch, clone_registers_for_fork, inherit_linux_files, map_request_error, read_register_value,
    write_guest_u32, LinuxSyscallContext, Runtime, Word, EINVAL, EIO, ENOMEM,
    ENOSYS, ESRCH, LINUX_CLONE_CHILD_SETTID, LINUX_CLONE_PARENT_SETTID, LINUX_CLONE_SETTLS,
    LINUX_CLONE_VM, LINUX_SIGCHLD, LINUX_TASK_PRIORITY, REG_FS_BASE, SYS_CLONE,
};

pub(super) fn sys_fork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    if context.number == SYS_CLONE && (context.args[0] & LINUX_CLONE_VM) != 0 {
        return super::threads::clone_vm(runtime, parent_pid, context);
    }
    if context.number == SYS_CLONE && context.args[0] & (0x800 | 0x10000) != 0 {
        return Err(EINVAL);
    }
    if context.number == SYS_CLONE {
        let allowed = 0xff
            | LINUX_CLONE_PARENT_SETTID
            | LINUX_CLONE_CHILD_SETTID
            | LINUX_CLONE_SETTLS
            | 0x0020_0000;
        if context.args[0] & !allowed != 0 {
            return Err(ENOSYS);
        }
        if context.args[0] & 0xff != LINUX_SIGCHLD {
            return Err(EINVAL);
        }
    }
    // Keep the consumed parent values at the original snapshot point. The
    // image name must remain owned across process construction and mutation.
    let (parent_pcb, parent_fs_base, parent_owner_pid, parent_terminal_id,
        parent_image_name, parent_image_name_len, parent_program_break, parent_mapped_break,
        parent_trace_enabled, parent_diagnostics_enabled, parent_graphics_enabled,
        parent_framebuffer_size, parent_graphics_session, parent_personality,
        parent_terminal_canonical, parent_terminal_echo, parent_terminal_termios, parent_tgid,
        parent_has_tracked_image) = {
        let parent = runtime.managed_process(parent_pid).ok_or(ESRCH)?;
        (parent.pcb, parent.fs_base, parent.owner_pid, parent.terminal_id,
         parent.image_name, parent.image_name_len, parent.program_break, parent.mapped_break,
         parent.trace_enabled, parent.diagnostics_enabled, parent.graphics_enabled,
         parent.framebuffer_size, parent.graphics_session, parent.personality,
         parent.terminal_canonical, parent.terminal_echo, parent.terminal_termios, parent.tgid,
         parent.rootfs.root_len != 0)
    };
    let image_name =
        ::core::str::from_utf8(&parent_image_name[..parent_image_name_len]).map_err(|_| EINVAL)?;
    let clone_options = clone_options(context);
    let current_fs_base = read_register_value(parent_pcb, REG_FS_BASE).unwrap_or(parent_fs_base);
    if current_fs_base != parent_fs_base {
        let _ = runtime.set_fs_base(parent_pid, current_fs_base);
    }

    let child_fs_base = if clone_options.set_tls {
        clone_options.tls
    } else {
        current_fs_base
    };

    let pcb_slot = runtime.next_pcb_slot().ok_or(ENOMEM)?;
    // Mark conservatively before construction: a failed unpublished child can
    // leave a source marker until its references are rolled back by Alpha.
    // RootFS loading tracks ELF permissions as well as stack/heap VMAs.
    // Legacy named-image launches only track the stack; keep eager fork until
    // their data-page rights can be validated by the write-fault handler too.
    let private_cow = cfg!(target_arch = "x86_64") && parent_has_tracked_image;
    let created = if private_cow {
        super::private_memory::mark_shared(runtime, parent_pid);
        super::private_memory::with_vm_quiescence(runtime, parent_pid, || {
            // Revoke writable leases before Alpha downgrades any source leaf.
            // A failed revocation keeps its physical reference pinned, so the
            // fork admission check takes the eager path for that page.
            super::guest_views::invalidate(runtime, parent_pid);
            nanami_runtime::request_process_fork_shared_readonly(
                parent_pid,
                pcb_slot,
                LINUX_TASK_PRIORITY,
            )
            .map_err(map_request_error)
        })
    } else {
        super::private_memory::with_vm_quiescence(runtime, parent_pid, || {
            nanami_runtime::request_process_fork(parent_pid, pcb_slot, LINUX_TASK_PRIORITY)
                .map_err(map_request_error)
        })
    };
    let child_pid = match created {
        Ok(pid) => pid,
        Err(errno) => {
            nanami_runtime::println!(
                "[alter/linux] fork failed stage=spawn image={} pcb_slot={} errno={}",
                image_name,
                pcb_slot,
                errno
            );
            return Err(errno);
        }
    };
    let child_pcb = nanami_runtime::ipc::process_slot_descriptor(pcb_slot);

    if let Err(error) = fork_step(
        "clone registers",
        match clone_registers_for_fork(
            parent_pcb,
            child_pcb,
            context,
            clone_options.child_stack,
            child_fs_base,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                nanami_runtime::println!(
                    "[alter/linux] fork register clone failed parent_pcb={:#x} child_pcb={:#x} err={:?}",
                    parent_pcb,
                    child_pcb,
                    error
                );
                Err(EIO)
            }
        },
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if !runtime.install_managed_child_process(
        child_pid,
        parent_pid,
        parent_owner_pid,
        child_pcb,
        parent_terminal_id,
        &parent_image_name[..parent_image_name_len],
    ) {
        nanami_runtime::println!(
            "[alter/linux] fork failed stage=install child parent={} child={} errno={}",
            parent_pid,
            child_pid,
            ENOMEM
        );
        discard_spawned_child(child_pid);
        return Err(ENOMEM);
    }
    if private_cow {
        super::private_memory::mark_shared(runtime, child_pid);
    }
    // Constructor-time memory operations must not mistake this suspended task
    // for a running peer and resume it before files/VMA/TID setup completes.
    if let Some(child) = runtime.managed_process_mut(child_pid) {
        child.rootfs.prepared = true;
    }
    if let Err(error) = write_clone_tid_pointers(
        runtime,
        parent_pid,
        child_pid,
        clone_options.parent_tid,
        clone_options.child_tid,
        clone_options.write_parent_tid,
        clone_options.write_child_tid,
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone files",
        inherit_linux_files(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    if !runtime.inherit_mappings(parent_pid, child_pid) {
        nanami_runtime::println!(
            "[alter/linux] fork failed stage=track mappings parent={} child={} errno={}",
            parent_pid,
            child_pid,
            ENOMEM
        );
        super::close_process_files(runtime, child_pid);
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(ENOMEM);
    }
    if let Err(error) = super::memfd::inherit(runtime, parent_pid, child_pid) {
        super::close_process_files(runtime, child_pid);
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    {
        let Some(child) = runtime.managed_process_mut(child_pid) else {
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ESRCH);
        };
        child.program_break = parent_program_break;
        child.mapped_break = parent_mapped_break;
        child.fs_base = child_fs_base;
        child.trace_enabled = parent_trace_enabled;
        child.diagnostics_enabled = parent_diagnostics_enabled;
        child.graphics_enabled = parent_graphics_enabled;
        child.framebuffer_size = parent_framebuffer_size;
        child.graphics_session = parent_graphics_session;
        child.personality = parent_personality;
        child.terminal_canonical = parent_terminal_canonical;
        child.terminal_echo = parent_terminal_echo;
        child.terminal_termios = parent_terminal_termios;
        child.parent_pid = parent_tgid;
    }
    runtime.inherit_signals(parent_pid, child_pid);
    super::sysv_shm::inherited(runtime, child_pid);
    if context.number == SYS_CLONE && context.args[0] & 0x0020_0000 != 0 {
        let Some(task) = runtime.tasks.ensure(child_pid) else {
            super::close_process_files(runtime, child_pid);
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ENOMEM);
        };
        task.clear_tid = clone_options.child_tid;
    }
    if cfg!(debug_assertions) {
        if let Ok(read_back_fs_base) = read_register_value(child_pcb, REG_FS_BASE) {
            if read_back_fs_base != child_fs_base {
                nanami_runtime::println!(
                    "[alter/linux] fork fsbase mismatch parent={} child={} expected={:#x} actual={:#x}",
                    parent_pid,
                    child_pid,
                    child_fs_base,
                    read_back_fs_base
                );
            }
        }
    }
    if let Err(error) = fork_step(
        "resume child",
        a9n_abi::arch::process_control_block::resume(child_pcb).map_err(|_| EIO),
    ) {
        let _ = a9n_abi::arch::process_control_block::suspend(child_pcb);
        super::close_process_files(runtime, child_pid);
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    if let Some(child) = runtime.managed_process_mut(child_pid) {
        child.rootfs.prepared = false;
    }
    Ok(child_pid)
}

pub(super) fn discard_spawned_child(pid: Word) {
    let _ = nanami_runtime::request_process_kill(pid, 1);
    let _ = nanami_runtime::request_process_reap(pid);
}

pub(super) fn fork_step<T>(stage: &str, result: Result<T, i32>) -> Result<T, i32> {
    result.map_err(|error| {
        nanami_runtime::println!("[alter/linux] fork failed stage={} errno={}", stage, error);
        error
    })
}

#[derive(Clone, Copy)]
pub(super) struct CloneOptions {
    pub(super) child_stack: Word,
    pub(super) parent_tid: Word,
    pub(super) child_tid: Word,
    pub(super) tls: Word,
    pub(super) write_parent_tid: bool,
    pub(super) write_child_tid: bool,
    pub(super) set_tls: bool,
}

pub(super) fn clone_options(context: LinuxSyscallContext) -> CloneOptions {
    let flags = if context.number == SYS_CLONE {
        context.args[0]
    } else {
        LINUX_SIGCHLD
    };
    CloneOptions {
        child_stack: if context.number == SYS_CLONE {
            context.args[1]
        } else {
            0
        },
        parent_tid: if context.number == SYS_CLONE {
            context.args[2]
        } else {
            0
        },
        child_tid: if context.number == SYS_CLONE {
            arch::clone_child_tid(context.args)
        } else {
            0
        },
        tls: if context.number == SYS_CLONE {
            arch::clone_tls(context.args)
        } else {
            0
        },
        write_parent_tid: (flags & LINUX_CLONE_PARENT_SETTID) != 0,
        write_child_tid: (flags & LINUX_CLONE_CHILD_SETTID) != 0,
        set_tls: (flags & LINUX_CLONE_SETTLS) != 0,
    }
}

pub(super) fn write_clone_tid_pointers(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
    parent_tid: Word,
    child_tid: Word,
    write_parent_tid: bool,
    write_child_tid: bool,
) -> Result<(), i32> {
    if write_parent_tid {
        write_guest_u32(runtime, parent_pid, parent_tid, child_pid as u32)?;
    }
    if write_child_tid {
        write_guest_u32(runtime, child_pid, child_tid, child_pid as u32)?;
    }
    Ok(())
}
