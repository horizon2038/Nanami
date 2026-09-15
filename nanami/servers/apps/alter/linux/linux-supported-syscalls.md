# Alter/Linux Syscall Support

Alter/Linux implements developing x86_64 and AArch64 Linux syscall
personalities over Nanami's POSIX, VFS, terminal, process, memory, timer, and
network services. Syscall numbers, trap registers, ELF machine checks, and
architecture-dependent Linux layouts are selected for the build target.
This list reflects syscalls dispatched by the current implementation; it does
not imply complete Linux semantics for every flag or edge case.

## File and Directory I/O

`read`, `write`, `readv`, `writev`, `pread64`, `pwrite64`, `open`, `openat`, `creat`, `close`, `lseek`,
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

Regular-file `MAP_PRIVATE` copies the file without disturbing its offset and
zero-fills the final partial page. `MAP_FIXED` replaces overlapping mappings;
`munmap` accepts holes. File-backed `MAP_SHARED` and file-backed `PROT_NONE`
are explicitly rejected. File mappings are eager copies, not a demand-paged
page cache, and pages wholly past EOF do not yet deliver Linux `SIGBUS`.
Hardware enforcement of `mprotect` permissions and preservation of data across
`PROT_NONE` transitions remain incomplete; this is not a security isolation
boundary equivalent to Linux VM protection.

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

`gettimeofday`, `clock_gettime`, `nanosleep`, `setitimer`, `rt_sigaction`,
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

General filesystem symlinks, arbitrary-length `truncate`/`ftruncate`, complete
signals/threads, and full Linux file-permission semantics are still missing.
Packaging a distro rootfs does not imply those facilities are supported.

The dispatch implementation in
`../shared/src/personality/linux.rs` is the canonical source of truth.
