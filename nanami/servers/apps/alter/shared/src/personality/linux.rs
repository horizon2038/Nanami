use libnanami::{RequestError, Word};
use nanami_services::{gfx::honoka, input, net, posix, vfs};

use crate::abi::{
    ALTER_DEFAULT_SHM_BYTES, ALTER_IO_OFFSET, ALTER_LAUNCH_MAX_ARGS, ALTER_LAUNCH_MAX_ENVS,
    SLOT_HONOKA_PRESENT_NOTIFICATION_BASE, SLOT_HONOKA_SERVICE, SLOT_INPUT_SERVICE,
    SLOT_NETWORK_SERVICE,
};
use crate::common::virtual_fs::{self, VirtualNode};
use crate::elf::ElfMetadata;
use crate::loader::{load_cached_fork_linux_elf_image, load_linux_elf_image, LoadError};
use crate::personality;
use crate::process::{
    clone_registers_for_fork, read_register_value, write_exec_registers, write_register_value,
    write_syscall_return, LinuxSyscallContext, REG_FS_BASE,
};
use crate::state::{
    LinuxFile, LinuxFileKind, OsPersonality, Runtime, LINUX_CWD_MAX, LINUX_FD_MAX,
    LINUX_PIPE_BYTES, LINUX_TERMINAL_LINE_MAX,
};

#[derive(Clone, Copy)]
pub enum EmulationAction {
    Return(isize),
    Resume,
    Park,
    Exit(Word),
    Unsupported(Word),
}

pub fn dispatch_syscall(
    runtime: &mut Runtime,
    native_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let result = match context.number {
        SYS_READ => {
            let action = sys_read_action(
                runtime,
                native_pid,
                context.args[0],
                context.args[1],
                context.args[2],
                context,
            );
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_WRITE => sys_write(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_READV => sys_readv(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_WRITEV => sys_writev(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_NANOSLEEP => {
            let action = sys_nanosleep_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_GETDENTS64 => sys_getdents64(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_CREAT => sys_open(
            runtime,
            native_pid,
            context.args[0],
            LINUX_O_CREAT | LINUX_O_TRUNC,
        ),
        SYS_OPEN => sys_open(runtime, native_pid, context.args[0], context.args[1]),
        SYS_OPENAT => sys_openat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_CLOSE => sys_close(runtime, native_pid, context.args[0]),
        SYS_DUP => sys_dup(runtime, native_pid, context.args[0]),
        SYS_DUP2 => sys_dup2(runtime, native_pid, context.args[0], context.args[1]),
        SYS_DUP3 => sys_dup3(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_PIPE => sys_pipe(runtime, native_pid, context.args[0], 0),
        SYS_PIPE2 => sys_pipe(runtime, native_pid, context.args[0], context.args[1]),
        SYS_SOCKET => sys_socket(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_CONNECT => {
            let action = sys_connect_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_BIND => sys_bind(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_LISTEN => sys_listen(runtime, native_pid, context.args[0]),
        SYS_ACCEPT | SYS_ACCEPT4 => {
            let action = sys_accept_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_SENDTO => sys_sendto(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[4],
            context.args[5],
        ),
        SYS_SENDMSG => sys_sendmsg(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_RECVMSG => {
            let action = sys_recvmsg_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_RECVFROM => {
            let action = sys_recvfrom_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_SHUTDOWN => sys_shutdown(runtime, native_pid, context.args[0]),
        SYS_GETSOCKNAME => sys_getsockname(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            false,
        ),
        SYS_GETPEERNAME => sys_getsockname(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            true,
        ),
        SYS_SETSOCKOPT => sys_setsockopt(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.args[4],
        ),
        SYS_GETSOCKOPT => sys_getsockopt(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.args[4],
        ),
        SYS_POLL | SYS_PPOLL => sys_poll(runtime, native_pid, context.args[0], context.args[1]),
        SYS_SELECT | SYS_PSELECT6 => sys_select(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_STAT | SYS_LSTAT => sys_stat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_FSTAT => sys_fstat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_CHOWN | SYS_LCHOWN => sys_chown(runtime, native_pid, context.args[0]),
        SYS_FCHOWN => sys_fchown(runtime, native_pid, context.args[0]),
        SYS_FCHOWNAT => sys_fchownat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[4],
        ),
        SYS_NEWFSTATAT => sys_stat(runtime, native_pid, context.args[1], context.args[2]),
        SYS_STATX => sys_statx(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[4],
        ),
        SYS_LSEEK => sys_lseek(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_MMAP => sys_mmap(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.args[4],
            context.args[5],
        ),
        SYS_MPROTECT => sys_mprotect(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_MUNMAP => sys_munmap(runtime, native_pid, context.args[0], context.args[1]),
        SYS_MSYNC => sys_msync(runtime, native_pid, context.args[0], context.args[1]),
        SYS_MADVISE => sys_madvise(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_MREMAP => sys_mremap(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.args[4],
        ),
        SYS_BRK => sys_brk(runtime, native_pid, context.args[0]),
        SYS_IOCTL => sys_ioctl(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_FCNTL => sys_fcntl(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_RT_SIGACTION => sys_rt_sigaction(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_RT_SIGPROCMASK => sys_rt_sigprocmask(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_SIGALTSTACK => sys_sigaltstack(runtime, native_pid, context.args[0], context.args[1]),
        SYS_RT_SIGSUSPEND => {
            let action = sys_rt_sigsuspend(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_GETPID => Ok(native_pid),
        SYS_CLONE | SYS_FORK => sys_fork(runtime, native_pid, context),
        SYS_VFORK => {
            let action = sys_vfork(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_EXECVE => {
            let action = match sys_execve(
                runtime,
                native_pid,
                context.args[0],
                context.args[1],
                context.args[2],
            ) {
                Ok(()) => EmulationAction::Resume,
                Err(errno) => EmulationAction::Return(-(errno as isize)),
            };
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_WAIT4 => {
            let action = sys_wait4(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_GETPPID => Ok(1),
        SYS_GETTIMEOFDAY => sys_gettimeofday(runtime, native_pid, context.args[0]),
        SYS_SETITIMER => sys_setitimer(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_EXIT | SYS_EXIT_GROUP => {
            let action = EmulationAction::Exit(context.args[0]);
            let last = runtime
                .managed_process(native_pid)
                .map(|process| (process.last_syscall, process.last_syscall_return))
                .unwrap_or((0, 0));
            if process_trace_enabled(runtime, native_pid) {
                libnanami::println!(
                    "[alter/linux] exit pid={} syscall={} status={} last_syscall={} last_ret={}",
                    native_pid,
                    context.number,
                    context.args[0],
                    last.0,
                    last.1
                );
            }
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_UNAME => sys_uname(runtime, native_pid, context.args[0]),
        SYS_GETCWD => sys_getcwd(runtime, native_pid, context.args[0], context.args[1]),
        SYS_CHDIR => sys_chdir(runtime, native_pid, context.args[0]),
        SYS_MKDIR => sys_mkdir(runtime, native_pid, context.args[0]),
        SYS_RMDIR => sys_rmdir(runtime, native_pid, context.args[0]),
        SYS_RENAME => sys_rename(runtime, native_pid, context.args[0], context.args[1]),
        SYS_UNLINK => sys_unlink(runtime, native_pid, context.args[0]),
        SYS_READLINK => sys_readlink(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_ACCESS => sys_access(runtime, native_pid, context.args[0]),
        SYS_FACCESSAT2 => sys_faccessat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_MKDIRAT => sys_mkdirat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_MKNOD => sys_mknod(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_MKNODAT => sys_mknodat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_UNLINKAT => sys_unlinkat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_RENAMEAT => sys_renameat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_READLINKAT => sys_readlinkat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_FACCESSAT => sys_faccessat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            0,
        ),
        SYS_GETUID | SYS_GETEUID => map_word(posix::posix_getuid(runtime.posix_port)),
        SYS_GETGID | SYS_GETEGID => map_word(posix::posix_getgid(runtime.posix_port)),
        SYS_GETRESUID => sys_getresid(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            true,
        ),
        SYS_GETRESGID => sys_getresid(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            false,
        ),
        SYS_SETPGID => Ok(0),
        SYS_GETPGID => sys_getpgid(runtime, native_pid, context.args[0]),
        SYS_KILL => sys_kill(runtime, native_pid, context.args[0], context.args[1]),
        SYS_ARCH_PRCTL => sys_arch_prctl(runtime, native_pid, context.args[0], context.args[1]),
        SYS_GETTID => Ok(native_pid),
        SYS_SCHED_GETAFFINITY => sys_sched_getaffinity(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_SET_TID_ADDRESS => Ok(0),
        SYS_CLOCK_GETTIME => {
            sys_clock_gettime(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_UTIMES => sys_utime_path(runtime, native_pid, context.args[0]),
        SYS_FUTIMESAT => sys_utime_path(runtime, native_pid, context.args[1]),
        SYS_UTIMENSAT => sys_utime_path(runtime, native_pid, context.args[1]),
        SYS_SET_ROBUST_LIST => Ok(0),
        SYS_GETRANDOM => sys_getrandom(runtime, native_pid, context.args[0], context.args[1]),
        SYS_GETRLIMIT | SYS_PRLIMIT64 => sys_getrlimit(runtime, native_pid, context),
        SYS_FUTEX => Ok(0),
        SYS_RSEQ => Err(ENOSYS),
        _ => {
            let action = EmulationAction::Unsupported(context.number);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
    };

    let value = result_to_linux_return(result);
    record_syscall_result(runtime, native_pid, context.number, value);
    trace_critical_syscall(runtime, native_pid, context, value);
    let action = EmulationAction::Return(value);
    trace_syscall_action(runtime, native_pid, context, action);
    action
}

fn sys_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::Terminal => return sys_terminal_read(runtime, pid, user_buffer, len),
        LinuxFileKind::PipeRead => {
            return sys_pipe_read(runtime, pid, file.posix_fd, user_buffer, len);
        }
        LinuxFileKind::PipeWrite => return Err(EBADF),
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp => {
            return sys_socket_recv(runtime, pid, fd, user_buffer, len, 0, 0);
        }
        LinuxFileKind::SocketNetlink => return Err(EOPNOTSUPP),
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        LinuxFileKind::VirtualDirectory => return Err(EISDIR),
        LinuxFileKind::VirtualFile => return sys_virtual_read(runtime, pid, fd, user_buffer, len),
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            return sys_evdev_read(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_read(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Posix => {}
        LinuxFileKind::Empty => return Err(EBADF),
    }
    let mut done = 0;
    while done < len {
        let chunk = ::core::cmp::min(len - done, runtime.posix_read_buffer_size().max(1));
        let (bytes, source) = runtime
            .read_posix(file.posix_fd, 0, chunk)
            .map_err(map_request_error)?;
        if bytes > chunk {
            return Err(EIO);
        }
        if bytes == 0 {
            break;
        }
        write_target_memory_from(pid, user_buffer + done, source, bytes)?;
        done += bytes;
    }
    Ok(done)
}

fn sys_read_action(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if user_buffer == 0 && len != 0 {
        return EmulationAction::Return(-(EFAULT as isize));
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let Some(file) = runtime.linux_file(pid, fd) else {
        return EmulationAction::Return(-(EBADF as isize));
    };
    if matches!(
        file.kind,
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp
    ) {
        let nonblocking = (file.flags & LINUX_SOCK_NONBLOCK) != 0;
        let result = sys_read(runtime, pid, fd, user_buffer, len);
        return network_result_action(runtime, pid, context, result, nonblocking);
    }
    if file.kind != LinuxFileKind::Terminal {
        if matches!(
            file.kind,
            LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse
        ) {
            pump_input_events(runtime);
            let result = sys_evdev_read(runtime, pid, fd, user_buffer, len);
            if result != Err(EAGAIN) || (file.flags & LINUX_O_NONBLOCK) != 0 {
                return EmulationAction::Return(result_to_linux_return(result));
            }
            if runtime.park_device_reader(pid, fd, user_buffer, len, context) {
                return EmulationAction::Park;
            }
            return EmulationAction::Return(-(ESRCH as isize));
        }
        return EmulationAction::Return(result_to_linux_return(sys_read(
            runtime,
            pid,
            fd,
            user_buffer,
            len,
        )));
    }

    match sys_terminal_read_now(runtime, pid, user_buffer, len) {
        Ok(Some(bytes)) => EmulationAction::Return(bytes as isize),
        Ok(None) => {
            if runtime.park_terminal_reader(pid, user_buffer, len, context) {
                EmulationAction::Park
            } else {
                EmulationAction::Return(-(ESRCH as isize))
            }
        }
        Err(errno) => EmulationAction::Return(-(errno as isize)),
    }
}

fn pump_input_events(runtime: &mut Runtime) {
    let source = runtime.input_queue;
    let mut graphics_index = 0usize;
    while graphics_index < runtime.graphics.len() {
        let session = runtime.graphics[graphics_index];
        if session.active && session.input_queue != 0 {
            drain_input_queue(runtime, session.input_queue, graphics_index as Word + 1);
        }
        graphics_index += 1;
    }
    if source != 0 {
        drain_input_queue(runtime, source, 0);
    }
}

fn drain_input_queue(runtime: &mut Runtime, source: Word, session_id: Word) {
    let mut queue = input::InputEventQueue::new(source);
    while let Some(packed) = queue.pop() {
        let (kind, code, value0, value1, flags) = input::unpack_input_event(packed);
        match kind {
            input::INPUT_EVENT_KIND_KEY => {
                push_keyboard_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_KEY, linux_key_code(code), value0 as i32),
                );
                push_keyboard_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            input::INPUT_EVENT_KIND_MOUSE_MOVE => {
                let (dx, dy) = normalize_mouse_movement(
                    runtime,
                    session_id,
                    value0 as i32,
                    value1 as i32,
                    flags,
                );
                if dx != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_REL, LINUX_REL_X, dx),
                    );
                }
                if dy != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_REL, LINUX_REL_Y, dy),
                    );
                }
                if dx != 0 || dy != 0 {
                    push_mouse_event(
                        runtime,
                        session_id,
                        pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                    );
                }
            }
            input::INPUT_EVENT_KIND_MOUSE_BUTTON => {
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_KEY, linux_mouse_button(code), value0 as i32),
                );
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            input::INPUT_EVENT_KIND_MOUSE_WHEEL => {
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_REL, LINUX_REL_WHEEL, value0 as i32),
                );
                push_mouse_event(
                    runtime,
                    session_id,
                    pack_linux_input_event(LINUX_EV_SYN, LINUX_SYN_REPORT, 0),
                );
            }
            _ => {}
        }
    }
}

fn normalize_mouse_movement(
    runtime: &mut Runtime,
    session_id: Word,
    x: i32,
    y: i32,
    flags: Word,
) -> (i32, i32) {
    if session_id == 0 || (flags & honoka::HONOKA_INPUT_FLAG_ABSOLUTE) == 0 {
        return (x, y);
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return (0, 0);
    };
    let movement = if session.mouse_position_valid {
        (
            x.saturating_sub(session.mouse_x),
            y.saturating_sub(session.mouse_y),
        )
    } else {
        (0, 0)
    };
    session.mouse_x = x;
    session.mouse_y = y;
    session.mouse_position_valid = true;
    movement
}

fn push_keyboard_event(runtime: &mut Runtime, session_id: Word, event: Word) {
    if session_id == 0 {
        runtime.push_keyboard_event(event);
        return;
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return;
    };
    push_event_ring(
        &mut session.keyboard_events,
        &mut session.keyboard_head,
        &mut session.keyboard_tail,
        event,
    );
}

fn push_mouse_event(runtime: &mut Runtime, session_id: Word, event: Word) {
    if session_id == 0 {
        runtime.push_mouse_event(event);
        return;
    }
    let Some(session) = runtime.graphics.get_mut(session_id as usize - 1) else {
        return;
    };
    push_event_ring(
        &mut session.mouse_events,
        &mut session.mouse_head,
        &mut session.mouse_tail,
        event,
    );
}

fn pop_keyboard_event(runtime: &mut Runtime, session_id: Word) -> Option<Word> {
    if session_id == 0 {
        return runtime.pop_keyboard_event();
    }
    let session = runtime.graphics.get_mut(session_id as usize - 1)?;
    pop_event_ring(
        &session.keyboard_events,
        &mut session.keyboard_head,
        session.keyboard_tail,
    )
}

fn pop_mouse_event(runtime: &mut Runtime, session_id: Word) -> Option<Word> {
    if session_id == 0 {
        return runtime.pop_mouse_event();
    }
    let session = runtime.graphics.get_mut(session_id as usize - 1)?;
    pop_event_ring(
        &session.mouse_events,
        &mut session.mouse_head,
        session.mouse_tail,
    )
}

fn keyboard_event_ready(runtime: &Runtime, session_id: Word) -> bool {
    if session_id == 0 {
        return runtime.keyboard_event_ready();
    }
    runtime
        .graphics
        .get(session_id as usize - 1)
        .map(|session| session.keyboard_head != session.keyboard_tail)
        .unwrap_or(false)
}

fn mouse_event_ready(runtime: &Runtime, session_id: Word) -> bool {
    if session_id == 0 {
        return runtime.mouse_event_ready();
    }
    runtime
        .graphics
        .get(session_id as usize - 1)
        .map(|session| session.mouse_head != session.mouse_tail)
        .unwrap_or(false)
}

fn push_event_ring(
    events: &mut [Word; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
    head: &mut usize,
    tail: &mut usize,
    event: Word,
) {
    let next = (*tail + 1) % events.len();
    if next == *head {
        *head = (*head + 1) % events.len();
    }
    events[*tail] = event;
    *tail = next;
}

fn pop_event_ring(
    events: &[Word; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
    head: &mut usize,
    tail: usize,
) -> Option<Word> {
    if *head == tail {
        return None;
    }
    let event = events[*head];
    *head = (*head + 1) % events.len();
    Some(event)
}

fn sys_evdev_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if len < LINUX_INPUT_EVENT_BYTES {
        return Err(EINVAL);
    }
    pump_input_events(runtime);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let session_id = file.resource >> 32;
    let mut written = 0usize;
    while written + LINUX_INPUT_EVENT_BYTES as usize <= len as usize {
        let packed = match file.kind {
            LinuxFileKind::EvdevKeyboard => pop_keyboard_event(runtime, session_id),
            LinuxFileKind::EvdevMouse => pop_mouse_event(runtime, session_id),
            _ => return Err(ENODEV),
        };
        let Some(packed) = packed else { break };
        let (event_type, code, value) = unpack_linux_input_event(packed);
        write_input_event(runtime.posix_shm + written as Word, event_type, code, value);
        written += LINUX_INPUT_EVENT_BYTES as usize;
    }
    if written == 0 {
        return Err(EAGAIN);
    }
    write_target_memory(runtime, pid, user_buffer, written as Word)?;
    Ok(written as Word)
}

fn pack_linux_input_event(event_type: u16, code: u16, value: i32) -> Word {
    event_type as Word | ((code as Word) << 16) | (((value as u32) as Word) << 32)
}

fn unpack_linux_input_event(event: Word) -> (u16, u16, i32) {
    (
        event as u16,
        (event >> 16) as u16,
        (event >> 32) as u32 as i32,
    )
}

fn write_input_event(base: Word, event_type: u16, code: u16, value: i32) {
    unsafe {
        write_u64(base, 0);
        write_u64(base + 8, 0);
        write_u16(base + 16, event_type);
        write_u16(base + 18, code);
        write_u32(base + 20, value as u32);
    }
}

fn linux_key_code(code: Word) -> u16 {
    match code {
        0x11c => 96,
        0x11d => 97,
        0x135 => 98,
        0x138 => 100,
        0x147 => 102,
        0x148 => 103,
        0x149 => 104,
        0x14b => 105,
        0x14d => 106,
        0x14f => 107,
        0x150 => 108,
        0x151 => 109,
        0x152 => 110,
        0x153 => 111,
        _ => (code & 0x7f) as u16,
    }
}

fn linux_mouse_button(code: Word) -> u16 {
    match code {
        1 => 0x110,
        2 => 0x111,
        3 => 0x112,
        _ => 0x110,
    }
}

fn sys_framebuffer_read(
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

fn sys_framebuffer_write(
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

fn sys_framebuffer_mmap(
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

fn ensure_framebuffer_mapping(
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
    ensure_clock_timer(runtime)?;
    let (shared, bytes) = honoka::honoka_attach_logical_framebuffer_to_process(
        session.honoka_port,
        session.window_id,
        pid,
    )
    .map_err(map_request_error)?;
    if bytes == 0 {
        return Err(EIO);
    }
    let framebuffer = shared;
    let framebuffer_bytes = bytes;
    let mapped = align_up_word(framebuffer_bytes, LINUX_PAGE_SIZE);
    if !runtime.add_mapping(pid, framebuffer, mapped, prot) {
        return Err(ENOMEM);
    }
    let index = id.checked_sub(1).ok_or(ENODEV)? as usize;
    runtime.graphics[index].damage_queue = 0;
    runtime.graphics[index].framebuffer = framebuffer;
    runtime.graphics[index].framebuffer_bytes = framebuffer_bytes;
    runtime.graphics[index].guest_pid = pid;
    runtime.graphics[index].guest_framebuffer = framebuffer;
    runtime.graphics[index].guest_framebuffer_bytes = framebuffer_bytes;
    Ok(framebuffer)
}

fn graphics_session(runtime: &Runtime, id: Word) -> Result<crate::state::GraphicsSession, i32> {
    let index = id.checked_sub(1).ok_or(ENODEV)? as usize;
    let session = *runtime.graphics.get(index).ok_or(ENODEV)?;
    if !session.active {
        return Err(ENODEV);
    }
    Ok(session)
}

fn present_graphics_session(runtime: &mut Runtime, id: Word, pid: Word) -> Result<(), i32> {
    let session = graphics_session(runtime, id)?;
    if session.guest_pid != 0 && session.guest_pid != pid {
        return Err(EACCES);
    }
    notify_graphics_session(session)
}

fn notify_graphics_session(session: crate::state::GraphicsSession) -> Result<(), i32> {
    libnanami::ipc::notification_notify(session.present_notification).map_err(map_request_error)
}

fn present_mapped_framebuffers(runtime: &Runtime) {
    for session in runtime.graphics {
        if session.active && session.guest_pid != 0 && session.guest_framebuffer != 0 {
            let _ = notify_graphics_session(session);
        }
    }
}

fn network_result_action(
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

fn is_network_pending_errno(errno: i32) -> bool {
    errno == EAGAIN || errno == EINPROGRESS
}

fn socket_nonblocking(runtime: &Runtime, pid: Word, fd: Word, flags: Word) -> bool {
    runtime
        .linux_file(pid, fd)
        .map(|file| (file.flags | flags) & LINUX_SOCK_NONBLOCK != 0)
        .unwrap_or(false)
}

fn sys_connect_action(
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

fn sys_accept_action(
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

fn sys_recvfrom_action(
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

fn sys_recvmsg_action(
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

fn sys_write(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    ensure_standard_terminal_fd(runtime, pid, fd);
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::Terminal => return sys_terminal_write(runtime, pid, user_buffer, len),
        LinuxFileKind::PipeWrite => {
            return sys_pipe_write(runtime, pid, file.posix_fd, user_buffer, len);
        }
        LinuxFileKind::PipeRead => return Err(EBADF),
        LinuxFileKind::SocketUdp | LinuxFileKind::SocketTcp | LinuxFileKind::SocketIcmp => {
            return sys_socket_send(runtime, pid, fd, user_buffer, len, 0, 0);
        }
        LinuxFileKind::SocketNetlink => {
            return sys_netlink_send(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        LinuxFileKind::VirtualDirectory => return Err(EISDIR),
        LinuxFileKind::VirtualFile => return sys_virtual_write(runtime, pid, fd, len),
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => return Err(EBADF),
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_write(runtime, pid, fd, user_buffer, len);
        }
        LinuxFileKind::Posix => {}
        LinuxFileKind::Empty => return Err(EBADF),
    }
    let mut done = 0;
    while done < len {
        let chunk = bounded_len(runtime, len - done)?;
        read_target_memory(runtime, pid, user_buffer + done, chunk)?;
        let written = posix::posix_write(runtime.posix_port, file.posix_fd, 0, chunk)
            .map_err(map_request_error)?;
        done += written;
        if written == 0 || written < chunk {
            break;
        }
    }
    Ok(done)
}

fn ensure_network(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime.network_port != 0 && runtime.network_shm != 0 {
        return Ok(());
    }
    nanami_services::registry::connect_network_service(SLOT_NETWORK_SERVICE)
        .map_err(|_| ENETDOWN)?;
    let port = libnanami::ipc::process_slot_descriptor(SLOT_NETWORK_SERVICE);
    let self_pid = libnanami::get_self_pid().map_err(|_| ENETDOWN)?;
    let (status, peer, size) = net::net_service_control_ex(
        port,
        net::NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY,
        self_pid,
        ALTER_DEFAULT_SHM_BYTES,
    )
    .map_err(map_network_error)?;
    if status != libnanami::OS_RESPONSE_OK || peer == 0 || size == 0 {
        return Err(ENETDOWN);
    }
    net::net_service_attach_rx_notification(port, libnanami::PROCESS_SLOT_NOTIFICATION)
        .map_err(map_network_error)?;
    runtime.network_port = port;
    runtime.network_shm = peer;
    runtime.network_shm_size = size;
    Ok(())
}

fn sys_socket(
    runtime: &mut Runtime,
    pid: Word,
    domain: Word,
    socket_type: Word,
    protocol: Word,
) -> Result<Word, i32> {
    let base_type = socket_type & LINUX_SOCK_TYPE_MASK;
    let flags = (if (socket_type & LINUX_SOCK_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    }) | (socket_type & LINUX_SOCK_NONBLOCK);
    let file = match domain {
        LINUX_AF_INET => {
            ensure_network(runtime)?;
            match base_type {
                LINUX_SOCK_DGRAM if protocol == 0 || protocol == LINUX_IPPROTO_UDP => {
                    LinuxFile::socket_udp(flags)
                }
                LINUX_SOCK_STREAM if protocol == 0 || protocol == LINUX_IPPROTO_TCP => {
                    LinuxFile::socket_tcp(flags)
                }
                LINUX_SOCK_RAW if protocol == LINUX_IPPROTO_ICMP => LinuxFile::socket_icmp(flags),
                LINUX_SOCK_DGRAM | LINUX_SOCK_STREAM | LINUX_SOCK_RAW => {
                    return Err(EPROTONOSUPPORT);
                }
                _ => return Err(ESOCKTNOSUPPORT),
            }
        }
        LINUX_AF_NETLINK => {
            if base_type != LINUX_SOCK_RAW {
                return Err(ESOCKTNOSUPPORT);
            }
            if protocol != LINUX_NETLINK_ROUTE {
                return Err(EPROTONOSUPPORT);
            }
            ensure_network(runtime)?;
            LinuxFile::socket_netlink(flags)
        }
        _ => return Err(EAFNOSUPPORT),
    };
    runtime.allocate_linux_file(pid, file, 0).ok_or(EMFILE)
}

fn sys_connect(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::SocketNetlink {
        read_sockaddr_nl(runtime, pid, address, address_len)?;
        file.local_port = 1;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(0);
    }
    if file.kind != LinuxFileKind::SocketUdp && file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    let (ip, port) = read_sockaddr_in(runtime, pid, address, address_len)?;
    file.peer_ip = ip;
    file.peer_port = port;
    if file.kind == LinuxFileKind::SocketTcp {
        if file.local_port == 0 {
            file.local_port = allocate_ephemeral_port(runtime)?;
        }
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        match net::net_service_tcp_connect(
            runtime.network_port,
            file.local_port,
            file.peer_ip,
            file.peer_port,
        ) {
            Ok(connection_id) if connection_id != 0 => {
                file.posix_fd = connection_id;
                if !runtime.set_linux_file(pid, fd, file) {
                    return Err(EBADF);
                }
                return Ok(0);
            }
            Ok(_) | Err(RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)) => {
                return Err(EINPROGRESS);
            }
            Err(error) => return Err(map_network_error(error)),
        }
    }
    file = ensure_udp_bound(runtime, pid, fd, file)?;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

fn sys_bind(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::SocketNetlink {
        if file.local_port != 0 {
            return Err(EINVAL);
        }
        read_sockaddr_nl(runtime, pid, address, address_len)?;
        file.local_port = 1;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(0);
    }
    if file.kind != LinuxFileKind::SocketUdp && file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    if file.local_port != 0 {
        return Err(EINVAL);
    }
    let (ip, mut port) = read_sockaddr_in(runtime, pid, address, address_len)?;
    if ip != 0 && ip != network_ipv4(runtime)? {
        return Err(EADDRNOTAVAIL);
    }
    if port == 0 {
        port = allocate_ephemeral_port(runtime)?;
    } else if socket_port_in_use(runtime, file.kind, port) {
        return Err(EADDRINUSE);
    }
    if file.kind == LinuxFileKind::SocketUdp {
        bind_network_port(runtime, net::NET_SERVICE_CONTROL_UDP_BIND, port)?;
    }
    file.local_port = port;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

fn sys_listen(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    if file.posix_fd != 0 {
        return Err(EINVAL);
    }
    if file.local_port == 0 {
        file.local_port = allocate_ephemeral_port(runtime)?;
    }
    bind_network_port(runtime, net::NET_SERVICE_CONTROL_TCP_BIND, file.local_port)?;
    file.kind = LinuxFileKind::SocketTcpListener;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

fn sys_accept(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
    flags: Word,
) -> Result<Word, i32> {
    let listener = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if listener.kind != LinuxFileKind::SocketTcpListener {
        return Err(ENOTSOCK);
    }
    if flags & !(LINUX_SOCK_NONBLOCK | LINUX_SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let connection_id = net::net_service_tcp_accept(runtime.network_port, listener.local_port, 0)
        .map_err(map_network_error)?;
    if connection_id == 0 {
        return Err(EAGAIN);
    }
    let peer_ip = read_network_u32_be(runtime, 0);
    let peer_port = read_network_u16_be(runtime, 4);
    let mut accepted = LinuxFile::socket_tcp(
        (flags & LINUX_SOCK_NONBLOCK)
            | if (flags & LINUX_SOCK_CLOEXEC) != 0 {
                LINUX_FD_CLOEXEC
            } else {
                0
            },
    );
    accepted.posix_fd = connection_id;
    accepted.local_port = listener.local_port;
    accepted.peer_ip = peer_ip;
    accepted.peer_port = peer_port;
    let new_fd = runtime
        .allocate_linux_file(pid, accepted, 0)
        .ok_or(EMFILE)?;
    if let Err(errno) =
        write_sockaddr_result(runtime, pid, address, address_len, peer_ip, peer_port)
    {
        let _ = runtime.clear_linux_file(pid, new_fd);
        return Err(errno);
    }
    Ok(new_fd)
}

fn sys_sendto(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    sys_socket_send(runtime, pid, fd, buffer, len, address, address_len)
}

fn sys_socket_send(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::SocketUdp => {
            if len > UDP_SOCKET_PAYLOAD_MAX {
                return Err(EMSGSIZE);
            }
            let (peer_ip, peer_port) = if address != 0 {
                read_sockaddr_in(runtime, pid, address, address_len)?
            } else if file.peer_ip != 0 && file.peer_port != 0 {
                (file.peer_ip, file.peer_port)
            } else {
                return Err(EDESTADDRREQ);
            };
            file = ensure_udp_bound(runtime, pid, fd, file)?;
            read_target_memory_to_network(runtime, pid, buffer, len)?;
            let mut attempts = 0usize;
            loop {
                match net::net_service_udp_send(
                    runtime.network_port,
                    0,
                    len,
                    file.local_port,
                    peer_port,
                    peer_ip,
                ) {
                    Ok(_) => return Ok(len),
                    Err(error) if attempts < NETWORK_SEND_RETRIES => {
                        attempts += 1;
                        let _ = net::net_service_control(
                            runtime.network_port,
                            net::NET_SERVICE_CONTROL_POLL,
                            0,
                            0,
                        );
                        if !matches!(
                            error,
                            RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)
                        ) {
                            return Err(map_network_error(error));
                        }
                    }
                    Err(error) => return Err(map_network_error(error)),
                }
            }
        }
        LinuxFileKind::SocketTcp => {
            if file.posix_fd == 0 {
                return Err(ENOTCONN);
            }
            let mut done = 0;
            while done < len {
                let chunk = ::core::cmp::min(
                    len - done,
                    runtime.network_shm_size.min(TCP_SOCKET_PAYLOAD_MAX),
                );
                read_target_memory_to_network(runtime, pid, buffer + done, chunk)?;
                let sent = net::net_service_tcp_send_on_connection(
                    runtime.network_port,
                    file.posix_fd,
                    0,
                    chunk,
                    TCP_FLAG_ACK | TCP_FLAG_PSH,
                )
                .map_err(map_network_error)?;
                done += sent;
                if sent < chunk {
                    break;
                }
            }
            Ok(done)
        }
        LinuxFileKind::SocketIcmp => {
            if len < LINUX_ICMP_HEADER_LEN || len > ICMP_SOCKET_PAYLOAD_MAX {
                return Err(EMSGSIZE);
            }
            let (peer_ip, _) = if address != 0 {
                read_sockaddr_in(runtime, pid, address, address_len)?
            } else if file.peer_ip != 0 {
                (file.peer_ip, 0)
            } else {
                return Err(EDESTADDRREQ);
            };
            read_target_memory_to_network(runtime, pid, buffer, len)?;
            let identifier = read_network_u16_be(runtime, 4);
            file.local_port = identifier;
            file.peer_ip = peer_ip;
            if !runtime.set_linux_file(pid, fd, file) {
                return Err(EBADF);
            }
            let mut attempts = 0usize;
            loop {
                match net::net_service_icmp_send(runtime.network_port, 0, len, identifier, peer_ip)
                {
                    Ok(_) => return Ok(len),
                    Err(error) if attempts < NETWORK_SEND_RETRIES => {
                        attempts += 1;
                        let _ = net::net_service_control(
                            runtime.network_port,
                            net::NET_SERVICE_CONTROL_POLL,
                            0,
                            0,
                        );
                        if !matches!(
                            error,
                            RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)
                        ) {
                            return Err(map_network_error(error));
                        }
                    }
                    Err(error) => return Err(map_network_error(error)),
                }
            }
        }
        LinuxFileKind::SocketNetlink => sys_netlink_send(runtime, pid, fd, buffer, len),
        LinuxFileKind::SocketTcpListener => Err(ENOTCONN),
        _ => Err(ENOTSOCK),
    }
}

fn sys_recvfrom(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    sys_socket_recv(runtime, pid, fd, buffer, len, address, address_len)
}

fn sys_socket_recv(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    if len == 0 {
        return Ok(0);
    }
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let (received, source_offset, peer_ip, peer_port) = match file.kind {
        LinuxFileKind::SocketUdp => {
            file = ensure_udp_bound(runtime, pid, fd, file)?;
            let max_len = len.min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let received = net::net_service_udp_recv_on_port(
                runtime.network_port,
                file.local_port,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                return Err(EAGAIN);
            }
            (
                received,
                NETWORK_PAYLOAD_OFFSET,
                read_network_u32_be(runtime, 0),
                read_network_u16_be(runtime, 4),
            )
        }
        LinuxFileKind::SocketTcp => {
            if file.posix_fd == 0 {
                return Err(ENOTCONN);
            }
            let max_len = len.min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let (received, event_connection_id) = net::net_service_tcp_recv_on_connection(
                runtime.network_port,
                file.posix_fd,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                if event_connection_id == file.posix_fd {
                    return Ok(0);
                }
                return Err(EAGAIN);
            }
            (
                received,
                NETWORK_PAYLOAD_OFFSET,
                file.peer_ip,
                file.peer_port,
            )
        }
        LinuxFileKind::SocketIcmp => {
            if len <= LINUX_IPV4_HEADER_LEN {
                return Err(EMSGSIZE);
            }
            let max_len = (len - LINUX_IPV4_HEADER_LEN).min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let received = net::net_service_icmp_recv(
                runtime.network_port,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
                file.local_port,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                return Err(EAGAIN);
            }
            let peer_ip = read_network_u32_be(runtime, 0);
            let ttl = unsafe { ::core::ptr::read((runtime.network_shm + 4) as *const u8) };
            unsafe {
                ::core::ptr::copy(
                    (runtime.network_shm + NETWORK_PAYLOAD_OFFSET) as *const u8,
                    (runtime.network_shm + LINUX_IPV4_HEADER_LEN) as *mut u8,
                    received as usize,
                );
                write_linux_ipv4_header(
                    runtime.network_shm,
                    received + LINUX_IPV4_HEADER_LEN,
                    peer_ip,
                    network_ipv4(runtime)?,
                    ttl,
                    LINUX_IPPROTO_ICMP as u8,
                );
            }
            (received + LINUX_IPV4_HEADER_LEN, 0, peer_ip, 0)
        }
        LinuxFileKind::SocketNetlink => return Err(EOPNOTSUPP),
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        _ => return Err(ENOTSOCK),
    };
    if received != 0 {
        libnanami::request_process_memory_write(
            pid,
            buffer,
            runtime.network_shm + source_offset,
            received,
        )
        .map_err(map_request_error)?;
    }
    write_sockaddr_result(runtime, pid, address, address_len, peer_ip, peer_port)?;
    Ok(received)
}

fn sys_sendmsg(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    message: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(EOPNOTSUPP);
    }
    let (_, _, iov, iov_count) = read_linux_msghdr(runtime, pid, message)?;
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov, iov_count, &mut bases, &mut lens)?;

    let mut total: Word = 0;
    let mut index = 0usize;
    while index < iov_count as usize {
        let len = lens[index];
        if total
            .checked_add(len)
            .filter(|end| *end <= runtime.posix_shm_size)
            .is_none()
        {
            return Err(EMSGSIZE);
        }
        if len != 0 {
            libnanami::request_process_memory_read(
                pid,
                bases[index],
                runtime.posix_shm + total,
                len,
            )
            .map_err(map_request_error)?;
        }
        total += len;
        index += 1;
    }
    process_netlink_request(runtime, pid, fd, total)
}

fn sys_recvmsg(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    message: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(EOPNOTSUPP);
    }
    if file.peer_port == 0 {
        return Err(EAGAIN);
    }

    let (name, name_len, iov, iov_count) = read_linux_msghdr(runtime, pid, message)?;
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov, iov_count, &mut bases, &mut lens)?;
    let response_len = build_netlink_dump(runtime, pid, file.posix_fd as u16, file.peer_ip)?;

    let mut copied = 0;
    let mut index = 0usize;
    while index < iov_count as usize && copied < response_len {
        let chunk = lens[index].min(response_len - copied);
        if chunk != 0 {
            libnanami::request_process_memory_write(
                pid,
                bases[index],
                runtime.posix_shm + copied,
                chunk,
            )
            .map_err(map_request_error)?;
            copied += chunk;
        }
        index += 1;
    }
    if copied < response_len {
        return Err(EMSGSIZE);
    }

    if name != 0 && name_len >= LINUX_SOCKADDR_NL_LEN {
        write_sockaddr_nl_value(runtime.posix_shm, 0);
        write_target_memory(runtime, pid, name, LINUX_SOCKADDR_NL_LEN)?;
    }
    write_u32_to_target(runtime, pid, message + 8, LINUX_SOCKADDR_NL_LEN as u32)?;
    write_u32_to_target(runtime, pid, message + 48, 0)?;

    let mut updated = file;
    updated.peer_port = 0;
    if !runtime.set_linux_file(pid, fd, updated) {
        return Err(EBADF);
    }
    Ok(response_len)
}

fn sys_netlink_send(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if len > runtime.posix_shm_size {
        return Err(EMSGSIZE);
    }
    read_target_memory(runtime, pid, buffer, len)?;
    process_netlink_request(runtime, pid, fd, len)
}

fn process_netlink_request(
    runtime: &mut Runtime,
    _pid: Word,
    fd: Word,
    len: Word,
) -> Result<Word, i32> {
    if len < LINUX_NLMSG_HEADER_LEN {
        return Err(EINVAL);
    }
    let declared_len = read_shm_u32(runtime, 0) as Word;
    if declared_len < LINUX_NLMSG_HEADER_LEN || declared_len > len {
        return Err(EINVAL);
    }
    let message_type = read_shm_u16(runtime, 4);
    if message_type != LINUX_RTM_GETLINK && message_type != LINUX_RTM_GETADDR {
        return Err(EOPNOTSUPP);
    }
    let mut file = runtime.linux_file(_pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(ENOTSOCK);
    }
    file.posix_fd = message_type as Word;
    file.peer_ip = read_shm_u32(runtime, 8);
    file.peer_port = 1;
    if !runtime.set_linux_file(_pid, fd, file) {
        return Err(EBADF);
    }
    Ok(len)
}

fn read_linux_msghdr(
    runtime: &mut Runtime,
    pid: Word,
    message: Word,
) -> Result<(Word, Word, Word, Word), i32> {
    if message == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, message, LINUX_MSGHDR_LEN)?;
    let name = read_shm_u64(runtime, 0);
    let name_len = read_shm_u32(runtime, 8) as Word;
    let iov = read_shm_u64(runtime, 16);
    let iov_count = read_shm_u64(runtime, 24);
    if iov_count == 0 || iov_count > LINUX_IOV_MAX || iov == 0 {
        return Err(EINVAL);
    }
    Ok((name, name_len, iov, iov_count))
}

fn read_linux_iovecs(
    runtime: &mut Runtime,
    pid: Word,
    iov: Word,
    iov_count: Word,
    bases: &mut [Word; LINUX_IOV_MAX as usize],
    lens: &mut [Word; LINUX_IOV_MAX as usize],
) -> Result<(), i32> {
    if iov_count > LINUX_IOV_MAX {
        return Err(EINVAL);
    }
    if iov_count != 0 && iov == 0 {
        return Err(EFAULT);
    }
    let bytes = iov_count.checked_mul(LINUX_IOVEC_LEN).ok_or(EINVAL)?;
    read_target_memory(runtime, pid, iov, bytes)?;
    let mut index = 0usize;
    while index < iov_count as usize {
        bases[index] = read_shm_u64(runtime, index * LINUX_IOVEC_LEN as usize);
        lens[index] = read_shm_u64(runtime, index * LINUX_IOVEC_LEN as usize + 8);
        if bases[index] == 0 && lens[index] != 0 {
            return Err(EFAULT);
        }
        index += 1;
    }
    Ok(())
}

fn sys_shutdown(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketTcp || file.posix_fd == 0 {
        return Err(ENOTCONN);
    }
    net::net_service_tcp_send_on_connection(
        runtime.network_port,
        file.posix_fd,
        0,
        0,
        TCP_FLAG_FIN | TCP_FLAG_ACK,
    )
    .map_err(map_network_error)?;
    Ok(0)
}

fn sys_getsockname(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
    peer: bool,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if file.kind == LinuxFileKind::SocketNetlink {
        if peer {
            return Err(ENOTCONN);
        }
        return write_sockaddr_nl(runtime, pid, address, address_len);
    }
    let (ip, port) = if peer {
        if file.peer_ip == 0 || file.peer_port == 0 {
            return Err(ENOTCONN);
        }
        (file.peer_ip, file.peer_port)
    } else {
        (network_ipv4(runtime)?, file.local_port)
    };
    write_sockaddr_result(runtime, pid, address, address_len, ip, port)?;
    Ok(0)
}

fn sys_setsockopt(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    _level: Word,
    _option: Word,
    value: Word,
    value_len: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if value == 0 && value_len != 0 {
        return Err(EFAULT);
    }
    Ok(0)
}

fn sys_getsockopt(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    level: Word,
    option: Word,
    value: Word,
    value_len: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if value == 0 || value_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, value_len, 4)?;
    let requested = read_shm_u32(runtime, 0) as Word;
    let result = if level == LINUX_SOL_SOCKET && option == LINUX_SO_TYPE {
        match file.kind {
            LinuxFileKind::SocketUdp => LINUX_SOCK_DGRAM,
            LinuxFileKind::SocketIcmp | LinuxFileKind::SocketNetlink => LINUX_SOCK_RAW,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener => LINUX_SOCK_STREAM,
            _ => return Err(ENOTSOCK),
        }
    } else if level == LINUX_SOL_SOCKET && option == LINUX_SO_ACCEPTCONN {
        (file.kind == LinuxFileKind::SocketTcpListener) as Word
    } else {
        0
    };
    let copy_len = requested.min(4);
    unsafe {
        write_u32(runtime.posix_shm, result as u32);
    }
    write_target_memory(runtime, pid, value, copy_len)?;
    unsafe {
        write_u32(runtime.posix_shm, 4);
    }
    write_target_memory(runtime, pid, value_len, 4)?;
    Ok(0)
}

fn ensure_udp_bound(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    mut file: LinuxFile,
) -> Result<LinuxFile, i32> {
    if file.local_port != 0 {
        return Ok(file);
    }
    file.local_port = allocate_ephemeral_port(runtime)?;
    bind_network_port(runtime, net::NET_SERVICE_CONTROL_UDP_BIND, file.local_port)?;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(file)
}

fn bind_network_port(runtime: &Runtime, control: Word, port: u16) -> Result<(), i32> {
    net::net_service_control(runtime.network_port, control, port as Word, 0)
        .map_err(map_network_bind_error)
}

fn allocate_ephemeral_port(runtime: &mut Runtime) -> Result<u16, i32> {
    let mut attempts = 0usize;
    while attempts < 16384 {
        let port = runtime.next_ephemeral_port.max(49152);
        runtime.next_ephemeral_port = if port == 65535 { 49152 } else { port + 1 };
        if !socket_any_port_in_use(runtime, port) {
            return Ok(port);
        }
        attempts += 1;
    }
    Err(EADDRINUSE)
}

fn socket_port_in_use(runtime: &Runtime, kind: LinuxFileKind, port: u16) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|file| {
                file.is_open()
                    && file.local_port == port
                    && socket_kinds_share_port_space(file.kind, kind)
            })
    })
}

fn socket_kinds_share_port_space(left: LinuxFileKind, right: LinuxFileKind) -> bool {
    matches!(left, LinuxFileKind::SocketUdp) && matches!(right, LinuxFileKind::SocketUdp)
        || matches!(
            left,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener
        ) && matches!(
            right,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener
        )
}

fn socket_any_port_in_use(runtime: &Runtime, port: u16) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|file| {
                file.is_open()
                    && file.local_port == port
                    && matches!(
                        file.kind,
                        LinuxFileKind::SocketUdp
                            | LinuxFileKind::SocketTcp
                            | LinuxFileKind::SocketTcpListener
                    )
            })
    })
}

fn is_socket_kind(kind: LinuxFileKind) -> bool {
    matches!(
        kind,
        LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink
    )
}

fn read_sockaddr_in(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<(u32, u16), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    if address_len < LINUX_SOCKADDR_IN_LEN {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, address, LINUX_SOCKADDR_IN_LEN)?;
    if read_shm_u16(runtime, 0) as Word != LINUX_AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let port = ((read_shm_u8(runtime, 2) as u16) << 8) | read_shm_u8(runtime, 3) as u16;
    let ip = ((read_shm_u8(runtime, 4) as u32) << 24)
        | ((read_shm_u8(runtime, 5) as u32) << 16)
        | ((read_shm_u8(runtime, 6) as u32) << 8)
        | read_shm_u8(runtime, 7) as u32;
    Ok((ip, port))
}

fn read_sockaddr_nl(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<(), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    if address_len < LINUX_SOCKADDR_NL_LEN {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, address, LINUX_SOCKADDR_NL_LEN)?;
    if read_shm_u16(runtime, 0) as Word != LINUX_AF_NETLINK {
        return Err(EAFNOSUPPORT);
    }
    Ok(())
}

fn write_sockaddr_nl(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if address == 0 || address_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, address_len, 4)?;
    let available = read_shm_u32(runtime, 0) as Word;
    write_sockaddr_nl_value(runtime.posix_shm, pid as u32);
    write_target_memory(runtime, pid, address, available.min(LINUX_SOCKADDR_NL_LEN))?;
    write_u32_to_target(runtime, pid, address_len, LINUX_SOCKADDR_NL_LEN as u32)?;
    Ok(0)
}

fn write_sockaddr_nl_value(base: Word, nl_pid: u32) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_SOCKADDR_NL_LEN as usize);
        write_u16(base, LINUX_AF_NETLINK as u16);
        write_u32(base + 4, nl_pid);
    }
}

fn build_netlink_dump(
    runtime: &mut Runtime,
    pid: Word,
    request_type: u16,
    sequence: u32,
) -> Result<Word, i32> {
    let (ip, _, _) =
        net::net_service_ipv4_config(runtime.network_port).map_err(map_network_error)?;
    let mac = net::net_service_mac_address(runtime.network_port).map_err(map_network_error)?;
    let mut offset = 0usize;
    match request_type {
        LINUX_RTM_GETLINK => {
            offset = append_netlink_link(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                1,
                LINUX_ARPHRD_LOOPBACK,
                LINUX_IFF_UP | LINUX_IFF_LOOPBACK | LINUX_IFF_RUNNING,
                b"lo\0",
                &[0; 6],
            )?;
            offset = append_netlink_link(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                2,
                LINUX_ARPHRD_ETHER,
                LINUX_IFF_UP | LINUX_IFF_BROADCAST | LINUX_IFF_RUNNING | LINUX_IFF_MULTICAST,
                b"eth0\0",
                &mac,
            )?;
        }
        LINUX_RTM_GETADDR => {
            offset = append_netlink_address(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                1,
                8,
                LINUX_RT_SCOPE_HOST,
                [127, 0, 0, 1],
                [127, 255, 255, 255],
                b"lo\0",
            )?;
            offset = append_netlink_address(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                2,
                24,
                LINUX_RT_SCOPE_UNIVERSE,
                ip,
                [ip[0], ip[1], ip[2], 255],
                b"eth0\0",
            )?;
        }
        _ => return Err(EOPNOTSUPP),
    }
    offset = append_netlink_done(
        runtime.posix_shm,
        runtime.posix_shm_size as usize,
        offset,
        sequence,
        pid as u32,
    )?;
    Ok(offset as Word)
}

fn append_netlink_link(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
    index: u32,
    hardware_type: u16,
    flags: u32,
    name: &[u8],
    address: &[u8; 6],
) -> Result<usize, i32> {
    let start = offset;
    let mut cursor = offset + LINUX_NLMSG_HEADER_LEN as usize + LINUX_IFINFOMSG_LEN;
    if cursor > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + start as Word) as *mut u8, 0, cursor - start);
        write_u16(base + start as Word + 4, LINUX_RTM_NEWLINK);
        write_u16(base + start as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + start as Word + 8, sequence);
        write_u32(base + start as Word + 12, pid);
        write_u16(
            base + start as Word + LINUX_NLMSG_HEADER_LEN + 2,
            hardware_type,
        );
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 4, index);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 8, flags);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 12, u32::MAX);
    }
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_IFNAME, name)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_ADDRESS, address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_BROADCAST, &[0xff; 6])?;
    cursor = append_netlink_attr(
        base,
        capacity,
        cursor,
        LINUX_IFLA_MTU,
        &1500u32.to_ne_bytes(),
    )?;
    unsafe { write_u32(base + start as Word, (cursor - start) as u32) };
    Ok(align_up_usize(cursor, 4))
}

fn append_netlink_address(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
    index: u32,
    prefix_len: u8,
    scope: u8,
    address: [u8; 4],
    broadcast: [u8; 4],
    label: &[u8],
) -> Result<usize, i32> {
    let start = offset;
    let mut cursor = offset + LINUX_NLMSG_HEADER_LEN as usize + LINUX_IFADDRMSG_LEN;
    if cursor > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + start as Word) as *mut u8, 0, cursor - start);
        write_u16(base + start as Word + 4, LINUX_RTM_NEWADDR);
        write_u16(base + start as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + start as Word + 8, sequence);
        write_u32(base + start as Word + 12, pid);
        write_u8(
            base + start as Word + LINUX_NLMSG_HEADER_LEN,
            LINUX_AF_INET as u8,
        );
        write_u8(
            base + start as Word + LINUX_NLMSG_HEADER_LEN + 1,
            prefix_len,
        );
        write_u8(base + start as Word + LINUX_NLMSG_HEADER_LEN + 3, scope);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 4, index);
    }
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_ADDRESS, &address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_LOCAL, &address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_BROADCAST, &broadcast)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_LABEL, label)?;
    unsafe { write_u32(base + start as Word, (cursor - start) as u32) };
    Ok(align_up_usize(cursor, 4))
}

fn append_netlink_attr(
    base: Word,
    capacity: usize,
    offset: usize,
    attr_type: u16,
    value: &[u8],
) -> Result<usize, i32> {
    let len = 4usize.checked_add(value.len()).ok_or(EMSGSIZE)?;
    let end = offset.checked_add(align_up_usize(len, 4)).ok_or(EMSGSIZE)?;
    if end > capacity || len > u16::MAX as usize {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + offset as Word) as *mut u8, 0, end - offset);
        write_u16(base + offset as Word, len as u16);
        write_u16(base + offset as Word + 2, attr_type);
        ::core::ptr::copy_nonoverlapping(
            value.as_ptr(),
            (base + offset as Word + 4) as *mut u8,
            value.len(),
        );
    }
    Ok(end)
}

fn append_netlink_done(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
) -> Result<usize, i32> {
    let end = offset + LINUX_NLMSG_HEADER_LEN as usize;
    if end > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes(
            (base + offset as Word) as *mut u8,
            0,
            LINUX_NLMSG_HEADER_LEN as usize,
        );
        write_u32(base + offset as Word, LINUX_NLMSG_HEADER_LEN as u32);
        write_u16(base + offset as Word + 4, LINUX_NLMSG_DONE);
        write_u16(base + offset as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + offset as Word + 8, sequence);
        write_u32(base + offset as Word + 12, pid);
    }
    Ok(end)
}

