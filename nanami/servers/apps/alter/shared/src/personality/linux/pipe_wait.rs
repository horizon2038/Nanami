//! Reuse the existing readiness record for blocking pipe I/O; no polling loop.
use super::{
    LinuxFileKind, LinuxSyscallContext, Runtime, Word, LINUX_FD_MAX, LINUX_O_NONBLOCK,
    LINUX_POLLIN, LINUX_POLLOUT, SYS_READ, SYS_READV, SYS_WRITE, SYS_WRITEV,
};
use crate::state::readiness::{PollFd, ReadinessWait, READY_PIPE};

pub(super) fn park(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> bool {
    let events = match context.number {
        SYS_READ | SYS_READV => LINUX_POLLIN,
        SYS_WRITE | SYS_WRITEV => LINUX_POLLOUT,
        _ => return false,
    };
    let fd = context.args[0];
    let Some(file) = runtime.linux_file(pid, fd) else {
        return false;
    };
    if !matches!(
        file.kind,
        LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite
    ) || file.flags & LINUX_O_NONBLOCK != 0
    {
        return false;
    }
    let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
    entries[0] = PollFd {
        fd: fd as i32,
        events,
        revents: 0,
    };
    let Some(process) = runtime.managed_process_mut(pid) else {
        return false;
    };
    process.readiness_wait = Some(ReadinessWait {
        context,
        entries,
        count: 1,
        sources: READY_PIPE,
        deadline: None,
        select: false,
    });
    true
}

pub(super) fn retry(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Option<Result<Word, i32>> {
    let [fd, buffer, bytes, ..] = context.args;
    Some(match context.number {
        SYS_READ => super::sys_read(runtime, pid, fd, buffer, bytes),
        SYS_WRITE => super::sys_write(runtime, pid, fd, buffer, bytes),
        SYS_READV => super::sys_readv(runtime, pid, fd, buffer, bytes),
        SYS_WRITEV => super::sys_writev(runtime, pid, fd, buffer, bytes),
        _ => return None,
    })
}
