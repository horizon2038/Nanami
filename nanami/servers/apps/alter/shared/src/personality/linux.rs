#[path = "linux/threads.rs"]
mod threads;
#[path = "linux/vectored.rs"]
mod vectored;

#[path = "linux/file_io.rs"]
mod file_io;
use file_io::*;

#[path = "linux/sendfile.rs"]
mod sendfile;

#[path = "linux/input_device.rs"]
mod input_device;
use input_device::*;

#[path = "linux/framebuffer.rs"]
mod framebuffer;
use framebuffer::*;

#[path = "linux/network_wait.rs"]
mod network_wait;
use network_wait::*;

#[path = "linux/sockets.rs"]
mod sockets;
use sockets::*;

#[path = "linux/socket_ioctl.rs"]
mod socket_ioctl;

#[path = "linux/socket_io.rs"]
mod socket_io;
use socket_io::*;

#[path = "linux/netlink.rs"]
mod netlink;
use netlink::*;

#[path = "linux/guest_memory.rs"]
mod guest_memory;
use guest_memory::*;

#[path = "linux/network_packet.rs"]
mod network_packet;
use network_packet::*;

#[path = "linux/terminal.rs"]
mod terminal;
use terminal::*;
#[path = "linux/job_control.rs"]
mod job_control;
#[path = "linux/terminal_signals.rs"]
mod terminal_signals;

#[path = "linux/pipe.rs"]
mod pipe;
use pipe::*;
#[path = "linux/eventfd.rs"]
mod eventfd;
#[path = "linux/fd_events.rs"]
mod fd_events;
#[path = "linux/flock.rs"]
mod flock;
#[path = "linux/futex.rs"]
mod futex;
#[path = "linux/io_uring.rs"]
mod io_uring;
#[path = "linux/memfd.rs"]
mod memfd;
#[path = "linux/namespace.rs"]
mod namespace;
use namespace::*;
#[path = "linux/pipe_wait.rs"]
mod pipe_wait;
pub(crate) use memfd::bus_fault;
#[path = "linux/epoll.rs"]
mod epoll;
#[path = "linux/unix.rs"]
mod unix;
#[path = "linux/unix_message.rs"]
mod unix_message;

#[path = "linux/filesystem.rs"]
mod filesystem;
use filesystem::*;

#[path = "linux/directory.rs"]
mod directory;
use directory::sys_getdents64;

#[path = "linux/virtual_files.rs"]
mod virtual_files;
use virtual_files::*;

#[path = "linux/proc_net.rs"]
mod proc_net;

#[path = "linux/resolver.rs"]
mod resolver;

#[path = "linux/descriptors.rs"]
mod descriptors;
use descriptors::*;

#[path = "linux/sync.rs"]
mod sync;
use sync::*;
#[path = "linux/truncate.rs"]
mod truncate;

#[path = "linux/graphics_session.rs"]
mod graphics_session;
use graphics_session::*;

#[path = "linux/framebuffer_info.rs"]
mod framebuffer_info;

#[path = "linux/fork.rs"]
mod fork;
use fork::*;

#[path = "linux/exec_image.rs"]
mod exec_image;
use exec_image::*;

#[path = "linux/exec.rs"]
mod exec;
use exec::*;

#[path = "linux/metadata.rs"]
mod metadata;
use metadata::*;

#[path = "linux/system.rs"]
mod system;
use system::*;

#[path = "linux/vm.rs"]
mod vm;
use vm::*;

#[path = "linux/wait_signals.rs"]
mod wait_signals;
use wait_signals::*;
#[path = "linux/signal_syscalls.rs"]
mod signal_syscalls;
use signal_syscalls::*;
#[path = "linux/signal_frame.rs"]
mod signal_frame;
#[path = "linux/signal_timers.rs"]
mod signal_timers;
#[path = "linux/signals_runtime.rs"]
pub mod signals_runtime;

#[path = "linux/readiness.rs"]
mod readiness;
use readiness::*;
#[path = "linux/poll_set.rs"]
mod poll_set;
use poll_set::*;
#[path = "linux/poll_timeout.rs"]
mod poll_timeout;
use poll_timeout::*;
#[path = "linux/poll_wait.rs"]
mod poll_wait;
use poll_wait::sys_poll_action;
#[path = "linux/socket_readiness.rs"]
mod socket_readiness;
use socket_readiness::socket_readiness;