fn align_up_usize(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

fn write_sockaddr_result(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
    ip: u32,
    port: u16,
) -> Result<(), i32> {
    if address == 0 {
        return Ok(());
    }
    if address_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, address_len, 4)?;
    let available = read_shm_u32(runtime, 0) as Word;
    unsafe {
        ::core::ptr::write_bytes(
            runtime.posix_shm as *mut u8,
            0,
            LINUX_SOCKADDR_IN_LEN as usize,
        );
        write_u16(runtime.posix_shm, LINUX_AF_INET as u16);
        write_u8(runtime.posix_shm + 2, (port >> 8) as u8);
        write_u8(runtime.posix_shm + 3, port as u8);
        write_u8(runtime.posix_shm + 4, (ip >> 24) as u8);
        write_u8(runtime.posix_shm + 5, (ip >> 16) as u8);
        write_u8(runtime.posix_shm + 6, (ip >> 8) as u8);
        write_u8(runtime.posix_shm + 7, ip as u8);
    }
    write_target_memory(runtime, pid, address, available.min(LINUX_SOCKADDR_IN_LEN))?;
    unsafe {
        write_u32(runtime.posix_shm, LINUX_SOCKADDR_IN_LEN as u32);
    }
    write_target_memory(runtime, pid, address_len, 4)
}

fn read_target_memory_to_network(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    len: Word,
) -> Result<(), i32> {
    if len > runtime.network_shm_size {
        return Err(EMSGSIZE);
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.network_shm, len)
        .map_err(map_request_error)
}

fn network_ipv4(runtime: &Runtime) -> Result<u32, i32> {
    let (ip, _, _) =
        net::net_service_ipv4_config(runtime.network_port).map_err(map_network_error)?;
    Ok(((ip[0] as u32) << 24) | ((ip[1] as u32) << 16) | ((ip[2] as u32) << 8) | ip[3] as u32)
}

fn read_network_u16_be(runtime: &Runtime, offset: Word) -> u16 {
    unsafe {
        let base = (runtime.network_shm + offset) as *const u8;
        ((*base as u16) << 8) | *base.add(1) as u16
    }
}

fn read_network_u32_be(runtime: &Runtime, offset: Word) -> u32 {
    unsafe {
        let base = (runtime.network_shm + offset) as *const u8;
        ((*base as u32) << 24)
            | ((*base.add(1) as u32) << 16)
            | ((*base.add(2) as u32) << 8)
            | *base.add(3) as u32
    }
}

unsafe fn write_linux_ipv4_header(
    base: Word,
    total_len: Word,
    src_ip: u32,
    dst_ip: u32,
    ttl: u8,
    protocol: u8,
) {
    ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_IPV4_HEADER_LEN as usize);
    write_u8(base, 0x45);
    write_u8(base + 2, (total_len >> 8) as u8);
    write_u8(base + 3, total_len as u8);
    write_u8(base + 6, 0x40);
    write_u8(base + 8, ttl);
    write_u8(base + 9, protocol);
    write_u8(base + 12, (src_ip >> 24) as u8);
    write_u8(base + 13, (src_ip >> 16) as u8);
    write_u8(base + 14, (src_ip >> 8) as u8);
    write_u8(base + 15, src_ip as u8);
    write_u8(base + 16, (dst_ip >> 24) as u8);
    write_u8(base + 17, (dst_ip >> 16) as u8);
    write_u8(base + 18, (dst_ip >> 8) as u8);
    write_u8(base + 19, dst_ip as u8);
    let checksum = checksum_bytes(base, LINUX_IPV4_HEADER_LEN as usize);
    write_u8(base + 10, (checksum >> 8) as u8);
    write_u8(base + 11, checksum as u8);
}

