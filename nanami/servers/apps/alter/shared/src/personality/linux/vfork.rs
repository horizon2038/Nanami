//! vfork inherits the stopped parent's stack; it has no clone argument words.
use super::*;

pub(super) fn sys_vfork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let child = if runtime.thread_group(parent_pid) == parent_pid
        && !runtime.has_live_threads(parent_pid)
    {
        let mut translated = context;
        translated.args = [LINUX_CLONE_VM | super::clone3::CLONE_VFORK | LINUX_SIGCHLD, 0, 0, 0, 0, 0];
        super::threads::clone_vm(runtime, parent_pid, translated)
    } else {
        // Preserve the existing private snapshot behavior for thread groups
        // until exec can unshare/retire their other tasks as well.
        sys_fork(runtime, parent_pid, context)
    };
    match child {
        Ok(child_pid) => {
            if runtime.park_signal_waiter(parent_pid, child_pid, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}
