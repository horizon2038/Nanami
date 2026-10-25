# Alter/Linux Syscall Support

Alter/Linux implements developing x86_64 and AArch64 Linux syscall
personalities over Nanami's POSIX, VFS, terminal, process, memory, timer, and
network services. Syscall numbers, trap registers, ELF machine checks, and
architecture-dependent Linux layouts are selected for the build target.
This list reflects syscalls dispatched by the current implementation; it does
not imply complete Linux semantics for every flag or edge case.

## File and Directory I/O

`read`, `write`, `readv`, `writev`, `pread64`, `pwrite64`, `sendfile`, `open`, `openat`, `creat`, `close`, `lseek`,
`getdents64`, `stat`, `lstat`, `fstat`, `newfstatat`, `statx`, `access`,
`faccessat`, `faccessat2`, `readlink`, `readlinkat`, `mkdir`, `mkdirat`,
`mknod`, `mknodat`, `rmdir`, `unlink`, `unlinkat`, `rename`, `renameat`,
`link`, `linkat`, `chown`, `lchown`, `fchown`, `fchownat`, `utimes`,
`futimesat`, and `utimensat`.

Positioned regular-file I/O does not change the open-file description's offset,
including descriptors shared through `dup`/`fork`. `newfstatat` supports
`AT_EMPTY_PATH`, including `AT_FDCWD`; other relative-dirfd operations remain
partial. `O_APPEND` is applied on each POSIX write (also on `pwrite64`, matching
Linux), and `F_GETFL`/`F_SETFL` share append/nonblocking state across duplicates.

The guest root merges its real VFS directory with virtual mountpoints, suppressing
duplicate mount names. Directory reads preserve entries that do not fit the
caller's buffer; root directory cookies and positions are shared across
`dup`/`fork`, with `lseek` supporting rewind and resuming from `d_off`.

`sendfile` transfers regular-file input through service shared memory to regular
files, terminals, TCP sockets, pipes, or the null/zero devices. Each call returns
at most one buffer's worth of data; callers must handle short transfers. Explicit
offsets preserve the input fd position, output errors do not consume input, and
append-mode file outputs are rejected. Full-pipe/TCP backpressure currently
returns `EAGAIN`; blocking sendfile parking is not implemented. Other input
types and output devices return an error rather than fabricated success.

## File Descriptors and Polling

`dup`, `dup2`, `dup3`, `pipe`, `pipe2`, `fcntl`, `ioctl`, `poll`, `ppoll`,
`select`, `pselect6`, `eventfd`, `eventfd2`, `epoll_create`, `epoll_create1`,
`epoll_ctl`, `epoll_wait`, and `epoll_pwait`. Legacy entry points are provided
where the architecture has them.

Event counters support semaphore mode, nonblocking I/O, shared dup/fork state,
and blocking read/write through the existing readiness waiters. Epoll supports
level-triggered and one-shot watches, packed architecture-specific user data,
timeouts, and final-close cleanup. Registrations follow the open description
rather than a reused descriptor number. Edge-triggered/exclusive watches,
nested epoll, and nonempty temporary signal masks are explicitly rejected.
Interest sets are bounded by the 64-descriptor guest limit; Alter holds at most
256 counter/epoll objects in total and allocates their storage only on use.

`pipe2` accepts `O_CLOEXEC | O_NONBLOCK`. `FIONBIO` shares the `F_SETFL`
nonblocking implementation, including anonymous pipes used by libuv; each pipe
end's status is shared by its dup/fork aliases, independently of the other end.
`F_GETFL` reports the pipe end's actual access direction and the native terminal's
read/write access. A nonblocking terminal read with no input returns `EAGAIN`
instead of parking the guest event loop.

## Networking

`socket`, `socketpair`, `connect`, `bind`, `listen`, `accept`, `accept4`, `sendto`,
`sendmsg`, `recvfrom`, `recvmsg`, `shutdown`, `getsockname`, `getpeername`,
`setsockopt`, and `getsockopt`.

Supported socket paths include TCP/UDP through `net-server`, raw ICMP used by
`ping`, and read-only route netlink queries used by tools such as BusyBox
`ip a`.

### UNIX domain sockets

