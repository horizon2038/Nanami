use super::*;

pub(super) fn sys_fsync(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::VirtualDirectory && file.resource == VirtualNode::Root.id() {
        // Alter overlays virtual entries on the guest root directory. Its real
        // directory entries still live on ext2 and must be synchronized too.
        posix::posix_sync(runtime.posix_port).map_err(map_request_error)?;
        return Ok(0);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(EINVAL);
    }
    // fdatasync currently uses the stronger fsync operation. ext2 flushes its
    // shared metadata as well, so syncfs on a real filesystem fd uses this path.
    posix::posix_fsync(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
    Ok(0)
}

pub(super) fn sys_sync(runtime: &mut Runtime) -> Result<Word, i32> {
    // Linux sync(2) has no error return. ext2 retains any writeback error for
    // subsequent fsync/fdatasync/syncfs calls, which do report it.
    let _ = posix::posix_sync(runtime.posix_port);
    Ok(0)
}
