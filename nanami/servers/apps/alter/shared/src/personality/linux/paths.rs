use super::{
    graphics_enabled, map_request_error, personality, virtual_fs, Runtime, VirtualNode, Word,
    EFAULT, EINVAL, ENAMETOOLONG, EROFS, ESRCH, LINUX_AT_FDCWD, LINUX_CWD_MAX, LINUX_PAGE_SIZE,
};

pub(super) fn read_c_string(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let max = ::core::cmp::min(runtime.posix_shm_size as usize, 4096);
    let mut copied = 0usize;
    while copied < max {
        let source = user_ptr.checked_add(copied as Word).ok_or(EFAULT)?;
        let page_remaining = (LINUX_PAGE_SIZE - (source & (LINUX_PAGE_SIZE - 1))) as usize;
        // Most paths terminate near the start: avoid copying an entire page.
        let chunk = ::core::cmp::min(max - copied, page_remaining).min(128.max(copied));
        libnanami::request_process_memory_read(
            pid,
            source,
            runtime.posix_shm + copied as Word,
            chunk as Word,
        )
        .map_err(map_request_error)?;

        let end = copied + chunk;
        while copied < end {
            let byte =
                unsafe { ::core::ptr::read((runtime.posix_shm + copied as Word) as *const u8) };
            if byte == 0 {
                return Ok(copied as Word);
            }
            copied += 1;
        }
    }
    Err(ENAMETOOLONG)
}

pub(super) fn resolve_path(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    let len = read_c_string(runtime, pid, user_ptr)?;
    resolve_current_shm_path(runtime, pid, len)
}

pub(super) fn translate_guest_path_for_vfs(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
) -> Result<Word, i32> {
    translate_guest_path_at(runtime, pid, 0, len)
}

pub(super) fn translate_guest_path_at(
    runtime: &mut Runtime,
    pid: Word,
    offset: Word,
    len: Word,
) -> Result<Word, i32> {
    if len == 0 {
        return Err(EINVAL);
    }
    if offset
        .checked_add(len)
        .and_then(|value| value.checked_add(1))
        .filter(|end| *end <= runtime.posix_shm_size)
        .is_none()
    {
        return Err(ENAMETOOLONG);
    }

    let base = runtime.posix_shm + offset;
    if !path_is_absolute(base, len) || is_linux_virtual_path(base, len) {
        return Ok(len);
    }

    if path_has_component_prefix(base, len, b"/temp") {
        let suffix_len = len as usize - b"/temp".len();
        unsafe {
            ::core::ptr::copy(
                (base + b"/temp".len() as Word) as *const u8,
                (base + b"/tmp".len() as Word) as *mut u8,
                suffix_len,
            );
            ::core::ptr::copy_nonoverlapping(b"/tmp".as_ptr(), base as *mut u8, b"/tmp".len());
            ::core::ptr::write(
                (base + b"/tmp".len() as Word + suffix_len as Word) as *mut u8,
                0,
            );
        }
        return translate_guest_path_at(
            runtime,
            pid,
            offset,
            b"/tmp".len() as Word + suffix_len as Word,
        );
    }

    let guest_root = guest_root_for_process(runtime, pid)?;
    if path_equals(base, len, guest_root) || path_has_root_prefix(base, len, guest_root) {
        return Ok(len);
    }

    let len_usize = len as usize;
    if len_usize >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut original = [0u8; LINUX_CWD_MAX];
    unsafe {
        ::core::ptr::copy_nonoverlapping(base as *const u8, original.as_mut_ptr(), len_usize);
    }

    let suffix_len = if len_usize == 1 { 0 } else { len_usize };
    let new_len = guest_root.len() + suffix_len;
    if offset
        .checked_add(new_len as Word)
        .and_then(|value| value.checked_add(1))
        .filter(|end| *end <= runtime.posix_shm_size)
        .is_none()
    {
        return Err(ENAMETOOLONG);
    }

    unsafe {
        ::core::ptr::copy_nonoverlapping(guest_root.as_ptr(), base as *mut u8, guest_root.len());
        if suffix_len != 0 {
            ::core::ptr::copy_nonoverlapping(
                original.as_ptr(),
                (base + guest_root.len() as Word) as *mut u8,
                suffix_len,
            );
        }
        ::core::ptr::write((base + new_len as Word) as *mut u8, 0);
    }
    Ok(new_len as Word)
}

pub(super) fn is_linux_virtual_path(base: Word, len: Word) -> bool {
    path_has_component_prefix(base, len, b"/dev")
        || path_has_component_prefix(base, len, b"/proc")
        || path_has_component_prefix(base, len, b"/sys")
}

pub(super) fn reject_virtual_fs_mutation(base: Word, len: Word) -> Result<(), i32> {
    if is_linux_virtual_path(base, len) {
        Err(EROFS)
    } else {
        Ok(())
    }
}

