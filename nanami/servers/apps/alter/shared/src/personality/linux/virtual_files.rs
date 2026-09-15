use super::{
    align_up_word, ensure_graphics_session, ensure_input, graphics_enabled, input_resource,
    is_linux_virtual_path, map_request_error, virtual_fs, write_target_memory, write_u16,
    write_u64, LinuxFile, LinuxFileKind, Runtime, VirtualNode, Word, EBADF, EINVAL, EISDIR, EMFILE,
    ENOENT, ENOTDIR, EROFS, ESRCH, LINUX_DIRENT64_NAME_OFFSET, LINUX_DT_CHR, LINUX_DT_DIR,
    LINUX_DT_REG, LINUX_FD_CLOEXEC, LINUX_O_ACCMODE, LINUX_O_CLOEXEC, LINUX_O_CREAT,
    LINUX_O_DIRECTORY, LINUX_O_NONBLOCK, LINUX_O_RDONLY, LINUX_O_TRUNC,
};

pub(super) fn sys_virtual_getdents(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    count: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let directory = VirtualNode::from_id(file.resource).ok_or(ENOTDIR)?;
    if !directory.is_directory() {
        return Err(ENOTDIR);
    }
    let graphics = graphics_enabled(runtime, pid);
    let mut out = 0usize;
    let mut index = file.offset as usize;
    while let Some(entry) = virtual_fs::directory_entry(directory, index, graphics) {
        let reclen = align_up_word(
            (LINUX_DIRENT64_NAME_OFFSET + entry.name.len() + 1) as Word,
            8,
        ) as usize;
        if out + reclen > count as usize || out + reclen > runtime.posix_shm_size as usize {
            break;
        }
        unsafe {
            let base = runtime.posix_shm + out as Word;
            ::core::ptr::write_bytes(base as *mut u8, 0, reclen);
            write_u64(base, entry.node.id());
            write_u64(base + 8, (index + 1) as Word);
            write_u16(base + 16, reclen as u16);
            let dtype = if entry.node.is_directory() {
                LINUX_DT_DIR
            } else if entry.node.is_regular_file() {
                LINUX_DT_REG
            } else {
                LINUX_DT_CHR
            };
            ::core::ptr::write((base + 18) as *mut u8, dtype as u8);
            ::core::ptr::copy_nonoverlapping(
                entry.name.as_ptr(),
                (base + LINUX_DIRENT64_NAME_OFFSET as Word) as *mut u8,
                entry.name.len(),
            );
        }
        out += reclen;
        index += 1;
    }
    file.offset = index as Word;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    if out != 0 {
        write_target_memory(runtime, pid, user_buffer, out as Word)?;
    }
    Ok(out as Word)
}

pub(super) fn sys_virtual_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let node = VirtualNode::from_id(file.resource).ok_or(EBADF)?;
    if node == VirtualNode::DevNull {
        return Ok(0);
    }
    if node == VirtualNode::DevZero {
        let bytes = len.min(runtime.posix_shm_size);
        unsafe { ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, bytes as usize) };
        write_target_memory(runtime, pid, user_buffer, bytes)?;
        return Ok(bytes);
    }
    let memory_text;
    let memory_text_len;
    let image_name;
    let bytes = if node == VirtualNode::ProcMemInfo {
        let info = libnanami::request_nanami_info_memory().map_err(map_request_error)?;
        (memory_text, memory_text_len) = format_proc_meminfo(info.total_bytes, info.free_bytes);
        &memory_text[..memory_text_len]
    } else if node == VirtualNode::ProcSelfExe {
        let process = runtime.managed_process(pid).ok_or(ESRCH)?;
        image_name = process.image_name;
        &image_name[..process.image_name_len]
    } else {
        virtual_fs::static_file(node).ok_or(EINVAL)?
    };
    let offset = file.offset as usize;
    if offset >= bytes.len() {
        return Ok(0);
    }
    let amount = (bytes.len() - offset)
        .min(len as usize)
        .min(runtime.posix_shm_size as usize);
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes[offset..offset + amount].as_ptr(),
            runtime.posix_shm as *mut u8,
            amount,
        );
    }
    write_target_memory(runtime, pid, user_buffer, amount as Word)?;
    file.offset += amount as Word;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(amount as Word)
}

