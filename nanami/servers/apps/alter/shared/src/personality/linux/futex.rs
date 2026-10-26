//! Serialized compare/register and wake at the Linux personality boundary.
//! Shared keys use backing identity plus byte offset, never a process's alias VA.
use super::*;
use crate::state::task_sync::{FutexKey, FutexWait};
use alloc::vec::Vec;

const PRIVATE: Word = 128;
const CLOCK_REALTIME: Word = 256;
const WAIT: Word = 0;
const WAKE: Word = 1;
const WAIT_BITSET: Word = 9;
const WAKE_BITSET: Word = 10;

fn key(runtime: &Runtime, pid: Word, address: Word, private: bool) -> Result<FutexKey, i32> {
    if address & 3 != 0 {
        return Err(EINVAL);
    }
    if !private {
        if let Some((id, mapping)) = super::memfd::mapping_at(runtime, pid, address) {
            if mapping.shared {
                return Ok(FutexKey {
                    scope: id,
                    address: mapping.offset + address - mapping.base,
                    shared: true,
                });
            }
        }
    }
    Ok(FutexKey {
        scope: runtime.vm_group(pid),
        address,
        shared: false,
    })
}

pub(super) fn set_tid(runtime: &mut Runtime, pid: Word, address: Word) -> Result<Word, i32> {
    runtime.managed_process(pid).ok_or(ESRCH)?;
    runtime.tasks.ensure(pid).ok_or(ENOMEM)?.clear_tid = address;
    Ok(pid)
}

pub(super) fn set_robust(
    runtime: &mut Runtime,
    pid: Word,
    head: Word,
    bytes: Word,
) -> Result<Word, i32> {
    if bytes != 24 {
        return Err(EINVAL);
    }
    runtime.tasks.ensure(pid).ok_or(ENOMEM)?.robust_list = head;
    Ok(0)
}

pub(super) fn get_robust(
    runtime: &mut Runtime,
    pid: Word,
    target: Word,
    head: Word,
    bytes: Word,
) -> Result<Word, i32> {
    let target = if target == 0 { pid } else { target };
    runtime
        .managed_process(target)
        .filter(|process| !process.exited)
        .ok_or(ESRCH)?;
    let pointer = runtime.tasks.get(target).map_or(0, |task| task.robust_list);
    write_guest_u64(runtime, pid, head, pointer)?;
    write_guest_u64(runtime, pid, bytes, 24)?;
    Ok(0)
}

fn abandon_lock(runtime: &mut Runtime, pid: Word, node: Word, offset: isize) {
    const TID_MASK: u32 = 0x3fff_ffff;
    const OWNER_DIED: u32 = 1 << 30;
    const WAITERS: u32 = 1 << 31;
    // PI robust mutexes require priority inheritance, not ordinary futex wake.
    if node & 1 != 0 {
        return;
    }
    let Some(address) = node
        .checked_add_signed(offset)
        .filter(|address| *address != 0 && *address & 3 == 0)
    else {
        return;
    };
    if read_target_memory(runtime, pid, address, 4).is_err() {
        return;
    }
    if !runtime.memory_access_allowed(pid, address, 4, LINUX_PROT_WRITE) {
        return;
    }
    let mut observed = read_shm_u32(runtime, 0);
    for _ in 0..8 {
        if observed & TID_MASK != pid as u32 {
            return;
        }
        let desired = (observed & WAITERS) | OWNER_DIED;
        let Ok(previous) = nanami_runtime::request_process_memory_compare_exchange_u32(
            pid, address, observed, desired,
        ) else {
            return;
        };
        if previous == observed {
            if observed & WAITERS != 0 {
                if let Ok(key) = key(runtime, pid, address, false) {
                    wake(runtime, key, 1, u32::MAX);
                }
            }
            return;
        }
        observed = previous;
    }
}

fn abandon_robust_list(runtime: &mut Runtime, pid: Word, head: Word) {
    if head == 0 || read_target_memory(runtime, pid, head, 24).is_err() {
        return;
    }
    let mut node = read_shm_u64(runtime, 0);
    let offset = read_shm_u64(runtime, 8) as isize;
    let pending = read_shm_u64(runtime, 16);
    // Linux bounds malicious/cyclic lists too. Save next before overwriting a lock.
    for _ in 0..2048 {
        if node & !1 == head || node & !1 == 0 {
            break;
        }
        if read_target_memory(runtime, pid, node & !1, 8).is_err() {
            break;
        }
        let next = read_shm_u64(runtime, 0);
        if node != pending {
            abandon_lock(runtime, pid, node, offset);
        }
        node = next;
    }
    if pending != 0 {
        abandon_lock(runtime, pid, pending, offset);
    }
}

fn waiting(runtime: &Runtime, pid: Word) -> bool {
    runtime.managed_process(pid).is_some_and(|process| {
        !process.exited && process.signal_waiting && process.signal_context.number == SYS_FUTEX
    })
}

pub(super) fn next_deadline(runtime: &Runtime) -> Option<Word> {
    runtime
        .tasks
        .waiters()
        .filter(|(pid, wait)| wait.result.is_none() && waiting(runtime, *pid))
        .filter_map(|(_, wait)| wait.deadline)
        .min()
}