#[path = "linux/clocks.rs"]
mod clocks;
use clocks::*;
#[path = "linux/clock_events.rs"]
mod clock_events;
#[path = "linux/realtime.rs"]
mod realtime;
use clock_events::arm_clock_timer;

#[path = "linux/tracing.rs"]
mod tracing;
use tracing::*;

#[path = "linux/memory_copy.rs"]
mod memory_copy;
use memory_copy::*;

#[path = "linux/exec_strings.rs"]
mod exec_strings;
use exec_strings::*;

#[path = "linux/exec_stack.rs"]
mod exec_stack;
use exec_stack::*;

#[path = "linux/paths.rs"]
mod paths;
use paths::*;

#[path = "linux/errors.rs"]
mod errors;
use errors::*;

#[path = "linux/constants.rs"]
mod constants;
use constants::*;

use nanami_client::{gfx::honoka, input, net, posix, vfs};
use nanami_runtime::{RequestError, Word};

use crate::abi::{
    ALTER_DEFAULT_SHM_BYTES, ALTER_IO_OFFSET, ALTER_LAUNCH_MAX_ARGS, ALTER_LAUNCH_MAX_ENVS,
    SLOT_HONOKA_PRESENT_NOTIFICATION_BASE, SLOT_HONOKA_SERVICE, SLOT_INPUT_SERVICE,
    SLOT_NETWORK_SERVICE,
};
use crate::arch::{self, PLATFORM, STAT_SIZE, UNAME_MACHINE};
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

#[cfg(target_arch = "x86_64")]
#[path = "linux/arch/x86_64.rs"]
mod arch_syscalls;

#[cfg(target_arch = "aarch64")]
#[path = "linux/arch/aarch64.rs"]
mod arch_syscalls;

pub use arch_syscalls::*;

#[derive(Clone, Copy)]
pub enum EmulationAction {
    Return(isize),
    Resume,
    RestoreSignalFrame,
    Park,
    Exit(Word),
    Unsupported(Word),
}

