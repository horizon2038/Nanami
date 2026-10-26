use super::*;
use crate::state::readiness::{PollFd, ReadinessWait, READY_FLOCK};

pub(super) fn lock(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    operation: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let op = operation & !4;
    if !matches!(op, 1 | 2 | 8) || operation & !15 != 0 {
        return Err(EINVAL);
    }
    if !file.has_posix_backend() {
        return Err(EOPNOTSUPP);
    }
    let notification = if operation & 4 == 0 && op != 8 {
        nanami_runtime::PROCESS_SLOT_NOTIFICATION
    } else {
        0
    };
    if !posix::posix_flock(runtime.posix_port, file.posix_fd, op, notification).map_err(
        |error| match error {
            RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR) => EBADF,
            RequestError::Status(nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION) => EOPNOTSUPP,
            error => map_request_error(error),
        },
    )? {
        return Err(EAGAIN);
    }
    Ok(0)
}

pub(super) fn park(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> bool {
    if context.number != SYS_FLOCK || context.args[1] & 4 != 0 {
        return false;
    }
    let Some(process) = runtime.managed_process_mut(pid) else {
        return false;
    };
    process.readiness_wait = Some(ReadinessWait {
        context,
        entries: [PollFd::EMPTY; LINUX_FD_MAX],
        count: 0,
        sources: READY_FLOCK,
        deadline: None,
        select: false,
    });
    true
}