`AF_UNIX` supports `SOCK_STREAM`, `SOCK_DGRAM`, and `SOCK_SEQPACKET`, including
`socketpair`, pathname and length-sensitive abstract addresses. Stream/seqpacket
listeners implement a bounded accept backlog. `accept4`, `SOCK_NONBLOCK`,
`SOCK_CLOEXEC`, shared `F_SETFL`/`FIONBIO`, `shutdown`, `getsockname`, and
`getpeername` are supported. A full backlog or data queue parks a blocking caller
on the existing readiness mechanism; nonblocking calls return `EAGAIN`.
Poll/select and level-triggered epoll observe input, output, half-close and hangup.

Local transport is owned by the Alter instance, with no `net-server` round trip.
Forked and independently launched guests managed by that instance can connect.
Abstract names are scoped to the prepared execution context (ordinary guests
share the default context). Pathnames use the guest's cwd/root translation and
real ext2 socket inodes: stat reports `S_IFSOCK`, readdir reports `DT_SOCK`,
rename/hardlink preserve identity, and unlink does not invalidate established
connections. A hidden native FD pins a bound inode until the endpoint is freed,
preventing inode reuse from reconnecting a new pathname to the wrong listener.
Closing an endpoint does not unlink its pathname; a stale socket file must be
removed before binding the same path again.

`sendmsg`/`recvmsg` implement LP64 `SCM_RIGHTS` on all three socket types.
Ancillary queues own file descriptions, not sender FD numbers, so closing a
sender's FD does not destroy an in-flight resource. Regular-file offset/status
state stays shared through native POSIX dup. Receive supports `MSG_PEEK`,
`MSG_CMSG_CLOEXEC` and `MSG_CTRUNC`; undisclosed/truncated FDs are closed, while
copyout failures roll back newly installed FDs without consuming the message.
Plain read discards attached rights. Unreachable cycles of passed UNIX FDs are
traced and collected without retaining peers merely because they are connected.

`read`/`write`, vectored I/O, `sendto`/`recvfrom`, `MSG_DONTWAIT`, record
`MSG_TRUNC`, and `MSG_NOSIGNAL` are supported. Writing a closed stream queues
`SIGPIPE` unless suppressed. Socket options include `SO_TYPE`, `SO_ERROR`,
`SO_SNDBUF`, `SO_RCVBUF`, `SO_ACCEPTCONN`, `SO_PROTOCOL`, `SO_DOMAIN`, and
`SO_PEERCRED`; credentials use Alter's current PID and uid/gid-zero identity.

Limits are 128 local endpoints per Alter instance, 64 KiB queued payload,
256 queued messages and 64 in-flight FDs per receiving endpoint, plus the
existing 64-FD process limit. Buffer options can reduce the payload limit.
Transferable descriptions are POSIX-backed files/directories, pipes, UNIX
sockets, eventfd and epoll. Process-relative terminal/evdev/framebuffer/virtual
descriptions and legacy Internet/netlink socket descriptions return
`EOPNOTSUPP` rather than transferring a copy of mutable per-FD state. `SCM_CREDENTIALS`,
`SO_PASSCRED`, `MSG_WAITALL`, and unsupported options/flags are rejected, not
silently treated as successful operations.