unsafe fn checksum_bytes(base: Word, len: usize) -> u16 {
    let bytes = ::core::slice::from_raw_parts(base as *const u8, len);
    let mut sum = 0u32;
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        sum = sum.wrapping_add(((bytes[index] as u32) << 8) | bytes[index + 1] as u32);
        index += 2;
    }
    if index < bytes.len() {
        sum = sum.wrapping_add((bytes[index] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn sys_terminal_read(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    sys_terminal_read_now(runtime, pid, user_buffer, len)?.ok_or(EAGAIN)
}

fn sys_terminal_read_now(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Option<Word>, i32> {
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    let chunk = terminal_bounded_len(runtime, len)?;
    if chunk == 0 {
        return Ok(Some(0));
    }
    ensure_terminal_input_notification(runtime, terminal_id)?;

    let canonical = runtime
        .managed_process(pid)
        .map(|process| process.terminal_canonical)
        .unwrap_or(true);
    let bytes = if canonical {
        drain_terminal_canonical_line(runtime, pid, chunk)?
    } else {
        drain_terminal_raw_input(runtime, terminal_id, chunk)?
    };
    if let Some(bytes) = bytes {
        if bytes != 0 {
            libnanami::request_process_memory_write(pid, user_buffer, runtime.terminal_shm, bytes)
                .map_err(map_request_error)?;
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn drain_terminal_raw_input(
    runtime: &mut Runtime,
    terminal_id: Word,
    max_len: Word,
) -> Result<Option<Word>, i32> {
    let bytes = nanami_services::terminal::terminal_read_input(
        runtime.terminal_port,
        terminal_id,
        0,
        max_len,
    )
    .map_err(map_request_error)?;
    if bytes == 0 {
        Ok(None)
    } else {
        Ok(Some(bytes))
    }
}

fn drain_terminal_canonical_line(
    runtime: &mut Runtime,
    pid: Word,
    max_len: Word,
) -> Result<Option<Word>, i32> {
    if let Some(bytes) = pop_terminal_line(runtime, pid, max_len) {
        return Ok(Some(bytes));
    }

    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    loop {
        let bytes = nanami_services::terminal::terminal_read_input(
            runtime.terminal_port,
            terminal_id,
            0,
            1,
        )
        .map_err(map_request_error)?;
        if bytes == 0 {
            return Ok(None);
        }
        let byte = unsafe { ::core::ptr::read(runtime.terminal_shm as *const u8) };
        push_terminal_input_byte(runtime, pid, byte)?;
        if let Some(bytes) = pop_terminal_line(runtime, pid, max_len) {
            return Ok(Some(bytes));
        }
    }
}

fn pop_terminal_line(runtime: &mut Runtime, pid: Word, max_len: Word) -> Option<Word> {
    let terminal_shm = runtime.terminal_shm;
    let process = runtime.managed_process_mut(pid)?;
    if !process.terminal_line_ready {
        return None;
    }
    if process.terminal_line_len == 0 {
        process.terminal_line_read = 0;
        process.terminal_line_ready = false;
        return Some(0);
    }
    if process.terminal_line_read >= process.terminal_line_len {
        process.terminal_line_read = 0;
        process.terminal_line_len = 0;
        process.terminal_line_ready = false;
        return None;
    }
    let remaining = process.terminal_line_len - process.terminal_line_read;
    let bytes = ::core::cmp::min(remaining, max_len as usize);
    if bytes == 0 {
        return None;
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            process
                .terminal_line
                .as_ptr()
                .add(process.terminal_line_read),
            terminal_shm as *mut u8,
            bytes,
        );
    }
    process.terminal_line_read += bytes;
    if process.terminal_line_read >= process.terminal_line_len {
        process.terminal_line_read = 0;
        process.terminal_line_len = 0;
        process.terminal_line_ready = false;
    }
    Some(bytes as Word)
}

fn push_terminal_input_byte(runtime: &mut Runtime, pid: Word, byte: u8) -> Result<(), i32> {
    let Some(process) = runtime.managed_process_mut(pid) else {
        return Err(ESRCH);
    };
    match byte {
        b'\r' | b'\n' => {
            if process.terminal_line_len < LINUX_TERMINAL_LINE_MAX {
                process.terminal_line[process.terminal_line_len] = b'\n';
                process.terminal_line_len += 1;
            }
            process.terminal_line_ready = true;
        }
        0x04 => {
            // Alter uses the terminal input stream to wake a blocked emulated read
            // while terminating a foreground process. Treat EOT as a wake byte
            // so stale control input is not exposed to the next process.
        }
        0x7f | 0x08 => {
            if process.terminal_line_len != 0 {
                process.terminal_line_len -= 1;
            }
        }
        0x20..=0x7e | b'\t' => {
            if process.terminal_line_len + 1 < LINUX_TERMINAL_LINE_MAX {
                process.terminal_line[process.terminal_line_len] = byte;
                process.terminal_line_len += 1;
            }
        }
        _ => {}
    }
    Ok(())
}

fn ensure_terminal_input_notification(runtime: &mut Runtime, terminal_id: Word) -> Result<(), i32> {
    if runtime.terminal_input_notification_id == terminal_id {
        return Ok(());
    }
    nanami_services::terminal::terminal_attach_input_notification(
        runtime.terminal_port,
        terminal_id,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )
    .map_err(map_request_error)?;
    runtime.terminal_input_notification_id = terminal_id;
    Ok(())
}

fn sys_terminal_write(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let terminal_id = terminal_id_for_pid(runtime, pid)?;
    let mut done = 0;
    while done < len {
        let chunk = terminal_bounded_len(runtime, len - done)?;
        libnanami::request_process_memory_read(
            pid,
            user_buffer + done,
            runtime.terminal_shm,
            chunk,
        )
        .map_err(map_request_error)?;
        let written = nanami_services::terminal::terminal_write_output(
            runtime.terminal_port,
            terminal_id,
            0,
            chunk,
        )
        .map_err(map_request_error)?;
        if written == 0 {
            return Err(EIO);
        }
        done += written;
        if written < chunk {
            break;
        }
    }
    Ok(done)
}

fn sys_pipe_read(
    runtime: &mut Runtime,
    pid: Word,
    pipe_id: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let max = bounded_len(runtime, len)? as usize;
    if max == 0 {
        return Ok(0);
    }
    let shm = runtime.posix_shm;
    let pipe = runtime.pipe_mut(pipe_id).ok_or(EBADF)?;
    if pipe.len == 0 {
        return Ok(0);
    }
    let mut done = 0usize;
    while done < max && pipe.len != 0 {
        unsafe {
            ::core::ptr::write((shm + done as Word) as *mut u8, pipe.buffer[pipe.read]);
        }
        pipe.read = (pipe.read + 1) % LINUX_PIPE_BYTES;
        pipe.len -= 1;
        done += 1;
    }
    write_target_memory(runtime, pid, user_buffer, done as Word)?;
    Ok(done as Word)
}

fn sys_pipe_write(
    runtime: &mut Runtime,
    pid: Word,
    pipe_id: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let max = bounded_len(runtime, len)?;
    if max == 0 {
        return Ok(0);
    }
    read_target_memory(runtime, pid, user_buffer, max)?;
    let shm = runtime.posix_shm;
    let pipe = runtime.pipe_mut(pipe_id).ok_or(EBADF)?;
    if pipe.readers == 0 {
        return Err(EPIPE);
    }
    let mut done = 0usize;
    while done < max as usize && pipe.len < LINUX_PIPE_BYTES {
        let byte = unsafe { ::core::ptr::read((shm + done as Word) as *const u8) };
        pipe.buffer[pipe.write] = byte;
        pipe.write = (pipe.write + 1) % LINUX_PIPE_BYTES;
        pipe.len += 1;
        done += 1;
    }
    Ok(done as Word)
}

fn sys_writev(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    iov_ptr: Word,
    iov_count: Word,
) -> Result<Word, i32> {
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov_ptr, iov_count, &mut bases, &mut lens)?;

    let mut total = 0 as Word;
    let mut i = 0usize;
    while i < iov_count as usize {
        let base = bases[i];
        let len = lens[i];
        if len != 0 {
            match sys_write(runtime, pid, fd, base, len) {
                Ok(written) => {
                    total = total.checked_add(written).ok_or(EINVAL)?;
                    if written < len {
                        break;
                    }
                }
                Err(_) if total != 0 => break,
                Err(errno) => return Err(errno),
            }
        }
        i += 1;
    }
    Ok(total)
}

fn sys_readv(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    iov_ptr: Word,
    iov_count: Word,
) -> Result<Word, i32> {
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov_ptr, iov_count, &mut bases, &mut lens)?;

    let mut total = 0 as Word;
    let mut i = 0usize;
    while i < iov_count as usize {
        let len = lens[i];
        if len != 0 {
            match sys_read(runtime, pid, fd, bases[i], len) {
                Ok(read) => {
                    total = total.checked_add(read).ok_or(EINVAL)?;
                    if read < len {
                        break;
                    }
                }
                Err(_) if total != 0 => break,
                Err(errno) => return Err(errno),
            }
        }
        i += 1;
    }
    Ok(total)
}

fn sys_getdents64(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    count: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 {
        return Err(EFAULT);
    }
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::VirtualDirectory {
        return sys_virtual_getdents(runtime, pid, fd, user_buffer, count);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(ENOTDIR);
    }
    let posix_offset = ALTER_IO_OFFSET as Word;
    let max_records = ::core::cmp::max(
        1,
        ::core::cmp::min(
            16,
            runtime.posix_shm_size.saturating_sub(posix_offset)
                / vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES as Word,
        ),
    );
    let (entries, next_index) =
        posix::posix_read_dir(runtime.posix_port, file.posix_fd, max_records, posix_offset)
            .map_err(map_request_error)?;
    if entries == 0 {
        return Ok(0);
    }

    let source_base = runtime.posix_shm + posix_offset;
    let first_next_index = next_index.saturating_sub(entries);
    let mut out_offset = 0 as Word;
    let mut index = 0 as Word;
    while index < entries {
        let entry = source_base + index * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES as Word;
        let inode = unsafe { ::core::ptr::read_unaligned(entry as *const Word) };
        let kind = unsafe {
            ::core::ptr::read_unaligned(
                (entry + vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET as Word) as *const Word,
            )
        };
        let name_len = unsafe {
            ::core::ptr::read_unaligned(
                (entry + vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET as Word) as *const Word,
            )
        } as usize;
        if name_len == 0 || name_len >= vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES {
            return Err(EIO);
        }
        let reclen = align_up_word((LINUX_DIRENT64_NAME_OFFSET + name_len + 1) as Word, 8);
        if out_offset + reclen > count || out_offset + reclen > posix_offset {
            break;
        }
        let dtype = match kind {
            posix::POSIX_FILE_TYPE_DIRECTORY => LINUX_DT_DIR,
            posix::POSIX_FILE_TYPE_CHAR_DEVICE => LINUX_DT_CHR,
            posix::POSIX_FILE_TYPE_BLOCK_DEVICE => LINUX_DT_BLK,
            posix::POSIX_FILE_TYPE_REGULAR => LINUX_DT_REG,
            _ => LINUX_DT_UNKNOWN,
        };
        unsafe {
            let out = runtime.posix_shm + out_offset;
            ::core::ptr::write_bytes(out as *mut u8, 0, reclen as usize);
            write_u64(out, inode);
            write_u64(out + 8, first_next_index + index + 1);
            write_u16(out + 16, reclen as u16);
            ::core::ptr::write((out + 18) as *mut u8, dtype as u8);
            ::core::ptr::copy_nonoverlapping(
                (entry + vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET as Word) as *const u8,
                (out + LINUX_DIRENT64_NAME_OFFSET as Word) as *mut u8,
                name_len,
            );
        }
        out_offset += reclen;
        index += 1;
    }
    if out_offset == 0 {
        return Err(EINVAL);
    }
    write_target_memory(runtime, pid, user_buffer, out_offset)?;
    Ok(out_offset)
}

fn sys_virtual_getdents(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    count: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let directory = VirtualNode::from_id(file.resource).ok_or(ENOTDIR)?;
    if !directory.is_directory() {
        return Err(ENOTDIR);
    }
    let graphics = graphics_enabled(runtime, pid);
    let mut out = 0usize;
    let mut index = file.offset as usize;
    while let Some(entry) = virtual_fs::directory_entry(directory, index, graphics) {
        let reclen = align_up_word(
            (LINUX_DIRENT64_NAME_OFFSET + entry.name.len() + 1) as Word,
            8,
        ) as usize;
        if out + reclen > count as usize || out + reclen > runtime.posix_shm_size as usize {
            break;
        }
        unsafe {
            let base = runtime.posix_shm + out as Word;
            ::core::ptr::write_bytes(base as *mut u8, 0, reclen);
            write_u64(base, entry.node.id());
            write_u64(base + 8, (index + 1) as Word);
            write_u16(base + 16, reclen as u16);
            let dtype = if entry.node.is_directory() {
                LINUX_DT_DIR
            } else if entry.node.is_regular_file() {
                LINUX_DT_REG
            } else {
                LINUX_DT_CHR
            };
            ::core::ptr::write((base + 18) as *mut u8, dtype as u8);
            ::core::ptr::copy_nonoverlapping(
                entry.name.as_ptr(),
                (base + LINUX_DIRENT64_NAME_OFFSET as Word) as *mut u8,
                entry.name.len(),
            );
        }
        out += reclen;
        index += 1;
    }
    file.offset = index as Word;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    if out != 0 {
        write_target_memory(runtime, pid, user_buffer, out as Word)?;
    }
    Ok(out as Word)
}

fn sys_virtual_read(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let node = VirtualNode::from_id(file.resource).ok_or(EBADF)?;
    if node == VirtualNode::DevNull {
        return Ok(0);
    }
    if node == VirtualNode::DevZero {
        let bytes = len.min(runtime.posix_shm_size);
        unsafe { ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, bytes as usize) };
        write_target_memory(runtime, pid, user_buffer, bytes)?;
        return Ok(bytes);
    }
    let memory_text;
    let memory_text_len;
    let image_name;
    let bytes = if node == VirtualNode::ProcMemInfo {
        let info = libnanami::request_nanami_info_memory().map_err(map_request_error)?;
        (memory_text, memory_text_len) = format_proc_meminfo(info.total_bytes, info.free_bytes);
        &memory_text[..memory_text_len]
    } else if node == VirtualNode::ProcSelfExe {
        let process = runtime.managed_process(pid).ok_or(ESRCH)?;
        image_name = process.image_name;
        &image_name[..process.image_name_len]
    } else {
        virtual_fs::static_file(node).ok_or(EINVAL)?
    };
    let offset = file.offset as usize;
    if offset >= bytes.len() {
        return Ok(0);
    }
    let amount = (bytes.len() - offset)
        .min(len as usize)
        .min(runtime.posix_shm_size as usize);
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes[offset..offset + amount].as_ptr(),
            runtime.posix_shm as *mut u8,
            amount,
        );
    }
    write_target_memory(runtime, pid, user_buffer, amount as Word)?;
    file.offset += amount as Word;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(amount as Word)
}

fn sys_virtual_write(runtime: &Runtime, pid: Word, fd: Word, len: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let node = VirtualNode::from_id(file.resource).ok_or(EBADF)?;
    match node {
        VirtualNode::DevNull | VirtualNode::DevZero => Ok(len),
        _ => Err(EBADF),
    }
}

fn format_proc_meminfo(total: Word, free: Word) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    let mut pos = 0usize;
    pos = append_bytes_to_array(&mut out, pos, b"MemTotal:       ");
    pos = append_decimal_to_array(&mut out, pos, total / 1024);
    pos = append_bytes_to_array(&mut out, pos, b" kB\nMemFree:        ");
    pos = append_decimal_to_array(&mut out, pos, free / 1024);
    pos = append_bytes_to_array(&mut out, pos, b" kB\n");
    (out, pos)
}

fn append_bytes_to_array(out: &mut [u8], mut pos: usize, value: &[u8]) -> usize {
    for byte in value {
        if pos >= out.len() {
            break;
        }
        out[pos] = *byte;
        pos += 1;
    }
    pos
}

fn append_decimal_to_array(out: &mut [u8], pos: usize, mut value: Word) -> usize {
    if value == 0 {
        return append_bytes_to_array(out, pos, b"0");
    }
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    while value != 0 {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    let mut out_pos = pos;
    while count != 0 {
        count -= 1;
        out_pos = append_bytes_to_array(out, out_pos, &digits[count..count + 1]);
    }
    out_pos
}

fn sys_open(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    linux_flags: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    if let Some(fd) = open_virtual_path(runtime, pid, len, linux_flags)? {
        return Ok(fd);
    }
    let flags = translate_open_flags(linux_flags);
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd =
        posix::posix_open(runtime.posix_port, 0, vfs_len, flags).map_err(map_path_request_error)?;
    if (linux_flags & LINUX_O_APPEND) != 0 {
        let _ = posix::posix_seek(runtime.posix_port, fd, 0, posix::POSIX_SEEK_END);
    }
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    runtime
        .allocate_linux_file(pid, LinuxFile::posix(fd, fd_flags), 0)
        .ok_or(EMFILE)
}

fn sys_openat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    linux_flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    if let Some(fd) = open_virtual_path(runtime, pid, len, linux_flags)? {
        return Ok(fd);
    }
    let flags = translate_open_flags(linux_flags);
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd =
        posix::posix_open(runtime.posix_port, 0, vfs_len, flags).map_err(map_path_request_error)?;
    if (linux_flags & LINUX_O_APPEND) != 0 {
        let _ = posix::posix_seek(runtime.posix_port, fd, 0, posix::POSIX_SEEK_END);
    }
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    runtime
        .allocate_linux_file(pid, LinuxFile::posix(fd, fd_flags), 0)
        .ok_or(EMFILE)
}

fn open_virtual_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    linux_flags: Word,
) -> Result<Option<Word>, i32> {
    let graphics_enabled = runtime
        .managed_process(pid)
        .map(|process| process.graphics_enabled)
        .ok_or(ESRCH)?;
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    let Some(node) = virtual_fs::lookup(path, graphics_enabled) else {
        return if is_linux_virtual_path(runtime.posix_shm, len) {
            Err(ENOENT)
        } else {
            Ok(None)
        };
    };
    if (linux_flags & LINUX_O_DIRECTORY) != 0 && !node.is_directory() {
        return Err(ENOTDIR);
    }
    let access_mode = linux_flags & LINUX_O_ACCMODE;
    if node.is_directory() && access_mode != LINUX_O_RDONLY {
        return Err(EISDIR);
    }
    if node.is_regular_file()
        && (access_mode != LINUX_O_RDONLY || (linux_flags & (LINUX_O_CREAT | LINUX_O_TRUNC)) != 0)
    {
        return Err(EROFS);
    }
    let fd_flags = if (linux_flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    } | (linux_flags & LINUX_O_NONBLOCK);
    let file = match node {
        VirtualNode::DevTty => LinuxFile::terminal(),
        VirtualNode::DevNull | VirtualNode::DevZero => {
            LinuxFile::virtual_node(LinuxFileKind::VirtualFile, node.id(), fd_flags)
        }
        VirtualNode::DevKeyboard => {
            ensure_input(runtime, pid)?;
            LinuxFile::virtual_node(
                LinuxFileKind::EvdevKeyboard,
                input_resource(runtime, pid, node.id())?,
                fd_flags,
            )
        }
        VirtualNode::DevMouse => {
            ensure_input(runtime, pid)?;
            LinuxFile::virtual_node(
                LinuxFileKind::EvdevMouse,
                input_resource(runtime, pid, node.id())?,
                fd_flags,
            )
        }
        VirtualNode::DevFramebuffer => {
            let session = ensure_graphics_session(runtime, pid)?;
            let mut file = LinuxFile::virtual_node(LinuxFileKind::Framebuffer, node.id(), fd_flags);
            file.resource = session;
            file
        }
        node if node.is_directory() => {
            LinuxFile::virtual_node(LinuxFileKind::VirtualDirectory, node.id(), fd_flags)
        }
        _ => LinuxFile::virtual_node(LinuxFileKind::VirtualFile, node.id(), fd_flags),
    };
    runtime
        .allocate_linux_file(pid, file, 0)
        .map(Some)
        .ok_or(EMFILE)
}

fn graphics_enabled(runtime: &Runtime, pid: Word) -> bool {
    runtime
        .managed_process(pid)
        .map(|process| process.graphics_enabled)
        .unwrap_or(false)
}

fn input_resource(runtime: &Runtime, pid: Word, node: Word) -> Result<Word, i32> {
    let session = runtime
        .managed_process(pid)
        .map(|process| process.graphics_session)
        .ok_or(ESRCH)?;
    Ok((session << 32) | node)
}

fn ensure_input(runtime: &mut Runtime, pid: Word) -> Result<(), i32> {
    if graphics_enabled(runtime, pid) {
        ensure_graphics_session(runtime, pid)?;
        return Ok(());
    }
    if runtime.input_port != 0 && runtime.input_queue != 0 {
        return Ok(());
    }
    nanami_services::registry::connect_input_service(SLOT_INPUT_SERVICE).map_err(|_| ENODEV)?;
    let port = libnanami::ipc::process_slot_descriptor(SLOT_INPUT_SERVICE);
    let (queue, bytes) = input::input_service_subscribe_shared(
        port,
        input::INPUT_SUBSCRIBE_KEYBOARD | input::INPUT_SUBSCRIBE_MOUSE,
    )
    .map_err(map_request_error)?;
    if queue == 0 || bytes == 0 {
        return Err(ENODEV);
    }
    runtime.input_port = port;
    runtime.input_queue = queue;
    runtime.input_queue_size = bytes;
    Ok(())
}

fn ensure_graphics_session(runtime: &mut Runtime, pid: Word) -> Result<Word, i32> {
    if !graphics_enabled(runtime, pid) {
        return Err(ENOENT);
    }
    if let Some(process) = runtime.managed_process(pid) {
        if process.graphics_session != 0 {
            return Ok(process.graphics_session);
        }
    }
    let root_pid = process_tree_root(runtime, pid)?;
    let mut index = 0usize;
    while index < runtime.graphics.len() {
        if runtime.graphics[index].active && runtime.graphics[index].root_pid == root_pid {
            let id = index as Word + 1;
            let _ = runtime.set_graphics_session(pid, id);
            return Ok(id);
        }
        index += 1;
    }
    let Some(index) = runtime.graphics.iter().position(|entry| !entry.active) else {
        return Err(ENOMEM);
    };
    if runtime.honoka_port == 0 {
        runtime.honoka_pid =
            nanami_services::registry::connect_honoka_service_with_pid(SLOT_HONOKA_SERVICE)
                .map_err(|_| ENODEV)?;
        runtime.honoka_port = libnanami::ipc::process_slot_descriptor(SLOT_HONOKA_SERVICE);
    }
    let honoka_pid = runtime.honoka_pid;
    let port = runtime.honoka_port;
    let window = honoka::honoka_create_window_with_title(
        port,
        80 + (index as Word * 32),
        80 + (index as Word * 32),
        ALTER_FB_WIDTH,
        ALTER_FB_HEIGHT,
        b"Alter/Linux fb0",
    )
    .map_err(map_request_error)?;
    let present_slot = SLOT_HONOKA_PRESENT_NOTIFICATION_BASE + index as Word;
    if let Err(error) = libnanami::request_notification_port_copy(
        honoka_pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        present_slot,
        honoka::HONOKA_NOTIFICATION_PRESENT | (window & 0xffff_ffff),
    ) {
        let _ = honoka::honoka_destroy_window(port, window);
        return Err(map_request_error(error));
    }
    let present_notification = libnanami::ipc::process_slot_descriptor(present_slot);
    let (input_queue, _) = match honoka::honoka_attach_input_queue(port, window) {
        Ok(attached) => attached,
        Err(error) => {
            let _ = honoka::honoka_destroy_window(port, window);
            return Err(map_request_error(error));
        }
    };
    if let Err(error) = honoka::honoka_attach_input_notification(port, window) {
        let _ = libnanami::request_mapping_release(input_queue, input::INPUT_EVENT_QUEUE_BYTES);
        let _ = honoka::honoka_destroy_window(port, window);
        return Err(map_request_error(error));
    }
    runtime.graphics[index] = crate::state::GraphicsSession {
        active: true,
        root_pid,
        honoka_port: port,
        present_notification,
        window_id: window,
        width: ALTER_FB_WIDTH,
        height: ALTER_FB_HEIGHT,
        damage_queue: 0,
        framebuffer: 0,
        framebuffer_bytes: ALTER_FB_BYTES,
        input_queue,
        keyboard_events: [0; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
        keyboard_head: 0,
        keyboard_tail: 0,
        mouse_events: [0; crate::state::ALTER_EVDEV_QUEUE_CAPACITY],
        mouse_head: 0,
        mouse_tail: 0,
        mouse_x: 0,
        mouse_y: 0,
        mouse_position_valid: false,
        guest_pid: 0,
        guest_framebuffer: 0,
        guest_framebuffer_bytes: 0,
    };
    let id = index as Word + 1;
    let _ = runtime.set_graphics_session(pid, id);
    Ok(id)
}

fn process_tree_root(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    let mut current = pid;
    let mut depth = 0usize;
    while depth < runtime.managed.len() {
        let process = runtime.managed_process(current).ok_or(ESRCH)?;
        if process.parent_pid == 0 {
            return Ok(process.pid);
        }
        current = process.parent_pid;
        depth += 1;
    }
    Err(EINVAL)
}

fn sys_close(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.clear_linux_file(pid, fd).ok_or(EBADF)?;
    if fd <= 2 {
        libnanami::println!(
            "[alter/linux] close stdio pid={} fd={} kind={}",
            pid,
            fd,
            linux_file_kind_name(file.kind)
        );
    }
    match file.kind {
        LinuxFileKind::Posix => {
            posix::posix_close(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
        }
        LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => release_pipe_file(runtime, file),
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => close_socket_file(runtime, file),
        LinuxFileKind::Terminal
        | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::VirtualFile
        | LinuxFileKind::EvdevKeyboard
        | LinuxFileKind::EvdevMouse
        | LinuxFileKind::Framebuffer
        | LinuxFileKind::Empty => {}
    }
    Ok(0)
}

fn sys_dup(runtime: &mut Runtime, pid: Word, old_fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, old_fd).ok_or(EBADF)?;
    let mut new_file = duplicate_linux_file(runtime, file)?;
    new_file.flags &= !LINUX_FD_CLOEXEC;
    runtime.allocate_linux_file(pid, new_file, 0).ok_or(EMFILE)
}

fn sys_dup2(runtime: &mut Runtime, pid: Word, old_fd: Word, new_fd: Word) -> Result<Word, i32> {
    if new_fd as usize >= LINUX_FD_MAX {
        return Err(EBADF);
    }
    let file = runtime.linux_file(pid, old_fd).ok_or(EBADF)?;
    if old_fd == new_fd {
        return Ok(new_fd);
    }
    close_linux_fd(runtime, pid, new_fd)?;
    let mut new_file = duplicate_linux_file(runtime, file)?;
    new_file.flags &= !LINUX_FD_CLOEXEC;
    if runtime.set_linux_file(pid, new_fd, new_file) {
        if new_fd <= 2 {
            libnanami::println!(
                "[alter/linux] dup2 stdio pid={} old_fd={} new_fd={} kind={}",
                pid,
                old_fd,
                new_fd,
                linux_file_kind_name(new_file.kind)
            );
        }
        Ok(new_fd)
    } else {
        if new_file.kind == LinuxFileKind::Posix {
            let _ = posix::posix_close(runtime.posix_port, new_file.posix_fd);
        }
        Err(EBADF)
    }
}

fn sys_dup3(
    runtime: &mut Runtime,
    pid: Word,
    old_fd: Word,
    new_fd: Word,
    flags: Word,
) -> Result<Word, i32> {
    if old_fd == new_fd {
        return Err(EINVAL);
    }
    if (flags & !LINUX_O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let fd = sys_dup2(runtime, pid, old_fd, new_fd)?;
    if (flags & LINUX_O_CLOEXEC) != 0 {
        let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        file.flags |= LINUX_FD_CLOEXEC;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
    }
    Ok(fd)
}

fn sys_pipe(runtime: &mut Runtime, pid: Word, pipefd_ptr: Word, flags: Word) -> Result<Word, i32> {
    if pipefd_ptr == 0 {
        return Err(EFAULT);
    }
    if (flags & !LINUX_O_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let fd_flags = if (flags & LINUX_O_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    };
    let pipe_id = runtime.allocate_pipe().ok_or(EMFILE)?;
    let read_fd = match runtime.allocate_linux_file(pid, LinuxFile::pipe_read(pipe_id, fd_flags), 0)
    {
        Some(fd) => fd,
        None => {
            release_pipe_id(runtime, pipe_id);
            return Err(EMFILE);
        }
    };
    let write_fd =
        match runtime.allocate_linux_file(pid, LinuxFile::pipe_write(pipe_id, fd_flags), 0) {
            Some(fd) => fd,
            None => {
                let _ = runtime.clear_linux_file(pid, read_fd);
                release_pipe_id(runtime, pipe_id);
                return Err(EMFILE);
            }
        };
    unsafe {
        write_u32(runtime.posix_shm, read_fd as u32);
        write_u32(runtime.posix_shm + 4, write_fd as u32);
    }
    write_target_memory(runtime, pid, pipefd_ptr, 8)?;
    Ok(0)
}

fn sys_fork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    if context.number == SYS_CLONE && (context.args[0] & LINUX_CLONE_VM) != 0 {
        return Err(ENOSYS);
    }
    reap_exited_children(runtime, parent_pid);
    let parent = runtime.managed_process(parent_pid).ok_or(ESRCH)?;
    let image_name =
        ::core::str::from_utf8(&parent.image_name[..parent.image_name_len]).map_err(|_| EINVAL)?;
    let personality = parent.personality;
    let clone_options = clone_options(context);
    let current_fs_base = read_register_value(parent.pcb, REG_FS_BASE).unwrap_or(parent.fs_base);
    if current_fs_base != parent.fs_base {
        let _ = runtime.set_fs_base(parent_pid, current_fs_base);
    }

    let child_fs_base = if clone_options.set_tls {
        clone_options.tls
    } else {
        current_fs_base
    };

    let pcb_slot = runtime.next_pcb_slot().ok_or(ENOMEM)?;
    let child_pid = match spawn_fork_child(runtime, image_name.as_bytes(), pcb_slot, personality) {
        Ok(pid) => pid,
        Err(errno) => {
            libnanami::println!(
                "[alter/linux] fork failed stage=spawn image={} pcb_slot={} errno={}",
                image_name,
                pcb_slot,
                errno
            );
            return Err(errno);
        }
    };
    let child_pcb = libnanami::ipc::process_slot_descriptor(pcb_slot);

    if let Err(error) = fork_step(
        "clone image",
        clone_process_image(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone mappings",
        clone_process_mappings(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone stack",
        clone_process_stack(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone tids",
        write_clone_tid_pointers(
            runtime,
            parent_pid,
            child_pid,
            clone_options.parent_tid,
            clone_options.child_tid,
            clone_options.write_parent_tid,
            clone_options.write_child_tid,
        ),
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if let Err(error) = fork_step(
        "clone registers",
        match clone_registers_for_fork(
            parent.pcb,
            child_pcb,
            context,
            clone_options.child_stack,
            child_fs_base,
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                libnanami::println!(
                    "[alter/linux] fork register clone failed parent_pcb={:#x} child_pcb={:#x} err={:?}",
                    parent.pcb,
                    child_pcb,
                    error
                );
                Err(EIO)
            }
        },
    ) {
        discard_spawned_child(child_pid);
        return Err(error);
    }
    if !runtime.install_managed_child_process(
        child_pid,
        parent_pid,
        parent.owner_pid,
        child_pcb,
        parent.terminal_id,
        &parent.image_name[..parent.image_name_len],
    ) {
        libnanami::println!(
            "[alter/linux] fork failed stage=install child parent={} child={} errno={}",
            parent_pid,
            child_pid,
            ENOMEM
        );
        discard_spawned_child(child_pid);
        return Err(ENOMEM);
    }
    if let Err(error) = fork_step(
        "clone files",
        inherit_linux_files(runtime, parent_pid, child_pid),
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    let mut i = 0usize;
    while i < parent.mappings.len() {
        let mapping = parent.mappings[i];
        if mapping.base != 0
            && !runtime.add_mapping(child_pid, mapping.base, mapping.size, mapping.prot)
        {
            libnanami::println!(
                "[alter/linux] fork failed stage=track mappings parent={} child={} errno={}",
                parent_pid,
                child_pid,
                ENOMEM
            );
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ENOMEM);
        }
        i += 1;
    }
    {
        let Some(child) = runtime.managed_process_mut(child_pid) else {
            discard_spawned_child(child_pid);
            runtime.remove_process(child_pid);
            return Err(ESRCH);
        };
        child.program_break = parent.program_break;
        child.mapped_break = parent.mapped_break;
        child.fs_base = child_fs_base;
        child.trace_enabled = parent.trace_enabled;
        child.diagnostics_enabled = parent.diagnostics_enabled;
        child.graphics_enabled = parent.graphics_enabled;
        child.graphics_session = parent.graphics_session;
        child.personality = parent.personality;
        child.terminal_canonical = parent.terminal_canonical;
        child.terminal_echo = parent.terminal_echo;
    }
    if cfg!(debug_assertions) {
        if let Ok(read_back_fs_base) = read_register_value(child_pcb, REG_FS_BASE) {
            if read_back_fs_base != child_fs_base {
                libnanami::println!(
                    "[alter/linux] fork fsbase mismatch parent={} child={} expected={:#x} actual={:#x}",
                    parent_pid,
                    child_pid,
                    child_fs_base,
                    read_back_fs_base
                );
            }
        }
    }
    if let Err(error) = fork_step(
        "resume child",
        a9n_abi::arch::process_control_block::resume(child_pcb).map_err(|_| EIO),
    ) {
        discard_spawned_child(child_pid);
        runtime.remove_process(child_pid);
        return Err(error);
    }
    Ok(child_pid)
}

fn spawn_fork_child(
    runtime: &mut Runtime,
    image_name: &[u8],
    pcb_slot: Word,
    personality: OsPersonality,
) -> Result<Word, i32> {
    spawn_rootfs_fork_child(runtime, image_name, pcb_slot, personality)
}

fn spawn_rootfs_fork_child(
    runtime: &mut Runtime,
    image_name: &[u8],
    pcb_slot: Word,
    personality: OsPersonality,
) -> Result<Word, i32> {
    let path_len = write_rootfs_image_path(runtime, image_name, personality)?;
    let loaded =
        load_cached_fork_linux_elf_image(runtime, 0, path_len as Word).map_err(map_load_error)?;
    libnanami::request_process_spawn_memory_fault_handler_suspended(
        loaded.address,
        loaded.size,
        4,
        pcb_slot,
    )
    .map_err(map_request_error)
}

fn write_rootfs_image_path(
    runtime: &mut Runtime,
    image_name: &[u8],
    personality: OsPersonality,
) -> Result<usize, i32> {
    let prefix = personality::bin_prefix(personality);
    let prefix_len = if image_name.first() == Some(&b'/') {
        0
    } else {
        prefix.len()
    };
    let len = prefix_len
        .checked_add(image_name.len())
        .ok_or(ENAMETOOLONG)?;
    if len == 0 || len as Word > runtime.client_shm_size {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        if prefix_len != 0 {
            ::core::ptr::copy_nonoverlapping(
                prefix.as_ptr(),
                runtime.client_shm as *mut u8,
                prefix_len,
            );
        }
        ::core::ptr::copy_nonoverlapping(
            image_name.as_ptr(),
            (runtime.client_shm as usize + prefix_len) as *mut u8,
            image_name.len(),
        );
    }
    Ok(len)
}

fn copy_current_path_to_client_shm(runtime: &mut Runtime, len: Word) -> Result<(), i32> {
    if len == 0 || len > runtime.client_shm_size || len > runtime.posix_shm_size {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            runtime.client_shm as *mut u8,
            len as usize,
        );
    }
    Ok(())
}

fn map_load_error(error: LoadError) -> i32 {
    match error {
        LoadError::InvalidArgument => EINVAL,
        LoadError::NotFound => ENOENT,
        LoadError::Io => EIO,
        LoadError::InvalidElf | LoadError::UnsupportedElf => ENOEXEC,
    }
}

fn log_execve_load_error(runtime: &Runtime, pid: Word, path_len: Word, error: LoadError) {
    let path = unsafe {
        ::core::slice::from_raw_parts(
            runtime.client_shm as *const u8,
            ::core::cmp::min(path_len as usize, 96),
        )
    };
    match ::core::str::from_utf8(path) {
        Ok(text) => libnanami::println!(
            "[alter/linux] execve load failed pid={} path={} err={}",
            pid,
            text,
            load_error_name(error)
        ),
        Err(_) => libnanami::println!(
            "[alter/linux] execve load failed pid={} path-len={} err={}",
            pid,
            path_len,
            load_error_name(error)
        ),
    }
}

fn load_error_name(error: LoadError) -> &'static str {
    match error {
        LoadError::InvalidArgument => "invalid-argument",
        LoadError::NotFound => "not-found",
        LoadError::Io => "io",
        LoadError::InvalidElf => "invalid-elf",
        LoadError::UnsupportedElf => "unsupported-elf",
    }
}

fn sys_vfork(
    runtime: &mut Runtime,
    parent_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    match sys_fork(runtime, parent_pid, context) {
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

fn discard_spawned_child(pid: Word) {
    let _ = libnanami::request_process_kill(pid, 1);
    let _ = libnanami::request_process_reap(pid);
}

fn reap_exited_children(runtime: &mut Runtime, parent_pid: Word) {
    loop {
        let Some((child_pid, _status)) = runtime.exited_child(parent_pid, 0) else {
            return;
        };
        match libnanami::request_process_reap(child_pid) {
            Ok(()) => {
                close_process_files(runtime, child_pid);
                runtime.remove_process(child_pid);
            }
            Err(error) => {
                let errno = map_request_error(error);
                libnanami::println!(
                    "[alter/linux] fork pre-clean reap failed parent={} child={} errno={}",
                    parent_pid,
                    child_pid,
                    errno
                );
                close_process_files(runtime, child_pid);
                runtime.remove_process(child_pid);
                return;
            }
        }
    }
}

fn fork_step(stage: &str, result: Result<(), i32>) -> Result<(), i32> {
    if let Err(error) = result {
        libnanami::println!("[alter/linux] fork failed stage={} errno={}", stage, error);
        return Err(error);
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct CloneOptions {
    child_stack: Word,
    parent_tid: Word,
    child_tid: Word,
    tls: Word,
    write_parent_tid: bool,
    write_child_tid: bool,
    set_tls: bool,
}

fn clone_options(context: LinuxSyscallContext) -> CloneOptions {
    let flags = if context.number == SYS_CLONE {
        context.args[0]
    } else {
        LINUX_SIGCHLD
    };
    CloneOptions {
        child_stack: if context.number == SYS_CLONE {
            context.args[1]
        } else {
            0
        },
        parent_tid: if context.number == SYS_CLONE {
            context.args[2]
        } else {
            0
        },
        child_tid: if context.number == SYS_CLONE {
            context.args[3]
        } else {
            0
        },
        tls: if context.number == SYS_CLONE {
            context.args[4]
        } else {
            0
        },
        write_parent_tid: (flags & LINUX_CLONE_PARENT_SETTID) != 0,
        write_child_tid: (flags & LINUX_CLONE_CHILD_SETTID) != 0,
        set_tls: (flags & LINUX_CLONE_SETTLS) != 0,
    }
}

fn write_clone_tid_pointers(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
    parent_tid: Word,
    child_tid: Word,
    write_parent_tid: bool,
    write_child_tid: bool,
) -> Result<(), i32> {
    if write_parent_tid && parent_tid != 0 {
        write_guest_u32(runtime, parent_pid, parent_tid, child_pid as u32)?;
    }
    if write_child_tid && child_tid != 0 {
        write_guest_u32(runtime, child_pid, child_tid, child_pid as u32)?;
    }
    Ok(())
}

fn sys_execve(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    argv_ptr: Word,
    envp_ptr: Word,
) -> Result<(), i32> {
    let len = resolve_path(runtime, pid, path_ptr).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "resolve-path", 0, errno);
        errno
    })?;
    if len as usize > LINUX_EXEC_PATH_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut exec_path = [0u8; LINUX_EXEC_PATH_MAX];
    let path_len = len as usize;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            exec_path.as_mut_ptr(),
            path_len,
        );
    }
    let snapshot = snapshot_exec_strings(runtime, pid, argv_ptr, envp_ptr).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "snapshot-strings", len, errno);
        errno
    })?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            exec_path.as_ptr(),
            runtime.posix_shm as *mut u8,
            path_len,
        );
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "translate-path", len, errno);
        errno
    })?;
    copy_current_path_to_client_shm(runtime, vfs_len).map_err(|errno| {
        log_execve_stage_error(runtime, pid, "copy-path", vfs_len, errno);
        errno
    })?;
    let loaded = match load_linux_elf_image(runtime, 0, vfs_len) {
        Ok(loaded) => loaded,
        Err(error) => {
            log_execve_load_error(runtime, pid, vfs_len, error);
            return Err(map_load_error(error));
        }
    };
    if loaded.metadata.has_interpreter {
        libnanami::println!(
            "[alter/linux] execve rejected dynamic ELF pid={} interp=PT_INTERP",
            pid
        );
        return Err(ENOEXEC);
    }
    let personality = runtime
        .managed_process(pid)
        .map(|process| process.personality)
        .ok_or(ESRCH)?;
    let preferred_image_base = preferred_exec_image_base(personality, &loaded.metadata);
    let exec_elf = current_elf_metadata_from_loaded(&loaded.metadata, preferred_image_base);
    close_cloexec_files(runtime, pid);
    if let Err(error) = libnanami::request_process_exec_memory(pid, loaded.address, loaded.size, 4)
    {
        let errno = map_request_error(error);
        libnanami::println!(
            "[alter/linux] execve exec-memory failed pid={} image_base={:#x} bytes={:#x} errno={}",
            pid,
            preferred_image_base,
            loaded.size,
            errno
        );
        return Err(errno);
    }
    if !runtime.reset_process_runtime_for_exec(pid) {
        return Err(ESRCH);
    }
    let (base, base_len) = basename_in_bytes(&exec_path, path_len);
    if !runtime.set_process_image_name(pid, &exec_path[base..base + base_len]) {
        return Err(ENOMEM);
    }
    if let Err(errno) = rewrite_linux_stack(
        runtime,
        pid,
        &exec_path,
        path_len,
        &snapshot,
        preferred_image_base,
        Some(exec_elf),
    ) {
        libnanami::println!(
            "[alter/linux] execve stack rewrite failed pid={} image_base={:#x} errno={}",
            pid,
            preferred_image_base,
            errno
        );
        return Err(errno);
    }
    Ok(())
}

