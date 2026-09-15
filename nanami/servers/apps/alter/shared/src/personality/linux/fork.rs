use super::{
    align_down_word, align_up_word, arch, clone_registers_for_fork, close_process_files,
    copy_present_process_range, inherit_linux_files, map_request_error, read_register_value,
    read_shm_u16, read_shm_u32, read_shm_u64, read_shm_u8, spawn_fork_child, write_guest_u32,
    EmulationAction, LinuxSyscallContext, Runtime, Word, EINVAL, EIO, ENOMEM, ENOSYS, ESRCH,
    LINUX_CLONE_CHILD_SETTID, LINUX_CLONE_PARENT_SETTID, LINUX_CLONE_SETTLS, LINUX_CLONE_VM,
    LINUX_ELF_HEADER_BYTES, LINUX_IMAGE_BASE, LINUX_MAX_LOAD_SEGMENTS, LINUX_PAGE_SIZE, LINUX_PF_W,
    LINUX_PROT_NONE, LINUX_PT_LOAD, LINUX_SIGCHLD, LINUX_STACK_BYTES, LINUX_STACK_GUARD_BYTES,
    LINUX_STACK_TOP, REG_FS_BASE, SYS_CLONE,
};

pub(super) fn sys_fork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    if context.number == SYS_CLONE && (context.args[0] & LINUX_CLONE_VM) != 0 {
        return Err(ENOSYS);
    }
    reap_exited_children(runtime, parent_pid);
    let parent = runtime.managed_process(parent_pid).copied().ok_or(ESRCH)?;
    let image_name =
        ::core::str::from_utf8(&parent.image_name[..parent.image_name_len]).map_err(|_| EINVAL)?;
    let personality = parent.personality;
    let clone_options = clone_options(context);
    let current_fs_base = read_register_value(parent.pcb, REG_FS_BASE).unwrap_or(parent.fs_base);
    if current_fs_base != parent.fs_base {
        let _ = runtime.set_fs_base(parent_pid, current_fs_base);
    }

    let child_fs_base = if clone_options.set_tls {
        clone_options.tls
    } else {
        current_fs_base
    };

    let pcb_slot = runtime.next_pcb_slot().ok_or(ENOMEM)?;
    let child_pid = match spawn_fork_child(runtime, image_name.as_bytes(), pcb_slot, personality) {
        Ok(pid) => pid,
        Err(errno) => {
            libnanami::println!(
                "[alter/linux] fork failed stage=spawn image={} pcb_slot={} errno={}",
                image_name,
                pcb_slot,
                errno
            );
            return Err(errno);
        }
    };
    let child_pcb = libnanami::ipc::process_slot_descriptor(pcb_slot);

    let image_ranges = match fork_step(
        "clone image",
        clone_process_image(runtime, parent_pid, child_pid),
    ) {
        Ok(ranges) => ranges,
        Err(error) => {
            discard_spawned_child(child_pid);
            return Err(error);
        }
    };
    if let Err(error) = fork_step(
        "clone mappings",
        clone_process_mappings(runtime, parent_pid, child_pid, &image_ranges),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone stack",
        clone_process_stack(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone tids",
        write_clone_tid_pointers(
            runtime,
            parent_pid,
            child_pid,
            clone_options.parent_tid,
            clone_options.child_tid,
            clone_options.write_parent_tid,
            clone_options.write_child_tid,
        ),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone registers",
        match clone_registers_for_fork(
            parent.pcb,
            child_pcb,
            context,
            clone_options.child_stack,
            child_fs_base,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                libnanami::println!(
                    "[alter/linux] fork register clone failed parent_pcb={:#x} child_pcb={:#x} err={:?}",
                    parent.pcb,
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
        parent.owner_pid,
        child_pcb,
        parent.terminal_id,
        &parent.image_name[..parent.image_name_len],
    ) {
        libnanami::println!(
            "[alter/linux] fork failed stage=install child parent={} child={} errno={}",
            parent_pid,
            child_pid,
            ENOMEM
        );
        discard_spawned_child(child_pid);
        return Err(ENOMEM);
    }
    if let Err(error) = fork_step(
        "clone files",
        inherit_linux_files(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    let mut i = 0usize;
    while i < parent.mappings.len() {
        let mapping = parent.mappings[i];
        if mapping.base != 0
            && !runtime.add_mapping(child_pid, mapping.base, mapping.size, mapping.prot)
        {
            libnanami::println!(
                "[alter/linux] fork failed stage=track mappings parent={} child={} errno={}",
                parent_pid,
                child_pid,
                ENOMEM
            );
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ENOMEM);
        }
        i += 1;
    }
    {
        let Some(child) = runtime.managed_process_mut(child_pid) else {
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ESRCH);
        };
        child.program_break = parent.program_break;
        child.mapped_break = parent.mapped_break;
        child.fs_base = child_fs_base;
        child.trace_enabled = parent.trace_enabled;
        child.diagnostics_enabled = parent.diagnostics_enabled;
        child.graphics_enabled = parent.graphics_enabled;
        child.graphics_session = parent.graphics_session;
        child.personality = parent.personality;
        child.terminal_canonical = parent.terminal_canonical;
        child.terminal_echo = parent.terminal_echo;
    }
    if cfg!(debug_assertions) {
        if let Ok(read_back_fs_base) = read_register_value(child_pcb, REG_FS_BASE) {
            if read_back_fs_base != child_fs_base {
                libnanami::println!(
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
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    Ok(child_pid)
}

pub(super) fn sys_vfork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    match sys_fork(runtime, parent_pid, context) {
        Ok(child_pid) => {
            if runtime.park_signal_waiter(parent_pid, child_pid, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn discard_spawned_child(pid: Word) {
    let _ = libnanami::request_process_kill(pid, 1);
    let _ = libnanami::request_process_reap(pid);
}

pub(super) fn reap_exited_children(runtime: &mut Runtime, parent_pid: Word) {
    loop {
        let Some((child_pid, _status)) = runtime.exited_child(parent_pid, 0) else {
            return;
        };
        match libnanami::request_process_reap(child_pid) {
            Ok(()) => {
                close_process_files(runtime, child_pid);
                runtime.remove_process(child_pid);
            }
            Err(error) => {
                let errno = map_request_error(error);
                libnanami::println!(
                    "[alter/linux] fork pre-clean reap failed parent={} child={} errno={}",
                    parent_pid,
                    child_pid,
                    errno
                );
                close_process_files(runtime, child_pid);
                runtime.remove_process(child_pid);
                return;
            }
        }
    }
}

pub(super) fn fork_step<T>(stage: &str, result: Result<T, i32>) -> Result<T, i32> {
    result.map_err(|error| {
        libnanami::println!("[alter/linux] fork failed stage={} errno={}", stage, error);
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
    if write_parent_tid && parent_tid != 0 {
        write_guest_u32(runtime, parent_pid, parent_tid, child_pid as u32)?;
    }
    if write_child_tid && child_tid != 0 {
        write_guest_u32(runtime, child_pid, child_tid, child_pid as u32)?;
    }
    Ok(())
}

pub(super) fn clone_process_image(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<[(Word, Word); LINUX_MAX_LOAD_SEGMENTS], i32> {
    libnanami::request_process_memory_read(
        parent_pid,
        LINUX_IMAGE_BASE,
        runtime.posix_shm,
        LINUX_ELF_HEADER_BYTES,
    )
    .map_err(map_request_error)?;
    if read_shm_u8(runtime, 0) != 0x7f
        || read_shm_u8(runtime, 1) != b'E'
        || read_shm_u8(runtime, 2) != b'L'
        || read_shm_u8(runtime, 3) != b'F'
    {
        return Err(EINVAL);
    }

    let phoff = read_shm_u64(runtime, 32) as Word;
    let phentsize = read_shm_u16(runtime, 54) as Word;
    let phnum = read_shm_u16(runtime, 56) as Word;
    if phentsize < 56 || phnum == 0 {
        return Err(EINVAL);
    }
    let elf_type = read_shm_u16(runtime, 16);
    let load_bias = if elf_type == 3 { LINUX_IMAGE_BASE } else { 0 };

    let table_bytes = phentsize.checked_mul(phnum).ok_or(EINVAL)?;
    if table_bytes > runtime.posix_shm_size {
        return Err(EINVAL);
    }
    libnanami::request_process_memory_read(
        parent_pid,
        LINUX_IMAGE_BASE + phoff,
        runtime.posix_shm,
        table_bytes,
    )
    .map_err(map_request_error)?;

    let mut ranges = [(0, 0); LINUX_MAX_LOAD_SEGMENTS];
    let mut writable = [false; LINUX_MAX_LOAD_SEGMENTS];
    let mut range_count = 0usize;
    let mut i = 0;
    while i < phnum {
        let base = i * phentsize;
        if read_shm_u32(runtime, base as usize) == LINUX_PT_LOAD {
            let vaddr = read_shm_u64(runtime, (base + 16) as usize) + load_bias;
            let memsz = read_shm_u64(runtime, (base + 40) as usize);
            let start = align_down_word(vaddr, LINUX_PAGE_SIZE);
            let end = align_up_word(vaddr.saturating_add(memsz), LINUX_PAGE_SIZE);
            if range_count == ranges.len() {
                return Err(EINVAL);
            }
            ranges[range_count] = (start, end.saturating_sub(start));
            writable[range_count] = (read_shm_u32(runtime, (base + 4) as usize) & LINUX_PF_W) != 0;
            range_count += 1;
        }
        i += 1;
    }
    let mut range_index = 0usize;
    while range_index < range_count {
        let (start, size) = ranges[range_index];
        if writable[range_index] {
            copy_present_process_range(runtime, parent_pid, child_pid, start, size)?;
        }
        range_index += 1;
    }
    Ok(ranges)
}

pub(super) fn clone_process_mappings(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
    image_ranges: &[(Word, Word); LINUX_MAX_LOAD_SEGMENTS],
) -> Result<(), i32> {
    let parent = runtime.managed_process(parent_pid).copied().ok_or(ESRCH)?;
    let mut i = 0usize;
    while i < parent.mappings.len() {
        let mapping = parent.mappings[i];
        if mapping.base != 0 {
            if is_initial_stack_mapping(mapping.base, mapping.size) {
                i += 1;
                continue;
            }
            // Alpha already loaded the executable; clone_process_image copied
            // its mutable segments. RELRO can split those tracked ranges.
            if crate::elf::ranges_cover(image_ranges, mapping.base, mapping.size) {
                i += 1;
                continue;
            }
            if mapping.prot == LINUX_PROT_NONE {
                i += 1;
                continue;
            }
            let (base, mapped) =
                libnanami::request_process_map_anonymous_at(child_pid, mapping.base, mapping.size)
                    .map_err(|error| {
                        let errno = map_request_error(error);
                        libnanami::println!(
                            "[alter/linux] fork map clone failed parent={} child={} base={:#x} size={:#x} errno={}",
                            parent_pid,
                            child_pid,
                            mapping.base,
                            mapping.size,
                            errno
                        );
                        errno
                    })?;
            if base != mapping.base || mapped < mapping.size {
                libnanami::println!(
                    "[alter/linux] fork map clone mismatch parent={} child={} want=[{:#x}..{:#x}) got=[{:#x}..{:#x})",
                    parent_pid,
                    child_pid,
                    mapping.base,
                    mapping.base + mapping.size,
                    base,
                    base + mapped
                );
                return Err(ENOMEM);
            }
            copy_present_process_range(runtime, parent_pid, child_pid, mapping.base, mapping.size)?;
        }
        i += 1;
    }
    Ok(())
}

pub(super) fn is_initial_stack_mapping(base: Word, size: Word) -> bool {
    let Some(end) = base.checked_add(size) else {
        return false;
    };
    let stack_base = LINUX_STACK_TOP - LINUX_STACK_BYTES;
    let stack_end = LINUX_STACK_TOP + LINUX_STACK_GUARD_BYTES;
    base < stack_end && stack_base < end
}

pub(super) fn clone_process_stack(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    copy_present_process_range(
        runtime,
        parent_pid,
        child_pid,
        LINUX_STACK_TOP - LINUX_STACK_BYTES,
        LINUX_STACK_BYTES,
    )
}
