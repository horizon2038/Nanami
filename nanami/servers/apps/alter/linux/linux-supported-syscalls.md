# Alter/Linux Syscall Support

Alter/Linux implements a developing x86_64 Linux syscall personality over
Nanami's POSIX, VFS, terminal, process, memory, timer, and network services.
This list reflects syscalls dispatched by the current implementation; it does
not imply complete Linux semantics for every flag or edge case.

## File and Directory I/O

`read`, `write`, `writev`, `open`, `openat`, `creat`, `close`, `lseek`,
`getdents64`, `stat`, `lstat`, `fstat`, `newfstatat`, `statx`, `access`,
`faccessat`, `faccessat2`, `readlink`, `readlinkat`, `mkdir`, `mkdirat`,
`mknod`, `mknodat`, `rmdir`, `unlink`, `unlinkat`, `rename`, `renameat`,
`chown`, `lchown`, `fchown`, `fchownat`, `utimes`, `futimesat`, and
`utimensat`.

## File Descriptors and Polling

`dup`, `dup2`, `dup3`, `pipe`, `pipe2`, `fcntl`, `ioctl`, `poll`, `ppoll`,
`select`, and `pselect6`.

## Networking

`socket`, `connect`, `bind`, `listen`, `accept`, `accept4`, `sendto`,
`sendmsg`, `recvfrom`, `recvmsg`, `shutdown`, `getsockname`, `getpeername`,
`setsockopt`, and `getsockopt`.

Supported socket paths include TCP/UDP through `net-server`, raw ICMP used by
`ping`, and read-only route netlink queries used by tools such as BusyBox
`ip a`.

## Memory

`mmap`, `mprotect`, `munmap`, `mremap`, `madvise`, and `brk`.

## Processes and Execution

`clone`, `fork`, `vfork`, `execve`, `wait4`, `exit`, `exit_group`, `getpid`,
`getppid`, `gettid`, `set_tid_address`, `set_robust_list`, `setpgid`,
`getpgid`, and `kill`.

Process operations are translated onto Alpha's process primitives and the
POSIX service. Thread-like `clone` combinations and signal behavior remain
partial.

## Time, Signals, and Scheduling

`gettimeofday`, `clock_gettime`, `setitimer`, `rt_sigaction`,
`rt_sigprocmask`, `rt_sigsuspend`, `sigaltstack`, `sched_getaffinity`, and
`futex`.

Several signal and synchronization calls currently provide compatibility
behavior rather than a complete Linux signal/thread implementation.

## Identity and System Information

`uname`, `getcwd`, `chdir`, `getuid`, `geteuid`, `getgid`, `getegid`,
`getresuid`, `getresgid`, `arch_prctl`, `getrandom`, `getrlimit`, and
`prlimit64`.

## Explicitly Unsupported

`rseq` currently returns `ENOSYS`. Unknown syscall numbers also return
`ENOSYS` and are logged when syscall tracing is enabled.

The dispatch implementation in
`../shared/src/personality/linux.rs` is the canonical source of truth.