fn log_execve_stage_error(runtime: &Runtime, pid: Word, stage: &str, path_len: Word, errno: i32) {
    libnanami::println!(
        "[alter/linux] execve failed pid={} stage={} path-len={} client-shm={:#x} posix-shm={:#x} errno={}",
        pid,
        stage,
        path_len,
        runtime.client_shm_size,
        runtime.posix_shm_size,
        errno
    );
}

fn sys_access(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    if current_virtual_node(runtime, pid, len).is_some() {
        return Ok(0);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_path_request_error)
}

fn sys_faccessat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    _mode: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    if current_virtual_node(runtime, pid, len).is_some() {
        return Ok(0);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_path_request_error)
}

fn sys_chdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let mut guest_path = [0u8; LINUX_CWD_MAX];
    if len as usize >= guest_path.len() {
        return Err(ENAMETOOLONG);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            guest_path.as_mut_ptr(),
            len as usize,
        );
    }
    if let Some(node) = current_virtual_node(runtime, pid, len) {
        if !node.is_directory() {
            return Err(ENOTDIR);
        }
        return if runtime.set_cwd(pid, &guest_path[..len as usize]) {
            Ok(0)
        } else {
            Err(EINVAL)
        };
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let stat = posix::posix_stat(runtime.posix_port, 0, vfs_len).map_err(map_path_request_error)?;
    if stat.2 != posix::POSIX_FILE_TYPE_DIRECTORY {
        return Err(ENOTDIR);
    }
    let path = &guest_path[..len as usize];
    if runtime.set_cwd(pid, path) {
        Ok(0)
    } else {
        Err(EINVAL)
    }
}

fn sys_mkdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_mkdir(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_create_request_error)
}

fn sys_mkdirat(runtime: &mut Runtime, pid: Word, dirfd: Word, path_ptr: Word) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_mkdir(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_create_request_error)
}

fn sys_mknod(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    mode: Word,
    dev: Word,
) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    mknod_current_path(runtime, pid, len, mode, dev)
}

fn sys_mknodat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    mode: Word,
    dev: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    mknod_current_path(runtime, pid, len, mode, dev)
}

fn mknod_current_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    mode: Word,
    _dev: Word,
) -> Result<Word, i32> {
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let file_type = mode & LINUX_S_IFMT;
    if file_type == LINUX_S_IFCHR || file_type == LINUX_S_IFBLK {
        return Err(ENOSYS);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    let fd = posix::posix_open(
        runtime.posix_port,
        0,
        vfs_len,
        posix::POSIX_O_CREAT | posix::POSIX_O_TRUNC,
    )
    .map_err(map_create_request_error)?;
    let _ = posix::posix_close(runtime.posix_port, fd);
    Ok(0)
}

fn sys_unlink(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    map_unit(posix::posix_unlink(runtime.posix_port, 0, vfs_len), 0)
}

fn sys_unlinkat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    if (flags & LINUX_AT_REMOVEDIR) != 0 {
        return map_unit(posix::posix_rmdir(runtime.posix_port, 0, vfs_len), 0);
    }
    map_unit(posix::posix_unlink(runtime.posix_port, 0, vfs_len), 0)
}

fn sys_rmdir(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, len)?;
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    map_unit(posix::posix_rmdir(runtime.posix_port, 0, vfs_len), 0)
}

fn sys_rename(
    runtime: &mut Runtime,
    pid: Word,
    old_path_ptr: Word,
    new_path_ptr: Word,
) -> Result<Word, i32> {
    let old_len = resolve_path(runtime, pid, old_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, ALTER_IO_OFFSET as Word, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, ALTER_IO_OFFSET as Word, old_len)?;
    let new_len = resolve_path(runtime, pid, new_path_ptr)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_rename(
        runtime.posix_port,
        ALTER_IO_OFFSET as Word,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_request_error)
}

fn sys_renameat(
    runtime: &mut Runtime,
    pid: Word,
    old_dirfd: Word,
    old_path_ptr: Word,
    new_dirfd: Word,
    new_path_ptr: Word,
) -> Result<Word, i32> {
    let old_raw_len = read_c_string(runtime, pid, old_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, old_raw_len) && !is_at_fdcwd(old_dirfd) {
        return Err(ENOSYS);
    }
    let old_len = resolve_current_shm_path(runtime, pid, old_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, old_len)?;
    move_shm_bytes(runtime, 0, ALTER_IO_OFFSET as Word, old_len);
    let old_vfs_len = translate_guest_path_at(runtime, pid, ALTER_IO_OFFSET as Word, old_len)?;
    let new_raw_len = read_c_string(runtime, pid, new_path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, new_raw_len) && !is_at_fdcwd(new_dirfd) {
        return Err(ENOSYS);
    }
    let new_len = resolve_current_shm_path(runtime, pid, new_raw_len)?;
    reject_virtual_fs_mutation(runtime.posix_shm, new_len)?;
    let new_vfs_len = translate_guest_path_for_vfs(runtime, pid, new_len)?;
    posix::posix_rename(
        runtime.posix_port,
        ALTER_IO_OFFSET as Word,
        old_vfs_len,
        0,
        new_vfs_len,
    )
    .map(|_| 0)
    .map_err(map_request_error)
}

fn sys_readlink(
    runtime: &mut Runtime,
    pid: Word,
    path_ptr: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let path_len = resolve_path(runtime, pid, path_ptr)?;
    readlink_from_current_path(runtime, pid, path_len, user_buffer, len)
}

fn sys_readlinkat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let path_len = resolve_current_shm_path(runtime, pid, raw_len)?;
    readlink_from_current_path(runtime, pid, path_len, user_buffer, len)
}

fn readlink_from_current_path(
    runtime: &mut Runtime,
    pid: Word,
    path_len: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    if path_equals(runtime.posix_shm, path_len, b"/proc/self/exe") {
        let Some(process) = runtime.managed_process(pid) else {
            return Err(ESRCH);
        };
        let bytes = ::core::cmp::min(process.image_name_len as Word, len);
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                process.image_name.as_ptr(),
                runtime.posix_shm as *mut u8,
                bytes as usize,
            );
        }
        write_target_memory(runtime, pid, user_buffer, bytes)?;
        return Ok(bytes);
    }
    Err(EINVAL)
}

fn sys_utime_path(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    if path_ptr == 0 {
        return Ok(0);
    }
    let len = resolve_path(runtime, pid, path_ptr)?;
    if current_virtual_node(runtime, pid, len).is_some() {
        return Ok(0);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len)
        .map(|_| 0)
        .map_err(map_path_request_error)
}

fn sys_getcwd(runtime: &mut Runtime, pid: Word, user_buffer: Word, len: Word) -> Result<Word, i32> {
    if user_buffer == 0 || len == 0 {
        return Err(EINVAL);
    }
    let max_len = bounded_len(runtime, len)?;
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    let bytes = process.cwd_len as Word;
    if bytes + 1 > max_len {
        return Err(ERANGE);
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            process.cwd.as_ptr(),
            runtime.posix_shm as *mut u8,
            process.cwd_len,
        );
        ::core::ptr::write((runtime.posix_shm + bytes) as *mut u8, 0);
    }
    let total = bytes + 1;
    write_target_memory(runtime, pid, user_buffer, total)?;
    Ok(total)
}

fn sys_stat(runtime: &mut Runtime, pid: Word, path_ptr: Word, stat_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let stat = stat_current_path(runtime, pid, len)?;
    write_linux_stat(runtime, pid, stat_ptr, stat)?;
    Ok(0)
}

fn sys_chown(runtime: &mut Runtime, pid: Word, path_ptr: Word) -> Result<Word, i32> {
    let len = resolve_path(runtime, pid, path_ptr)?;
    let _ = stat_current_path(runtime, pid, len)?;
    Ok(0)
}

fn sys_fchown(runtime: &Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    runtime.linux_file(pid, fd).ok_or(EBADF)?;
    Ok(0)
}

fn sys_fchownat(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
) -> Result<Word, i32> {
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if raw_len == 0 && (flags & LINUX_AT_EMPTY_PATH) != 0 {
        return sys_fchown(runtime, pid, dirfd);
    }
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(ENOSYS);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    let _ = stat_current_path(runtime, pid, len)?;
    Ok(0)
}

fn sys_statx(
    runtime: &mut Runtime,
    pid: Word,
    dirfd: Word,
    path_ptr: Word,
    flags: Word,
    statx_ptr: Word,
) -> Result<Word, i32> {
    if statx_ptr == 0 {
        return Err(EFAULT);
    }
    let raw_len = read_c_string(runtime, pid, path_ptr)?;
    if raw_len == 0 && (flags & LINUX_AT_EMPTY_PATH) != 0 {
        let file = runtime.linux_file(pid, dirfd).ok_or(EBADF)?;
        let stat = match file.kind {
            LinuxFileKind::Terminal => (0, 0, posix::POSIX_FILE_TYPE_CHAR_DEVICE, 5, 0),
            LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => {
                (0, 0, POSIX_FILE_TYPE_PIPE, 0, 0)
            }
            LinuxFileKind::Posix => {
                posix::posix_fstat(runtime.posix_port, file.posix_fd).map_err(map_request_error)?
            }
            LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink => (0, 0, POSIX_FILE_TYPE_SOCKET, 0, 0),
            LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer => virtual_node_stat(runtime, pid, file)?,
            LinuxFileKind::Empty => return Err(EBADF),
        };
        write_linux_statx(runtime, pid, statx_ptr, stat)?;
        return Ok(0);
    }
    if !path_is_absolute(runtime.posix_shm, raw_len) && !is_at_fdcwd(dirfd) {
        return Err(EINVAL);
    }
    let len = resolve_current_shm_path(runtime, pid, raw_len)?;
    let stat = stat_current_path(runtime, pid, len)?;
    write_linux_statx(runtime, pid, statx_ptr, stat)?;
    Ok(0)
}

fn stat_current_path(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    if let Some(node) = virtual_fs::lookup(path, graphics_enabled(runtime, pid)) {
        return virtual_node_stat_from_node(runtime, pid, node);
    }
    if is_linux_virtual_path(runtime.posix_shm, len) {
        return Err(ENOENT);
    }
    let vfs_len = translate_guest_path_for_vfs(runtime, pid, len)?;
    posix::posix_stat(runtime.posix_port, 0, vfs_len).map_err(map_path_request_error)
}

fn sys_fstat(runtime: &mut Runtime, pid: Word, fd: Word, stat_ptr: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::Terminal {
        write_linux_stat(
            runtime,
            pid,
            stat_ptr,
            (0, 0, posix::POSIX_FILE_TYPE_CHAR_DEVICE, 5, 0),
        )?;
        return Ok(0);
    }
    if file.kind == LinuxFileKind::PipeRead || file.kind == LinuxFileKind::PipeWrite {
        write_linux_pipe_stat(runtime, pid, stat_ptr)?;
        return Ok(0);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer
    ) {
        let stat = virtual_node_stat(runtime, pid, file)?;
        write_linux_stat(runtime, pid, stat_ptr, stat)?;
        return Ok(0);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(EBADF);
    }
    let stat = posix::posix_fstat(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
    write_linux_stat(runtime, pid, stat_ptr, stat)?;
    Ok(0)
}

fn virtual_node_stat(
    runtime: &Runtime,
    pid: Word,
    file: LinuxFile,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let node = if file.kind == LinuxFileKind::Framebuffer {
        VirtualNode::DevFramebuffer
    } else {
        VirtualNode::from_id(file.resource & 0xffff_ffff).ok_or(EBADF)?
    };
    virtual_node_stat_from_node(runtime, pid, node)
}

fn virtual_node_stat_from_node(
    runtime: &Runtime,
    pid: Word,
    node: VirtualNode,
) -> Result<(Word, Word, Word, Word, Word), i32> {
    let (size, major, minor) = match node {
        VirtualNode::DevNull => (0, 1, 3),
        VirtualNode::DevZero => (0, 1, 5),
        VirtualNode::DevTty => (0, 5, 0),
        VirtualNode::DevKeyboard => (0, 13, 64),
        VirtualNode::DevMouse => (0, 13, 65),
        VirtualNode::DevFramebuffer => {
            if !graphics_enabled(runtime, pid) {
                return Err(ENOENT);
            }
            (ALTER_FB_BYTES, 29, 0)
        }
        VirtualNode::ProcSelfExe => runtime
            .managed_process(pid)
            .map(|process| (process.image_name_len as Word, 0, 0))
            .ok_or(ESRCH)?,
        _ => (
            virtual_fs::static_file(node)
                .map(|bytes| bytes.len() as Word)
                .unwrap_or(0),
            0,
            0,
        ),
    };
    Ok((
        node.id(),
        size,
        if node.is_directory() {
            posix::POSIX_FILE_TYPE_DIRECTORY
        } else if node.is_regular_file() {
            posix::POSIX_FILE_TYPE_REGULAR
        } else {
            posix::POSIX_FILE_TYPE_CHAR_DEVICE
        },
        major,
        minor,
    ))
}

fn sys_lseek(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    offset: Word,
    whence: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::PipeRead
        || file.kind == LinuxFileKind::PipeWrite
        || file.kind == LinuxFileKind::Terminal
    {
        return Err(ESPIPE);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualFile | LinuxFileKind::Framebuffer
    ) {
        let end = virtual_node_stat(runtime, pid, file)?.1;
        let next = match whence {
            0 => offset,
            1 => file.offset.checked_add(offset).ok_or(EINVAL)?,
            2 => end.checked_add(offset).ok_or(EINVAL)?,
            _ => return Err(EINVAL),
        };
        file.offset = next;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(next);
    }
    if matches!(
        file.kind,
        LinuxFileKind::VirtualDirectory | LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse
    ) {
        return Err(ESPIPE);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(EBADF);
    }
    map_word(posix::posix_seek(
        runtime.posix_port,
        file.posix_fd,
        offset,
        whence,
    ))
}

fn write_linux_pipe_stat(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_STAT_SIZE);
        write_u64(runtime.posix_shm, 0);
        write_u64(runtime.posix_shm + 8, 0);
        write_u64(runtime.posix_shm + 16, 1);
        write_u32(runtime.posix_shm + 24, (LINUX_S_IFIFO | 0o600) as u32);
        write_u64(runtime.posix_shm + 56, 4096);
    }
    write_target_memory(runtime, pid, user_ptr, LINUX_STAT_SIZE as Word)
}

fn sys_uname(runtime: &mut Runtime, pid: Word, user_buffer: Word) -> Result<Word, i32> {
    if user_buffer == 0 {
        return Err(EFAULT);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, 390);
    }
    write_uts_field(runtime.posix_shm, 0, b"Nanami Alter/Linux");
    write_uts_field(runtime.posix_shm, 65, b"nanami");
    write_uts_field(runtime.posix_shm, 130, b"0.1.0");
    write_uts_field(runtime.posix_shm, 195, b"#1 Nanami/A9N");
    write_uts_field(runtime.posix_shm, 260, b"x86_64");
    write_uts_field(runtime.posix_shm, 325, b"(none)");
    write_target_memory(runtime, pid, user_buffer, 390)?;
    Ok(0)
}

