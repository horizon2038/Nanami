use super::{
    basename_in_bytes, close_cloexec_files, copy_current_path_to_client_shm,
    current_elf_metadata_from_loaded, load_linux_elf_image, log_execve_load_error, map_load_error,
    map_request_error, preferred_exec_image_base, resolve_path, rewrite_linux_stack,
    snapshot_exec_strings, translate_guest_path_for_vfs, OsPersonality, Runtime, Word,
    ENAMETOOLONG, ENOEXEC, ESRCH, LINUX_EXEC_PATH_MAX,
};

pub(super) enum ExecError {
    Errno(i32),
    ImageReplaced,
}

impl From<i32> for ExecError {
    fn from(errno: i32) -> Self {
        Self::Errno(errno)
    }
}

pub(super) fn sys_execve(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    argv_ptr: Word,
    envp_ptr: Word,
) -> Result<(), ExecError> {
    let len = resolve_path(runtime, pid, path_ptr).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "resolve-path", 0, errno);
        errno
    })?;
    if len as usize > LINUX_EXEC_PATH_MAX {
        return Err(ExecError::Errno(ENAMETOOLONG));
    }
    let mut exec_path = [0u8; LINUX_EXEC_PATH_MAX];
    let path_len = len as usize;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            exec_path.as_mut_ptr(),
            path_len,
        );
    }
    let snapshot = snapshot_exec_strings(runtime, pid, argv_ptr, envp_ptr).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "snapshot-strings", len, errno);
        errno
    })?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            exec_path.as_ptr(),
            runtime.posix_shm as *mut u8,
            path_len,
        );
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "translate-path", len, errno);
        errno
    })?;
    copy_current_path_to_client_shm(runtime, vfs_len).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "copy-path", vfs_len, errno);
        errno
    })?;
    let loaded = match load_linux_elf_image(runtime, 0, vfs_len) {
        Ok(loaded) => loaded,
        Err(error) => {
            log_execve_load_error(runtime, pid, vfs_len, error);
            return Err(ExecError::Errno(map_load_error(error)));
        }
    };
    let personality = runtime
        .managed_process(pid)
        .map(|process| process.personality)
        .ok_or(ESRCH)?;
    if loaded.metadata.has_interpreter && personality != OsPersonality::Linux {
        return Err(ExecError::Errno(ENOEXEC));
    }
    let interpreter = crate::common::dynamic::prepare(runtime, &loaded).map_err(map_load_error)?;
    let preferred_image_base = preferred_exec_image_base(personality, &loaded.metadata);
    let exec_elf = current_elf_metadata_from_loaded(&loaded.metadata, preferred_image_base);
    if let Err(error) = libnanami::request_process_exec_memory(pid, loaded.address, loaded.size, 4)
    {
        let errno = map_request_error(error);
        libnanami::println!(
            "[alter/linux] execve exec-memory failed pid={} image_base={:#x} bytes={:#x} errno={}",
            pid,
            preferred_image_base,
            loaded.size,
            errno
        );
        return Err(ExecError::Errno(errno));
    }
    close_cloexec_files(runtime, pid);
    if !runtime.reset_process_runtime_for_exec(pid) {
        return Err(ExecError::ImageReplaced);
    }
    if let Some(interpreter) = interpreter.as_ref() {
        if crate::common::dynamic::install(pid, interpreter).is_err() {
            return Err(ExecError::ImageReplaced);
        }
    }
    if !crate::common::dynamic::track(runtime, pid, &loaded.metadata, interpreter.as_ref()) {
        return Err(ExecError::ImageReplaced);
    }
    let (base, base_len) = basename_in_bytes(&exec_path, path_len);
    if !runtime.set_process_image_name(pid, &exec_path[base..base + base_len]) {
        return Err(ExecError::ImageReplaced);
    }
    if let Err(errno) = rewrite_linux_stack(
        runtime,
        pid,
        &exec_path,
        path_len,
        &snapshot,
        preferred_image_base,
        Some(exec_elf),
        interpreter.as_ref(),
    ) {
        libnanami::println!(
            "[alter/linux] execve stack rewrite failed pid={} image_base={:#x} errno={}",
            pid,
            preferred_image_base,
            errno
        );
        return Err(ExecError::ImageReplaced);
    }
    Ok(())
}

pub(super) fn log_execve_stage_error(
    runtime: &Runtime,
    pid: Word,
    stage: &str,
    path_len: Word,
    errno: i32,
) {
    libnanami::println!(
        "[alter/linux] execve failed pid={} stage={} path-len={} client-shm={:#x} posix-shm={:#x} errno={}",
        pid,
        stage,
        path_len,
        runtime.client_shm_size,
        runtime.posix_shm_size,
        errno
    );
}