This supplies the stream and FD-passing transport used by the
[Wayland wire protocol](https://wayland.freedesktop.org/docs/book/Protocol.html).
It does not add file-backed shared mappings, memfd, DRM, or a compositor; those
are separate compatibility requirements. A9N and its IPC fastpath are unchanged.

## Memory

`mmap`, `mprotect`, `munmap`, `mremap`, `madvise`, and `brk`.

Regular-file `MAP_PRIVATE` copies the file without disturbing its offset and
zero-fills the final partial page. `MAP_FIXED` replaces overlapping mappings;
`munmap` accepts holes. File-backed `MAP_SHARED` and file-backed `PROT_NONE`
are explicitly rejected. File mappings are eager copies, not a demand-paged
page cache, and pages wholly past EOF do not yet deliver Linux `SIGBUS`.
Hardware enforcement of `mprotect` permissions and preservation of data across
`PROT_NONE` transitions remain incomplete; this is not a security isolation
boundary equivalent to Linux VM protection.

VMA metadata supports up to 128 tracked ranges per process. The table is
allocated lazily and released on process removal, rather than increasing
Alter's startup stack or every process snapshot with an inline array.

## Terminal Applications

The Alter launcher supplies `TERM=vt100` and `TERMINFO=/usr/share/terminfo`.
New rootfs images include a compact terminfo entry for Shell's implemented
cursor, erase, scrolling, text-attribute and application-cursor-key sequences.
It deliberately does not advertise DEC line drawing or other unsupported
features. Rebuild an existing rootfs explicitly to add the entry; recompiling
Alter alone does not change an already-created filesystem image.

## Processes and Execution

`clone`, `fork`, `vfork`, `execve`, `wait4`, `exit`, `exit_group`, `getpid`,
`getppid`, `gettid`, `set_tid_address`, `set_robust_list`, `setpgid`,
`getpgid`, and `kill`.

Process operations are translated onto Alpha's process primitives and the
POSIX service. Thread-like `clone` combinations and signal behavior remain
partial.

### Dynamic executables

Rootfs launch and `execve` accept `PT_INTERP`. Alter maps the interpreter's
`PT_LOAD` segments, enters its entry point, and supplies the main executable's
`AT_PHDR`/`AT_PHNUM`/`AT_ENTRY` plus the interpreter's load bias in `AT_BASE`.
The guest linker performs relocation, DSO loading, TLS setup, and `dlopen`.
`execve` preserves the supplied `argv[0]`; missing interpreters fail before
replacing the running image or closing its `CLOEXEC` descriptors.

Supply matching-architecture loader/libc files at their Linux paths under
`/alter/linux`, e.g. `/alter/linux/lib64/ld-linux-x86-64.so.2` and
`/alter/linux/lib/x86_64-linux-gnu/libc.so.6`. `LINUX_ROOTFS_DIR` adds a regular
file/directory tree to the filesystem image without overwriting seeded files.
This is not arbitrary-distribution compatibility: interpreter images must be
`ET_DYN`, at most 16 MiB, and must not overlap the executable. Initramfs-name
launches and the FreeBSD personality do not gain dynamic-loader support.
See `tests/alter-linux/README.md` at the repository root for reproducible tests.

## Time, Signals, and Scheduling

`time` (x86_64), `gettimeofday`, `clock_gettime`, `clock_getres`, `nanosleep`, `clock_nanosleep`,
`setitimer`, `rt_sigaction`,
`rt_sigprocmask`, `rt_sigsuspend`, `sigaltstack`, `sched_getaffinity`, and
`futex`.

`time` returns 64-bit Unix seconds and optionally stores the same value through
`tloc`, using the same RTC epoch anchor and fresh monotonic sample as
`gettimeofday`/`CLOCK_REALTIME`. AArch64 has no separate `time` syscall; its libc
uses `clock_gettime` instead.

`clock_getres` reports the timer backend's resolution for the implemented
clocks and accepts a NULL result pointer. It does not fabricate CPU-clock
accounting for unsupported clock IDs.

`clock_nanosleep` supports relative and absolute waits against `CLOCK_REALTIME`,
`CLOCK_MONOTONIC`, and `CLOCK_BOOTTIME`, sharing the existing one-shot alarm and
park/resume path with `nanosleep`. Known clocks without a sleep backend return
`EOPNOTSUPP`; invalid IDs and the calling thread's CPU clock return `EINVAL`.
Successful waits leave `remain` untouched. Signal interruption and clock-setting
semantics remain bounded by the existing signal/clock implementation.

Several signal and synchronization calls currently provide compatibility
behavior rather than a complete Linux signal/thread implementation.

## Identity and System Information

`uname`, `getcwd`, `chdir`, `getuid`, `geteuid`, `getgid`, `getegid`,
`getresuid`, `getresgid`, `arch_prctl`, `getrandom`, `getrlimit`, and
`prlimit64`.

## Explicitly Unsupported

`rseq` currently returns `ENOSYS`. Unknown syscall numbers also return
`ENOSYS` and are logged when syscall tracing is enabled.

General filesystem symlinks, arbitrary-length `truncate`/`ftruncate`, complete
signals/threads, and full Linux file-permission semantics are still missing.
Packaging a distro rootfs does not imply those facilities are supported.

The dispatch implementation in
`../shared/src/personality/linux.rs` is the canonical source of truth.