fn sys_brk(runtime: &mut Runtime, pid: Word, requested: Word) -> Result<Word, i32> {
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    if process.program_break == 0 {
        let (base, mapped) =
            map_anonymous_tracked(runtime, pid, 4096, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        let Some(process) = runtime.managed_process_mut(pid) else {
            return Err(ESRCH);
        };
        process.program_break = base;
        process.mapped_break = base + mapped;
    }
    let current = runtime
        .managed_process(pid)
        .map(|process| process.program_break)
        .ok_or(ESRCH)?;
    if requested == 0 || requested <= current {
        return Ok(current);
    }
    let mapped = runtime
        .managed_process(pid)
        .map(|process| process.mapped_break)
        .ok_or(ESRCH)?;
    if requested > mapped {
        let extra = align_up_word(requested - mapped, 4096);
        let (base, size) =
            map_anonymous_tracked(runtime, pid, extra, LINUX_PROT_READ | LINUX_PROT_WRITE)?;
        if base != mapped {
            return Ok(current);
        }
        let Some(process) = runtime.managed_process_mut(pid) else {
            return Err(ESRCH);
        };
        process.mapped_break = mapped + size;
    }
    let Some(process) = runtime.managed_process_mut(pid) else {
        return Err(ESRCH);
    };
    process.program_break = requested;
    Ok(requested)
}

fn sys_mmap(
    runtime: &mut Runtime,
    pid: Word,
    requested_addr: Word,
    len: Word,
    _prot: Word,
    flags: Word,
    fd: Word,
    offset: Word,
) -> Result<Word, i32> {
    if len == 0 {
        return Err(EINVAL);
    }
    if (_prot & !LINUX_PROT_ALL) != 0 {
        return Err(EINVAL);
    }
    if (fd as isize) >= 0 && (flags & LINUX_MAP_ANONYMOUS) == 0 {
        let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
        return match file.kind {
            LinuxFileKind::Framebuffer => {
                if requested_addr != 0 && (flags & LINUX_MAP_FIXED) != 0 {
                    return Err(EINVAL);
                }
                sys_framebuffer_mmap(runtime, pid, file, len, _prot, offset)
            }
            _ => Err(ENOSYS),
        };
    }
    if requested_addr != 0 && (flags & LINUX_MAP_FIXED) != 0 {
        let fixed_base = requested_addr & !(LINUX_PAGE_SIZE - 1);
        let fixed_offset = requested_addr - fixed_base;
        let mapped = align_up_word(
            len.checked_add(fixed_offset).ok_or(ENOMEM)?,
            LINUX_PAGE_SIZE,
        );
        if runtime.has_mapping(pid, fixed_base, mapped) {
            sys_munmap(runtime, pid, fixed_base, mapped)?;
        }
        if _prot == LINUX_PROT_NONE {
            if !runtime.add_mapping(pid, fixed_base, mapped, _prot) {
                return Err(ENOMEM);
            }
            return Ok(requested_addr);
        }
        let (base, granted) = libnanami::request_process_map_anonymous_at(pid, fixed_base, mapped)
            .map_err(|err| {
                let errno = map_request_error(err);
                libnanami::println!(
                    "[alter/linux] mmap fixed failed pid={} addr={:#x} base={:#x} len={:#x} mapped={:#x} prot={:#x} flags={:#x} fd={:#x} off={:#x} errno={}",
                    pid,
                    requested_addr,
                    fixed_base,
                    len,
                    mapped,
                    _prot,
                    flags,
                    fd,
                    offset,
                    errno
                );
                errno
            })?;
        if base != fixed_base || granted < mapped || !runtime.add_mapping(pid, base, granted, _prot)
        {
            return Err(ENOMEM);
        }
        return Ok(requested_addr);
    }
    if _prot == LINUX_PROT_NONE {
        let (base, _mapped) = reserve_none_mapping(runtime, pid, len)?;
        return Ok(base);
    }

    let (base, _) = map_anonymous_tracked(runtime, pid, len, _prot).map_err(|errno| {
        libnanami::println!(
            "[alter/linux] mmap failed pid={} addr={:#x} len={:#x} prot={:#x} flags={:#x} fd={:#x} off={:#x} errno={}",
            pid,
            requested_addr,
            len,
            _prot,
            flags,
            fd,
            offset,
            errno
        );
        errno
    })?;
    Ok(base)
}

fn sys_munmap(runtime: &mut Runtime, pid: Word, addr: Word, len: Word) -> Result<Word, i32> {
    if addr == 0 || len == 0 || (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let framebuffer_index = runtime.graphics.iter().position(|session| {
        session.active
            && session.guest_pid == pid
            && session.guest_framebuffer == addr
            && align_up_word(session.guest_framebuffer_bytes, LINUX_PAGE_SIZE) == mapped
    });
    if let Some(index) = framebuffer_index {
        let _ = present_graphics_session(runtime, index as Word + 1, pid);
    }
    if !runtime.has_mapping(pid, addr, mapped) {
        return Err(EINVAL);
    }
    if runtime.mapping_prot(pid, addr, mapped) != Some(LINUX_PROT_NONE) {
        release_present_mapping_pages(runtime, pid, addr, mapped)?;
    }
    if !runtime.remove_mapping(pid, addr, mapped) {
        return Err(EINVAL);
    }
    if let Some(index) = framebuffer_index {
        let session = runtime.graphics[index];
        honoka::honoka_detach_logical_framebuffer(session.honoka_port, session.window_id)
            .map_err(map_request_error)?;
        runtime.graphics[index].guest_pid = 0;
        runtime.graphics[index].guest_framebuffer = 0;
        runtime.graphics[index].guest_framebuffer_bytes = 0;
        runtime.graphics[index].framebuffer = 0;
        runtime.graphics[index].framebuffer_bytes = ALTER_FB_BYTES;
    }
    Ok(0)
}

fn sys_msync(runtime: &mut Runtime, pid: Word, addr: Word, len: Word) -> Result<Word, i32> {
    if addr == 0 || len == 0 {
        return Err(EINVAL);
    }
    let mut index = 0usize;
    while index < runtime.graphics.len() {
        let session = runtime.graphics[index];
        if session.active
            && session.guest_pid == pid
            && session.guest_framebuffer != 0
            && runtime
                .managed_process(pid)
                .map(|process| process.graphics_session == index as Word + 1)
                .unwrap_or(false)
            && addr >= session.guest_framebuffer
            && addr.saturating_add(len)
                <= session
                    .guest_framebuffer
                    .saturating_add(session.guest_framebuffer_bytes)
        {
            present_graphics_session(runtime, index as Word + 1, pid)?;
            return Ok(0);
        }
        index += 1;
    }
    Ok(0)
}

fn sys_mprotect(
    runtime: &mut Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    prot: Word,
) -> Result<Word, i32> {
    if addr == 0 || len == 0 || (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if (prot & !LINUX_PROT_ALL) != 0 {
        return Err(EINVAL);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    if !runtime.has_mapping(pid, addr, mapped) {
        return Err(ENOMEM);
    }
    let old_prot = runtime.mapping_prot(pid, addr, mapped).unwrap_or(Word::MAX);
    if old_prot == prot {
        return Ok(0);
    }
    if prot == LINUX_PROT_NONE {
        release_present_mapping_pages(runtime, pid, addr, mapped)?;
    } else {
        ensure_present_mapping_pages(runtime, pid, addr, mapped)?;
    }
    if !runtime.protect_mapping(pid, addr, mapped, prot) {
        return Err(ENOMEM);
    }
    Ok(0)
}

fn sys_madvise(
    runtime: &Runtime,
    pid: Word,
    addr: Word,
    len: Word,
    advice: Word,
) -> Result<Word, i32> {
    if (addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if len == 0 {
        return Ok(0);
    }
    if !matches!(
        advice,
        LINUX_MADV_NORMAL
            | LINUX_MADV_RANDOM
            | LINUX_MADV_SEQUENTIAL
            | LINUX_MADV_WILLNEED
            | LINUX_MADV_DONTNEED
            | LINUX_MADV_FREE
            | LINUX_MADV_MERGEABLE
            | LINUX_MADV_UNMERGEABLE
            | LINUX_MADV_HUGEPAGE
            | LINUX_MADV_NOHUGEPAGE
            | LINUX_MADV_DONTDUMP
            | LINUX_MADV_DODUMP
            | LINUX_MADV_COLD
            | LINUX_MADV_PAGEOUT
            | LINUX_MADV_POPULATE_READ
            | LINUX_MADV_POPULATE_WRITE
    ) {
        return Err(EINVAL);
    }
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    if !runtime.has_mapping(pid, addr, mapped) {
        return Err(ENOMEM);
    }

    // Nanami currently keeps anonymous mappings resident. These Linux advice
    // values are optimization hints, so preserving the mapping is compatible.
    Ok(0)
}

fn reserve_none_mapping(runtime: &mut Runtime, pid: Word, len: Word) -> Result<(Word, Word), i32> {
    let mapped = len.checked_add(LINUX_PAGE_SIZE - 1).ok_or(ENOMEM)? & !(LINUX_PAGE_SIZE - 1);
    let process = runtime.managed_process(pid).ok_or(ESRCH)?;
    let mut base = LINUX_VIRTUAL_RESERVATION_BASE;

    loop {
        let end = base.checked_add(mapped).ok_or(ENOMEM)?;
        if end > LINUX_VIRTUAL_RESERVATION_LIMIT {
            return Err(ENOMEM);
        }

        let mut conflict_end = base;
        let mut i = 0usize;
        while i < process.mappings.len() {
            let mapping = process.mappings[i];
            if mapping.base != 0 {
                let mapping_end = mapping.base.checked_add(mapping.size).ok_or(ENOMEM)?;
                if base < mapping_end && mapping.base < end {
                    conflict_end = ::core::cmp::max(conflict_end, mapping_end);
                }
            }
            i += 1;
        }
        if conflict_end == base {
            if !runtime.add_mapping(pid, base, mapped, LINUX_PROT_NONE) {
                return Err(ENOMEM);
            }
            return Ok((base, mapped));
        }
        base = conflict_end
            .checked_add(LINUX_PAGE_SIZE - 1)
            .ok_or(ENOMEM)?
            & !(LINUX_PAGE_SIZE - 1);
    }
}

fn release_present_mapping_pages(
    runtime: &Runtime,
    pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut cursor = base;
    let end = base.checked_add(size).ok_or(ENOMEM)?;
    while cursor < end {
        if runtime.mapping_prot(pid, cursor, LINUX_PAGE_SIZE) != Some(LINUX_PROT_NONE) {
            libnanami::request_process_mapping_release(pid, cursor, LINUX_PAGE_SIZE)
                .map_err(map_request_error)?;
        }
        cursor += LINUX_PAGE_SIZE;
    }
    Ok(())
}

fn ensure_present_mapping_pages(
    runtime: &Runtime,
    pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut cursor = base;
    let end = base.checked_add(size).ok_or(ENOMEM)?;
    while cursor < end {
        if runtime.mapping_prot(pid, cursor, LINUX_PAGE_SIZE) == Some(LINUX_PROT_NONE) {
            let (mapped_base, mapped) =
                libnanami::request_process_map_anonymous_at(pid, cursor, LINUX_PAGE_SIZE)
                    .map_err(map_request_error)?;
            if mapped_base != cursor || mapped < LINUX_PAGE_SIZE {
                return Err(ENOMEM);
            }
        }
        cursor += LINUX_PAGE_SIZE;
    }
    Ok(())
}

fn sys_mremap(
    runtime: &mut Runtime,
    pid: Word,
    old_addr: Word,
    old_size: Word,
    new_size: Word,
    flags: Word,
    new_addr: Word,
) -> Result<Word, i32> {
    if old_addr == 0 || old_size == 0 || new_size == 0 || (old_addr & (LINUX_PAGE_SIZE - 1)) != 0 {
        return Err(EINVAL);
    }
    if (flags & !LINUX_MREMAP_SUPPORTED_FLAGS) != 0 {
        return Err(EINVAL);
    }
    if (flags & LINUX_MREMAP_FIXED) != 0
        && ((flags & LINUX_MREMAP_MAYMOVE) == 0
            || new_addr == 0
            || (new_addr & (LINUX_PAGE_SIZE - 1)) != 0)
    {
        return Err(EINVAL);
    }

    let old_mapped = align_up_word(old_size, LINUX_PAGE_SIZE);
    let new_mapped = align_up_word(new_size, LINUX_PAGE_SIZE);
    let prot = runtime
        .mapping_prot(pid, old_addr, old_mapped)
        .ok_or(EINVAL)?;

    if new_mapped == old_mapped {
        return Ok(old_addr);
    }
    if new_mapped < old_mapped {
        sys_munmap(runtime, pid, old_addr + new_mapped, old_mapped - new_mapped)?;
        return Ok(old_addr);
    }

    let old_end = old_addr.checked_add(old_mapped).ok_or(ENOMEM)?;
    let extra = new_mapped - old_mapped;
    if (flags & LINUX_MREMAP_FIXED) == 0 {
        if let Ok((base, granted)) =
            libnanami::request_process_map_anonymous_at(pid, old_end, extra)
                .map_err(map_request_error)
        {
            if base == old_end && granted >= extra && runtime.add_mapping(pid, base, granted, prot)
            {
                return Ok(old_addr);
            }
        }
    }

    if (flags & LINUX_MREMAP_MAYMOVE) == 0 {
        return Err(ENOMEM);
    }

    let target = if (flags & LINUX_MREMAP_FIXED) != 0 {
        sys_mmap(
            runtime,
            pid,
            new_addr,
            new_mapped,
            prot,
            LINUX_MAP_FIXED | LINUX_MAP_ANONYMOUS,
            !0,
            0,
        )?
    } else {
        sys_mmap(
            runtime,
            pid,
            0,
            new_mapped,
            prot,
            LINUX_MAP_ANONYMOUS,
            !0,
            0,
        )?
    };
    copy_same_process_range(
        runtime,
        pid,
        old_addr,
        target,
        ::core::cmp::min(old_mapped, new_mapped),
    )?;
    sys_munmap(runtime, pid, old_addr, old_mapped)?;
    Ok(target)
}

fn sys_arch_prctl(runtime: &mut Runtime, pid: Word, code: Word, value: Word) -> Result<Word, i32> {
    match code {
        ARCH_SET_FS => {
            let pcb = runtime
                .managed_process(pid)
                .map(|process| process.pcb)
                .ok_or(ESRCH)?;
            write_register_value(pcb, REG_FS_BASE, value).map_err(|_| EIO)?;
            if !runtime.set_fs_base(pid, value) {
                return Err(ESRCH);
            }
            Ok(0)
        }
        ARCH_GET_FS => {
            let pcb = runtime
                .managed_process(pid)
                .map(|process| process.pcb)
                .ok_or(ESRCH)?;
            let fs_base = read_register_value(pcb, REG_FS_BASE).unwrap_or_else(|_| {
                runtime
                    .managed_process(pid)
                    .map(|process| process.fs_base)
                    .unwrap_or(0)
            });
            write_guest_u64(runtime, pid, value, fs_base)?;
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}

fn sys_wait4(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> EmulationAction {
    let target_pid = context.args[0];
    let status_ptr = context.args[1];
    let options = context.args[2];
    if (options & !LINUX_WNOHANG) != 0 {
        return EmulationAction::Return(-(EINVAL as isize));
    }
    if let Some((child_pid, exit_status)) = runtime.exited_child(pid, target_pid) {
        if status_ptr != 0 {
            if write_u32_to_target(runtime, pid, status_ptr, ((exit_status & 0xff) << 8) as u32)
                .is_err()
            {
                return EmulationAction::Return(-(EFAULT as isize));
            }
        }
        if let Err(error) = libnanami::request_process_reap(child_pid) {
            let errno = map_request_error(error);
            libnanami::println!(
                "[alter/linux] wait4 reap failed parent={} child={} errno={}",
                pid,
                child_pid,
                errno
            );
            return EmulationAction::Return(-(errno as isize));
        }
        close_process_files(runtime, child_pid);
        runtime.remove_process(child_pid);
        return EmulationAction::Return(child_pid as isize);
    }
    if runtime.has_child(pid, target_pid) {
        if (options & LINUX_WNOHANG) != 0 {
            return EmulationAction::Return(0);
        }
        if runtime.park_signal_waiter(pid, target_pid, context) {
            return EmulationAction::Park;
        }
        return EmulationAction::Return(-(ESRCH as isize));
    }
    EmulationAction::Return(-(ECHILD as isize))
}

fn sys_poll(runtime: &mut Runtime, pid: Word, pollfds: Word, nfds: Word) -> Result<Word, i32> {
    if pollfds == 0 && nfds != 0 {
        return Err(EFAULT);
    }
    let count = ::core::cmp::min(nfds, LINUX_POLLFD_MAX);
    let bytes = count.checked_mul(LINUX_POLLFD_BYTES).ok_or(EINVAL)?;
    read_target_memory(runtime, pid, pollfds, bytes)?;
    let mut ready = 0;
    let mut index = 0;
    while index < count {
        let entry = runtime.posix_shm + index * LINUX_POLLFD_BYTES;
        let fd = unsafe { ::core::ptr::read_unaligned(entry as *const i32) };
        let events = unsafe { ::core::ptr::read_unaligned((entry + 4) as *const i16) };
        let revents = linux_poll_revents(runtime, pid, fd, events);
        if revents != 0 {
            ready += 1;
        }
        unsafe {
            ::core::ptr::write_unaligned((entry + 6) as *mut i16, revents);
        }
        index += 1;
    }
    write_target_memory(runtime, pid, pollfds, bytes)?;
    Ok(ready)
}

fn linux_poll_revents(runtime: &mut Runtime, pid: Word, fd: i32, events: i16) -> i16 {
    if fd < 0 {
        return LINUX_POLLNVAL;
    }
    let Some(file) = runtime.linux_file(pid, fd as Word) else {
        return LINUX_POLLNVAL;
    };
    match file.kind {
        LinuxFileKind::Terminal => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::Posix => {
            if (events & LINUX_POLLIN) != 0 {
                LINUX_POLLIN
            } else if (events & LINUX_POLLOUT) != 0 {
                LINUX_POLLOUT
            } else {
                0
            }
        }
        LinuxFileKind::PipeRead => {
            let pipe = runtime.pipe(file.posix_fd);
            if (events & LINUX_POLLIN) != 0
                && pipe
                    .map(|pipe| pipe.len != 0 || pipe.writers == 0)
                    .unwrap_or(false)
            {
                LINUX_POLLIN
            } else {
                0
            }
        }
        LinuxFileKind::PipeWrite => {
            let pipe = runtime.pipe(file.posix_fd);
            if (events & LINUX_POLLOUT) != 0
                && pipe
                    .map(|pipe| pipe.readers != 0 && pipe.len < LINUX_PIPE_BYTES)
                    .unwrap_or(false)
            {
                LINUX_POLLOUT
            } else {
                0
            }
        }
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 && file.kind != LinuxFileKind::SocketTcpListener {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::VirtualDirectory | LinuxFileKind::VirtualFile => {
            let mut revents = 0;
            if (events & LINUX_POLLIN) != 0 {
                revents |= LINUX_POLLIN;
            }
            if (events & LINUX_POLLOUT) != 0 && file.kind == LinuxFileKind::VirtualFile {
                revents |= LINUX_POLLOUT;
            }
            revents
        }
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            pump_input_events(runtime);
            let session_id = file.resource >> 32;
            let ready = match file.kind {
                LinuxFileKind::EvdevKeyboard => keyboard_event_ready(runtime, session_id),
                LinuxFileKind::EvdevMouse => mouse_event_ready(runtime, session_id),
                _ => false,
            };
            if ready && (events & LINUX_POLLIN) != 0 {
                LINUX_POLLIN
            } else {
                0
            }
        }
        LinuxFileKind::Framebuffer => events & (LINUX_POLLIN | LINUX_POLLOUT),
        LinuxFileKind::Empty => LINUX_POLLNVAL,
    }
}

fn sys_select(
    runtime: &mut Runtime,
    pid: Word,
    nfds: Word,
    readfds: Word,
    writefds: Word,
) -> Result<Word, i32> {
    if nfds == 0 {
        return Ok(0);
    }
    let mut ready = 0;
    if readfds != 0 {
        let mut bits = read_fdset_word(runtime, pid, readfds)?;
        let readable = readable_fdset_mask(runtime, pid, bits);
        bits = readable;
        if readable != 0 {
            ready += count_low_fd_bits(readable);
        }
        write_fdset_word(runtime, pid, readfds, bits)?;
    }
    if writefds != 0 {
        let requested = read_fdset_word(runtime, pid, writefds)?;
        let writable = writable_fdset_mask(runtime, pid, requested);
        if writable != 0 {
            ready += count_low_fd_bits(writable);
        }
        write_fdset_word(runtime, pid, writefds, writable)?;
    }
    Ok(ready)
}

fn read_fdset_word(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    read_target_memory(runtime, pid, user_ptr, 8)?;
    Ok(unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const Word) })
}

fn write_fdset_word(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: Word,
) -> Result<(), i32> {
    unsafe {
        write_u64(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 8)
}

fn count_low_fd_bits(bits: Word) -> Word {
    let mut count = 0;
    let mut bit = 0;
    while bit < 64 {
        if (bits & (1usize << bit)) != 0 {
            count += 1;
        }
        bit += 1;
    }
    count
}

fn readable_fdset_mask(runtime: &mut Runtime, pid: Word, requested: Word) -> Word {
    let mut out = 0;
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX && fd < 64 {
        let bit = 1usize << fd;
        if (requested & bit) != 0 {
            if let Some(file) = runtime.linux_file(pid, fd as Word) {
                match file.kind {
                    LinuxFileKind::Terminal => {
                        out |= bit;
                    }
                    LinuxFileKind::PipeRead => {
                        if runtime
                            .pipe(file.posix_fd)
                            .map(|pipe| pipe.len != 0 || pipe.writers == 0)
                            .unwrap_or(false)
                        {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::Posix => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualDirectory
                    | LinuxFileKind::VirtualFile
                    | LinuxFileKind::Framebuffer => {
                        out |= bit;
                    }
                    LinuxFileKind::EvdevKeyboard => {
                        pump_input_events(runtime);
                        if keyboard_event_ready(runtime, file.resource >> 32) {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::EvdevMouse => {
                        pump_input_events(runtime);
                        if mouse_event_ready(runtime, file.resource >> 32) {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::SocketUdp
                    | LinuxFileKind::SocketTcp
                    | LinuxFileKind::SocketTcpListener
                    | LinuxFileKind::SocketIcmp
                    | LinuxFileKind::SocketNetlink => {
                        out |= bit;
                    }
                    LinuxFileKind::PipeWrite => {}
                    LinuxFileKind::Empty => {}
                }
            }
        }
        fd += 1;
    }
    out
}

fn writable_fdset_mask(runtime: &Runtime, pid: Word, requested: Word) -> Word {
    let mut out = 0;
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX && fd < 64 {
        let bit = 1usize << fd;
        if (requested & bit) != 0 {
            if let Some(file) = runtime.linux_file(pid, fd as Word) {
                match file.kind {
                    LinuxFileKind::PipeWrite => {
                        if runtime
                            .pipe(file.posix_fd)
                            .map(|pipe| pipe.readers != 0 && pipe.len < LINUX_PIPE_BYTES)
                            .unwrap_or(false)
                        {
                            out |= bit;
                        }
                    }
                    LinuxFileKind::Empty | LinuxFileKind::PipeRead => {}
                    LinuxFileKind::Terminal | LinuxFileKind::Posix => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualFile | LinuxFileKind::Framebuffer => {
                        out |= bit;
                    }
                    LinuxFileKind::VirtualDirectory
                    | LinuxFileKind::EvdevKeyboard
                    | LinuxFileKind::EvdevMouse => {}
                    LinuxFileKind::SocketUdp
                    | LinuxFileKind::SocketTcp
                    | LinuxFileKind::SocketIcmp
                    | LinuxFileKind::SocketNetlink => {
                        out |= bit;
                    }
                    LinuxFileKind::SocketTcpListener => {}
                }
            }
        }
        fd += 1;
    }
    out
}

fn sys_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::EvdevKeyboard | LinuxFileKind::EvdevMouse => {
            return sys_evdev_ioctl(runtime, pid, file, request, argument);
        }
        LinuxFileKind::Framebuffer => {
            return sys_framebuffer_ioctl(runtime, pid, file, request, argument);
        }
        LinuxFileKind::Terminal => {}
        _ => return Err(ENOTTY),
    }
    if request == LINUX_TCGETS {
        if argument == 0 {
            return Err(EFAULT);
        }
        let (canonical, echo) = runtime
            .managed_process(pid)
            .map(|process| (process.terminal_canonical, process.terminal_echo))
            .unwrap_or((true, true));
        write_linux_termios(runtime.posix_shm, canonical, echo);
        write_target_memory(runtime, pid, argument, LINUX_TERMIOS_BYTES)?;
        return Ok(0);
    }
    if request == LINUX_TCSETS || request == LINUX_TCSETSW || request == LINUX_TCSETSF {
        if argument == 0 {
            return Err(EFAULT);
        }
        read_target_memory(runtime, pid, argument, LINUX_TERMIOS_BYTES)?;
        let lflag = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 12) as *const u32) };
        let echo_enabled = (lflag & LINUX_ECHO) != 0;
        let terminal_id = terminal_id_for_pid(runtime, pid)?;
        if let Some(process) = runtime.managed_process_mut(pid) {
            process.terminal_canonical = (lflag & LINUX_ICANON) != 0;
            process.terminal_echo = echo_enabled;
        }
        nanami_services::terminal::terminal_set_echo(
            runtime.terminal_port,
            terminal_id,
            echo_enabled,
        )
        .map_err(map_request_error)?;
        return Ok(0);
    }
    if request == LINUX_TIOCGWINSZ {
        if argument == 0 {
            return Err(EFAULT);
        }
        let terminal_id = terminal_id_for_pid(runtime, pid)?;
        let (columns, rows) =
            nanami_services::terminal::terminal_get_size(runtime.terminal_port, terminal_id)
                .map_err(map_request_error)?;
        unsafe {
            write_u16(runtime.posix_shm, rows as u16);
            write_u16(runtime.posix_shm + 2, columns as u16);
            write_u16(runtime.posix_shm + 4, 0);
            write_u16(runtime.posix_shm + 6, 0);
        }
        write_target_memory(runtime, pid, argument, 8)?;
        return Ok(0);
    }
    Ok(0)
}

fn sys_evdev_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    if argument == 0 {
        return Err(EFAULT);
    }
    if request == LINUX_EVIOCGVERSION {
        unsafe { write_u32(runtime.posix_shm, 0x0001_0001) };
        write_target_memory(runtime, pid, argument, 4)?;
        return Ok(0);
    }
    if request == LINUX_EVIOCGID {
        unsafe {
            write_u16(runtime.posix_shm, 0x0011);
            write_u16(runtime.posix_shm + 2, 0);
            write_u16(runtime.posix_shm + 4, 0);
            write_u16(runtime.posix_shm + 6, 1);
        }
        write_target_memory(runtime, pid, argument, 8)?;
        return Ok(0);
    }

    let direction = (request >> 30) & 0x3;
    let ioctl_type = (request >> 8) & 0xff;
    let number = request & 0xff;
    let requested_len = ((request >> 16) & 0x3fff).min(runtime.posix_shm_size);
    if direction == 2 && ioctl_type == 0x45 && number == 0x06 {
        let name = if file.kind == LinuxFileKind::EvdevKeyboard {
            b"Nanami PS/2 Keyboard\0" as &[u8]
        } else {
            b"Nanami PS/2 Mouse\0" as &[u8]
        };
        let bytes = requested_len.min(name.len() as Word);
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                name.as_ptr(),
                runtime.posix_shm as *mut u8,
                bytes as usize,
            );
        }
        write_target_memory(runtime, pid, argument, bytes)?;
        return Ok(bytes);
    }
    if direction == 2 && ioctl_type == 0x45 && (0x20..=0x3f).contains(&number) {
        let bytes = requested_len;
        unsafe { ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, bytes as usize) };
        let event_type = number - 0x20;
        if event_type == 0 && bytes != 0 {
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_SYN as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_KEY as Word);
            if file.kind == LinuxFileKind::EvdevMouse {
                set_bitmap_bit(runtime.posix_shm, bytes, LINUX_EV_REL as Word);
            }
        } else if event_type == LINUX_EV_KEY as Word {
            if file.kind == LinuxFileKind::EvdevKeyboard {
                let mut bit = 0;
                while bit <= 0xff {
                    set_bitmap_bit(runtime.posix_shm, bytes, bit);
                    bit += 1;
                }
            } else {
                set_bitmap_bit(runtime.posix_shm, bytes, 0x110);
                set_bitmap_bit(runtime.posix_shm, bytes, 0x111);
                set_bitmap_bit(runtime.posix_shm, bytes, 0x112);
            }
        } else if event_type == LINUX_EV_REL as Word && file.kind == LinuxFileKind::EvdevMouse {
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_X as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_Y as Word);
            set_bitmap_bit(runtime.posix_shm, bytes, LINUX_REL_WHEEL as Word);
        }
        write_target_memory(runtime, pid, argument, bytes)?;
        return Ok(bytes);
    }
    Err(ENOTTY)
}

fn set_bitmap_bit(base: Word, bytes: Word, bit: Word) {
    let byte = bit / 8;
    if byte >= bytes {
        return;
    }
    unsafe {
        let pointer = (base + byte) as *mut u8;
        *pointer |= 1 << (bit & 7);
    }
}

fn sys_framebuffer_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    match request {
        LINUX_FBIOGET_FSCREENINFO => {
            if argument == 0 {
                return Err(EFAULT);
            }
            write_fb_fix_screeninfo(runtime.posix_shm);
            write_target_memory(runtime, pid, argument, LINUX_FB_FIX_SCREENINFO_BYTES)?;
            Ok(0)
        }
        LINUX_FBIOGET_VSCREENINFO => {
            if argument == 0 {
                return Err(EFAULT);
            }
            write_fb_var_screeninfo(runtime.posix_shm);
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

fn write_fb_fix_screeninfo(base: Word) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_FB_FIX_SCREENINFO_BYTES as usize);
        let id = b"Nanami Honoka fb";
        ::core::ptr::copy_nonoverlapping(id.as_ptr(), base as *mut u8, id.len());
        write_u64(base + 16, 0);
        write_u32(base + 24, ALTER_FB_BYTES as u32);
        write_u32(base + 28, 0);
        write_u32(base + 32, 0);
        write_u32(base + 36, 2);
        write_u32(base + 48, ALTER_FB_STRIDE as u32);
    }
}

fn write_fb_var_screeninfo(base: Word) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_FB_VAR_SCREENINFO_BYTES as usize);
        write_u32(base, ALTER_FB_WIDTH as u32);
        write_u32(base + 4, ALTER_FB_HEIGHT as u32);
        write_u32(base + 8, ALTER_FB_WIDTH as u32);
        write_u32(base + 12, ALTER_FB_HEIGHT as u32);
        write_u32(base + 24, 32);
        write_u32(base + 32, 16);
        write_u32(base + 36, 8);
        write_u32(base + 44, 8);
        write_u32(base + 48, 8);
        write_u32(base + 56, 0);
        write_u32(base + 60, 8);
        write_u32(base + 68, 24);
        write_u32(base + 72, 8);
    }
}

fn sys_fcntl(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    command: Word,
    argument: Word,
) -> Result<Word, i32> {
    match command {
        LINUX_F_DUPFD | LINUX_F_DUPFD_CLOEXEC => {
            let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            let mut new_file = duplicate_linux_file(runtime, file)?;
            new_file.flags = if command == LINUX_F_DUPFD_CLOEXEC {
                new_file.flags | LINUX_FD_CLOEXEC
            } else {
                new_file.flags & !LINUX_FD_CLOEXEC
            };
            runtime
                .allocate_linux_file(pid, new_file, argument)
                .ok_or(EMFILE)
        }
        LINUX_F_GETFD => runtime
            .linux_file(pid, fd)
            .map(|file| file.flags & LINUX_FD_CLOEXEC)
            .ok_or(EBADF),
        LINUX_F_SETFD => {
            let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            file.flags = (file.flags & !LINUX_FD_CLOEXEC) | (argument & LINUX_FD_CLOEXEC);
            if runtime.set_linux_file(pid, fd, file) {
                Ok(0)
            } else {
                Err(EBADF)
            }
        }
        LINUX_F_GETFL => runtime
            .linux_file(pid, fd)
            .map(|file| file.flags & !LINUX_FD_CLOEXEC)
            .ok_or(EBADF),
        LINUX_F_SETFL => {
            let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            file.flags = (file.flags & LINUX_FD_CLOEXEC) | (argument & LINUX_SOCK_NONBLOCK);
            if runtime.set_linux_file(pid, fd, file) {
                Ok(0)
            } else {
                Err(EBADF)
            }
        }
        _ => Ok(0),
    }
}

fn write_linux_termios(base: Word, canonical: bool, echo: bool) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_TERMIOS_BYTES as usize);
        write_u32(base, LINUX_ICRNL | LINUX_IXON);
        write_u32(base + 4, LINUX_OPOST | LINUX_ONLCR);
        write_u32(base + 8, LINUX_CREAD | LINUX_CS8);
        let mut lflag = LINUX_ISIG | LINUX_ECHOE | LINUX_ECHOK | LINUX_IEXTEN;
        if canonical {
            lflag |= LINUX_ICANON;
        }
        if echo {
            lflag |= LINUX_ECHO;
        }
        write_u32(base + 12, lflag);
        write_u8(base + 16, 0);
        write_u8(base + 17 + LINUX_VEOF, 4);
        write_u8(base + 17 + LINUX_VEOL, 0);
        write_u8(base + 17 + LINUX_VERASE, 0x7f);
        write_u8(base + 17 + LINUX_VINTR, 3);
        write_u8(base + 17 + LINUX_VKILL, 21);
        write_u8(base + 17 + LINUX_VMIN, 1);
        write_u8(base + 17 + LINUX_VQUIT, 28);
        write_u8(base + 17 + LINUX_VSTART, 17);
        write_u8(base + 17 + LINUX_VSTOP, 19);
        write_u8(base + 17 + LINUX_VSUSP, 26);
        write_u8(base + 17 + LINUX_VTIME, 0);
    }
}

fn sys_gettimeofday(runtime: &mut Runtime, pid: Word, timeval: Word) -> Result<Word, i32> {
    if timeval != 0 {
        unsafe {
            write_u64(runtime.posix_shm, 0);
            write_u64(runtime.posix_shm + 8, 0);
        }
        write_target_memory(runtime, pid, timeval, 16)?;
    }
    Ok(0)
}

fn sys_setitimer(
    runtime: &mut Runtime,
    pid: Word,
    which: Word,
    new_value: Word,
    old_value: Word,
) -> Result<Word, i32> {
    if which > LINUX_ITIMER_PROF {
        return Err(EINVAL);
    }
    if new_value != 0 {
        read_target_memory(runtime, pid, new_value, LINUX_ITIMERVAL_BYTES)?;
    }
    if old_value != 0 {
        unsafe {
            ::core::ptr::write_bytes(
                runtime.posix_shm as *mut u8,
                0,
                LINUX_ITIMERVAL_BYTES as usize,
            );
        }
        write_target_memory(runtime, pid, old_value, LINUX_ITIMERVAL_BYTES)?;
    }
    Ok(0)
}

fn sys_sched_getaffinity(
    runtime: &mut Runtime,
    current_pid: Word,
    target_pid: Word,
    cpuset_size: Word,
    mask: Word,
) -> Result<Word, i32> {
    if mask == 0 || cpuset_size < LINUX_CPU_MASK_BYTES || cpuset_size > LINUX_PAGE_SIZE {
        return Err(EINVAL);
    }
    if target_pid != 0 && target_pid != current_pid && runtime.managed_process(target_pid).is_none()
    {
        return Err(ESRCH);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, cpuset_size as usize);
        ::core::ptr::write_unaligned(runtime.posix_shm as *mut u64, 1);
    }
    write_target_memory(runtime, current_pid, mask, cpuset_size)?;
    Ok(LINUX_CPU_MASK_BYTES)
}

fn sys_clock_gettime(
    runtime: &mut Runtime,
    pid: Word,
    clock_id: Word,
    timespec: Word,
) -> Result<Word, i32> {
    if timespec == 0 {
        return Err(EFAULT);
    }
    match clock_id {
        0..=9 | 11 => {}
        _ => return Err(EINVAL),
    }
    ensure_clock_timer(runtime)?;
    let tick_hz = runtime.monotonic_tick_hz;
    if tick_hz == 0 {
        return Err(EIO);
    }
    let seconds = runtime.monotonic_ticks / tick_hz;
    let nanoseconds = (runtime.monotonic_ticks % tick_hz).saturating_mul(1_000_000_000 / tick_hz);
    unsafe {
        write_u64(runtime.posix_shm, seconds);
        write_u64(runtime.posix_shm + 8, nanoseconds);
    }
    write_target_memory(runtime, pid, timespec, LINUX_TIMESPEC_BYTES)?;
    Ok(0)
}

fn sys_nanosleep_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    let request = context.args[0];
    if request == 0 {
        return EmulationAction::Return(-(EFAULT as isize));
    }
    if let Err(errno) = read_target_memory(runtime, pid, request, LINUX_TIMESPEC_BYTES) {
        return EmulationAction::Return(-(errno as isize));
    }

    let seconds = unsafe { ::core::ptr::read_unaligned(runtime.posix_shm as *const i64) };
    let nanoseconds = unsafe { ::core::ptr::read_unaligned((runtime.posix_shm + 8) as *const i64) };
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return EmulationAction::Return(-(EINVAL as isize));
    }

    let ticks = (seconds as Word)
        .saturating_mul(ALTER_SLEEP_TICK_HZ)
        .saturating_add(
            (nanoseconds as Word).saturating_add(ALTER_SLEEP_TICK_NANOSECONDS - 1)
                / ALTER_SLEEP_TICK_NANOSECONDS,
        );
    if ticks == 0 {
        return EmulationAction::Return(0);
    }
    if let Err(errno) = ensure_clock_timer(runtime) {
        return EmulationAction::Return(-(errno as isize));
    }

    let Some(process) = runtime.managed_process_mut(pid) else {
        return EmulationAction::Return(-(ESRCH as isize));
    };
    process.sleep_waiting = true;
    process.sleep_ticks_remaining = ticks;
    process.sleep_context = context;
    EmulationAction::Park
}

fn ensure_clock_timer(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime.clock_timer_armed {
        return Ok(());
    }
    let (ticks, tick_hz) =
        nanami_services::timer::timer_service_monotonic_ticks(runtime.timer_port)
            .map_err(map_request_error)?;
    if tick_hz == 0 {
        return Err(EIO);
    }
    nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        runtime.timer_port,
        ALTER_SLEEP_TICK_MILLISECONDS,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )
    .map_err(map_request_error)?;
    runtime.monotonic_ticks = ticks;
    runtime.monotonic_tick_hz = tick_hz;
    runtime.clock_timer_armed = true;
    Ok(())
}

fn sys_getrandom(
    runtime: &mut Runtime,
    pid: Word,
    user_buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let bytes = bounded_len(runtime, len)?;
    let mut i = 0;
    while i < bytes {
        unsafe {
            ::core::ptr::write(
                (runtime.posix_shm + i) as *mut u8,
                (i as u8).wrapping_mul(37),
            );
        }
        i += 1;
    }
    if bytes != 0 {
        write_target_memory(runtime, pid, user_buffer, bytes)?;
    }
    Ok(bytes)
}

fn sys_getrlimit(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let out = if context.number == SYS_PRLIMIT64 {
        context.args[3]
    } else {
        context.args[1]
    };
    if out == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u64(runtime.posix_shm, Word::MAX);
        write_u64(runtime.posix_shm + 8, Word::MAX);
    }
    write_target_memory(runtime, pid, out, 16)?;
    Ok(0)
}

fn sys_getresid(
    runtime: &mut Runtime,
    pid: Word,
    real_ptr: Word,
    effective_ptr: Word,
    saved_ptr: Word,
    is_user: bool,
) -> Result<Word, i32> {
    let id = if is_user {
        map_word(posix::posix_getuid(runtime.posix_port))?
    } else {
        map_word(posix::posix_getgid(runtime.posix_port))?
    } as u32;
    write_guest_u32(runtime, pid, real_ptr, id)?;
    write_guest_u32(runtime, pid, effective_ptr, id)?;
    write_guest_u32(runtime, pid, saved_ptr, id)?;
    Ok(0)
}

fn sys_getpgid(runtime: &Runtime, current_pid: Word, target_pid: Word) -> Result<Word, i32> {
    if target_pid == 0 {
        return Ok(current_pid);
    }
    if runtime.managed_process(target_pid).is_some() {
        return Ok(target_pid);
    }
    map_word(posix::posix_getpgid(runtime.posix_port, target_pid))
}

fn sys_kill(
    runtime: &mut Runtime,
    _current_pid: Word,
    target_pid: Word,
    signal: Word,
) -> Result<Word, i32> {
    if target_pid == 0 || (target_pid as isize) < 0 {
        return Ok(0);
    }
    if signal == 0 {
        return if runtime.managed_process(target_pid).is_some() {
            Ok(0)
        } else {
            Err(ESRCH)
        };
    }
    if runtime.managed_process(target_pid).is_some() {
        libnanami::request_process_kill(target_pid, signal).map_err(map_request_error)?;
        return Ok(0);
    }
    map_unit(posix::posix_kill(runtime.posix_port, target_pid, signal), 0)
}

fn trace_syscall_action(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    action: EmulationAction,
) {
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    if !process.trace_enabled
        || runtime.terminal_port == 0
        || runtime.terminal_shm == 0
        || process.terminal_id == 0
    {
        return;
    }

    let mut line = [0u8; 192];
    let mut pos = 0usize;
    pos = trace_append_bytes(&mut line, pos, b"[strace ");
    pos = trace_append_decimal(&mut line, pos, pid);
    pos = trace_append_bytes(&mut line, pos, b"] ");
    pos = trace_append_bytes(&mut line, pos, syscall_name(context.number));
    pos = trace_append_bytes(&mut line, pos, b"(");
    let argc = syscall_arg_count(context.number);
    let mut i = 0usize;
    while i < argc {
        if i != 0 {
            pos = trace_append_bytes(&mut line, pos, b", ");
        }
        pos = trace_append_hex(&mut line, pos, context.args[i]);
        i += 1;
    }
    pos = trace_append_bytes(&mut line, pos, b") = ");
    match action {
        EmulationAction::Return(value) => {
            pos = trace_append_isize(&mut line, pos, value);
        }
        EmulationAction::Resume => {
            pos = trace_append_bytes(&mut line, pos, b"0 <resume>");
        }
        EmulationAction::Park => {
            pos = trace_append_bytes(&mut line, pos, b"? <park>");
        }
        EmulationAction::Exit(status) => {
            pos = trace_append_bytes(&mut line, pos, b"? <exit ");
            pos = trace_append_decimal(&mut line, pos, status);
            pos = trace_append_bytes(&mut line, pos, b">");
        }
        EmulationAction::Unsupported(_) => {
            pos = trace_append_bytes(&mut line, pos, b"-38 <ENOSYS>");
        }
    }
    pos = trace_append_bytes(&mut line, pos, b"\n");
    unsafe {
        ::core::ptr::copy_nonoverlapping(line.as_ptr(), runtime.terminal_shm as *mut u8, pos);
    }
    let _ = nanami_services::terminal::terminal_write_output(
        runtime.terminal_port,
        process.terminal_id,
        0,
        pos as Word,
    );
}

fn record_syscall_result(runtime: &mut Runtime, pid: Word, syscall: Word, value: isize) {
    if let Some(process) = runtime.managed_process_mut(pid) {
        if !process.trace_enabled {
            return;
        }
        process.last_syscall = syscall;
        process.last_syscall_return = value;
    }
}

fn record_action_result(runtime: &mut Runtime, pid: Word, syscall: Word, action: EmulationAction) {
    let value = match action {
        EmulationAction::Return(value) => value,
        EmulationAction::Resume => 0,
        EmulationAction::Park => isize::MIN,
        EmulationAction::Exit(status) => status as isize,
        EmulationAction::Unsupported(_) => -(ENOSYS as isize),
    };
    record_syscall_result(runtime, pid, syscall, value);
}

fn process_trace_enabled(runtime: &Runtime, pid: Word) -> bool {
    runtime
        .managed_process(pid)
        .map(|process| process.trace_enabled)
        .unwrap_or(false)
}

fn trace_critical_syscall(
    runtime: &Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    value: isize,
) {
    if !process_trace_enabled(runtime, pid) {
        return;
    }
    match context.number {
        SYS_READ if context.args[0] == 0 => {
            libnanami::println!(
                "[alter/linux] read stdin pid={} buf={:#x} len={} ret={}",
                pid,
                context.args[1],
                context.args[2],
                value
            );
        }
        SYS_WAIT4 | SYS_RT_SIGSUSPEND | SYS_VFORK | SYS_FORK | SYS_CLONE => {
            libnanami::println!(
                "[alter/linux] sync syscall pid={} nr={} a0={:#x} a1={:#x} ret={}",
                pid,
                context.number,
                context.args[0],
                context.args[1],
                value
            );
        }
        _ => {}
    }
}

