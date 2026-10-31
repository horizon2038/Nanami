# Alter/Linux Syscall Support

Alter/Linux implements developing x86_64 and AArch64 Linux syscall
personalities over Nanami's POSIX, VFS, terminal, process, memory, timer, and
network services. Syscall numbers, trap registers, ELF machine checks, and
architecture-dependent Linux layouts are selected for the build target.
This list reflects syscalls dispatched by the current implementation; it does
not imply complete Linux semantics for every flag or edge case.

## File and Directory I/O

Path-based stat/lstat/newfstatat/statx and access/faccessat collect inode, size,
mode and type in the rooted pathname-resolution request. They do not issue a
second POSIX stat and repeat the filesystem walk. Missing final components
retain their canonical name for the hosts/resolver fallback, also without a
second disk request. Virtual /dev, /proc and /sys retain personality-local
metadata. This is a per-request snapshot, not a cross-syscall pathname cache;
chmod, rename and inode-based dirfd semantics remain live.

`read`, `write`, `readv`, `writev`, `pread64`, `pwrite64`, `sendfile`, `open`, `openat`, `creat`, `close`, `lseek`,
`getdents64`, `stat`, `lstat`, `fstat`, `newfstatat`, `statx`, `access`,
`faccessat`, `faccessat2`, `readlink`, `readlinkat`, `mkdir`, `mkdirat`,
`mknod`, `mknodat`, `rmdir`, `unlink`, `unlinkat`, `rename`, `renameat`,
`link`, `linkat`, `chown`, `lchown`, `fchown`, `fchownat`, `utimes`,
`futimesat`, `utimensat`, `symlink`, `symlinkat`, `fchdir`, `chmod`, `fchmod`,
`fchmodat`, and `fchmodat2`.

`statfs` and `fstatfs` report the filesystem behind the pathname/open description,
including procfs, sysfs, devpts, RAM files, pipes, sockets and anonymous-inode
descriptors. Real ext2 files/directories query live cached superblock counters
through POSIX/VFS in one request per service, including pending writeback changes.
The 64-bit Linux statfs layout is 120 bytes. Synthetic filesystems report zero
capacity counters rather than inventing available memory or disk space.