pub(super) fn sys_virtual_write(
    runtime: &Runtime,
    pid: Word,
    fd: Word,
    len: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let node = VirtualNode::from_id(file.resource).ok_or(EBADF)?;
    match node {
        VirtualNode::DevNull | VirtualNode::DevZero => Ok(len),
        _ => Err(EBADF),
    }
}

pub(super) fn format_proc_meminfo(total: Word, free: Word) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    let mut pos = 0usize;
    pos = append_bytes_to_array(&mut out, pos, b"MemTotal:       ");
    pos = append_decimal_to_array(&mut out, pos, total / 1024);
    pos = append_bytes_to_array(&mut out, pos, b" kB\nMemFree:        ");
    pos = append_decimal_to_array(&mut out, pos, free / 1024);
    pos = append_bytes_to_array(&mut out, pos, b" kB\n");
    (out, pos)
}

pub(super) fn append_bytes_to_array(out: &mut [u8], mut pos: usize, value: &[u8]) -> usize {
    for byte in value {
        if pos >= out.len() {
            break;
        }
        out[pos] = *byte;
        pos += 1;
    }
    pos
}

pub(super) fn append_decimal_to_array(out: &mut [u8], pos: usize, mut value: Word) -> usize {
    if value == 0 {
        return append_bytes_to_array(out, pos, b"0");
    }
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    while value != 0 {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    let mut out_pos = pos;
    while count != 0 {
        count -= 1;
        out_pos = append_bytes_to_array(out, out_pos, &digits[count..count + 1]);
    }
    out_pos
}

pub(super) fn open_virtual_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    linux_flags: Word,
) -> Result<Option<Word>, i32> {
    let graphics_enabled = runtime
        .managed_process(pid)
        .map(|process| process.graphics_enabled)
        .ok_or(ESRCH)?;
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    let Some(node) = virtual_fs::lookup(path, graphics_enabled) else {
        return if is_linux_virtual_path(runtime.posix_shm, len) {
            Err(ENOENT)
        } else {
            Ok(None)
        };
    };
    if (linux_flags & LINUX_O_DIRECTORY) != 0 && !node.is_directory() {
        return Err(ENOTDIR);
    }
    let access_mode = linux_flags & LINUX_O_ACCMODE;
    if node.is_directory() && access_mode != LINUX_O_RDONLY {
        return Err(EISDIR);
    }
    if node.is_regular_file()
        && (access_mode != LINUX_O_RDONLY || (linux_flags & (LINUX_O_CREAT | LINUX_O_TRUNC)) != 0)
    {
        return Err(EROFS);
    }
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    } | (linux_flags & LINUX_O_NONBLOCK);
    let file = match node {
        VirtualNode::DevTty => LinuxFile::terminal(),
        VirtualNode::DevNull | VirtualNode::DevZero => {
            LinuxFile::virtual_node(LinuxFileKind::VirtualFile, node.id(), fd_flags)
        }
        VirtualNode::DevKeyboard => {
            ensure_input(runtime, pid)?;
            LinuxFile::virtual_node(
                LinuxFileKind::EvdevKeyboard,
                input_resource(runtime, pid, node.id())?,
                fd_flags,
            )
        }
        VirtualNode::DevMouse => {
            ensure_input(runtime, pid)?;
            LinuxFile::virtual_node(
                LinuxFileKind::EvdevMouse,
                input_resource(runtime, pid, node.id())?,
                fd_flags,
            )
        }
        VirtualNode::DevFramebuffer => {
            let session = ensure_graphics_session(runtime, pid)?;
            let mut file = LinuxFile::virtual_node(LinuxFileKind::Framebuffer, node.id(), fd_flags);
            file.resource = session;
            file
        }
        node if node.is_directory() => {
            LinuxFile::virtual_node(LinuxFileKind::VirtualDirectory, node.id(), fd_flags)
        }
        _ => LinuxFile::virtual_node(LinuxFileKind::VirtualFile, node.id(), fd_flags),
    };
    runtime
        .allocate_linux_file(pid, file, 0)
        .map(Some)
        .ok_or(EMFILE)
}