fn trace_critical_action(
    runtime: &Runtime,
    pid: Word,
    context: LinuxSyscallContext,
    action: EmulationAction,
) {
    let value = match action {
        EmulationAction::Return(value) => value,
        EmulationAction::Resume => 0,
        EmulationAction::Park => isize::MIN,
        EmulationAction::Exit(status) => status as isize,
        EmulationAction::Unsupported(_) => -(ENOSYS as isize),
    };
    trace_critical_syscall(runtime, pid, context, value);
}

fn sys_sigaltstack(
    runtime: &mut Runtime,
    pid: Word,
    new_stack: Word,
    old_stack: Word,
) -> Result<Word, i32> {
    if old_stack != 0 {
        unsafe {
            ::core::ptr::write_unaligned(runtime.posix_shm as *mut Word, 0);
            ::core::ptr::write_unaligned((runtime.posix_shm + 8) as *mut Word, LINUX_SS_DISABLE);
            ::core::ptr::write_unaligned((runtime.posix_shm + 16) as *mut Word, 0);
        }
        write_target_memory(runtime, pid, old_stack, LINUX_STACK_T_BYTES)?;
    }
    if new_stack != 0 {
        read_target_memory(runtime, pid, new_stack, LINUX_STACK_T_BYTES)?;
        let flags = read_shm_u64(runtime, 8);
        let size = read_shm_u64(runtime, 16);
        if (flags & !(LINUX_SS_DISABLE | LINUX_SS_AUTODISARM)) != 0 {
            return Err(EINVAL);
        }
        if (flags & LINUX_SS_DISABLE) == 0 && size < LINUX_MINSIGSTKSZ {
            return Err(ENOMEM);
        }
    }
    Ok(0)
}

fn sys_rt_sigaction(
    runtime: &mut Runtime,
    pid: Word,
    signum: Word,
    _act: Word,
    oldact: Word,
    sigset_size: Word,
) -> Result<Word, i32> {
    if signum == 0 || signum > LINUX_NSIG || sigset_size != LINUX_SIGSET_BYTES {
        return Err(EINVAL);
    }
    if oldact != 0 {
        unsafe {
            ::core::ptr::write_bytes(
                runtime.posix_shm as *mut u8,
                0,
                LINUX_KERNEL_SIGACTION_BYTES,
            );
        }
        write_target_memory(runtime, pid, oldact, LINUX_KERNEL_SIGACTION_BYTES as Word)?;
    }
    Ok(0)
}

fn sys_rt_sigprocmask(
    runtime: &mut Runtime,
    pid: Word,
    _how: Word,
    _set: Word,
    oldset: Word,
    sigset_size: Word,
) -> Result<Word, i32> {
    if sigset_size != LINUX_SIGSET_BYTES {
        return Err(EINVAL);
    }
    if oldset != 0 {
        unsafe {
            ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_SIGSET_BYTES as usize);
        }
        write_target_memory(runtime, pid, oldset, LINUX_SIGSET_BYTES)?;
    }
    Ok(0)
}

fn sys_rt_sigsuspend(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if runtime.exited_child(pid, 0).is_some() || !runtime.has_child(pid, 0) {
        return EmulationAction::Return(-(EINTR as isize));
    }
    if runtime.park_signal_waiter(pid, 0, context) {
        EmulationAction::Park
    } else {
        EmulationAction::Return(-(ESRCH as isize))
    }
}

pub fn wake_waiter_for_child(runtime: &mut Runtime, child_pid: Word) {
    let Some((parent_pid, parent_pcb, context)) = runtime.take_signal_waiter_for_child(child_pid)
    else {
        return;
    };

    let return_value = if context.number == SYS_WAIT4 {
        let status = runtime
            .managed_process(child_pid)
            .map(|process| process.exit_status)
            .unwrap_or(1);
        let status_ptr = context.args[1];
        if status_ptr != 0
            && write_u32_to_target(
                runtime,
                parent_pid,
                status_ptr,
                ((status & 0xff) << 8) as u32,
            )
            .is_err()
        {
            -(EFAULT as isize)
        } else if let Err(error) = libnanami::request_process_reap(child_pid) {
            let errno = map_request_error(error);
            libnanami::println!(
                "[alter/linux] wait4 wake reap failed parent={} child={} errno={}",
                parent_pid,
                child_pid,
                errno
            );
            -(errno as isize)
        } else {
            close_process_files(runtime, child_pid);
            runtime.remove_process(child_pid);
            child_pid as isize
        }
    } else if context.number == SYS_VFORK {
        child_pid as isize
    } else {
        -(EINTR as isize)
    };

    if write_syscall_return(parent_pcb, context, return_value).is_err() {
        libnanami::println!(
            "[alter/linux] waiter wake register write failed parent={} pcb={:#x}",
            parent_pid,
            parent_pcb
        );
        return;
    }
    if let Err(error) = a9n_abi::arch::process_control_block::resume(parent_pcb) {
        libnanami::println!(
            "[alter/linux] waiter wake resume failed parent={} pcb={:#x} err={:?}",
            parent_pid,
            parent_pcb,
            error
        );
    }
}

fn syscall_arg_count(number: Word) -> usize {
    match number {
        SYS_GETPID | SYS_GETPPID | SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID
        | SYS_FORK | SYS_VFORK | SYS_GETTID => 0,
        SYS_CLOSE | SYS_EXIT | SYS_EXIT_GROUP | SYS_PIPE | SYS_GETPGID => 1,
        SYS_OPEN | SYS_CREAT | SYS_STAT | SYS_LSTAT | SYS_FSTAT | SYS_ACCESS | SYS_ARCH_PRCTL
        | SYS_DUP | SYS_CLOCK_GETTIME | SYS_SET_TID_ADDRESS | SYS_GETRANDOM | SYS_GETRLIMIT
        | SYS_CHDIR | SYS_MKDIR | SYS_RMDIR | SYS_UNLINK | SYS_UTIMES | SYS_DUP2 | SYS_RENAME
        | SYS_RT_SIGSUSPEND | SYS_SIGALTSTACK | SYS_PIPE2 | SYS_KILL | SYS_SETPGID | SYS_MSYNC
        | SYS_NANOSLEEP => 2,
        SYS_READ
        | SYS_WRITE
        | SYS_READV
        | SYS_WRITEV
        | SYS_SENDMSG
        | SYS_RECVMSG
        | SYS_OPENAT
        | SYS_NEWFSTATAT
        | SYS_FACCESSAT
        | SYS_IOCTL
        | SYS_LSEEK
        | SYS_POLL
        | SYS_PPOLL
        | SYS_EXECVE
        | SYS_WAIT4
        | SYS_DUP3
        | SYS_GETDENTS64
        | SYS_GETCWD
        | SYS_UNAME
        | SYS_BRK
        | SYS_FCNTL
        | SYS_FUTIMESAT
        | SYS_MKDIRAT
        | SYS_MKNOD
        | SYS_UNLINKAT
        | SYS_READLINK
        | SYS_GETRESUID
        | SYS_GETRESGID
        | SYS_MADVISE
        | SYS_CHOWN
        | SYS_FCHOWN
        | SYS_LCHOWN
        | SYS_SETITIMER
        | SYS_SCHED_GETAFFINITY => 3,
        SYS_MMAP | SYS_SELECT | SYS_PSELECT6 | SYS_PRLIMIT64 | SYS_UTIMENSAT | SYS_RENAMEAT
        | SYS_READLINKAT | SYS_MKNODAT | SYS_FACCESSAT2 => 4,
        SYS_CLONE | SYS_STATX | SYS_FCHOWNAT => 5,
        _ => 6,
    }
}

fn syscall_name(number: Word) -> &'static [u8] {
    match number {
        SYS_READ => b"read",
        SYS_WRITE => b"write",
        SYS_READV => b"readv",
        SYS_SENDMSG => b"sendmsg",
        SYS_RECVMSG => b"recvmsg",
        SYS_OPEN => b"open",
        SYS_DUP => b"dup",
        SYS_DUP2 => b"dup2",
        SYS_DUP3 => b"dup3",
        SYS_PIPE => b"pipe",
        SYS_PIPE2 => b"pipe2",
        SYS_CREAT => b"creat",
        SYS_CLOSE => b"close",
        SYS_STAT => b"stat",
        SYS_LSTAT => b"lstat",
        SYS_STATX => b"statx",
        SYS_FSTAT => b"fstat",
        SYS_CHOWN => b"chown",
        SYS_FCHOWN => b"fchown",
        SYS_LCHOWN => b"lchown",
        SYS_FCHOWNAT => b"fchownat",
        SYS_POLL => b"poll",
        SYS_LSEEK => b"lseek",
        SYS_MMAP => b"mmap",
        SYS_MPROTECT => b"mprotect",
        SYS_MUNMAP => b"munmap",
        SYS_MSYNC => b"msync",
        SYS_MADVISE => b"madvise",
        SYS_MREMAP => b"mremap",
        SYS_BRK => b"brk",
        SYS_RT_SIGACTION => b"rt_sigaction",
        SYS_RT_SIGPROCMASK => b"rt_sigprocmask",
        SYS_RT_SIGSUSPEND => b"rt_sigsuspend",
        SYS_SIGALTSTACK => b"sigaltstack",
        SYS_IOCTL => b"ioctl",
        SYS_WRITEV => b"writev",
        SYS_NANOSLEEP => b"nanosleep",
        SYS_ACCESS => b"access",
        SYS_SELECT => b"select",
        SYS_GETPID => b"getpid",
        SYS_CLONE => b"clone",
        SYS_FORK => b"fork",
        SYS_VFORK => b"vfork",
        SYS_EXECVE => b"execve",
        SYS_EXIT => b"exit",
        SYS_WAIT4 => b"wait4",
        SYS_UNAME => b"uname",
        SYS_GETCWD => b"getcwd",
        SYS_CHDIR => b"chdir",
        SYS_RENAME => b"rename",
        SYS_MKDIR => b"mkdir",
        SYS_RMDIR => b"rmdir",
        SYS_UNLINK => b"unlink",
        SYS_READLINK => b"readlink",
        SYS_GETTIMEOFDAY => b"gettimeofday",
        SYS_SETITIMER => b"setitimer",
        SYS_GETRLIMIT => b"getrlimit",
        SYS_GETUID => b"getuid",
        SYS_GETGID => b"getgid",
        SYS_GETEUID => b"geteuid",
        SYS_GETEGID => b"getegid",
        SYS_GETPPID => b"getppid",
        SYS_SETPGID => b"setpgid",
        SYS_GETPGID => b"getpgid",
        SYS_KILL => b"kill",
        SYS_ARCH_PRCTL => b"arch_prctl",
        SYS_GETTID => b"gettid",
        SYS_SCHED_GETAFFINITY => b"sched_getaffinity",
        SYS_FUTEX => b"futex",
        SYS_GETDENTS64 => b"getdents64",
        SYS_SET_TID_ADDRESS => b"set_tid_address",
        SYS_CLOCK_GETTIME => b"clock_gettime",
        SYS_UTIMES => b"utimes",
        SYS_EXIT_GROUP => b"exit_group",
        SYS_OPENAT => b"openat",
        SYS_MKDIRAT => b"mkdirat",
        SYS_MKNOD => b"mknod",
        SYS_MKNODAT => b"mknodat",
        SYS_FUTIMESAT => b"futimesat",
        SYS_NEWFSTATAT => b"newfstatat",
        SYS_UNLINKAT => b"unlinkat",
        SYS_RENAMEAT => b"renameat",
        SYS_READLINKAT => b"readlinkat",
        SYS_FACCESSAT => b"faccessat",
        SYS_FACCESSAT2 => b"faccessat2",
        SYS_UTIMENSAT => b"utimensat",
        SYS_PSELECT6 => b"pselect6",
        SYS_PPOLL => b"ppoll",
        SYS_SET_ROBUST_LIST => b"set_robust_list",
        SYS_PRLIMIT64 => b"prlimit64",
        SYS_GETRANDOM => b"getrandom",
        SYS_GETRESUID => b"getresuid",
        SYS_GETRESGID => b"getresgid",
        SYS_RSEQ => b"rseq",
        _ => b"syscall",
    }
}

fn trace_append_bytes(out: &mut [u8], mut pos: usize, bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < bytes.len() && pos < out.len() {
        out[pos] = bytes[i];
        pos += 1;
        i += 1;
    }
    pos
}

fn trace_append_decimal(out: &mut [u8], pos: usize, value: Word) -> usize {
    if value == 0 {
        return trace_append_bytes(out, pos, b"0");
    }
    let mut digits = [0u8; 20];
    let mut count = 0usize;
    let mut n = value;
    while n != 0 && count < digits.len() {
        digits[count] = b'0' + (n % 10) as u8;
        n /= 10;
        count += 1;
    }
    let mut out_pos = pos;
    while count != 0 {
        count -= 1;
        out_pos = trace_append_bytes(out, out_pos, &digits[count..count + 1]);
    }
    out_pos
}

fn trace_append_isize(out: &mut [u8], pos: usize, value: isize) -> usize {
    if value < 0 {
        let pos = trace_append_bytes(out, pos, b"-");
        trace_append_decimal(out, pos, value.unsigned_abs() as Word)
    } else {
        trace_append_decimal(out, pos, value as Word)
    }
}

fn trace_append_hex(out: &mut [u8], pos: usize, value: Word) -> usize {
    let mut pos = trace_append_bytes(out, pos, b"0x");
    let mut shift = (::core::mem::size_of::<Word>() * 8) as isize - 4;
    let mut seen = false;
    while shift >= 0 {
        let nibble = ((value >> shift) & 0xf) as u8;
        if nibble != 0 || seen || shift == 0 {
            seen = true;
            let byte = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            pos = trace_append_bytes(out, pos, &[byte]);
        }
        shift -= 4;
    }
    pos
}

fn map_anonymous_tracked(
    runtime: &mut Runtime,
    pid: Word,
    len: Word,
    prot: Word,
) -> Result<(Word, Word), i32> {
    let (base, mapped) =
        libnanami::request_process_map_anonymous(pid, len).map_err(map_request_error)?;
    if !runtime.add_mapping(pid, base, mapped, prot) {
        return Err(ENOMEM);
    }
    Ok((base, mapped))
}

fn clone_process_image(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    libnanami::request_process_memory_read(
        parent_pid,
        LINUX_IMAGE_BASE,
        runtime.posix_shm,
        LINUX_ELF_HEADER_BYTES,
    )
    .map_err(map_request_error)?;
    if read_shm_u8(runtime, 0) != 0x7f
        || read_shm_u8(runtime, 1) != b'E'
        || read_shm_u8(runtime, 2) != b'L'
        || read_shm_u8(runtime, 3) != b'F'
    {
        return Err(EINVAL);
    }

    let phoff = read_shm_u64(runtime, 32) as Word;
    let phentsize = read_shm_u16(runtime, 54) as Word;
    let phnum = read_shm_u16(runtime, 56) as Word;
    if phentsize < 56 || phnum == 0 {
        return Err(EINVAL);
    }
    let elf_type = read_shm_u16(runtime, 16);
    let load_bias = if elf_type == 3 { LINUX_IMAGE_BASE } else { 0 };

    let table_bytes = phentsize.checked_mul(phnum).ok_or(EINVAL)?;
    if table_bytes > runtime.posix_shm_size {
        return Err(EINVAL);
    }
    libnanami::request_process_memory_read(
        parent_pid,
        LINUX_IMAGE_BASE + phoff,
        runtime.posix_shm,
        table_bytes,
    )
    .map_err(map_request_error)?;

    let mut ranges = [(0, 0); LINUX_MAX_LOAD_SEGMENTS];
    let mut range_count = 0usize;
    let mut i = 0;
    while i < phnum {
        let base = i * phentsize;
        if read_shm_u32(runtime, base as usize) == LINUX_PT_LOAD
            && (read_shm_u32(runtime, (base + 4) as usize) & LINUX_PF_W) != 0
        {
            let vaddr = read_shm_u64(runtime, (base + 16) as usize) + load_bias;
            let memsz = read_shm_u64(runtime, (base + 40) as usize);
            let start = align_down_word(vaddr, LINUX_PAGE_SIZE);
            let end = align_up_word(vaddr.saturating_add(memsz), LINUX_PAGE_SIZE);
            if range_count == ranges.len() {
                return Err(EINVAL);
            }
            ranges[range_count] = (start, end.saturating_sub(start));
            range_count += 1;
        }
        i += 1;
    }
    let mut range_index = 0usize;
    while range_index < range_count {
        let (start, size) = ranges[range_index];
        copy_present_process_range(runtime, parent_pid, child_pid, start, size)?;
        range_index += 1;
    }
    Ok(())
}

fn clone_process_mappings(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    let parent = runtime.managed_process(parent_pid).ok_or(ESRCH)?;
    let mut i = 0usize;
    while i < parent.mappings.len() {
        let mapping = parent.mappings[i];
        if mapping.base != 0 {
            if is_initial_stack_mapping(mapping.base, mapping.size) {
                i += 1;
                continue;
            }
            if mapping.prot == LINUX_PROT_NONE {
                i += 1;
                continue;
            }
            let (base, mapped) =
                libnanami::request_process_map_anonymous_at(child_pid, mapping.base, mapping.size)
                    .map_err(|error| {
                        let errno = map_request_error(error);
                        libnanami::println!(
                            "[alter/linux] fork map clone failed parent={} child={} base={:#x} size={:#x} errno={}",
                            parent_pid,
                            child_pid,
                            mapping.base,
                            mapping.size,
                            errno
                        );
                        errno
                    })?;
            if base != mapping.base || mapped < mapping.size {
                libnanami::println!(
                    "[alter/linux] fork map clone mismatch parent={} child={} want=[{:#x}..{:#x}) got=[{:#x}..{:#x})",
                    parent_pid,
                    child_pid,
                    mapping.base,
                    mapping.base + mapping.size,
                    base,
                    base + mapped
                );
                return Err(ENOMEM);
            }
            copy_present_process_range(runtime, parent_pid, child_pid, mapping.base, mapping.size)?;
        }
        i += 1;
    }
    Ok(())
}

fn is_initial_stack_mapping(base: Word, size: Word) -> bool {
    let Some(end) = base.checked_add(size) else {
        return false;
    };
    let stack_base = LINUX_STACK_TOP - LINUX_STACK_BYTES;
    let stack_end = LINUX_STACK_TOP + LINUX_STACK_GUARD_BYTES;
    base < stack_end && stack_base < end
}

fn clone_process_stack(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    copy_present_process_range(
        runtime,
        parent_pid,
        child_pid,
        LINUX_STACK_TOP - LINUX_STACK_BYTES,
        LINUX_STACK_BYTES,
    )
}

fn copy_present_process_range(
    runtime: &mut Runtime,
    src_pid: Word,
    dst_pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(LINUX_DIRECT_COPY_CHUNK, size - done);
        let src = base + done;
        if libnanami::request_process_memory_clone(src_pid, dst_pid, src, chunk).is_err() {
            copy_present_process_range_via_shm(runtime, src_pid, dst_pid, src, chunk)?;
        }
        done += chunk;
    }
    Ok(())
}

fn copy_present_process_range_via_shm(
    runtime: &mut Runtime,
    src_pid: Word,
    dst_pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(fork_copy_chunk(runtime), size - done);
        let src = base + done;
        if libnanami::request_process_memory_read(src_pid, src, runtime.posix_shm, chunk).is_ok() {
            if let Err(error) =
                libnanami::request_process_memory_write(dst_pid, src, runtime.posix_shm, chunk)
            {
                let errno = map_request_error(error);
                libnanami::println!(
                    "[alter/linux] fork copy failed src_pid={} dst_pid={} va=0x{:x} bytes=0x{:x} errno={}",
                    src_pid,
                    dst_pid,
                    src,
                    chunk,
                    errno
                );
                return Err(errno);
            }
        }
        done += chunk;
    }
    Ok(())
}

fn copy_same_process_range(
    runtime: &mut Runtime,
    pid: Word,
    src_base: Word,
    dst_base: Word,
    size: Word,
) -> Result<(), i32> {
    if size == 0
        || libnanami::request_process_memory_copy_within(pid, src_base, dst_base, size).is_ok()
    {
        return Ok(());
    }

    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(fork_copy_chunk(runtime), size - done);
        let src = src_base + done;
        let dst = dst_base + done;
        libnanami::request_process_memory_read(pid, src, runtime.posix_shm, chunk)
            .map_err(map_request_error)?;
        libnanami::request_process_memory_write(pid, dst, runtime.posix_shm, chunk)
            .map_err(map_request_error)?;
        done += chunk;
    }
    Ok(())
}

fn register_linux_stack_mappings(runtime: &mut Runtime, pid: Word) -> bool {
    let stack_base = LINUX_STACK_TOP - LINUX_STACK_BYTES;
    runtime.reset_stack_mapping(
        pid,
        stack_base,
        LINUX_STACK_BYTES,
        LINUX_PROT_READ | LINUX_PROT_WRITE,
    ) && runtime.add_mapping(
        pid,
        LINUX_STACK_TOP,
        LINUX_STACK_GUARD_BYTES,
        LINUX_PROT_NONE,
    )
}

struct ExecStringSnapshot {
    buffer: Word,
    used: usize,
    argc: usize,
    envc: usize,
    argv_offsets: [usize; ALTER_LAUNCH_MAX_ARGS],
    argv_lens: [usize; ALTER_LAUNCH_MAX_ARGS],
    env_offsets: [usize; ALTER_LAUNCH_MAX_ENVS],
    env_lens: [usize; ALTER_LAUNCH_MAX_ENVS],
}

struct ExecGuestPage {
    buffer: Word,
    pid: Word,
    page: Word,
    valid: bool,
}

impl ExecGuestPage {
    fn new(buffer: Word) -> Self {
        Self {
            buffer,
            pid: 0,
            page: 0,
            valid: false,
        }
    }

    fn ensure(&mut self, pid: Word, address: Word) -> Result<usize, i32> {
        let page = address & !(LINUX_PAGE_SIZE - 1);
        if !self.valid || self.pid != pid || self.page != page {
            libnanami::request_process_memory_read(pid, page, self.buffer, LINUX_PAGE_SIZE)
                .map_err(map_request_error)?;
            self.pid = pid;
            self.page = page;
            self.valid = true;
        }
        Ok((address - page) as usize)
    }

    fn copy(&mut self, pid: Word, address: Word, out: &mut [u8]) -> Result<(), i32> {
        let mut copied = 0usize;
        while copied < out.len() {
            let source = address.checked_add(copied as Word).ok_or(EFAULT)?;
            let page_offset = self.ensure(pid, source)?;
            let chunk =
                ::core::cmp::min(out.len() - copied, LINUX_PAGE_SIZE as usize - page_offset);
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    (self.buffer as usize + page_offset) as *const u8,
                    out.as_mut_ptr().add(copied),
                    chunk,
                );
            }
            copied += chunk;
        }
        Ok(())
    }

    fn read_word(&mut self, pid: Word, address: Word) -> Result<Word, i32> {
        let mut bytes = [0u8; ::core::mem::size_of::<Word>()];
        self.copy(pid, address, &mut bytes)?;
        Ok(Word::from_ne_bytes(bytes))
    }
}

fn snapshot_exec_strings(
    runtime: &mut Runtime,
    pid: Word,
    argv_ptr: Word,
    envp_ptr: Word,
) -> Result<ExecStringSnapshot, i32> {
    if runtime.exec_snapshot_buffer == 0
        || runtime.exec_snapshot_buffer_size < LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word
    {
        let (buffer, mapped) =
            libnanami::request_heap(LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word)
                .map_err(map_request_error)?;
        if mapped < LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES as Word {
            return Err(ENOMEM);
        }
        runtime.exec_snapshot_buffer = buffer;
        runtime.exec_snapshot_buffer_size = mapped;
    }

    let mut guest_page =
        ExecGuestPage::new(runtime.exec_snapshot_buffer + LINUX_EXEC_SNAPSHOT_BYTES as Word);
    let (argv, argc) = read_guest_pointer_array_args(&mut guest_page, pid, argv_ptr)?;
    let (envp, envc) = read_guest_pointer_array_envs(&mut guest_page, pid, envp_ptr)?;

    let mut snapshot = ExecStringSnapshot {
        buffer: runtime.exec_snapshot_buffer,
        used: 0,
        argc,
        envc,
        argv_offsets: [0; ALTER_LAUNCH_MAX_ARGS],
        argv_lens: [0; ALTER_LAUNCH_MAX_ARGS],
        env_offsets: [0; ALTER_LAUNCH_MAX_ENVS],
        env_lens: [0; ALTER_LAUNCH_MAX_ENVS],
    };
    let mut i = 1usize;
    while i < argc {
        let (offset, len) = snapshot_guest_string(
            &mut guest_page,
            pid,
            runtime.exec_snapshot_buffer,
            snapshot.used,
            argv[i],
        )?;
        snapshot.argv_offsets[i] = offset;
        snapshot.argv_lens[i] = len;
        snapshot.used = offset + len;
        i += 1;
    }
    i = 0;
    while i < envc {
        let (offset, len) = snapshot_guest_string(
            &mut guest_page,
            pid,
            runtime.exec_snapshot_buffer,
            snapshot.used,
            envp[i],
        )?;
        snapshot.env_offsets[i] = offset;
        snapshot.env_lens[i] = len;
        snapshot.used = offset + len;
        i += 1;
    }
    Ok(snapshot)
}

fn snapshot_guest_string(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    buffer: Word,
    offset: usize,
    user_ptr: Word,
) -> Result<(usize, usize), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let mut copied = 0usize;
    while copied < LINUX_EXEC_STRING_MAX {
        let source = user_ptr.checked_add(copied as Word).ok_or(EFAULT)?;
        let page_offset = guest_page.ensure(pid, source)?;
        let page_bytes = ::core::cmp::min(
            LINUX_EXEC_STRING_MAX - copied,
            LINUX_PAGE_SIZE as usize - page_offset,
        );
        let page = unsafe {
            ::core::slice::from_raw_parts(
                (guest_page.buffer as usize + page_offset) as *const u8,
                page_bytes,
            )
        };
        let chunk = page
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(page_bytes);
        let end = offset
            .checked_add(copied)
            .and_then(|value| value.checked_add(chunk))
            .filter(|end| *end <= LINUX_EXEC_SNAPSHOT_BYTES)
            .ok_or(ENOMEM)?;
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                page.as_ptr(),
                (buffer as usize + offset + copied) as *mut u8,
                chunk,
            );
        }
        copied += chunk;
        if chunk < page_bytes {
            return Ok((offset, end - offset));
        }
    }
    Err(ENAMETOOLONG)
}

fn rewrite_linux_stack(
    runtime: &mut Runtime,
    pid: Word,
    exec_path: &[u8; LINUX_EXEC_PATH_MAX],
    exec_path_len: usize,
    snapshot: &ExecStringSnapshot,
    preferred_image_base: Word,
    loaded_elf: Option<CurrentElfMetadata>,
) -> Result<(), i32> {
    let pcb = runtime
        .managed_process(pid)
        .map(|process| process.pcb)
        .ok_or(ESRCH)?;
    let personality = runtime
        .managed_process(pid)
        .map(|process| process.personality)
        .ok_or(ESRCH)?;
    let elf = match loaded_elf {
        Some(elf) => elf,
        None => read_current_elf_metadata(runtime, pid, preferred_image_base)?,
    };
    if runtime.exec_stack_buffer == 0
        || runtime.exec_stack_buffer_size < LINUX_EXEC_STACK_BYTES as Word
    {
        let (stack_buffer, mapped) =
            libnanami::request_heap(LINUX_EXEC_STACK_BYTES as Word).map_err(map_request_error)?;
        if mapped < LINUX_EXEC_STACK_BYTES as Word {
            return Err(ENOMEM);
        }
        runtime.exec_stack_buffer = stack_buffer;
        runtime.exec_stack_buffer_size = mapped;
    }
    let stack_buffer = runtime.exec_stack_buffer;
    unsafe {
        ::core::ptr::write_bytes(stack_buffer as *mut u8, 0, LINUX_EXEC_STACK_BYTES);
    }

    let mut argc = snapshot.argc;
    let stack_base = LINUX_STACK_TOP - LINUX_EXEC_STACK_BYTES as Word;
    let mut cursor = LINUX_EXEC_STACK_BYTES;
    let mut argv_guest = [0usize; ALTER_LAUNCH_MAX_ARGS];
    let mut env_guest = [0usize; ALTER_LAUNCH_MAX_ENVS];

    let mut i = 0usize;
    while i < snapshot.envc {
        cursor = push_snapshot_string_to_stack(
            snapshot,
            stack_buffer,
            cursor,
            snapshot.env_offsets[i],
            snapshot.env_lens[i],
            &mut env_guest[i],
            stack_base,
        )?;
        i += 1;
    }

    let (command_base, command_len) = basename_in_bytes(exec_path, exec_path_len);
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        &exec_path[command_base..command_base + command_len],
        &mut argv_guest[0],
        stack_base,
    )?;
    if argc == 0 {
        argc = 1;
    } else {
        i = 1;
        while i < argc {
            cursor = push_snapshot_string_to_stack(
                snapshot,
                stack_buffer,
                cursor,
                snapshot.argv_offsets[i],
                snapshot.argv_lens[i],
                &mut argv_guest[i],
                stack_base,
            )?;
            i += 1;
        }
    }

    let mut execfn_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        &exec_path[..exec_path_len],
        &mut execfn_guest,
        stack_base,
    )?;
    let random_bytes = [
        0x5d, 0x8f, 0x2a, 0x91, 0x48, 0x3c, 0x11, 0x67, 0x02, 0xee, 0x70, 0x31, 0xaa, 0x4b, 0xd2,
        0x09,
    ];
    let mut random_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        &random_bytes,
        &mut random_guest,
        stack_base,
    )?;
    let mut platform_guest = 0usize;
    cursor = push_bytes_to_stack(
        stack_buffer,
        cursor,
        b"x86_64",
        &mut platform_guest,
        stack_base,
    )?;

    let aux_pairs = 18usize;
    let word_count = 1 + argc + 1 + snapshot.envc + 1 + aux_pairs * 2;
    let table_bytes = word_count * ::core::mem::size_of::<Word>();
    let aligned_cursor = align_down_usize(cursor, 16);
    if aligned_cursor < table_bytes + 16 {
        return Err(ENOMEM);
    }
    let sp_offset = if personality == OsPersonality::FreeBsd {
        align_down_usize(aligned_cursor - table_bytes, 16)
            .checked_sub(8)
            .ok_or(ENOMEM)?
    } else {
        align_down_usize(aligned_cursor - table_bytes, 16)
    };
    let mut out = sp_offset;
    write_stack_word(stack_buffer, out, argc as Word);
    out += 8;
    i = 0;
    while i < argc {
        write_stack_word(stack_buffer, out, argv_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;
    i = 0;
    while i < snapshot.envc {
        write_stack_word(stack_buffer, out, env_guest[i] as Word);
        out += 8;
        i += 1;
    }
    write_stack_word(stack_buffer, out, 0);
    out += 8;

    out = write_aux(stack_buffer, out, AT_PHDR, elf.program_header_vaddr);
    out = write_aux(stack_buffer, out, AT_PHENT, elf.program_header_entry_size);
    out = write_aux(stack_buffer, out, AT_PHNUM, elf.program_header_count);
    out = write_aux(stack_buffer, out, AT_PAGESZ, LINUX_PAGE_SIZE);
    out = write_aux(stack_buffer, out, AT_BASE, 0);
    out = write_aux(stack_buffer, out, AT_FLAGS, 0);
    out = write_aux(stack_buffer, out, AT_ENTRY, elf.entry_point);
    out = write_aux(stack_buffer, out, AT_HWCAP, 0);
    out = write_aux(stack_buffer, out, AT_CLKTCK, 100);
    out = write_aux(stack_buffer, out, AT_UID, 0);
    out = write_aux(stack_buffer, out, AT_EUID, 0);
    out = write_aux(stack_buffer, out, AT_GID, 0);
    out = write_aux(stack_buffer, out, AT_EGID, 0);
    out = write_aux(stack_buffer, out, AT_SECURE, 0);
    out = write_aux(stack_buffer, out, AT_RANDOM, random_guest as Word);
    out = write_aux(stack_buffer, out, AT_EXECFN, execfn_guest as Word);
    out = write_aux(stack_buffer, out, AT_PLATFORM, platform_guest as Word);
    let _ = write_aux(stack_buffer, out, AT_NULL, 0);

    let guest_sp = stack_base + sp_offset as Word;
    libnanami::request_process_memory_write(
        pid,
        stack_base,
        stack_buffer,
        LINUX_EXEC_STACK_BYTES as Word,
    )
    .map_err(map_request_error)?;
    if !register_linux_stack_mappings(runtime, pid) {
        return Err(ENOMEM);
    }
    if process_diagnostics_enabled(runtime, pid) {
        log_exec_image_entry(runtime, pid, elf.entry_point);
        log_exec_stack(runtime, pid, guest_sp);
    }
    if personality == OsPersonality::FreeBsd {
        let fs_base = install_freebsd_exec_tls(runtime, pid, &elf)?;
        write_exec_registers(pcb, elf.entry_point, guest_sp, fs_base, 0, guest_sp, 0, 0)
            .map_err(|_| EIO)?;
        if !runtime.set_fs_base(pid, fs_base) {
            return Err(ESRCH);
        }
    } else {
        write_exec_registers(pcb, elf.entry_point, guest_sp, 0, 0, 0, 0, 0).map_err(|_| EIO)?;
        if !runtime.set_fs_base(pid, 0) {
            return Err(ESRCH);
        }
    }
    Ok(())
}

fn log_exec_image_entry(runtime: &mut Runtime, pid: Word, entry_point: Word) {
    if read_target_memory(runtime, pid, entry_point, 16).is_err() {
        libnanami::println!(
            "[alter/linux] execve entry read-back failed pid={} entry={:#x}",
            pid,
            entry_point
        );
        return;
    }
    let word0 = read_shm_u64(runtime, 0);
    let word1 = read_shm_u64(runtime, 8);
    libnanami::println!(
        "[alter/linux] execve entry pid={} entry={:#x} bytes={:#018x} {:#018x}",
        pid,
        entry_point,
        word0,
        word1
    );
}

fn log_exec_stack(runtime: &mut Runtime, pid: Word, guest_sp: Word) {
    if read_target_memory(runtime, pid, guest_sp, 64).is_err() {
        libnanami::println!(
            "[alter/linux] execve stack read-back failed pid={} sp={:#x}",
            pid,
            guest_sp
        );
        return;
    }
    let argc = read_shm_u64(runtime, 0);
    let argv0 = read_shm_u64(runtime, 8);
    let argv1 = read_shm_u64(runtime, 16);
    let env0 = read_shm_u64(runtime, 24);
    libnanami::println!(
        "[alter/linux] execve stack pid={} sp={:#x} argc={} argv0={:#x} argv1={:#x} env0={:#x}",
        pid,
        guest_sp,
        argc,
        argv0,
        argv1,
        env0
    );
}

fn process_diagnostics_enabled(runtime: &Runtime, pid: Word) -> bool {
    runtime
        .managed_process(pid)
        .map(|process| process.diagnostics_enabled)
        .unwrap_or(false)
}

fn read_guest_pointer_array_args(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
) -> Result<([Word; ALTER_LAUNCH_MAX_ARGS], usize), i32> {
    let mut out = [0; ALTER_LAUNCH_MAX_ARGS];
    let count = read_guest_pointer_array(guest_page, pid, array_ptr, &mut out)?;
    Ok((out, count))
}

fn read_guest_pointer_array_envs(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
) -> Result<([Word; ALTER_LAUNCH_MAX_ENVS], usize), i32> {
    let mut out = [0; ALTER_LAUNCH_MAX_ENVS];
    let count = read_guest_pointer_array(guest_page, pid, array_ptr, &mut out)?;
    Ok((out, count))
}

fn read_guest_pointer_array(
    guest_page: &mut ExecGuestPage,
    pid: Word,
    array_ptr: Word,
    out: &mut [Word],
) -> Result<usize, i32> {
    if array_ptr == 0 {
        return Ok(0);
    }
    let mut count = 0usize;
    while count < out.len() {
        let source = array_ptr
            .checked_add((count as Word).checked_mul(8).ok_or(EFAULT)?)
            .ok_or(EFAULT)?;
        let value = guest_page.read_word(pid, source)?;
        if value == 0 {
            return Ok(count);
        }
        out[count] = value;
        count += 1;
    }
    Ok(count)
}

fn push_snapshot_string_to_stack(
    snapshot: &ExecStringSnapshot,
    stack_buffer: Word,
    cursor: usize,
    snapshot_offset: usize,
    len: usize,
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, i32> {
    if snapshot_offset
        .checked_add(len)
        .filter(|end| *end <= snapshot.used)
        .is_none()
    {
        return Err(EINVAL);
    }
    let bytes = unsafe {
        ::core::slice::from_raw_parts(
            (snapshot.buffer as usize + snapshot_offset) as *const u8,
            len,
        )
    };
    push_bytes_to_stack(stack_buffer, cursor, bytes, guest_address, stack_base)
}

fn push_bytes_to_stack(
    stack_buffer: Word,
    cursor: usize,
    bytes: &[u8],
    guest_address: &mut usize,
    stack_base: Word,
) -> Result<usize, i32> {
    let mut next = cursor.checked_sub(bytes.len() + 1).ok_or(ENOMEM)?;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (stack_buffer as usize + next) as *mut u8,
            bytes.len(),
        );
        ::core::ptr::write((stack_buffer as usize + next + bytes.len()) as *mut u8, 0);
    }
    *guest_address = stack_base as usize + next;
    next = align_down_usize(next, 8);
    Ok(next)
}

