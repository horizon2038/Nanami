use super::*;

pub(super) fn sys_ftruncate(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    length: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if (length as isize) < 0
        || file.kind != LinuxFileKind::Posix
        || file.resource & LINUX_O_ACCMODE == LINUX_O_RDONLY
    {
        return Err(EINVAL);
    }
    posix::posix_ftruncate(runtime.posix_port, file.posix_fd, length).map_err(map_request_error)?;
    Ok(0)
}
