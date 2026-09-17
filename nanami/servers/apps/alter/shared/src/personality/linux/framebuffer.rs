use super::{
    align_up_word, arm_clock_timer, honoka, map_request_error, refresh_clock, write_target_memory,
    LinuxFile, Runtime, Word, EACCES, EBADF, EFAULT, EINVAL, EIO, ENODEV, ENOMEM, ENOTTY,
    LINUX_DIRECT_COPY_CHUNK, LINUX_FBIOGET_FSCREENINFO, LINUX_FBIOGET_VSCREENINFO,
    LINUX_FBIOPAN_DISPLAY, LINUX_FBIOPUT_VSCREENINFO, LINUX_FB_FIX_SCREENINFO_BYTES,
    LINUX_FB_VAR_SCREENINFO_BYTES, LINUX_PAGE_SIZE, LINUX_PROT_READ, LINUX_PROT_WRITE,
};
use super::framebuffer_info::{write_fb_fix_screeninfo, write_fb_var_screeninfo};
use crate::common::framebuffer_size::FramebufferSize;

pub(super) fn sys_framebuffer_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let base = ensure_framebuffer_mapping(
        runtime,
        pid,
        file.resource,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    )?;
    let session = graphics_session(runtime, file.resource)?;
    let available = session.framebuffer_bytes.saturating_sub(file.offset);
    let bytes = len.min(available);
    let mut copied = 0;
    while copied < bytes {
        let chunk = (bytes - copied).min(LINUX_DIRECT_COPY_CHUNK);
        libnanami::request_process_memory_copy_within(
            pid,
            base + file.offset + copied,
            user_buffer + copied,
            chunk,
        )
        .map_err(map_request_error)?;
        copied += chunk;
    }
    file.offset += copied;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(copied)
}

pub(super) fn sys_framebuffer_write(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let base = ensure_framebuffer_mapping(
        runtime,
        pid,
        file.resource,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    )?;
    let session = graphics_session(runtime, file.resource)?;
    let available = session.framebuffer_bytes.saturating_sub(file.offset);
    let bytes = len.min(available);
    let mut copied = 0;
    while copied < bytes {
        let chunk = (bytes - copied).min(LINUX_DIRECT_COPY_CHUNK);
        libnanami::request_process_memory_copy_within(
            pid,
            user_buffer + copied,
            base + file.offset + copied,
            chunk,
        )
        .map_err(map_request_error)?;
        copied += chunk;
    }
    file.offset += copied;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    notify_graphics_session(session)?;
    Ok(copied)
}

pub(super) fn sys_framebuffer_mmap(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    len: Word,
    prot: Word,
    offset: Word,
) -> Result<Word, i32> {
    let session = graphics_session(runtime, file.resource)?;
    if offset != 0 || len > align_up_word(session.framebuffer_bytes, LINUX_PAGE_SIZE) {
        return Err(EINVAL);
    }
    let base = ensure_framebuffer_mapping(runtime, pid, file.resource, prot)?;
    Ok(base)
}

pub(super) fn ensure_framebuffer_mapping(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    prot: Word,
) -> Result<Word, i32> {
    let session = graphics_session(runtime, id)?;
    if session.guest_framebuffer != 0 {
        return if session.guest_pid == pid {
            Ok(session.guest_framebuffer)
        } else {
            Err(EACCES)
        };
    }
    refresh_clock(runtime)?;
    let (shared, bytes) = honoka::honoka_attach_logical_framebuffer_to_process(
        session.honoka_port,
        session.window_id,
        pid,
    )
    .map_err(map_request_error)?;
    if bytes != session.framebuffer_bytes || shared == 0 {
        if shared != 0 && bytes != 0 {
            let _ = libnanami::request_process_mapping_release(pid, shared, bytes);
        }
        let _ = honoka::honoka_detach_logical_framebuffer(session.honoka_port, session.window_id);
        return Err(EIO);
    }
    let framebuffer = shared;
    let framebuffer_bytes = bytes;
    let mapped = align_up_word(framebuffer_bytes, LINUX_PAGE_SIZE);
    if !runtime.add_mapping(pid, framebuffer, mapped, prot) {
        let _ = libnanami::request_process_mapping_release(pid, shared, bytes);
        let _ = honoka::honoka_detach_logical_framebuffer(session.honoka_port, session.window_id);
        return Err(ENOMEM);
    }
    let index = id.checked_sub(1).ok_or(ENODEV)? as usize;
    runtime.graphics[index].damage_queue = 0;
    runtime.graphics[index].framebuffer = framebuffer;
    runtime.graphics[index].framebuffer_bytes = framebuffer_bytes;
    runtime.graphics[index].guest_pid = pid;
    runtime.graphics[index].guest_framebuffer = framebuffer;
    runtime.graphics[index].guest_framebuffer_bytes = framebuffer_bytes;
    arm_clock_timer(runtime)?;
    Ok(framebuffer)
}

pub(super) fn graphics_session(
    runtime: &Runtime,
    id: Word,
) -> Result<crate::state::GraphicsSession, i32> {
    let index = id.checked_sub(1).ok_or(ENODEV)? as usize;
    let session = *runtime.graphics.get(index).ok_or(ENODEV)?;
    if !session.active {
        return Err(ENODEV);
    }
    Ok(session)
}

pub(super) fn present_graphics_session(
    runtime: &mut Runtime,
    id: Word,
    pid: Word,
) -> Result<(), i32> {
    let session = graphics_session(runtime, id)?;
    if session.guest_pid != 0 && session.guest_pid != pid {
        return Err(EACCES);
    }
    notify_graphics_session(session)
}

pub(super) fn notify_graphics_session(session: crate::state::GraphicsSession) -> Result<(), i32> {
    libnanami::ipc::notification_notify(session.present_notification).map_err(map_request_error)
}

pub(super) fn present_mapped_framebuffers(runtime: &Runtime) {
    for session in runtime.graphics {
        if session.active && session.guest_pid != 0 && session.guest_framebuffer != 0 {
            let _ = notify_graphics_session(session);
        }
    }
}

pub(super) fn sys_framebuffer_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    let session = graphics_session(runtime, file.resource)?;
    let size = FramebufferSize { width: session.width, height: session.height };
    match request {
        LINUX_FBIOGET_FSCREENINFO => {
            if argument == 0 {
                return Err(EFAULT);
            }
            write_fb_fix_screeninfo(runtime.posix_shm, size);
            write_target_memory(runtime, pid, argument, LINUX_FB_FIX_SCREENINFO_BYTES)?;
            Ok(0)
        }
        LINUX_FBIOGET_VSCREENINFO => {
            if argument == 0 {
                return Err(EFAULT);
            }
            write_fb_var_screeninfo(runtime.posix_shm, size);
            write_target_memory(runtime, pid, argument, LINUX_FB_VAR_SCREENINFO_BYTES)?;
            Ok(0)
        }
        LINUX_FBIOPUT_VSCREENINFO | LINUX_FBIOPAN_DISPLAY => {
            if argument == 0 {
                return Err(EFAULT);
            }
            present_graphics_session(runtime, file.resource, pid)?;
            Ok(0)
        }
        _ => Err(ENOTTY),
    }
}