fn write_stack_word(stack_buffer: Word, offset: usize, value: Word) {
    unsafe {
        ::core::ptr::write_unaligned((stack_buffer as usize + offset) as *mut Word, value);
    }
}

fn write_aux(stack_buffer: Word, offset: usize, key: Word, value: Word) -> usize {
    write_stack_word(stack_buffer, offset, key);
    write_stack_word(stack_buffer, offset + 8, value);
    offset + 16
}

#[derive(Clone, Copy)]
struct CurrentElfMetadata {
    entry_point: Word,
    program_header_vaddr: Word,
    program_header_entry_size: Word,
    program_header_count: Word,
    tls_vaddr: Word,
    tls_file_size: Word,
    tls_memory_size: Word,
    tls_align: Word,
}

fn preferred_exec_image_base(personality: OsPersonality, metadata: &ElfMetadata) -> Word {
    if metadata.elf_type == 3 {
        return if personality == OsPersonality::FreeBsd {
            FREEBSD_IMAGE_BASE
        } else {
            LINUX_IMAGE_BASE
        };
    }
    align_down_word(metadata.first_load.virtual_address, LINUX_PAGE_SIZE)
}

fn current_elf_metadata_from_loaded(
    metadata: &ElfMetadata,
    image_base: Word,
) -> CurrentElfMetadata {
    let load_bias = if metadata.elf_type == 3 {
        image_base
    } else {
        0
    };
    CurrentElfMetadata {
        entry_point: metadata.entry_point + load_bias,
        program_header_vaddr: if metadata.program_header_vaddr == 0 {
            0
        } else {
            metadata.program_header_vaddr + load_bias
        },
        program_header_entry_size: metadata.program_header_entry_size,
        program_header_count: metadata.program_header_count,
        tls_vaddr: if metadata.tls_vaddr == 0 {
            0
        } else {
            metadata.tls_vaddr + load_bias
        },
        tls_file_size: metadata.tls_file_size,
        tls_memory_size: metadata.tls_memory_size,
        tls_align: metadata.tls_align,
    }
}

fn read_current_elf_metadata(
    runtime: &mut Runtime,
    pid: Word,
    preferred_image_base: Word,
) -> Result<CurrentElfMetadata, i32> {
    let candidates = [
        preferred_image_base,
        LINUX_IMAGE_BASE,
        FREEBSD_IMAGE_BASE,
        NANAMI_IMAGE_BASE,
        0,
    ];
    let mut i = 0usize;
    while i < candidates.len() {
        let candidate = candidates[i];
        if i != 0 && candidates[..i].contains(&candidate) {
            i += 1;
            continue;
        }
        if let Some(metadata) = read_current_elf_metadata_at(runtime, pid, candidate)? {
            return Ok(metadata);
        }
        i += 1;
    }
    Err(EINVAL)
}

fn read_current_elf_metadata_at(
    runtime: &mut Runtime,
    pid: Word,
    image_base: Word,
) -> Result<Option<CurrentElfMetadata>, i32> {
    if libnanami::request_process_memory_read(
        pid,
        image_base,
        runtime.posix_shm,
        LINUX_ELF_HEADER_BYTES,
    )
    .is_err()
    {
        return Ok(None);
    }
    if read_shm_u8(runtime, 0) != 0x7f
        || read_shm_u8(runtime, 1) != b'E'
        || read_shm_u8(runtime, 2) != b'L'
        || read_shm_u8(runtime, 3) != b'F'
    {
        return Ok(None);
    }
    let phoff = read_shm_u64(runtime, 32);
    let phentsize = read_shm_u16(runtime, 54) as Word;
    let phnum = read_shm_u16(runtime, 56) as Word;
    if phentsize < 56 || phnum == 0 {
        return Err(EINVAL);
    }

    let elf_type = read_shm_u16(runtime, 16);
    let load_bias = if elf_type == 3 { image_base } else { 0 };
    let mut metadata = CurrentElfMetadata {
        entry_point: read_shm_u64(runtime, 24) + load_bias,
        program_header_vaddr: image_base + phoff,
        program_header_entry_size: phentsize,
        program_header_count: phnum,
        tls_vaddr: 0,
        tls_file_size: 0,
        tls_memory_size: 0,
        tls_align: 0,
    };
    let mut i = 0;
    while i < phnum {
        let base = phoff + i * phentsize;
        if base + 56 > LINUX_ELF_HEADER_BYTES {
            break;
        }
        let p_type = read_shm_u32(runtime, base as usize);
        if p_type == LINUX_PT_TLS {
            metadata.tls_vaddr = read_shm_u64(runtime, (base + 16) as usize) + load_bias;
            metadata.tls_file_size = read_shm_u64(runtime, (base + 32) as usize);
            metadata.tls_memory_size = read_shm_u64(runtime, (base + 40) as usize);
            metadata.tls_align = read_shm_u64(runtime, (base + 48) as usize);
        }
        if p_type == LINUX_PT_LOAD {
            let offset = read_shm_u64(runtime, (base + 8) as usize);
            let vaddr = read_shm_u64(runtime, (base + 16) as usize);
            let filesz = read_shm_u64(runtime, (base + 32) as usize);
            if phoff >= offset && phoff < offset.saturating_add(filesz) {
                metadata.program_header_vaddr = vaddr + load_bias + (phoff - offset);
            }
        }
        i += 1;
    }
    Ok(Some(metadata))
}

fn install_freebsd_exec_tls(
    runtime: &mut Runtime,
    pid: Word,
    elf: &CurrentElfMetadata,
) -> Result<Word, i32> {
    if elf.tls_memory_size == 0 {
        return Ok(0);
    }
    if elf.tls_file_size > elf.tls_memory_size {
        return Err(EINVAL);
    }
    let total = align_up_word(elf.tls_memory_size + 16, 16);
    if total == 0 || total > runtime.posix_shm_size {
        return Err(ENOMEM);
    }
    let (tls_base, mapped) =
        libnanami::request_process_map_anonymous(pid, total).map_err(map_request_error)?;
    if mapped < total {
        return Err(ENOMEM);
    }
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, total as usize);
    }
    if elf.tls_file_size != 0 {
        libnanami::request_process_memory_read(
            pid,
            elf.tls_vaddr,
            runtime.posix_shm,
            elf.tls_file_size,
        )
        .map_err(map_request_error)?;
    }
    let fs_base = tls_base + elf.tls_memory_size;
    unsafe {
        ::core::ptr::write_unaligned(
            (runtime.posix_shm + elf.tls_memory_size) as *mut Word,
            fs_base,
        );
    }
    libnanami::request_process_memory_write(pid, tls_base, runtime.posix_shm, total)
        .map_err(map_request_error)?;
    libnanami::println!(
        "[alter/freebsd] exec tls pid={} base={:#x} fs={:#x} file={:#x} mem={:#x}",
        pid,
        tls_base,
        fs_base,
        elf.tls_file_size,
        elf.tls_memory_size
    );
    Ok(fs_base)
}

fn read_c_string(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let max = ::core::cmp::min(runtime.posix_shm_size as usize, 4096);
    let mut copied = 0usize;
    while copied < max {
        let source = user_ptr.checked_add(copied as Word).ok_or(EFAULT)?;
        let page_remaining = (LINUX_PAGE_SIZE - (source & (LINUX_PAGE_SIZE - 1))) as usize;
        let chunk = ::core::cmp::min(max - copied, page_remaining);
        libnanami::request_process_memory_read(
            pid,
            source,
            runtime.posix_shm + copied as Word,
            chunk as Word,
        )
        .map_err(map_request_error)?;

        let end = copied + chunk;
        while copied < end {
            let byte =
                unsafe { ::core::ptr::read((runtime.posix_shm + copied as Word) as *const u8) };
            if byte == 0 {
                return Ok(copied as Word);
            }
            copied += 1;
        }
    }
    Err(ENAMETOOLONG)
}

fn resolve_path(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    let len = read_c_string(runtime, pid, user_ptr)?;
    resolve_current_shm_path(runtime, pid, len)
}

fn translate_guest_path_for_vfs(runtime: &mut Runtime, pid: Word, len: Word) -> Result<Word, i32> {
    translate_guest_path_at(runtime, pid, 0, len)
}

fn translate_guest_path_at(
    runtime: &mut Runtime,
    pid: Word,
    offset: Word,
    len: Word,
) -> Result<Word, i32> {
    if len == 0 {
        return Err(EINVAL);
    }
    if offset
        .checked_add(len)
        .and_then(|value| value.checked_add(1))
        .filter(|end| *end <= runtime.posix_shm_size)
        .is_none()
    {
        return Err(ENAMETOOLONG);
    }

    let base = runtime.posix_shm + offset;
    if !path_is_absolute(base, len) || is_linux_virtual_path(base, len) {
        return Ok(len);
    }

    if path_has_component_prefix(base, len, b"/temp") {
        let suffix_len = len as usize - b"/temp".len();
        unsafe {
            ::core::ptr::copy(
                (base + b"/temp".len() as Word) as *const u8,
                (base + b"/tmp".len() as Word) as *mut u8,
                suffix_len,
            );
            ::core::ptr::copy_nonoverlapping(b"/tmp".as_ptr(), base as *mut u8, b"/tmp".len());
            ::core::ptr::write(
                (base + b"/tmp".len() as Word + suffix_len as Word) as *mut u8,
                0,
            );
        }
        return translate_guest_path_at(
            runtime,
            pid,
            offset,
            b"/tmp".len() as Word + suffix_len as Word,
        );
    }

    let guest_root = guest_root_for_process(runtime, pid)?;
    if path_equals(base, len, guest_root) || path_has_root_prefix(base, len, guest_root) {
        return Ok(len);
    }

    let len_usize = len as usize;
    if len_usize >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut original = [0u8; LINUX_CWD_MAX];
    unsafe {
        ::core::ptr::copy_nonoverlapping(base as *const u8, original.as_mut_ptr(), len_usize);
    }

    let suffix_len = if len_usize == 1 { 0 } else { len_usize };
    let new_len = guest_root.len() + suffix_len;
    if offset
        .checked_add(new_len as Word)
        .and_then(|value| value.checked_add(1))
        .filter(|end| *end <= runtime.posix_shm_size)
        .is_none()
    {
        return Err(ENAMETOOLONG);
    }

    unsafe {
        ::core::ptr::copy_nonoverlapping(guest_root.as_ptr(), base as *mut u8, guest_root.len());
        if suffix_len != 0 {
            ::core::ptr::copy_nonoverlapping(
                original.as_ptr(),
                (base + guest_root.len() as Word) as *mut u8,
                suffix_len,
            );
        }
        ::core::ptr::write((base + new_len as Word) as *mut u8, 0);
    }
    Ok(new_len as Word)
}

fn is_linux_virtual_path(base: Word, len: Word) -> bool {
    path_has_component_prefix(base, len, b"/dev")
        || path_has_component_prefix(base, len, b"/proc")
        || path_has_component_prefix(base, len, b"/sys")
}

fn reject_virtual_fs_mutation(base: Word, len: Word) -> Result<(), i32> {
    if is_linux_virtual_path(base, len) {
        Err(EROFS)
    } else {
        Ok(())
    }
}

fn path_has_component_prefix(base: Word, len: Word, prefix: &[u8]) -> bool {
    if (len as usize) < prefix.len() {
        return false;
    }
    let path = unsafe { ::core::slice::from_raw_parts(base as *const u8, len as usize) };
    path.starts_with(prefix) && (path.len() == prefix.len() || path[prefix.len()] == b'/')
}

fn current_virtual_node(runtime: &Runtime, pid: Word, len: Word) -> Option<VirtualNode> {
    let path =
        unsafe { ::core::slice::from_raw_parts(runtime.posix_shm as *const u8, len as usize) };
    virtual_fs::lookup(path, graphics_enabled(runtime, pid))
}

fn guest_root_for_process(runtime: &Runtime, pid: Word) -> Result<&'static [u8], i32> {
    let Some(process) = runtime.managed_process(pid) else {
        return Err(ESRCH);
    };
    Ok(personality::root(process.personality))
}

fn resolve_current_shm_path(
    runtime: &mut Runtime,
    pid: Word,
    input_len: Word,
) -> Result<Word, i32> {
    if input_len as usize >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    let mut input = [0u8; LINUX_CWD_MAX];
    let input_len = input_len as usize;
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            input.as_mut_ptr(),
            input_len,
        );
    }

    let mut out = [0u8; LINUX_CWD_MAX];
    let mut out_len: usize;
    if input_len != 0 && input[0] == b'/' {
        out[0] = b'/';
        out_len = 1;
    } else {
        let Some(process) = runtime.managed_process(pid) else {
            return Err(ESRCH);
        };
        if process.cwd_len == 0 || process.cwd_len >= LINUX_CWD_MAX {
            return Err(EINVAL);
        }
        out[..process.cwd_len].copy_from_slice(&process.cwd[..process.cwd_len]);
        out_len = process.cwd_len;
    }

    let mut i = 0usize;
    while i < input_len {
        while i < input_len && input[i] == b'/' {
            i += 1;
        }
        let start = i;
        while i < input_len && input[i] != b'/' {
            i += 1;
        }
        let len = i - start;
        if len == 0 || (len == 1 && input[start] == b'.') {
            continue;
        }
        if len == 2 && input[start] == b'.' && input[start + 1] == b'.' {
            out_len = pop_path_component(&mut out, out_len);
            continue;
        }
        out_len = push_path_component(&mut out, out_len, &input[start..start + len])?;
    }
    if out_len == 0 {
        out[0] = b'/';
        out_len = 1;
    }
    unsafe {
        ::core::ptr::copy_nonoverlapping(out.as_ptr(), runtime.posix_shm as *mut u8, out_len);
        ::core::ptr::write((runtime.posix_shm + out_len as Word) as *mut u8, 0);
    }
    Ok(out_len as Word)
}

fn push_path_component(
    out: &mut [u8; LINUX_CWD_MAX],
    mut out_len: usize,
    component: &[u8],
) -> Result<usize, i32> {
    if out_len == 0 {
        out[0] = b'/';
        out_len = 1;
    }
    if out_len != 1 {
        if out_len + 1 >= LINUX_CWD_MAX {
            return Err(ENAMETOOLONG);
        }
        out[out_len] = b'/';
        out_len += 1;
    }
    if out_len + component.len() >= LINUX_CWD_MAX {
        return Err(ENAMETOOLONG);
    }
    out[out_len..out_len + component.len()].copy_from_slice(component);
    Ok(out_len + component.len())
}

fn pop_path_component(out: &mut [u8; LINUX_CWD_MAX], mut out_len: usize) -> usize {
    if out_len <= 1 {
        out[0] = b'/';
        return 1;
    }
    while out_len > 1 && out[out_len - 1] == b'/' {
        out_len -= 1;
    }
    while out_len > 1 && out[out_len - 1] != b'/' {
        out_len -= 1;
    }
    if out_len > 1 {
        out_len -= 1;
    }
    out_len.max(1)
}

fn read_target_memory(runtime: &Runtime, pid: Word, user_ptr: Word, len: Word) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.posix_shm, len)
        .map_err(map_request_error)
}

fn write_target_memory(runtime: &Runtime, pid: Word, user_ptr: Word, len: Word) -> Result<(), i32> {
    write_target_memory_from(pid, user_ptr, runtime.posix_shm, len)
}

fn write_target_memory_from(pid: Word, user_ptr: Word, source: Word, len: Word) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    libnanami::request_process_memory_write(pid, user_ptr, source, len).map_err(map_request_error)
}

fn write_u32_to_target(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: u32,
) -> Result<(), i32> {
    unsafe {
        write_u32(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 4)
}

fn write_guest_u32(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: u32,
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u32(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 4)
}

fn write_guest_u64(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: Word,
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u64(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 8)
}

fn write_linux_stat(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    stat: (Word, Word, Word, Word, Word),
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let (inode, size, kind, major, minor) = stat;
    let mode = linux_mode_for_kind(kind);
    let rdev = ((major & 0xfff) << 8) | (minor & 0xff);
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_STAT_SIZE);
        write_u64(runtime.posix_shm, 0);
        write_u64(runtime.posix_shm + 8, inode);
        write_u64(runtime.posix_shm + 16, 1);
        write_u32(runtime.posix_shm + 24, mode as u32);
        write_u32(runtime.posix_shm + 28, 0);
        write_u32(runtime.posix_shm + 32, 0);
        write_u64(runtime.posix_shm + 40, rdev);
        write_u64(runtime.posix_shm + 48, size);
        write_u64(runtime.posix_shm + 56, 4096);
        write_u64(runtime.posix_shm + 64, align_up_word(size, 512) / 512);
    }
    write_target_memory(runtime, pid, user_ptr, LINUX_STAT_SIZE as Word)
}

fn write_linux_statx(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    stat: (Word, Word, Word, Word, Word),
) -> Result<(), i32> {
    let (inode, size, kind, _major, _minor) = stat;
    let mode = linux_mode_for_kind(kind);
    unsafe {
        ::core::ptr::write_bytes(runtime.posix_shm as *mut u8, 0, LINUX_STATX_SIZE);
        write_u32(runtime.posix_shm, LINUX_STATX_BASIC_STATS);
        write_u32(runtime.posix_shm + 4, 4096);
        write_u32(runtime.posix_shm + 16, 1);
        write_u32(runtime.posix_shm + 20, 0);
        write_u32(runtime.posix_shm + 24, 0);
        write_u16(runtime.posix_shm + 28, mode as u16);
        write_u64(runtime.posix_shm + 32, inode);
        write_u64(runtime.posix_shm + 40, size);
        write_u64(runtime.posix_shm + 48, align_up_word(size, 512) / 512);
        write_u64(runtime.posix_shm + 56, LINUX_STATX_BASIC_STATS as Word);
    }
    write_target_memory(runtime, pid, user_ptr, LINUX_STATX_SIZE as Word)
}

fn linux_mode_for_kind(kind: Word) -> Word {
    match kind {
        posix::POSIX_FILE_TYPE_DIRECTORY => 0o040000 | 0o755,
        posix::POSIX_FILE_TYPE_CHAR_DEVICE => 0o020000 | 0o666,
        posix::POSIX_FILE_TYPE_BLOCK_DEVICE => 0o060000 | 0o666,
        POSIX_FILE_TYPE_PIPE => LINUX_S_IFIFO | 0o600,
        POSIX_FILE_TYPE_SOCKET => LINUX_S_IFSOCK | 0o600,
        _ => 0o100000 | 0o644,
    }
}

fn write_uts_field(base: Word, offset: Word, value: &[u8]) {
    let mut i = 0usize;
    while i < value.len() && i < 64 {
        unsafe {
            ::core::ptr::write((base + offset + i as Word) as *mut u8, value[i]);
        }
        i += 1;
    }
}

fn translate_open_flags(flags: Word) -> Word {
    let mut out = 0;
    let _ignored = flags & LINUX_O_LARGEFILE;
    if (flags & LINUX_O_CREAT) != 0 {
        out |= posix::POSIX_O_CREAT;
    }
    if (flags & LINUX_O_TRUNC) != 0 {
        out |= posix::POSIX_O_TRUNC;
    }
    if (flags & LINUX_O_DIRECTORY) != 0 {
        out |= posix::POSIX_O_DIRECTORY;
    }
    out
}

fn bounded_len(runtime: &Runtime, len: Word) -> Result<Word, i32> {
    let limit = runtime
        .posix_shm_size
        .saturating_sub(ALTER_IO_OFFSET as Word)
        .max(1);
    Ok(::core::cmp::min(len, limit))
}

fn terminal_bounded_len(runtime: &Runtime, len: Word) -> Result<Word, i32> {
    Ok(::core::cmp::min(len, runtime.terminal_shm_size.max(1)))
}

fn terminal_id_for_pid(runtime: &Runtime, pid: Word) -> Result<Word, i32> {
    let terminal_id = runtime
        .managed_process(pid)
        .map(|process| process.terminal_id)
        .ok_or(ESRCH)?;
    if terminal_id == 0 {
        return Err(ENOTTY);
    }
    Ok(terminal_id)
}

pub fn wake_terminal_readers(runtime: &mut Runtime) {
    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.terminal_read_waiting {
            index += 1;
            continue;
        }

        let result = sys_terminal_read_now(
            runtime,
            process.pid,
            process.terminal_read_buffer,
            process.terminal_read_len,
        );
        let return_value = match result {
            Ok(Some(bytes)) => bytes as isize,
            Ok(None) => {
                index += 1;
                continue;
            }
            Err(errno) => -(errno as isize),
        };

        runtime.managed[index].terminal_read_waiting = false;
        runtime.managed[index].terminal_read_buffer = 0;
        runtime.managed[index].terminal_read_len = 0;
        runtime.managed[index].terminal_read_context = LinuxSyscallContext::EMPTY;

        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.terminal_read_context,
            return_value,
            process.personality,
        )
        .is_err()
        {
            libnanami::println!(
                "[alter/{}] terminal wake register write failed pid={} pcb={:#x}",
                personality::name(process.personality),
                process.pid,
                process.pcb
            );
            index += 1;
            continue;
        }
        if let Err(error) = a9n_abi::arch::process_control_block::resume(process.pcb) {
            libnanami::println!(
                "[alter/linux] terminal wake resume failed pid={} pcb={:#x} err={:?}",
                process.pid,
                process.pcb,
                error
            );
        }
        index += 1;
    }
}

pub fn handle_timer_notification(runtime: &mut Runtime, identifier: Word) {
    if (identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) == 0
        || !runtime.clock_timer_armed
    {
        return;
    }
    let previous_ticks = runtime.monotonic_ticks;
    let elapsed_ticks =
        match nanami_services::timer::timer_service_monotonic_ticks(runtime.timer_port) {
            Ok((ticks, tick_hz)) if tick_hz != 0 => {
                runtime.monotonic_ticks = ticks;
                runtime.monotonic_tick_hz = tick_hz;
                ticks.saturating_sub(previous_ticks).max(1)
            }
            _ => {
                runtime.monotonic_ticks = runtime.monotonic_ticks.saturating_add(1);
                1
            }
        };

    let tick_hz = runtime.monotonic_tick_hz;
    if tick_hz != 0
        && previous_ticks.saturating_mul(ALTER_FB_PRESENT_HZ) / tick_hz
            != runtime.monotonic_ticks.saturating_mul(ALTER_FB_PRESENT_HZ) / tick_hz
    {
        present_mapped_framebuffers(runtime);
    }

    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.sleep_waiting {
            index += 1;
            continue;
        }
        if process.sleep_ticks_remaining > elapsed_ticks {
            runtime.managed[index].sleep_ticks_remaining -= elapsed_ticks;
            index += 1;
            continue;
        }

        runtime.managed[index].sleep_waiting = false;
        runtime.managed[index].sleep_ticks_remaining = 0;
        runtime.managed[index].sleep_context = LinuxSyscallContext::EMPTY;
        record_syscall_result(runtime, process.pid, SYS_NANOSLEEP, 0);
        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.sleep_context,
            0,
            process.personality,
        )
        .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(process.pcb);
        }
        index += 1;
    }
}

pub fn wake_device_readers(runtime: &mut Runtime) {
    pump_input_events(runtime);
    let mut index = 0usize;
    while index < runtime.managed.len() {
        let process = runtime.managed[index];
        if process.pid == 0 || !process.device_read_waiting {
            index += 1;
            continue;
        }
        let result = sys_evdev_read(
            runtime,
            process.pid,
            process.device_read_fd,
            process.device_read_buffer,
            process.device_read_len,
        );
        let return_value = match result {
            Err(EAGAIN) => {
                index += 1;
                continue;
            }
            Ok(bytes) => bytes as isize,
            Err(errno) => -(errno as isize),
        };
        runtime.managed[index].device_read_waiting = false;
        runtime.managed[index].device_read_fd = 0;
        runtime.managed[index].device_read_buffer = 0;
        runtime.managed[index].device_read_len = 0;
        runtime.managed[index].device_read_context = LinuxSyscallContext::EMPTY;
        if crate::process::write_personality_syscall_return(
            process.pcb,
            process.device_read_context,
            return_value,
            process.personality,
        )
        .is_ok()
        {
            let _ = a9n_abi::arch::process_control_block::resume(process.pcb);
        }
        index += 1;
    }
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

fn retry_network_syscall(
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

fn ensure_standard_terminal_fd(runtime: &mut Runtime, pid: Word, fd: Word) {
    if fd > 2 || runtime.linux_file(pid, fd).is_some() {
        return;
    }
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    if process.terminal_id == 0 {
        return;
    }
    if runtime.set_linux_file(pid, fd, LinuxFile::terminal()) {
        libnanami::println!(
            "[alter/linux] restored terminal stdio pid={} fd={}",
            pid,
            fd
        );
    }
}

fn linux_file_kind_name(kind: LinuxFileKind) -> &'static str {
    match kind {
        LinuxFileKind::Empty => "empty",
        LinuxFileKind::Posix => "posix",
        LinuxFileKind::Terminal => "terminal",
        LinuxFileKind::PipeRead => "pipe-read",
        LinuxFileKind::PipeWrite => "pipe-write",
        LinuxFileKind::SocketUdp => "socket-udp",
        LinuxFileKind::SocketTcp => "socket-tcp",
        LinuxFileKind::SocketTcpListener => "socket-tcp-listener",
        LinuxFileKind::SocketIcmp => "socket-icmp",
        LinuxFileKind::SocketNetlink => "socket-netlink",
        LinuxFileKind::VirtualDirectory => "virtual-directory",
        LinuxFileKind::VirtualFile => "virtual-file",
        LinuxFileKind::EvdevKeyboard => "evdev-keyboard",
        LinuxFileKind::EvdevMouse => "evdev-mouse",
        LinuxFileKind::Framebuffer => "framebuffer",
    }
}

fn duplicate_linux_file(runtime: &mut Runtime, file: LinuxFile) -> Result<LinuxFile, i32> {
    match file.kind {
        LinuxFileKind::Empty => Err(EBADF),
        LinuxFileKind::Terminal
        | LinuxFileKind::VirtualDirectory
        | LinuxFileKind::VirtualFile
        | LinuxFileKind::EvdevKeyboard
        | LinuxFileKind::EvdevMouse
        | LinuxFileKind::Framebuffer => Ok(file),
        LinuxFileKind::PipeRead => {
            let pipe = runtime.pipe_mut(file.posix_fd).ok_or(EBADF)?;
            pipe.readers = pipe.readers.saturating_add(1);
            Ok(file)
        }
        LinuxFileKind::PipeWrite => {
            let pipe = runtime.pipe_mut(file.posix_fd).ok_or(EBADF)?;
            pipe.writers = pipe.writers.saturating_add(1);
            Ok(file)
        }
        LinuxFileKind::SocketUdp
        | LinuxFileKind::SocketTcp
        | LinuxFileKind::SocketTcpListener
        | LinuxFileKind::SocketIcmp
        | LinuxFileKind::SocketNetlink => Ok(file),
        LinuxFileKind::Posix => {
            let fd = duplicate_posix_backend_fd(runtime, file.posix_fd)?;
            Ok(LinuxFile::posix(fd, file.flags))
        }
    }
}

fn duplicate_posix_backend_fd(runtime: &mut Runtime, old_fd: Word) -> Result<Word, i32> {
    let mut low_fds = [usize::MAX as Word; 3];
    let mut low_count = 0usize;
    loop {
        let fd = posix::posix_dup(runtime.posix_port, old_fd).map_err(map_request_error)?;
        if fd >= 3 {
            let mut i = 0usize;
            while i < low_count {
                let _ = posix::posix_close(runtime.posix_port, low_fds[i]);
                i += 1;
            }
            return Ok(fd);
        }
        if low_count >= low_fds.len() {
            let _ = posix::posix_close(runtime.posix_port, fd);
            let mut i = 0usize;
            while i < low_count {
                let _ = posix::posix_close(runtime.posix_port, low_fds[i]);
                i += 1;
            }
            return Err(EMFILE);
        }
        low_fds[low_count] = fd;
        low_count += 1;
    }
}

fn close_linux_fd(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<(), i32> {
    if let Some(file) = runtime.clear_linux_file(pid, fd) {
        match file.kind {
            LinuxFileKind::Posix => {
                posix::posix_close(runtime.posix_port, file.posix_fd).map_err(map_request_error)?;
            }
            LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite => release_pipe_file(runtime, file),
            LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink => close_socket_file(runtime, file),
            LinuxFileKind::Terminal
            | LinuxFileKind::VirtualDirectory
            | LinuxFileKind::VirtualFile
            | LinuxFileKind::EvdevKeyboard
            | LinuxFileKind::EvdevMouse
            | LinuxFileKind::Framebuffer
            | LinuxFileKind::Empty => {}
        }
    }
    Ok(())
}

fn close_socket_file(runtime: &mut Runtime, file: LinuxFile) {
    if socket_file_still_open(runtime, file) || runtime.network_port == 0 {
        return;
    }
    match file.kind {
        LinuxFileKind::SocketUdp if file.local_port != 0 => {
            let _ = net::net_service_control(
                runtime.network_port,
                net::NET_SERVICE_CONTROL_UDP_UNBIND,
                file.local_port as Word,
                0,
            );
        }
        LinuxFileKind::SocketTcpListener if file.local_port != 0 => {
            let _ = net::net_service_control(
                runtime.network_port,
                net::NET_SERVICE_CONTROL_TCP_UNBIND,
                file.local_port as Word,
                0,
            );
        }
        LinuxFileKind::SocketTcp if file.posix_fd != 0 => {
            let _ = net::net_service_tcp_send_on_connection(
                runtime.network_port,
                file.posix_fd,
                0,
                0,
                TCP_FLAG_FIN | TCP_FLAG_ACK,
            );
        }
        _ => {}
    }
}

fn socket_file_still_open(runtime: &Runtime, file: LinuxFile) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|candidate| {
                candidate.is_open()
                    && candidate.kind == file.kind
                    && candidate.posix_fd == file.posix_fd
                    && candidate.local_port == file.local_port
            })
    })
}

