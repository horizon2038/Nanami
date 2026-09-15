#[path = "linux/vectored.rs"]
mod vectored;

#[path = "linux/file_io.rs"]
mod file_io;
use file_io::*;

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

#[path = "linux/pipe.rs"]
mod pipe;
use pipe::*;

#[path = "linux/filesystem.rs"]
mod filesystem;
use filesystem::*;

#[path = "linux/virtual_files.rs"]
mod virtual_files;
use virtual_files::*;

#[path = "linux/descriptors.rs"]
mod descriptors;
use descriptors::*;

#[path = "linux/graphics_session.rs"]
mod graphics_session;
use graphics_session::*;

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

#[path = "linux/readiness.rs"]
mod readiness;
use readiness::*;

#[path = "linux/clocks.rs"]
mod clocks;
use clocks::*;

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

use libnanami::{RequestError, Word};
use nanami_services::{gfx::honoka, input, net, posix, vfs};

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
            LINUX_O_WRONLY | LINUX_O_CREAT | LINUX_O_TRUNC,
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

pub use clocks::handle_timer_notification;
pub use descriptors::close_process_files;
pub use input_device::wake_device_readers;
pub use network_wait::wake_network_waiters;
pub use terminal::wake_terminal_readers;
pub use wait_signals::wake_waiter_for_child;