Positioned regular-file I/O does not change the open-file description's offset,
including descriptors shared through `dup`/`fork`. `newfstatat` supports
`AT_EMPTY_PATH`, including `AT_FDCWD`. Path operations resolve relative dirfds
through their actual VFS directory inode, including after a directory rename.
`AT_FDCWD` is interpreted as a Linux signed 32-bit `int`, accepting both
zero-extended libc arguments and sign-extended raw syscall arguments.
Private OCI roots overlay `/dev`, `/proc` and `/sys`, not the legacy
personality's `/temp` alias. Ordinary rootfs entries remain visible.
The rooted walker expands relative/absolute symlinks before interpreting `..`,
limits expansion to 40 links, and keeps absolute link targets inside the guest
root. `lstat`, `readlinkat`, `AT_SYMLINK_NOFOLLOW`, and `O_NOFOLLOW` distinguish
the link from its target. Both inline and block-backed ext2 links are supported.
Alter's current pathname/cwd buffer limit is 127 bytes; VFS link targets may
contain up to 4096 bytes. Traversal from disk symlinks into personality-owned
virtual mounts is not yet supported. `O_APPEND` is applied on each POSIX write (also on `pwrite64`, matching
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

### File creation and advisory locks

`umask` stores a task's creation mask (initially `022`), returns the previous
mask, and retains it across fork/exec. `CLONE_FS` shares changes between tasks.
The mask is applied to open/openat/creat, mkdir/mkdirat, regular mknod and pathname
UNIX socket creation; symlinks retain mode `0777`. POSIX/VFS forward explicit
creation permissions to ext2, including mode zero, and stat/fstat/statx report
the actual persisted mode. Existing files are not chmodded by `O_CREAT`.

`chmod` follows symlinks; `fchmod` updates the actual open inode, including
read-only descriptors, dup/hard-link aliases and unlinked-but-open files.
`fchmodat` uses the Linux **three-argument syscall** ABI; only `fchmodat2`
reads flags. Relative dirfds remain inode-based after rename. `fchmodat2`
supports `AT_EMPTY_PATH` and `AT_SYMLINK_NOFOLLOW`; changing a symlink's own
mode returns `EOPNOTSUPP`. Mode bits `07777` are persisted without applying
umask or changing file type, reported by stat/fstat/statx and flushed by the
existing ext2 writeback/fsync path. Open-inode/handle caches update immediately.
Readonly execution contexts return `EROFS`, invalid FDs/flags return
`EBADF`/`EINVAL`, and personality-owned virtual mounts are not chmodded.
Personality-local FD types return `EOPNOTSUPP`. The current root-only identity
model remains: storing setuid/setgid/sticky metadata does not add multi-user
DAC enforcement, setuid exec identity changes or inode ctime maintenance.

`flock` supports shared/exclusive locks, unlock, nonblocking contention and
blocking waits interrupted by signals (including `SA_RESTART`). Locks are
advisory, keyed by inode and owned by POSIX open-file descriptions: dup, fork
and transferred descriptions share the lock until unlock or final close.
Release notifications wake Alter waiters without polling or blocking POSIX's
server loop. This backend covers POSIX-backed regular files and directories;
personality-local descriptor kinds currently return `EOPNOTSUPP`.

### File-cache advice

`fadvise64` (x86_64 221 / AArch64 223) accepts NORMAL, RANDOM, SEQUENTIAL,
WILLNEED, DONTNEED and NOREUSE. These are advisory no-ops with the current
cache policy, not synchronous reads, writeback, or mandatory eviction. File
contents, offsets and shared file-page leases remain unchanged. Invalid FDs
and POSIX O_PATH files return `EBADF`, pipes return `ESPIPE`, and negative
lengths or unknown advice return `EINVAL`. FD/advice follow the Linux int ABI;
the range remains 64-bit, with zero length, signed offsets and overflow-safe
hint acceptance following [Linux's generic implementation](https://github.com/torvalds/linux/blob/v6.12/mm/fadvise.c).

### io_uring

`io_uring_setup`, `io_uring_enter` and `io_uring_register` implement an
enter-driven shared-ring backend. SQEs are 64 bytes and CQEs 16 bytes, with
acquire/release ring counters, optional SQ arrays, CQSIZE/CLAMP, and at most 256
SQ / 512 CQ entries. `mmap` exposes real shared SQ/CQ/SQE pages; duplicate,
fork and SCM_RIGHTS aliases share ring state. Closing the final FD releases
the ring while existing mappings retain their pages until unmap/exit.

Supported operations: NOP, READ/WRITE, READV/WRITEV, FSYNC, OPENAT, CLOSE,
STATX, EPOLL_CTL, RENAMEAT, UNLINKAT, MKDIRAT, SYMLINKAT, LINKAT and FTRUNCATE.
Read/write operations cover regular POSIX files and memfd, including positioned
I/O and short transfers. The register PROBE operation reports this exact set.
CQ overflow is retained with bounded backpressure rather than dropping accepted
completions. GETEVENTS waits participate in readiness/signals and support a
temporary signal mask; poll/epoll observe completion readiness.

The advertised features are SINGLE_MMAP and NODROP only. SQPOLL/IOPOLL,
registered files/buffers/resource tags, linked/async SQE flags, direct
descriptors, socket/pipe async I/O and closing a ring through a CLOSE SQE are
explicitly unsupported. Libuv versions requiring RSRC_TAGS therefore select
their existing epoll/file-I/O fallback; no unsupported feature is advertised
merely to enable that path.

### Descriptor events

`dup`, `dup2`, `dup3`, `pipe`, `pipe2`, `fcntl`, `ioctl`, `poll`, `ppoll`,
`select`, `pselect6`, `eventfd`, `eventfd2`, `epoll_create`, `epoll_create1`,
`epoll_ctl`, `epoll_wait`, `epoll_pwait`, `timerfd_create`, `timerfd_settime`,
`timerfd_gettime`, `signalfd`, and `signalfd4`. Legacy entry points are provided
where the architecture has them.

Event counters support semaphore mode, nonblocking I/O, shared dup/fork state,
and blocking read/write through the existing readiness waiters. Epoll supports
level-triggered and one-shot watches, packed architecture-specific user data,
timeouts, and final-close cleanup. Registrations follow the open description
rather than a reused descriptor number. Edge-triggered/exclusive watches,
nested epoll are explicitly rejected. `ppoll`, `pselect6`, and `epoll_pwait`
temporarily install the supplied signal mask while parked and restore it on
completion or signal delivery.
Interest sets are bounded by the 64-descriptor guest limit; Alter holds at most
256 counter/timer/signal/memory/epoll/ring objects in total and allocates storage only on use.

Timer descriptors count missed periodic expirations, support relative and
absolute realtime/monotonic/boottime deadlines, and integrate with the same
one-shot timer and readiness queue used by blocking I/O. Signal descriptors
return Linux 128-byte `signalfd_siginfo` records from the reading task's pending
signals (including its thread group's pending queue). Both descriptor types
share nonblocking state through dup/fork/SCM_RIGHTS. Guest clock-setting is not
implemented, so realtime clock-change cancellation has no producer.

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
Memfd and shared-anonymous mappings provide the shared buffers used with this
transport. DRM, ordinary-file shared page caching, and a compositor remain
separate requirements. A9N and its IPC fastpath are unchanged.

## Memory

`mmap`, `mprotect`, `munmap`, `mremap`, `madvise`, `brk`, and `memfd_create`.

Regular-file `MAP_PRIVATE` copies the file without disturbing its offset and
zero-fills the final partial page. `MAP_FIXED` replaces overlapping mappings;
`munmap` accepts holes. Ordinary regular-file **read-only `MAP_SHARED`** uses
VFS-owned shared pages, not private copies. Mappings remain valid after FD close,
unlink and fork; native/POSIX writes within the file update existing aliases.
The hidden FD and VFS page leases survive until the last guest alias is removed.
`msync` forwards file synchronization to POSIX/VFS, including persistence errors.
The final partial page is zero-filled and `mprotect(PROT_WRITE)` is denied.
Writable shared file mappings and initial ranges extending beyond EOF return
`EOPNOTSUPP`. Size changes while read-page leases exist return `EBUSY`, before
any mutation; unmap and retry. File-backed `MAP_PRIVATE|PROT_NONE` remains
unsupported. Ordinary-file truncate/SIGBUS invalidation is not claimed.

Memfd supports sparse read/write, positioned I/O, seek, truncate, basic sealing,
`MAP_PRIVATE` and real `MAP_SHARED`. Shared anonymous mappings use the same
reclaimable backing. Shared aliases reference the same physical pages, including
after fork or `SCM_RIGHTS`; closing a descriptor does not discard live mappings.
Truncation removes pages past EOF from all shared aliases; access past a memfd's
EOF raises `SIGBUS`, and regrowth exposes zeroed data. This EOF handling does not
extend to ordinary VFS file mappings.

`mprotect` changes hardware R/W/X attributes through Alpha. `PROT_NONE` removes
the leaf mapping while retaining its backing, so restoring access, fork and
remap preserve data. Sparse fork copies visit backed pages instead of scanning
every page in a large reservation. Supervisor image/debug copies remain
privileged; checked Linux copyin/copyout refuses inaccessible guest buffers.

VMA metadata supports up to 128 tracked ranges per address space. The table is
allocated lazily and released on process removal, rather than increasing
Alter's startup stack or every process snapshot with an inline array.

## Terminal Applications

The Shell and Alter launchers supply `TERM=nanami-256color` and
`COLORTERM=truecolor`; Alter also supplies `TERMINFO=/usr/share/terminfo`.
New rootfs images include matching terminfo with 256-color foreground/background,
default-color reset, background-color erase and alternate-screen support, along
with the implemented cursor, scrolling, text-attribute and cursor-key sequences.
Neovim detects RGB support from COLORTERM; ncurses can use the indexed palette
through terminfo. The legacy monochrome `vt100` entry is retained for explicit
compatibility use. Neither entry claims unsupported DEC line drawing, blink,
function keys or a complete xterm implementation.

Rebuild an existing rootfs explicitly to install the new entry: recompiling
Alter alone does not change an existing filesystem image. Use a separate rootfs
path to preserve guest data; `--rebuild-rootfs` replaces that image's contents.

### Unix98 pseudoterminals

`/dev/ptmx` allocates a master with a locked `/dev/pts/N` slave. The personality
provides `TIOCGPTN`, `TIOCSPTLCK`, `TIOCGPTLCK` and `TIOCGPTPEER`, including
sign-extended libc ioctl commands. Numeric slave paths and directory entries
are local to the prepared rootfs execution context. `/dev/tty` opens the task's
actual controlling PTY; the launcher's GUI terminal remains independent.

PTY open descriptions share status flags through dup/fork/SCM_RIGHTS, while
CLOEXEC remains per descriptor. Termios, window size, input/output queues and
foreground group belong to the pair. Canonical input supports record boundaries,
partial reads, erase/kill/word erase, quoting and one-shot VEOF. Raw reads support
all four VMIN/VTIME combinations. Common input/output translations, echo,
software flow control, flushes, packet mode and input/output queue queries are
implemented. TCSETSW/TCSETSF and tcdrain wait for queued output to drain without
blocking Alter's server loop. Unsupported requests return an error.

Blocking I/O, poll/select and level-triggered epoll use the existing readiness
and one-shot timer machinery; there is no periodic PTY polling or additional
service RPC. Ctrl-C/quit/suspend target the PTY's foreground group; changing
window size sends SIGWINCH only when the value changes. Background reads and
terminal-setting operations enforce SIGTTIN/SIGTTOU, with blocked/ignored
signal and orphaned-group handling. TOSTOP also applies to background writes.

Final master close removes the slave pathname, sends SIGHUP/SIGCONT, detaches
the controlling terminal, and wakes readers/writers. Queued slave input drains
before EOF; writes to a hung-up slave fail with EIO. After the last slave closes,
the master drains pending output and then reports EIO/POLLHUP. Ancillary queues
keep passed descriptions alive until received or discarded. Storage is bounded
to 32 live pairs, 32 KiB per direction and a 4096-byte canonical line, allocated
only for opened PTYs. Line editing is byte-oriented; legacy BSD PTYs, TIOCSTI,
alternative line disciplines and hardware serial/modem controls are not emulated.

Static Neovim's `:terminal /bin/busybox sh -i` uses this path unchanged. When
BusyBox is packaged, xtask supplies `/bin/sh` and `/usr/bin/sh` hard-link aliases
only where no explicit shell exists; it does not copy the binary again or
replace shells provided by payloads/overlays. Existing images need an explicit
rootfs rebuild to acquire these aliases. This does not change Negoto's separate
OCI console-socket/PTY allocation policy.

## Processes and Execution

### Process inspection

The personality-owned `/proc` enumerates numeric thread-group directories from
Alter's managed process table, limited to the caller's execution context. It
provides `stat` (the 52-field Linux layout), `status`, `cmdline`, `comm`, `statm`,
`exe` readlink and `task/<tid>` views, plus `/proc/uptime`. `/proc/self` selects
the calling thread group. Directory FDs retain their selected PID across fork;
`openat`, metadata lookup, short reads, EOF and directory-cookie seeking use the
same process-backed objects. Reaped subjects disappear rather than exposing
another execution context.

BusyBox `ps` can enumerate these views. Thread-aware builds can use the `task`
directories; the availability of `ps -T` also depends on BusyBox's build options.
Command lines retain NUL separators and are read from the guest argument vector
installed at launch or exec; fork inherits that vector. State, parent/group/session IDs, terminal,
thread count and virtual mapping size come from Alter state. Start time is
sampled from the monotonic timer at task creation and preserved across exec.
PIDs are not renumbered into Linux PID namespaces. CPU time, fault counters,
resident-page accounting and aggregate idle time are not exported by A9N: their required
numeric fields are zero, not fabricated usage measurements. `VmRSS` is omitted
from `status` for the same reason.

### Task lifecycle

`clone`, `clone3`, `fork`, `vfork`, `execve`, `wait4`, `exit`, `exit_group`, `getpid`,
`getppid`, `gettid`, `set_tid_address`, `set_robust_list`, `setpgid`,
`getpgid`, `get_robust_list`, and `kill`.

`CLONE_VM` creates a suspended raw A9N task referencing the existing AddressSpace,
then installs its registers, stack, TLS and TID pointers before resuming it.
`CLONE_FILES`, `CLONE_FS`, `CLONE_SIGHAND`, `CLONE_THREAD`, `CLONE_SETTLS`,
parent/child TID writes and child-clear-tid are supported on this path.
Threads share VMA/FD/cwd/disposition state as requested, but retain independent
signal masks, targeted pending signals and alternate stacks. `getpid` returns
the thread group ID; `gettid` returns the native task ID. `exit` drops one task;
`exit_group` stops the other group members. Threads are not individual wait4
children. VM/frame arenas outlive the original task until their last user is reaped.

Exec of a still-shared VM, or from a non-owner raw task, is rejected instead of
revoking an address space used by another task. `clone3` decodes the extensible
64/80/88-byte clone_args ABI, checks unknown trailing bytes, translates stack
base/size and architecture-specific TLS/TID arguments, and uses the same real
fork/shared-VM implementation. Shared-VM vfork+exec, pidfds, set_tid selection,
cgroup and namespace creation are not provided; unsupported clone features
retain `ENOSYS` so existing libc fallback paths remain available. No private
copy is advertised as a shared-VM vfork. `CLONE_SYSVSEM` is
accepted for libc thread creation; there is no System V semaphore backend.

### Task identity and control

`setresuid` and `setresgid` accept the current root identity (0) and the Linux
32-bit unchanged sentinel (-1), in any combination. A nonzero identity returns
`EOPNOTSUPP`: POSIX/OCI still use root-only credentials and do not implement
multi-user DAC. A reported successful privilege drop would be misleading.

`prctl` implements PR_SET_NAME/PR_GET_NAME with a separate 16-byte per-task
comm, including truncation, NUL padding, fork inheritance and exec reset.
Procfs stat/status/comm reflect this name without changing executable identity.
PR_SET_NO_NEW_PRIVS/PR_GET_NO_NEW_PRIVS store an irreversible flag inherited
across fork/clone/exec; the current exec path never grants setuid/file capabilities.
PR_GET_SECCOMP reports zero; installing filters and other prctl options return
errors rather than accepting unenforced security settings.

### System V shared memory

`shmget`, `shmat`, `shmdt` and `shmctl` provide execution-context-scoped shared
RAM, including IPC_PRIVATE, keyed lookup/IPC_CREAT/IPC_EXCL, IPC_STAT, IPC_SET
of root-owned permission metadata and IPC_RMID. Segments start zero-filled;
multiple attachments and fork aliases refer to the same backing pages.
IPC_RMID removes the key immediately and retains attached pages until the last
detach. Partial munmap preserves attachment identity, and shmdt removes only
the remaining segment pieces, not replacement VMAs. Exec/exit detach mappings;
the last task leaving a context reclaims its remaining segments.

SHM_RDONLY prevents later mprotect from adding write access; SHM_RND and SHM_EXEC
are recognized. Explicit attachment addresses must be free and within Alter's
tracked reservation range (0x100000000..0x400000000000). SHM_REMAP, huge pages,
locking, SHM_INFO/SHM_STAT and mremap of attachments are rejected. Limits are
32 MiB per segment, 64 MiB per context and the existing 256-object server budget.
This supports X11 MIT-SHM without changing the Kernel or copying pixel buffers
between private guest snapshots.

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

Futex `WAIT`, `WAKE`, `WAIT_BITSET` and `WAKE_BITSET` use real parked waiters,
bitsets and deadlines. Private keys identify the address space, while shared
memfd keys identify the backing object plus offset, not an alias VA. Exit clears
child TIDs and wakes joiners. Robust-list cleanup atomically sets `OWNER_DIED`
only for locks held by the exiting TID and wakes their waiters. PI/requeue
operations remain unsupported; relative timed waits interrupted by a signal
return `EINTR` (no `restart_syscall` restart block).

Thread groups share process-directed pending signals and interval timers.
`tgkill` targets an individual task; signalfd can consume eligible thread-local
and group-pending records. Full Linux credentials, scheduling and FPU signal
frames are outside this personality's current scope.

## Identity and System Information

`uname`, `getcwd`, `chdir`, `getuid`, `geteuid`, `getgid`, `getegid`,
`getresuid`, `getresgid`, `arch_prctl`, `getrandom`, `getrlimit`, and
`prlimit64`, and `sysinfo`.

`sysinfo` (x86_64 99 / AArch64 179) writes the 112-byte Linux 64-bit layout,
including zeroed padding. Uptime uses the live monotonic boot clock, rounded
up to seconds as in Linux. Total/free RAM come from Alpha's physical-memory
accounting, with `mem_unit=1`, consistent with `/proc/meminfo`. The process
count is Alpha's live native process/thread count (including native services),
bounded to the ABI's 16-bit field; it is not a PID-namespace or container quota
report. Swap and separate high-memory zones are absent. Load averages and
shared/buffer RAM subdivisions have no accounting provider and are returned
as zero, not synthesized from live-process count or used RAM. Invalid,
read-only and partially unwritable destinations return `EFAULT` through the
existing checked guest copyout; the syscall does not touch host devices.
Layout and rounding follow the [Linux sysinfo ABI](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/sysinfo.h)
and [implementation](https://github.com/torvalds/linux/blob/v6.12/kernel/sys.c).

`RLIMIT_NOFILE` reports the actual 64-descriptor table and `RLIMIT_STACK`
reports the mapped stack size, not infinity. `prlimit64` validates the resource
and target; an unchanged limit may be supplied, while resource-policy changes
return `EOPNOTSUPP` instead of claiming enforcement.

With an explicit graphics grant, `/dev/tty0`, `/dev/tty1` and `/dev/console`
provide one window-local Linux VT (`VT_*`, `KDGETMODE`/`KDSETMODE`,
`KDGKBMODE`/`KDSKBMODE`, `KDGKBENT`, LEDs and termios). Mediumraw keycodes use
a US console map. `/dev/input/mice`/`psaux`/`mouse` provide PS/2 byte packets,
command acknowledgements and wheel negotiation. `poll`/`select` report queued
input and command-reply readiness. These are not host hardware devices.
Forked X11 clients share the context leader's graphics session. Linux VT
switching and writable ordinary-file shared mmap are not implemented.

## Explicitly Unsupported

`rseq` currently returns `ENOSYS`. Unknown syscall numbers also return
`ENOSYS` and are logged when syscall tracing is enabled.

Full Linux file-permission/credential semantics, shared ordinary-file page
caching, PI futexes and all clone/exec combinations are not implemented.
Packaging a distro rootfs does not imply arbitrary-distribution compatibility.

The dispatch implementation in
`../shared/src/personality/linux.rs` is the canonical source of truth.