fn release_pipe_file(runtime: &mut Runtime, file: LinuxFile) {
    let Some(pipe) = runtime.pipe_mut(file.posix_fd) else {
        return;
    };
    match file.kind {
        LinuxFileKind::PipeRead => {
            pipe.readers = pipe.readers.saturating_sub(1);
        }
        LinuxFileKind::PipeWrite => {
            pipe.writers = pipe.writers.saturating_sub(1);
        }
        _ => {}
    }
    if pipe.readers == 0 && pipe.writers == 0 {
        release_pipe_id(runtime, file.posix_fd);
    }
}

fn release_pipe_id(runtime: &mut Runtime, pipe_id: Word) {
    let index = pipe_id as usize;
    if index < runtime.pipes.len() {
        runtime.pipes[index] = crate::state::LinuxPipe::EMPTY;
    }
}

pub fn close_process_files(runtime: &mut Runtime, pid: Word) {
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        let _ = close_linux_fd(runtime, pid, fd as Word);
        fd += 1;
    }
    cleanup_graphics_for_process(runtime, pid);
}

fn cleanup_graphics_for_process(runtime: &mut Runtime, pid: Word) {
    let Some(process) = runtime.managed_process(pid) else {
        return;
    };
    let session_id = process.graphics_session;
    if session_id == 0 {
        return;
    }
    let index = session_id as usize - 1;
    if index >= runtime.graphics.len() || !runtime.graphics[index].active {
        return;
    }
    if runtime.graphics[index].root_pid != pid {
        return;
    }
    if let Some(successor) = runtime.managed.iter().find(|candidate| {
        candidate.pid != 0
            && candidate.pid != pid
            && candidate.graphics_session == session_id
            && !candidate.exited
    }) {
        runtime.graphics[index].root_pid = successor.pid;
        return;
    }
    let session = runtime.graphics[index];
    let _ = honoka::honoka_destroy_window(session.honoka_port, session.window_id);
    if session.input_queue != 0 {
        let _ =
            libnanami::request_mapping_release(session.input_queue, input::INPUT_EVENT_QUEUE_BYTES);
    }
    runtime.graphics[index] = crate::state::GraphicsSession::EMPTY;
}

fn close_cloexec_files(runtime: &mut Runtime, pid: Word) {
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        if let Some(file) = runtime.linux_file(pid, fd as Word) {
            if (file.flags & LINUX_FD_CLOEXEC) != 0 {
                let _ = close_linux_fd(runtime, pid, fd as Word);
            }
        }
        fd += 1;
    }
}

fn inherit_linux_files(
    runtime: &mut Runtime,
    parent_pid: Word,
    child_pid: Word,
) -> Result<(), i32> {
    let parent = runtime.managed_process(parent_pid).ok_or(ESRCH)?;
    if !runtime.set_cwd(child_pid, &parent.cwd[..parent.cwd_len]) {
        return Err(EINVAL);
    }
    let mut fd = 0usize;
    while fd < LINUX_FD_MAX {
        let file = parent.files[fd];
        if file.is_open() {
            let child_file = match duplicate_linux_file(runtime, file) {
                Ok(file) => file,
                Err(error) => {
                    close_process_files(runtime, child_pid);
                    return Err(error);
                }
            };
            if !runtime.set_linux_file(child_pid, fd as Word, child_file) {
                if child_file.kind == LinuxFileKind::Posix {
                    let _ = posix::posix_close(runtime.posix_port, child_file.posix_fd);
                }
                close_process_files(runtime, child_pid);
                return Err(EMFILE);
            }
        }
        fd += 1;
    }
    Ok(())
}

fn path_has_root_prefix(base: Word, len: Word, root: &[u8]) -> bool {
    if len as usize <= root.len() {
        return false;
    }
    let mut i = 0usize;
    while i < root.len() {
        let byte = unsafe { ::core::ptr::read((base + i as Word) as *const u8) };
        if byte != root[i] {
            return false;
        }
        i += 1;
    }
    unsafe { ::core::ptr::read((base + root.len() as Word) as *const u8) == b'/' }
}

fn path_is_absolute(base: Word, len: Word) -> bool {
    len != 0 && unsafe { ::core::ptr::read(base as *const u8) } == b'/'
}

fn is_at_fdcwd(fd: Word) -> bool {
    fd == LINUX_AT_FDCWD
}

fn basename_in_bytes(bytes: &[u8], len: usize) -> (usize, usize) {
    let mut start = 0usize;
    let mut i = 0usize;
    while i < len {
        if bytes[i] == b'/' {
            start = i + 1;
        }
        i += 1;
    }
    (start, len.saturating_sub(start))
}

fn path_equals(base: Word, len: Word, expected: &[u8]) -> bool {
    if len as usize != expected.len() {
        return false;
    }
    let mut i = 0usize;
    while i < expected.len() {
        let byte = unsafe { ::core::ptr::read((base + i as Word) as *const u8) };
        if byte != expected[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn map_word(result: Result<Word, RequestError>) -> Result<Word, i32> {
    result.map_err(map_request_error)
}

fn map_unit(result: Result<(), RequestError>, value: Word) -> Result<Word, i32> {
    result.map(|_| value).map_err(map_request_error)
}

fn map_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument => EINVAL,
        RequestError::Unsupported => ENOSYS,
        RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT) => EINVAL,
        RequestError::Status(libnanami::OS_RESPONSE_PERMISSION_DENIED) => EACCES,
        RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION) => EIO,
        RequestError::Status(_) | RequestError::Transport | RequestError::Protocol => EIO,
    }
}

fn map_path_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument
        | RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT)
        | RequestError::Status(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR)
        | RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION) => ENOENT,
        other => map_request_error(other),
    }
}

fn map_network_error(error: RequestError) -> i32 {
    match error {
        RequestError::InvalidArgument
        | RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT) => EINVAL,
        RequestError::Status(libnanami::OS_RESPONSE_PERMISSION_DENIED) => EACCES,
        RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION) => EAGAIN,
        _ => ENETDOWN,
    }
}

fn map_network_bind_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION) => EADDRINUSE,
        other => map_network_error(other),
    }
}

fn map_create_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT) => EEXIST,
        other => map_path_request_error(other),
    }
}

fn result_to_linux_return(result: Result<Word, i32>) -> isize {
    match result {
        Ok(value) => value as isize,
        Err(errno) => -(errno as isize),
    }
}

fn align_up_word(value: Word, align: Word) -> Word {
    (value + align - 1) & !(align - 1)
}

fn align_down_word(value: Word, align: Word) -> Word {
    value & !(align - 1)
}

fn align_down_usize(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

fn read_shm_u8(runtime: &Runtime, offset: usize) -> u8 {
    unsafe { ::core::ptr::read((runtime.posix_shm as usize + offset) as *const u8) }
}

fn move_shm_bytes(runtime: &Runtime, src_offset: Word, dst_offset: Word, len: Word) {
    if len == 0 {
        return;
    }
    unsafe {
        ::core::ptr::copy(
            (runtime.posix_shm + src_offset) as *const u8,
            (runtime.posix_shm + dst_offset) as *mut u8,
            len as usize,
        );
    }
}

fn read_shm_u16(runtime: &Runtime, offset: usize) -> u16 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u16) }
}

fn read_shm_u32(runtime: &Runtime, offset: usize) -> u32 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u32) }
}

fn read_shm_u64(runtime: &Runtime, offset: usize) -> Word {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const Word) }
}

unsafe fn write_u32(address: Word, value: u32) {
    ::core::ptr::write_unaligned(address as *mut u32, value);
}

unsafe fn write_u16(address: Word, value: u16) {
    ::core::ptr::write_unaligned(address as *mut u16, value);
}

unsafe fn write_u8(address: Word, value: u8) {
    ::core::ptr::write(address as *mut u8, value);
}

unsafe fn write_u64(address: Word, value: Word) {
    ::core::ptr::write_unaligned(address as *mut Word, value);
}

const ENOENT: i32 = 2;
const EAGAIN: i32 = 11;
const EINTR: i32 = 4;
const EIO: i32 = 5;
const EBADF: i32 = 9;
const EACCES: i32 = 13;
const EFAULT: i32 = 14;
const ECHILD: i32 = 10;
const EEXIST: i32 = 17;
const ENODEV: i32 = 19;
const EISDIR: i32 = 21;
const EPIPE: i32 = 32;
const EINVAL: i32 = 22;
const EMFILE: i32 = 24;
const ENOTTY: i32 = 25;
const ESPIPE: i32 = 29;
const EROFS: i32 = 30;
const ENOSYS: i32 = 38;
const ENAMETOOLONG: i32 = 36;
const ENOTDIR: i32 = 20;
const ENOMEM: i32 = 12;
const ERANGE: i32 = 34;
const ESRCH: i32 = 3;
const ENOEXEC: i32 = 8;
const EDESTADDRREQ: i32 = 89;
const EMSGSIZE: i32 = 90;
const EPROTONOSUPPORT: i32 = 93;
const ESOCKTNOSUPPORT: i32 = 94;
const EOPNOTSUPP: i32 = 95;
const EAFNOSUPPORT: i32 = 97;
const EADDRINUSE: i32 = 98;
const EADDRNOTAVAIL: i32 = 99;
const ENETDOWN: i32 = 100;
const ENOTCONN: i32 = 107;
const ENOTSOCK: i32 = 88;
const EINPROGRESS: i32 = 115;

const LINUX_STAT_SIZE: usize = 144;
const LINUX_STATX_SIZE: usize = 256;
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_CPU_MASK_BYTES: Word = 8;
const LINUX_ITIMERVAL_BYTES: Word = 32;
const LINUX_ITIMER_PROF: Word = 2;
const LINUX_VIRTUAL_RESERVATION_BASE: Word = 0x0000_0001_0000_0000;
const LINUX_VIRTUAL_RESERVATION_LIMIT: Word = 0x0000_4000_0000_0000;
const LINUX_IMAGE_BASE: Word = 0x400000;
const FREEBSD_IMAGE_BASE: Word = 0x200000;
const NANAMI_IMAGE_BASE: Word = 0x1000000;
const LINUX_ELF_HEADER_BYTES: Word = 4096;
const LINUX_PT_LOAD: u32 = 1;
const LINUX_PT_TLS: u32 = 7;
const LINUX_PF_W: u32 = 2;
const LINUX_MAX_LOAD_SEGMENTS: usize = 16;
const LINUX_STACK_TOP: Word = 0x4040000;
const LINUX_STACK_BYTES: Word = 0x40000;
const LINUX_STACK_GUARD_BYTES: Word = 0x3000;
const LINUX_EXEC_STACK_BYTES: usize = 0x4000;
const LINUX_EXEC_SNAPSHOT_BYTES: usize = 0x4000;
const LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES: usize =
    LINUX_EXEC_SNAPSHOT_BYTES + LINUX_PAGE_SIZE as usize;
const LINUX_EXEC_STRING_MAX: usize = 4096;
const LINUX_EXEC_PATH_MAX: usize = 256;
const LINUX_AT_FDCWD: Word = (-100isize) as Word;
const LINUX_AT_REMOVEDIR: Word = 0x200;
const LINUX_AT_EMPTY_PATH: Word = 0x1000;
const LINUX_PROT_NONE: Word = 0x0;
const LINUX_PROT_READ: Word = 0x1;
const LINUX_PROT_WRITE: Word = 0x2;
const LINUX_PROT_EXEC: Word = 0x4;
const LINUX_PROT_ALL: Word = LINUX_PROT_READ | LINUX_PROT_WRITE | LINUX_PROT_EXEC;
const LINUX_DIRECT_COPY_CHUNK: Word = 0x10_0000;
const NETWORK_PAYLOAD_OFFSET: Word = 32;
const UDP_SOCKET_PAYLOAD_MAX: Word = 1472;
const TCP_SOCKET_PAYLOAD_MAX: Word = 1460;
const ICMP_SOCKET_PAYLOAD_MAX: Word = 1480;
const NETWORK_SEND_RETRIES: usize = 16;
const LINUX_AF_INET: Word = 2;
const LINUX_AF_NETLINK: Word = 16;
const LINUX_SOCK_STREAM: Word = 1;
const LINUX_SOCK_DGRAM: Word = 2;
const LINUX_SOCK_RAW: Word = 3;
const LINUX_SOCK_TYPE_MASK: Word = 0xf;
const LINUX_SOCK_NONBLOCK: Word = 0x800;
const LINUX_SOCK_CLOEXEC: Word = 0x80000;
const LINUX_IPPROTO_TCP: Word = 6;
const LINUX_IPPROTO_UDP: Word = 17;
const LINUX_IPPROTO_ICMP: Word = 1;
const LINUX_NETLINK_ROUTE: Word = 0;
const LINUX_SOCKADDR_IN_LEN: Word = 16;
const LINUX_SOCKADDR_NL_LEN: Word = 12;
const LINUX_ICMP_HEADER_LEN: Word = 8;
const LINUX_IPV4_HEADER_LEN: Word = 20;
const LINUX_MSGHDR_LEN: Word = 56;
const LINUX_IOVEC_LEN: Word = 16;
const LINUX_NLMSG_HEADER_LEN: Word = 16;
const LINUX_IFINFOMSG_LEN: usize = 16;
const LINUX_IFADDRMSG_LEN: usize = 8;
const LINUX_NLMSG_DONE: u16 = 3;
const LINUX_RTM_NEWLINK: u16 = 16;
const LINUX_RTM_GETLINK: u16 = 18;
const LINUX_RTM_NEWADDR: u16 = 20;
const LINUX_RTM_GETADDR: u16 = 22;
const LINUX_NLM_F_MULTI: u16 = 2;
const LINUX_IFLA_ADDRESS: u16 = 1;
const LINUX_IFLA_BROADCAST: u16 = 2;
const LINUX_IFLA_IFNAME: u16 = 3;
const LINUX_IFLA_MTU: u16 = 4;
const LINUX_IFA_ADDRESS: u16 = 1;
const LINUX_IFA_LOCAL: u16 = 2;
const LINUX_IFA_LABEL: u16 = 3;
const LINUX_IFA_BROADCAST: u16 = 4;
const LINUX_ARPHRD_ETHER: u16 = 1;
const LINUX_ARPHRD_LOOPBACK: u16 = 772;
const LINUX_IFF_UP: u32 = 0x1;
const LINUX_IFF_BROADCAST: u32 = 0x2;
const LINUX_IFF_LOOPBACK: u32 = 0x8;
const LINUX_IFF_RUNNING: u32 = 0x40;
const LINUX_IFF_MULTICAST: u32 = 0x1000;
const LINUX_RT_SCOPE_UNIVERSE: u8 = 0;
const LINUX_RT_SCOPE_HOST: u8 = 254;
const LINUX_SOL_SOCKET: Word = 1;
const LINUX_SO_TYPE: Word = 3;
const LINUX_SO_ACCEPTCONN: Word = 30;
const TCP_FLAG_FIN: Word = 0x01;
const TCP_FLAG_PSH: Word = 0x08;
const TCP_FLAG_ACK: Word = 0x10;

fn fork_copy_chunk(runtime: &Runtime) -> Word {
    let limit = runtime.posix_shm_size.min(0x10000);
    if limit < LINUX_PAGE_SIZE {
        LINUX_PAGE_SIZE
    } else {
        limit & !(LINUX_PAGE_SIZE - 1)
    }
}
const LINUX_SIGCHLD: Word = 17;
const LINUX_CLONE_VM: Word = 0x0000_0100;
const LINUX_CLONE_PARENT_SETTID: Word = 0x0010_0000;
const LINUX_CLONE_SETTLS: Word = 0x0008_0000;
const LINUX_CLONE_CHILD_SETTID: Word = 0x0100_0000;
const LINUX_O_CREAT: Word = 0o100;
const LINUX_O_ACCMODE: Word = 0o3;
const LINUX_O_RDONLY: Word = 0;
const LINUX_O_TRUNC: Word = 0o1000;
const LINUX_O_APPEND: Word = 0o2000;
const LINUX_O_NONBLOCK: Word = 0o4000;
const LINUX_O_LARGEFILE: Word = 0o100000;
const LINUX_O_DIRECTORY: Word = 0o200000;
const LINUX_O_CLOEXEC: Word = 0o2000000;
const LINUX_MAP_FIXED: Word = 0x10;
const LINUX_MAP_ANONYMOUS: Word = 0x20;
const LINUX_MREMAP_MAYMOVE: Word = 0x1;
const LINUX_MREMAP_FIXED: Word = 0x2;
const LINUX_MREMAP_SUPPORTED_FLAGS: Word = LINUX_MREMAP_MAYMOVE | LINUX_MREMAP_FIXED;
const LINUX_MADV_NORMAL: Word = 0;
const LINUX_MADV_RANDOM: Word = 1;
const LINUX_MADV_SEQUENTIAL: Word = 2;
const LINUX_MADV_WILLNEED: Word = 3;
const LINUX_MADV_DONTNEED: Word = 4;
const LINUX_MADV_FREE: Word = 8;
const LINUX_MADV_MERGEABLE: Word = 12;
const LINUX_MADV_UNMERGEABLE: Word = 13;
const LINUX_MADV_HUGEPAGE: Word = 14;
const LINUX_MADV_NOHUGEPAGE: Word = 15;
const LINUX_MADV_DONTDUMP: Word = 16;
const LINUX_MADV_DODUMP: Word = 17;
const LINUX_MADV_COLD: Word = 20;
const LINUX_MADV_PAGEOUT: Word = 21;
const LINUX_MADV_POPULATE_READ: Word = 22;
const LINUX_MADV_POPULATE_WRITE: Word = 23;
const LINUX_IOV_MAX: Word = 16;
const LINUX_POLLFD_BYTES: Word = 8;
const LINUX_POLLFD_MAX: Word = 32;
const LINUX_POLLIN: i16 = 0x0001;
const LINUX_POLLOUT: i16 = 0x0004;
const LINUX_POLLNVAL: i16 = 0x0020;
const LINUX_TCGETS: Word = 0x5401;
const LINUX_TCSETS: Word = 0x5402;
const LINUX_TCSETSW: Word = 0x5403;
const LINUX_TCSETSF: Word = 0x5404;
const LINUX_TIOCGWINSZ: Word = 0x5413;
const LINUX_INPUT_EVENT_BYTES: Word = 24;
const LINUX_EV_SYN: u16 = 0;
const LINUX_EV_KEY: u16 = 1;
const LINUX_EV_REL: u16 = 2;
const LINUX_SYN_REPORT: u16 = 0;
const LINUX_REL_X: u16 = 0;
const LINUX_REL_Y: u16 = 1;
const LINUX_REL_WHEEL: u16 = 8;
const ALTER_FB_WIDTH: Word = 800;
const ALTER_FB_HEIGHT: Word = 600;
const ALTER_FB_STRIDE: Word = ALTER_FB_WIDTH * 4;
const ALTER_FB_BYTES: Word = ALTER_FB_STRIDE * ALTER_FB_HEIGHT;
const LINUX_FBIOGET_VSCREENINFO: Word = 0x4600;
const LINUX_FBIOPUT_VSCREENINFO: Word = 0x4601;
const LINUX_FBIOGET_FSCREENINFO: Word = 0x4602;
const LINUX_FBIOPAN_DISPLAY: Word = 0x4606;
const LINUX_EVIOCGVERSION: Word = 0x8004_4501;
const LINUX_EVIOCGID: Word = 0x8008_4502;
const LINUX_FB_FIX_SCREENINFO_BYTES: Word = 80;
const LINUX_FB_VAR_SCREENINFO_BYTES: Word = 160;
const LINUX_F_DUPFD: Word = 0;
const LINUX_F_DUPFD_CLOEXEC: Word = 1030;
const LINUX_F_GETFD: Word = 1;
const LINUX_F_SETFD: Word = 2;
const LINUX_F_GETFL: Word = 3;
const LINUX_F_SETFL: Word = 4;
const LINUX_FD_CLOEXEC: Word = 1;
const LINUX_S_IFMT: Word = 0o170000;
const LINUX_S_IFIFO: Word = 0o010000;
const LINUX_S_IFCHR: Word = 0o020000;
const LINUX_S_IFBLK: Word = 0o060000;
const LINUX_STATX_BASIC_STATS: u32 = 0x07ff;
const POSIX_FILE_TYPE_PIPE: Word = 0xffff_fffe;
const POSIX_FILE_TYPE_SOCKET: Word = 0xffff_fffd;
const LINUX_S_IFSOCK: Word = 0o140000;
const LINUX_TERMIOS_BYTES: Word = 36;
const LINUX_ICRNL: u32 = 0x0000_0100;
const LINUX_IXON: u32 = 0x0000_0400;
const LINUX_OPOST: u32 = 0x0000_0001;
const LINUX_ONLCR: u32 = 0x0000_0004;
const LINUX_CS8: u32 = 0x0000_0030;
const LINUX_CREAD: u32 = 0x0000_0080;
const LINUX_ISIG: u32 = 0x0000_0001;
const LINUX_ICANON: u32 = 0x0000_0002;
const LINUX_ECHO: u32 = 0x0000_0008;
const LINUX_ECHOE: u32 = 0x0000_0010;
const LINUX_ECHOK: u32 = 0x0000_0020;
const LINUX_IEXTEN: u32 = 0x0000_8000;
const LINUX_VINTR: Word = 0;
const LINUX_VQUIT: Word = 1;
const LINUX_VERASE: Word = 2;
const LINUX_VKILL: Word = 3;
const LINUX_VEOF: Word = 4;
const LINUX_VTIME: Word = 5;
const LINUX_VMIN: Word = 6;
const LINUX_VSTART: Word = 8;
const LINUX_VSTOP: Word = 9;
const LINUX_VSUSP: Word = 10;
const LINUX_VEOL: Word = 11;
const LINUX_WNOHANG: Word = 1;
const LINUX_STACK_T_BYTES: Word = 24;
const LINUX_SS_DISABLE: Word = 2;
const LINUX_SS_AUTODISARM: Word = 0x8000_0000;
const LINUX_MINSIGSTKSZ: Word = 2048;
const LINUX_NSIG: Word = 64;
const LINUX_SIGSET_BYTES: Word = 8;
const LINUX_KERNEL_SIGACTION_BYTES: usize = 32;
const LINUX_TIMESPEC_BYTES: Word = 16;
const ALTER_SLEEP_TICK_HZ: Word = 100;
const ALTER_SLEEP_TICK_MILLISECONDS: Word = 10;
const ALTER_SLEEP_TICK_NANOSECONDS: Word = 10_000_000;
const ALTER_FB_PRESENT_HZ: Word = 60;

const ARCH_SET_FS: Word = 0x1002;
const ARCH_GET_FS: Word = 0x1003;

const AT_NULL: Word = 0;
const AT_PHDR: Word = 3;
const AT_PHENT: Word = 4;
const AT_PHNUM: Word = 5;
const AT_PAGESZ: Word = 6;
const AT_BASE: Word = 7;
const AT_FLAGS: Word = 8;
const AT_ENTRY: Word = 9;
const AT_UID: Word = 11;
const AT_EUID: Word = 12;
const AT_GID: Word = 13;
const AT_EGID: Word = 14;
const AT_PLATFORM: Word = 15;
const AT_HWCAP: Word = 16;
const AT_CLKTCK: Word = 17;
const AT_SECURE: Word = 23;
const AT_RANDOM: Word = 25;
const AT_EXECFN: Word = 31;

pub const SYS_READ: Word = 0;
pub const SYS_WRITE: Word = 1;
pub const SYS_OPEN: Word = 2;
pub const SYS_CLOSE: Word = 3;
pub const SYS_POLL: Word = 7;
pub const SYS_STAT: Word = 4;
pub const SYS_FSTAT: Word = 5;
pub const SYS_LSTAT: Word = 6;
pub const SYS_LSEEK: Word = 8;
pub const SYS_RT_SIGACTION: Word = 13;
pub const SYS_RT_SIGPROCMASK: Word = 14;
pub const SYS_RT_SIGSUSPEND: Word = 130;
pub const SYS_SIGALTSTACK: Word = 131;
pub const SYS_IOCTL: Word = 16;
pub const SYS_READV: Word = 19;
pub const SYS_WRITEV: Word = 20;
pub const SYS_PIPE: Word = 22;
pub const SYS_MMAP: Word = 9;
pub const SYS_MPROTECT: Word = 10;
pub const SYS_MUNMAP: Word = 11;
pub const SYS_MSYNC: Word = 26;
pub const SYS_MREMAP: Word = 25;
pub const SYS_MADVISE: Word = 28;
pub const SYS_BRK: Word = 12;
pub const SYS_ACCESS: Word = 21;
pub const SYS_SELECT: Word = 23;
pub const SYS_DUP: Word = 32;
pub const SYS_DUP2: Word = 33;
pub const SYS_NANOSLEEP: Word = 35;
pub const SYS_SETITIMER: Word = 38;
pub const SYS_GETPID: Word = 39;
pub const SYS_SOCKET: Word = 41;
pub const SYS_CONNECT: Word = 42;
pub const SYS_ACCEPT: Word = 43;
pub const SYS_SENDTO: Word = 44;
pub const SYS_RECVFROM: Word = 45;
pub const SYS_SENDMSG: Word = 46;
pub const SYS_RECVMSG: Word = 47;
pub const SYS_SHUTDOWN: Word = 48;
pub const SYS_BIND: Word = 49;
pub const SYS_LISTEN: Word = 50;
pub const SYS_GETSOCKNAME: Word = 51;
pub const SYS_GETPEERNAME: Word = 52;
pub const SYS_SETSOCKOPT: Word = 54;
pub const SYS_GETSOCKOPT: Word = 55;
pub const SYS_FCNTL: Word = 72;
pub const SYS_CLONE: Word = 56;
pub const SYS_FORK: Word = 57;
pub const SYS_VFORK: Word = 58;
pub const SYS_EXECVE: Word = 59;
pub const SYS_EXIT: Word = 60;
pub const SYS_WAIT4: Word = 61;
pub const SYS_KILL: Word = 62;
pub const SYS_UNAME: Word = 63;
pub const SYS_GETCWD: Word = 79;
pub const SYS_CHDIR: Word = 80;
pub const SYS_RENAME: Word = 82;
pub const SYS_MKDIR: Word = 83;
pub const SYS_RMDIR: Word = 84;
pub const SYS_CREAT: Word = 85;
pub const SYS_UNLINK: Word = 87;
pub const SYS_READLINK: Word = 89;
pub const SYS_CHOWN: Word = 92;
pub const SYS_FCHOWN: Word = 93;
pub const SYS_LCHOWN: Word = 94;
pub const SYS_GETUID: Word = 102;
pub const SYS_GETGID: Word = 104;
pub const SYS_GETEUID: Word = 107;
pub const SYS_GETEGID: Word = 108;
pub const SYS_GETPPID: Word = 110;
pub const SYS_SETPGID: Word = 109;
pub const SYS_GETRESUID: Word = 118;
pub const SYS_GETRESGID: Word = 120;
pub const SYS_GETPGID: Word = 121;
pub const SYS_MKNOD: Word = 133;
pub const SYS_GETTIMEOFDAY: Word = 96;
pub const SYS_GETRLIMIT: Word = 97;
pub const SYS_ARCH_PRCTL: Word = 158;
pub const SYS_FUTEX: Word = 202;
pub const SYS_SCHED_GETAFFINITY: Word = 204;
pub const SYS_GETDENTS64: Word = 217;
pub const SYS_GETTID: Word = 186;
pub const SYS_SET_TID_ADDRESS: Word = 218;
pub const SYS_CLOCK_GETTIME: Word = 228;
pub const SYS_UTIMES: Word = 235;
pub const SYS_EXIT_GROUP: Word = 231;
pub const SYS_OPENAT: Word = 257;
pub const SYS_MKDIRAT: Word = 258;
pub const SYS_MKNODAT: Word = 259;
pub const SYS_FCHOWNAT: Word = 260;
pub const SYS_FUTIMESAT: Word = 261;
pub const SYS_NEWFSTATAT: Word = 262;
pub const SYS_UNLINKAT: Word = 263;
pub const SYS_RENAMEAT: Word = 264;
pub const SYS_READLINKAT: Word = 267;
pub const SYS_FACCESSAT: Word = 269;
pub const SYS_PSELECT6: Word = 270;
pub const SYS_PPOLL: Word = 271;
pub const SYS_SET_ROBUST_LIST: Word = 273;
pub const SYS_UTIMENSAT: Word = 280;
pub const SYS_PRLIMIT64: Word = 302;
pub const SYS_GETRANDOM: Word = 318;
pub const SYS_STATX: Word = 332;
pub const SYS_RSEQ: Word = 334;
pub const SYS_FACCESSAT2: Word = 439;
pub const SYS_DUP3: Word = 292;
pub const SYS_PIPE2: Word = 293;
pub const SYS_ACCEPT4: Word = 288;

const LINUX_DIRENT64_NAME_OFFSET: usize = 19;
const LINUX_DT_UNKNOWN: Word = 0;
const LINUX_DT_CHR: Word = 2;
const LINUX_DT_DIR: Word = 4;
const LINUX_DT_BLK: Word = 6;
const LINUX_DT_REG: Word = 8;
