use super::{
    read_target_memory, write_u32_to_target, Runtime, Word, EACCES, EFAULT, EINVAL, ENOMEM, ENOTTY,
    EPERM, ESRCH, LINUX_TIOCGPGRP, LINUX_TIOCGSID, LINUX_TIOCNOTTY, LINUX_TIOCSCTTY,
    LINUX_TIOCSPGRP,
};
use crate::state::JobControlError;

pub(super) fn errno(error: JobControlError) -> i32 {
    match error {
        JobControlError::NoProcess => ESRCH,
        JobControlError::Permission => EPERM,
        JobControlError::InvalidArgument => EINVAL,
        JobControlError::ExecutedChild => EACCES,
        JobControlError::NoTerminal => ENOTTY,
        JobControlError::NoSpace => ENOMEM,
    }
}

pub(super) fn terminal_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    request: Word,
    argument: Word,
) -> Option<Result<Word, i32>> {
    let id = runtime.managed_process(pid)?.terminal_id;
    terminal_ioctl_for_id(runtime, pid, id, request, argument)
}

pub(super) fn terminal_ioctl_for_id(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    request: Word,
    argument: Word,
) -> Option<Result<Word, i32>> {
    if !matches!(
        request,
        LINUX_TIOCGPGRP | LINUX_TIOCSPGRP | LINUX_TIOCGSID | LINUX_TIOCSCTTY | LINUX_TIOCNOTTY
    ) {
        return None;
    }
    Some((|| {
        match request {
            LINUX_TIOCSCTTY => {
                if argument > 1 {
                    return Err(EINVAL);
                }
                // No CAP_SYS_ADMIN is granted by Alter; do not steal another
                // session's terminal even when the caller passes argument 1.
                runtime.claim_controlling_terminal(pid, id).map_err(errno)?;
            }
            LINUX_TIOCNOTTY => runtime
                .detach_controlling_terminal(pid, id)
                .map_err(errno)?,
            _ => {
                let control = *runtime.controlling_terminal(pid, id).map_err(errno)?;
                if argument == 0 {
                    return Err(EFAULT);
                }
                match request {
                    LINUX_TIOCGPGRP => {
                        write_u32_to_target(runtime, pid, argument, control.foreground as u32)?
                    }
                    LINUX_TIOCGSID => {
                        write_u32_to_target(runtime, pid, argument, control.session as u32)?
                    }
                    LINUX_TIOCSPGRP => {
                        read_target_memory(runtime, pid, argument, 4)?;
                        let group =
                            unsafe { core::ptr::read_unaligned(runtime.posix_shm as *const i32) };
                        if group < 0 {
                            return Err(EINVAL);
                        }
                        runtime
                            .set_terminal_foreground(pid, id, group as Word)
                            .map_err(errno)?;
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(0)
    })())
}
