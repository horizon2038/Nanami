use super::*;
use crate::state::readiness::{PollFd, ReadinessWait};

pub(super) fn read_poll_set(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<ReadinessWait, i32> {
    let select = matches!(context.number, SYS_SELECT | SYS_PSELECT6);
    let mut wait = ReadinessWait {
        context,
        entries: [PollFd::EMPTY; LINUX_FD_MAX],
        count: 0,
        sources: 0,
        deadline: None,
        select,
    };
    if select {
        let nfds = context.args[0];
        if nfds > LINUX_FD_MAX {
            return Err(EINVAL);
        }
        // Snapshot all three sets before writing anything back (sets may alias).
        let mut sets = [0; 3];
        if nfds != 0 {
            for (set, ptr) in sets.iter_mut().zip(&context.args[1..4]) {
                if *ptr != 0 {
                    read_target_memory(runtime, pid, *ptr, 8)?;
                    *set = unsafe { (runtime.posix_shm as *const Word).read_unaligned() };
                }
            }
        }
        for fd in 0..nfds {
            let bit = 1usize << fd;
            let events = (if sets[0] & bit != 0 { LINUX_POLLIN } else { 0 })
                | (if sets[1] & bit != 0 { LINUX_POLLOUT } else { 0 })
                | (if sets[2] & bit != 0 { LINUX_POLLPRI } else { 0 });
            if events != 0 {
                wait.entries[wait.count] = PollFd {
                    fd: fd as i32,
                    events,
                    revents: 0,
                };
                wait.count += 1;
            }
        }
    } else {
        let [ptr, count, ..] = context.args;
        if count > LINUX_POLLFD_MAX {
            return Err(EINVAL);
        }
        if ptr == 0 && count != 0 {
            return Err(EFAULT);
        }
        if count != 0 {
            read_target_memory(runtime, pid, ptr, count * LINUX_POLLFD_BYTES)?;
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    runtime.posix_shm as *const u8,
                    wait.entries.as_mut_ptr() as *mut u8,
                    count * LINUX_POLLFD_BYTES,
                );
            }
        }
        wait.count = count;
    }
    Ok(wait)
}

pub(super) fn write_poll_set(
    runtime: &mut Runtime,
    pid: Word,
    wait: &ReadinessWait,
) -> Result<Word, i32> {
    if wait.select {
        let mut sets = [0usize; 3];
        for entry in &wait.entries[..wait.count] {
            let bit = 1usize << entry.fd;
            if entry.events & LINUX_POLLIN != 0
                && entry.revents & (LINUX_POLLIN | LINUX_POLLHUP | LINUX_POLLERR) != 0
            {
                sets[0] |= bit;
            }
            if entry.events & LINUX_POLLOUT != 0
                && entry.revents & (LINUX_POLLOUT | LINUX_POLLERR) != 0
            {
                sets[1] |= bit;
            }
            if entry.events & LINUX_POLLPRI != 0 && entry.revents & LINUX_POLLPRI != 0 {
                sets[2] |= bit;
            }
        }
        if wait.context.args[0] != 0 {
            for (set, ptr) in sets.iter().zip(&wait.context.args[1..4]) {
                if *ptr != 0 {
                    unsafe {
                        write_u64(runtime.posix_shm, *set);
                    }
                    write_target_memory(runtime, pid, *ptr, 8)?;
                }
            }
        }
        Ok(sets.iter().map(|set| set.count_ones() as Word).sum())
    } else {
        let count = wait.count;
        if count != 0 {
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    wait.entries.as_ptr() as *const u8,
                    runtime.posix_shm as *mut u8,
                    count * LINUX_POLLFD_BYTES,
                );
            }
            write_target_memory(
                runtime,
                pid,
                wait.context.args[0],
                count * LINUX_POLLFD_BYTES,
            )?;
        }
        Ok(wait.entries[..count]
            .iter()
            .filter(|entry| entry.revents != 0)
            .count())
    }
}