pub fn dispatch_syscall(
    runtime: &mut Runtime,
    native_pid: Word,
    context: LinuxSyscallContext,
) -> EmulationAction {
    if let Some(action) = unix::dispatch(runtime, native_pid, context) {
        record_action_result(runtime, native_pid, context.number, action);
        trace_syscall_action(runtime, native_pid, context, action);
        return action;
    }
    let result = match context.number {
        SYS_READ => {
            let mut action = sys_read_action(
                runtime,
                native_pid,
                context.args[0],
                context.args[1],
                context.args[2],
                context,
            );
            if matches!(action, EmulationAction::Return(value) if value == -(EAGAIN as isize))
                && pipe_wait::park(runtime, native_pid, context)
            {
                action = EmulationAction::Park;
            }
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
        SYS_SENDFILE => sendfile::sys_sendfile(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_PREAD64 | SYS_PWRITE64 => sys_positioned_io(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.number == SYS_PWRITE64,
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
        SYS_NANOSLEEP | SYS_CLOCK_NANOSLEEP => {
            let action = if context.number == SYS_CLOCK_NANOSLEEP {
                sys_clock_nanosleep_action(runtime, native_pid, context)
            } else {
                sys_nanosleep_action(runtime, native_pid, context)
            };
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
            LINUX_O_WRONLY | LINUX_O_CREAT | LINUX_O_TRUNC,
            context.args[1],
        ),
        SYS_OPEN => sys_open(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_OPENAT => sys_openat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_CLOSE => sys_close(runtime, native_pid, context.args[0]),
        SYS_FLOCK => flock::lock(runtime, native_pid, context.args[0], context.args[1]),
        SYS_FSYNC | SYS_FDATASYNC | SYS_SYNCFS => sys_fsync(runtime, native_pid, context.args[0]),
        SYS_SYNC => sys_sync(runtime),
        SYS_FTRUNCATE => {
            truncate::sys_ftruncate(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_MEMFD_CREATE => memfd::create(runtime, native_pid, context.args[0], context.args[1]),
        SYS_IO_URING_SETUP => {
            io_uring::setup(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_IO_URING_REGISTER => io_uring::register(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_IO_URING_ENTER => {
            let action = io_uring::enter(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
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
        SYS_TIMERFD_CREATE => {
            fd_events::timer_create(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_TIMERFD_SETTIME => fd_events::timer_set(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
        SYS_TIMERFD_GETTIME => {
            fd_events::timer_get(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_SIGNALFD | SYS_SIGNALFD4 => fd_events::signal_create(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            if context.number == SYS_SIGNALFD {
                0
            } else {
                context.args[3]
            },
        ),
        SYS_EVENTFD | SYS_EVENTFD2 => eventfd::create(
            runtime,
            native_pid,
            context.args[0],
            if context.number == SYS_EVENTFD {
                0
            } else {
                context.args[1]
            },
        ),
        SYS_EPOLL_CREATE | SYS_EPOLL_CREATE1 => {
            if context.number == SYS_EPOLL_CREATE && context.args[0] as i32 <= 0 {
                Err(EINVAL)
            } else {
                epoll::create(
                    runtime,
                    native_pid,
                    if context.number == SYS_EPOLL_CREATE {
                        0
                    } else {
                        context.args[0]
                    },
                )
            }
        }
        SYS_EPOLL_CTL => epoll::control(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
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
        SYS_POLL | SYS_PPOLL | SYS_SELECT | SYS_PSELECT6 | SYS_EPOLL_WAIT | SYS_EPOLL_PWAIT => {
            let action = sys_poll_action(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_STAT => sys_stat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_LSTAT => sys_lstat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_SYMLINK => namespace::symlink(
            runtime,
            native_pid,
            context.args[0],
            LINUX_AT_FDCWD,
            context.args[1],
        ),
        SYS_FCHDIR => namespace::fchdir(runtime, native_pid, context.args[0]),
        SYS_SYMLINKAT => namespace::symlink(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_FSTAT => sys_fstat(runtime, native_pid, context.args[0], context.args[1]),
        SYS_CHOWN => sys_chown(runtime, native_pid, context.args[0]),
        SYS_LCHOWN => sys_fchownat(runtime, native_pid, LINUX_AT_FDCWD, context.args[0], 0x100),
        SYS_FCHOWN => sys_fchown(runtime, native_pid, context.args[0]),
        SYS_FCHOWNAT => sys_fchownat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[4],
        ),
        SYS_NEWFSTATAT => sys_newfstatat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
        ),
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
        SYS_RT_SIGRETURN => return EmulationAction::RestoreSignalFrame,
        SYS_RT_SIGPENDING => {
            sys_rt_sigpending(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_RT_SIGTIMEDWAIT => return sys_rt_sigtimedwait(runtime, native_pid, context),
        SYS_PAUSE => {
            runtime.park_signal_waiter(native_pid, Word::MAX, context);
            return EmulationAction::Park;
        }
        SYS_TKILL => sys_tgkill(
            runtime,
            native_pid,
            context.args[0],
            context.args[0],
            context.args[1],
        ),
        SYS_TGKILL => sys_tgkill(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_RT_SIGQUEUEINFO => sys_sigqueue(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_SIGALTSTACK => sys_sigaltstack(runtime, native_pid, context.args[0], context.args[1]),
        SYS_RT_SIGSUSPEND => {
            let action = sys_rt_sigsuspend(runtime, native_pid, context);
            record_action_result(runtime, native_pid, context.number, action);
            trace_critical_action(runtime, native_pid, context, action);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
        SYS_GETPID => Ok(runtime.thread_group(native_pid)),
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
                // Once exec has replaced the image, its old syscall PC is invalid.
                Err(ExecError::ImageReplaced) => EmulationAction::Exit(127),
                Err(ExecError::Errno(errno)) => EmulationAction::Return(-(errno as isize)),
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
        SYS_GETPPID => runtime
            .managed_process(native_pid)
            .map(|process| process.parent_pid.max(1))
            .ok_or(ESRCH),
        SYS_TIME => sys_time(runtime, native_pid, context.args[0]),
        SYS_GETTIMEOFDAY => sys_gettimeofday(runtime, native_pid, context.args[0]),
        SYS_GETITIMER => signal_timers::get(runtime, native_pid, context.args[0], context.args[1]),
        SYS_ALARM => signal_timers::alarm(runtime, native_pid, context.args[0]),
        SYS_SETITIMER => signal_timers::set(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_EXIT | SYS_EXIT_GROUP => {
            if context.number == SYS_EXIT_GROUP {
                threads::exit_peers(runtime, native_pid, context.args[0]);
            }
            let action = EmulationAction::Exit(context.args[0]);
            let last = runtime
                .managed_process(native_pid)
                .map(|process| (process.last_syscall, process.last_syscall_return))
                .unwrap_or((0, 0));
            if process_trace_enabled(runtime, native_pid) {
                nanami_runtime::println!(
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
        SYS_UMASK => runtime.set_umask(native_pid, context.args[0]).ok_or(ESRCH),
        SYS_MKDIR => sys_mkdir(runtime, native_pid, context.args[0], context.args[1]),
        SYS_RMDIR => sys_rmdir(runtime, native_pid, context.args[0]),
        SYS_RENAME => sys_rename(runtime, native_pid, context.args[0], context.args[1]),
        SYS_LINK => sys_link(runtime, native_pid, context.args[0], context.args[1]),
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
        SYS_MKDIRAT => sys_mkdirat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
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
        SYS_LINKAT => sys_linkat(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
            context.args[3],
            context.args[4],
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
        SYS_SETPGID => runtime
            .set_process_group(native_pid, context.args[0], context.args[1])
            .map(|_| 0)
            .map_err(job_control::errno),
        SYS_GETPGID => sys_getpgid(runtime, native_pid, context.args[0]),
        SYS_GETPGRP => runtime
            .process_group(native_pid, 0)
            .map_err(job_control::errno),
        SYS_SETSID => runtime
            .create_session(native_pid)
            .map_err(job_control::errno),
        SYS_GETSID => runtime
            .process_session(native_pid, context.args[0])
            .map_err(job_control::errno),
        SYS_KILL => {
            let result = sys_kill(runtime, native_pid, context.args[0], context.args[1]);
            if context.args[1] != 0
                && runtime
                    .managed_process(native_pid)
                    .is_none_or(|process| process.exited)
            {
                return EmulationAction::Exit(128 + context.args[1]);
            }
            result
        }
        SYS_ARCH_PRCTL => sys_arch_prctl(runtime, native_pid, context.args[0], context.args[1]),
        SYS_GETTID => Ok(native_pid),
        SYS_SCHED_GETAFFINITY => sys_sched_getaffinity(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_SET_TID_ADDRESS => futex::set_tid(runtime, native_pid, context.args[0]),
        SYS_CLOCK_GETTIME => {
            sys_clock_gettime(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_CLOCK_GETRES => sys_clock_getres(runtime, native_pid, context.args[0], context.args[1]),
        SYS_UTIMES => sys_utime_path(runtime, native_pid, context.args[0]),
        SYS_FUTIMESAT => sys_utime_at(runtime, native_pid, context.args[0], context.args[1], 0),
        SYS_UTIMENSAT => sys_utime_at(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[3],
        ),
        SYS_SET_ROBUST_LIST => {
            futex::set_robust(runtime, native_pid, context.args[0], context.args[1])
        }
        SYS_GET_ROBUST_LIST => futex::get_robust(
            runtime,
            native_pid,
            context.args[0],
            context.args[1],
            context.args[2],
        ),
        SYS_GETRANDOM => sys_getrandom(runtime, native_pid, context.args[0], context.args[1]),
        SYS_GETRLIMIT | SYS_PRLIMIT64 => sys_getrlimit(runtime, native_pid, context),
        SYS_FUTEX => return futex::dispatch(runtime, native_pid, context),
        SYS_RSEQ => Err(ENOSYS),
        _ => {
            let action = EmulationAction::Unsupported(context.number);
            trace_syscall_action(runtime, native_pid, context, action);
            return action;
        }
    };

    if result == Err(EAGAIN)
        && (pipe_wait::park(runtime, native_pid, context)
            || flock::park(runtime, native_pid, context))
    {
        let action = EmulationAction::Park;
        trace_syscall_action(runtime, native_pid, context, action);
        return action;
    }
    if result == Err(EPIPE) {
        let _ = signals_runtime::queue(
            runtime,
            native_pid,
            crate::common::signals::Info::new(13, 0, native_pid),
        );
    }
    let value = result_to_linux_return(result);
    record_syscall_result(runtime, native_pid, context.number, value);
    trace_critical_syscall(runtime, native_pid, context, value);
    let action = EmulationAction::Return(value);
    trace_syscall_action(runtime, native_pid, context, action);
    action
}

pub use clock_events::handle_timer_notification;
pub use descriptors::{close_process_files, close_process_files_checked};
pub use input_device::wake_device_readers;
pub use network_wait::wake_network_waiters;
pub use poll_wait::{
    handle_readiness_changes, handle_readiness_notification, wake_readiness_waiters,
};
pub use terminal::wake_terminal_readers;
pub use terminal_signals::handle_terminal_input;
pub use threads::exit_peers as exit_thread_group;
pub use wait_signals::wake_waiter_for_child;