pub(super) fn path_has_component_prefix(base: Word, len: Word, prefix: &[u8]) -> bool {
    if (len as usize) < prefix.len() {
        return false;
    }
    let path = unsafe { ::core::slice::from_raw_parts(base as *const u8, len as usize) };
    path.starts_with(prefix) && (path.len() == prefix.len() || path[prefix.len()] == b'/')
}

pub(super) fn current_virtual_node(runtime: &Runtime, pid: Word, len: Word) -> Option<VirtualNode> {
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    virtual_fs::lookup(path, graphics_enabled(runtime, pid))
}

pub(super) fn guest_root_for_process(runtime: &Runtime, pid: Word) -> Result<&'static [u8], i32> {
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    Ok(personality::root(process.personality))
}

pub(super) fn resolve_current_shm_path(
    runtime: &mut Runtime,
    pid: Word,
    input_len: Word,
) -> Result<Word, i32> {
    if input_len as usize >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut input = [0u8; LINUX_CWD_MAX];
    let input_len = input_len as usize;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            input.as_mut_ptr(),
            input_len,
        );
    }

    let mut out = [0u8; LINUX_CWD_MAX];
    let mut out_len: usize;
    if input_len != 0 && input[0] == b'/' {
        out[0] = b'/';
        out_len = 1;
    } else {
        let Some(process) = runtime.managed_process(pid) else {
            return Err(ESRCH);
        };
        if process.cwd_len == 0 || process.cwd_len >= LINUX_CWD_MAX {
            return Err(EINVAL);
        }
        out[..process.cwd_len].copy_from_slice(&process.cwd[..process.cwd_len]);
        out_len = process.cwd_len;
    }

    let mut i = 0usize;
    while i < input_len {
        while i < input_len && input[i] == b'/' {
            i += 1;
        }
        let start = i;
        while i < input_len && input[i] != b'/' {
            i += 1;
        }
        let len = i - start;
        if len == 0 || (len == 1 && input[start] == b'.') {
            continue;
        }
        if len == 2 && input[start] == b'.' && input[start + 1] == b'.' {
            out_len = pop_path_component(&mut out, out_len);
            continue;
        }
        out_len = push_path_component(&mut out, out_len, &input[start..start + len])?;
    }
    if out_len == 0 {
        out[0] = b'/';
        out_len = 1;
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(out.as_ptr(), runtime.posix_shm as *mut u8, out_len);
        ::core::ptr::write((runtime.posix_shm + out_len as Word) as *mut u8, 0);
    }
    Ok(out_len as Word)
}

pub(super) fn push_path_component(
    out: &mut [u8; LINUX_CWD_MAX],
    mut out_len: usize,
    component: &[u8],
) -> Result<usize, i32> {
    if out_len == 0 {
        out[0] = b'/';
        out_len = 1;
    }
    if out_len != 1 {
        if out_len + 1 >= LINUX_CWD_MAX {
            return Err(ENAMETOOLONG);
        }
        out[out_len] = b'/';
        out_len += 1;
    }
    if out_len + component.len() >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    out[out_len..out_len + component.len()].copy_from_slice(component);
    Ok(out_len + component.len())
}

pub(super) fn pop_path_component(out: &mut [u8; LINUX_CWD_MAX], mut out_len: usize) -> usize {
    if out_len <= 1 {
        out[0] = b'/';
        return 1;
    }
    while out_len > 1 && out[out_len - 1] == b'/' {
        out_len -= 1;
    }
    while out_len > 1 && out[out_len - 1] != b'/' {
        out_len -= 1;
    }
    if out_len > 1 {
        out_len -= 1;
    }
    out_len.max(1)
}

pub(super) fn path_has_root_prefix(base: Word, len: Word, root: &[u8]) -> bool {
    if len as usize <= root.len() {
        return false;
    }
    let mut i = 0usize;
    while i < root.len() {
        let byte = unsafe { ::core::ptr::read((base + i as Word) as *const u8) };
        if byte != root[i] {
            return false;
        }
        i += 1;
    }
    unsafe { ::core::ptr::read((base + root.len() as Word) as *const u8) == b'/' }
}

pub(super) fn path_is_absolute(base: Word, len: Word) -> bool {
    len != 0 && unsafe { ::core::ptr::read(base as *const u8) } == b'/'
}

pub(super) fn is_at_fdcwd(fd: Word) -> bool {
    fd == LINUX_AT_FDCWD
}

pub(super) fn basename_in_bytes(bytes: &[u8], len: usize) -> (usize, usize) {
    let mut start = 0usize;
    let mut i = 0usize;
    while i < len {
        if bytes[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    (start, len.saturating_sub(start))
}

pub(super) fn path_equals(base: Word, len: Word, expected: &[u8]) -> bool {
    if len as usize != expected.len() {
        return false;
    }
    let mut i = 0usize;
    while i < expected.len() {
        let byte = unsafe { ::core::ptr::read((base + i as Word) as *const u8) };
        if byte != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}
