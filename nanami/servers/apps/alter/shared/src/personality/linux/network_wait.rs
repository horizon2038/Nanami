use super::{
    personality, record_syscall_result, sys_accept, sys_connect, sys_read, sys_recvfrom,
    sys_recvmsg, EmulationAction, LinuxSyscallContext, Runtime, Word, EAGAIN, EINPROGRESS, EINVAL,
    ESRCH, LINUX_SOCK_NONBLOCK, SYS_ACCEPT, SYS_ACCEPT4, SYS_CONNECT, SYS_READ, SYS_RECVFROM,
    SYS_RECVMSG,
};

pub(super) fn network_result_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    result: Result<Word, i32>,
    nonblocking: bool,
) -> EmulationAction {
    match result {
        Ok(value) => EmulationAction::Return(value as isize),
        Err(errno) if !nonblocking && is_network_pending_errno(errno) => {
            if runtime.park_network_waiter(pid, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

pub(super) fn is_network_pending_errno(errno: i32) -> bool {
    errno == EAGAIN || errno == EINPROGRESS
}

pub(super) fn socket_nonblocking(runtime: &Runtime, pid: Word, fd: Word, flags: Word) -> bool {
    runtime
        .linux_file(pid, fd)
        .map(|file| (file.flags | flags) & LINUX_SOCK_NONBLOCK != 0)
        .unwrap_or(false)
}

pub(super) fn sys_connect_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let nonblocking = socket_nonblocking(runtime, pid, context.args[0], 0);
    let result = sys_connect(
        runtime,
        pid,
        context.args[0],
        context.args[1],
        context.args[2],
    );
    network_result_action(runtime, pid, context, result, nonblocking)
}

pub(super) fn sys_accept_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let flags = if context.number == SYS_ACCEPT4 {
        context.args[3]
    } else {
        0
    };
    let nonblocking = socket_nonblocking(runtime, pid, context.args[0], flags);
    let result = sys_accept(
        runtime,
        pid,
        context.args[0],
        context.args[1],
        context.args[2],
        flags,
    );
    network_result_action(runtime, pid, context, result, nonblocking)
}

pub(super) fn sys_recvfrom_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let nonblocking = socket_nonblocking(runtime, pid, context.args[0], 0);
    let result = sys_recvfrom(
        runtime,
        pid,
        context.args[0],
        context.args[1],
        context.args[2],
        context.args[4],
        context.args[5],
    );
    network_result_action(runtime, pid, context, result, nonblocking)
}

pub(super) fn sys_recvmsg_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let nonblocking = socket_nonblocking(runtime, pid, context.args[0], context.args[2]);
    let result = sys_recvmsg(
        runtime,
        pid,
        context.args[0],
        context.args[1],
        context.args[2],
    );
    network_result_action(runtime, pid, context, result, nonblocking)
}

pub fn wake_network_waiters(runtime: &mut Runtime) {
    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.network_waiting {
            index += 1;
            continue;
        }

        let result = retry_network_syscall(runtime, process.pid, process.network_wait_context);
        let return_value = match result {
            Err(errno) if is_network_pending_errno(errno) => {
                index += 1;
                continue;
            }
            Ok(value) => value as isize,
            Err(errno) => -(errno as isize),
        };

        runtime.managed[index].network_waiting = false;
        runtime.managed[index].network_wait_context = LinuxSyscallContext::EMPTY;
        record_syscall_result(
            runtime,
            process.pid,
            process.network_wait_context.number,
            return_value,
        );

        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.network_wait_context,
            return_value,
            process.personality,
        )
        .is_err()
        {
            libnanami::println!(
                "[alter/{}] network wake register write failed pid={} pcb={:#x}",
                personality::name(process.personality),
                process.pid,
                process.pcb
            );
            index += 1;
            continue;
        }
        if let Err(error) = a9n_abi::arch::process_control_block::resume(process.pcb) {
            libnanami::println!(
                "[alter/linux] network wake resume failed pid={} pcb={:#x} err={:?}",
                process.pid,
                process.pcb,
                error
            );
        }
        index += 1;
    }
}

pub(super) fn retry_network_syscall(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    match context.number {
        SYS_READ => sys_read(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_CONNECT => sys_connect(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_ACCEPT => sys_accept(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
            0,
        ),
        SYS_ACCEPT4 => sys_accept(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_RECVFROM => sys_recvfrom(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[4],
            context.args[5],
        ),
        SYS_RECVMSG => sys_recvmsg(
            runtime,
            pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        _ => Err(EINVAL),
    }
}
