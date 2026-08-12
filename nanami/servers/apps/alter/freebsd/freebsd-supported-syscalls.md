# Alter/FreeBSD Syscall Support

Alter/FreeBSD implements a developing x86_64 FreeBSD syscall personality. Most
common operations are translated to the shared Alter implementation used by
the Linux personality, with FreeBSD-specific flags and structures handled at
the personality boundary.

This list indicates current dispatch support, not complete FreeBSD semantic or
binary compatibility.

## File and Directory I/O

`read`, `write`, `writev`, `open`, `openat`, `close`, `lseek`, `stat`,
`lstat`, `fstat`, `fstatat`, `getdirentries`, `access`, `faccessat`,
`readlink`, `readlinkat`, `mkdir`, `mkdirat`, `mknod`, `mknodat`, `rmdir`,
`unlink`, `unlinkat`, `rename`, `renameat`, and `utimes`.

`readv` is recognized but currently returns `ENOSYS`.

## File Descriptors and Polling

`dup`, `dup2`, `pipe`, `fcntl`, `ioctl`, `poll`, and `select`.

## Memory

`mmap`, `mprotect`, `munmap`, and `break`.

## Processes

`fork`, `execve`, `wait4`, `exit`, `getpid`, `getppid`, `getpgid`, `kill`,
`thr_self`, and `thr_exit`.

## Time, Identity, and Signals

`gettimeofday`, `clock_gettime`, `getuid`, `geteuid`, `getgid`, `getegid`,
`umask`, `issetugid`, `sigaction`, and `sigprocmask`.

Signal-related calls currently provide compatibility behavior rather than a
complete FreeBSD signal implementation.

## FreeBSD-Specific Calls

- `sysarch` supports amd64 FS-base get/set operations used by supported static
  binaries.
- `__sysctl` implements a bounded set of kernel, hardware, and user MIB values,
  including OS identity, hostname, CPU count, memory size, page size, user
  stack, random data, and `_CS_PATH` information.

Unknown syscall numbers return FreeBSD `ENOSYS`.

The dispatch implementation in
`../shared/src/personality/freebsd.rs` is the canonical source of truth.