fn complete(runtime: &mut Runtime, pid: Word, value: isize) {
    let Some(process) = runtime.managed_process_mut(pid) else {
        return;
    };
    let (pcb, context, personality) = (process.pcb, process.signal_context, process.personality);
    process.signal_waiting = false;
    if let Some(task) = runtime.tasks.ensure(pid) {
        task.wait = None;
    }
    record_syscall_result(runtime, pid, context.number, value);
    if crate::process::write_personality_syscall_return(pcb, context, value, personality).is_ok() {
        let _ = a9n_abi::arch::process_control_block::resume(pcb);
    }
}

pub(super) fn pump(runtime: &mut Runtime) {
    let now = runtime.monotonic_ticks;
    let waiters: Vec<_> = runtime.tasks.waiters().collect();
    for (pid, wait) in waiters {
        if !waiting(runtime, pid) {
            if let Some(task) = runtime.tasks.ensure(pid) {
                task.wait = None;
            }
            continue;
        }
        let value = wait.result.or_else(|| {
            wait.deadline
                .filter(|deadline| *deadline <= now)
                .map(|_| -(ETIMEDOUT as isize))
        });
        if let Some(value) = value {
            if runtime.is_signal_stopped(pid) {
                if let Some(wait) = runtime
                    .tasks
                    .ensure(pid)
                    .and_then(|task| task.wait.as_mut())
                {
                    wait.result = Some(value);
                }
            } else {
                complete(runtime, pid, value);
            }
        }
    }
}

fn wake(runtime: &mut Runtime, key: FutexKey, count: Word, bitset: u32) -> Word {
    let waiters: Vec<_> = runtime
        .tasks
        .waiters()
        .filter(|(pid, wait)| {
            wait.result.is_none()
                && wait.key == key
                && wait.bitset & bitset != 0
                && waiting(runtime, *pid)
        })
        .take(count)
        .map(|(pid, _)| pid)
        .collect();
    let count = waiters.len();
    for pid in waiters {
        if let Some(wait) = runtime
            .tasks
            .ensure(pid)
            .and_then(|task| task.wait.as_mut())
        {
            wait.result = Some(0);
        }
    }
    pump(runtime);
    count
}

pub(super) fn dispatch(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let result = (|| {
        let [address, operation, value, timeout, _, bits] = context.args;
        let command = operation & !(PRIVATE | CLOCK_REALTIME);
        if !matches!(command, WAIT | WAKE | WAIT_BITSET | WAKE_BITSET) {
            return Err(ENOSYS);
        }
        if operation & CLOCK_REALTIME != 0 && command != WAIT_BITSET {
            return Err(ENOSYS);
        }
        let key = key(runtime, pid, address, operation & PRIVATE != 0)?;
        let bitset = if matches!(command, WAIT_BITSET | WAKE_BITSET) {
            bits as u32
        } else {
            u32::MAX
        };
        if bitset == 0 {
            return Err(EINVAL);
        }
        if matches!(command, WAKE | WAKE_BITSET) {
            if (value as i32) < 0 {
                return Err(EINVAL);
            }
            let count = wake(runtime, key, value, bitset);
            arm_clock_timer(runtime)?;
            return Ok(Some(count));
        }
        // The server accepts no wake syscall between this read and queue insertion.
        // A concurrent waker blocks in Alter IPC until registration is complete.
        let deadline = if timeout == 0 {
            None
        } else {
            let absolute = command == WAIT_BITSET;
            let clock = if operation & CLOCK_REALTIME != 0 {
                0
            } else {
                1
            };
            let deadline = clocks::sleep_deadline(runtime, pid, timeout, clock, absolute)?;
            Some(deadline.unwrap_or(runtime.monotonic_ticks))
        };
        read_target_memory(runtime, pid, address, 4)?;
        if read_shm_u32(runtime, 0) != value as u32 {
            return Err(EAGAIN);
        }
        if deadline.is_some_and(|deadline| deadline <= runtime.monotonic_ticks) {
            return Err(ETIMEDOUT);
        }
        runtime.tasks.ensure(pid).ok_or(ENOMEM)?.wait = Some(FutexWait {
            key,
            bitset,
            deadline,
            result: None,
        });
        if !runtime.park_signal_waiter(pid, Word::MAX, context) {
            return Err(ESRCH);
        }
        if let Err(errno) = arm_clock_timer(runtime) {
            runtime.tasks.ensure(pid).unwrap().wait = None;
            runtime.managed_process_mut(pid).unwrap().signal_waiting = false;
            return Err(errno);
        }
        Ok(None)
    })();
    let action = match result {
        Ok(None) => EmulationAction::Park,
        Ok(Some(value)) => EmulationAction::Return(value as isize),
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    };
    record_action_result(runtime, pid, context.number, action);
    action
}

pub(super) fn exit(runtime: &mut Runtime, pid: Word) {
    let robust = runtime.tasks.get(pid).map_or(0, |task| task.robust_list);
    abandon_robust_list(runtime, pid, robust);
    let address = runtime.tasks.get(pid).map_or(0, |task| task.clear_tid);
    runtime.tasks.remove(pid);
    if address != 0 && write_guest_u32(runtime, pid, address, 0).is_ok() {
        for private in [false, true] {
            if let Ok(key) = key(runtime, pid, address, private) {
                wake(runtime, key, 1, u32::MAX);
            }
        }
    }
}
