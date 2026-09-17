use super::Word;

pub(super) const ENOENT: i32 = 2;

pub(super) const EAGAIN: i32 = 11;

pub(super) const EINTR: i32 = 4;

pub(super) const EIO: i32 = 5;

pub(super) const EBADF: i32 = 9;

pub(super) const EACCES: i32 = 13;

pub(super) const EFAULT: i32 = 14;

pub(super) const ECHILD: i32 = 10;

pub(super) const EEXIST: i32 = 17;

pub(super) const ENODEV: i32 = 19;

pub(super) const EISDIR: i32 = 21;

pub(super) const EPIPE: i32 = 32;

pub(super) const EINVAL: i32 = 22;

pub(super) const EMFILE: i32 = 24;

pub(super) const ENOTTY: i32 = 25;

pub(super) const ESPIPE: i32 = 29;

pub(super) const EROFS: i32 = 30;

pub(super) const ENOSYS: i32 = 38;

pub(super) const ENAMETOOLONG: i32 = 36;

pub(super) const ENOTDIR: i32 = 20;

pub(super) const ENOMEM: i32 = 12;

pub(super) const ERANGE: i32 = 34;

pub(super) const ESRCH: i32 = 3;

pub(super) const ENOEXEC: i32 = 8;

pub(super) const EDESTADDRREQ: i32 = 89;

pub(super) const EMSGSIZE: i32 = 90;

pub(super) const EPROTONOSUPPORT: i32 = 93;

pub(super) const ESOCKTNOSUPPORT: i32 = 94;

pub(super) const EOPNOTSUPP: i32 = 95;

pub(super) const EAFNOSUPPORT: i32 = 97;

pub(super) const EADDRINUSE: i32 = 98;

pub(super) const EADDRNOTAVAIL: i32 = 99;

pub(super) const ENETDOWN: i32 = 100;

pub(super) const ENOTCONN: i32 = 107;

pub(super) const ENOTSOCK: i32 = 88;

pub(super) const EINPROGRESS: i32 = 115;

pub(super) const LINUX_STATX_SIZE: usize = 256;

pub(super) const LINUX_PAGE_SIZE: Word = 4096;

// read_c_string() uses [0, LINUX_PAGE_SIZE) as its page-sized scratch area.
// Keep the saved first pathname outside it for two-path syscalls.
pub(super) const LINUX_SECOND_PATH_OFFSET: Word = LINUX_PAGE_SIZE;

pub(super) const LINUX_CPU_MASK_BYTES: Word = 8;

pub(super) const LINUX_ITIMERVAL_BYTES: Word = 32;

pub(super) const LINUX_ITIMER_PROF: Word = 2;

pub(super) const LINUX_VIRTUAL_RESERVATION_BASE: Word = 0x0000_0001_0000_0000;

pub(super) const LINUX_VIRTUAL_RESERVATION_LIMIT: Word = 0x0000_4000_0000_0000;

pub(super) const LINUX_IMAGE_BASE: Word = 0x400000;

pub(super) const FREEBSD_IMAGE_BASE: Word = 0x200000;

pub(super) const NANAMI_IMAGE_BASE: Word = 0x1000000;

pub(super) const LINUX_ELF_HEADER_BYTES: Word = 4096;

pub(super) const LINUX_PT_LOAD: u32 = 1;

pub(super) const LINUX_PT_TLS: u32 = 7;

pub(super) const LINUX_PF_W: u32 = 2;

pub(super) const LINUX_MAX_LOAD_SEGMENTS: usize = 16;

pub(super) const LINUX_STACK_TOP: Word = 0x4040000;

pub(super) const LINUX_STACK_BYTES: Word = 0x40000;

pub(super) const LINUX_STACK_GUARD_BYTES: Word = 0x3000;

pub(super) const LINUX_EXEC_STACK_BYTES: usize = 0x4000;

pub(super) const LINUX_EXEC_SNAPSHOT_BYTES: usize = 0x4000;

pub(super) const LINUX_EXEC_SNAPSHOT_ALLOCATION_BYTES: usize =
    LINUX_EXEC_SNAPSHOT_BYTES + LINUX_PAGE_SIZE as usize;

pub(super) const LINUX_EXEC_STRING_MAX: usize = 4096;

pub(super) const LINUX_EXEC_PATH_MAX: usize = 256;

pub(super) const LINUX_AT_FDCWD: Word = (-100isize) as Word;

pub(super) const LINUX_AT_REMOVEDIR: Word = 0x200;

pub(super) const LINUX_AT_SYMLINK_FOLLOW: Word = 0x400;

pub(super) const LINUX_AT_EMPTY_PATH: Word = 0x1000;

pub(super) const LINUX_PROT_NONE: Word = 0x0;

pub(super) const LINUX_PROT_READ: Word = 0x1;

pub(super) const LINUX_PROT_WRITE: Word = 0x2;

pub(super) const LINUX_PROT_EXEC: Word = 0x4;

pub(super) const LINUX_PROT_ALL: Word = LINUX_PROT_READ | LINUX_PROT_WRITE | LINUX_PROT_EXEC;

pub(super) const LINUX_DIRECT_COPY_CHUNK: Word = 0x10_0000;

pub(super) const NETWORK_PAYLOAD_OFFSET: Word = 32;

pub(super) const UDP_SOCKET_PAYLOAD_MAX: Word = 1472;

pub(super) const TCP_SOCKET_PAYLOAD_MAX: Word = 1460;

pub(super) const ICMP_SOCKET_PAYLOAD_MAX: Word = 1480;

pub(super) const NETWORK_SEND_RETRIES: usize = 16;

pub(super) const LINUX_AF_INET: Word = 2;

pub(super) const LINUX_AF_NETLINK: Word = 16;

pub(super) const LINUX_SOCK_STREAM: Word = 1;

pub(super) const LINUX_SOCK_DGRAM: Word = 2;

pub(super) const LINUX_SOCK_RAW: Word = 3;

pub(super) const LINUX_SOCK_TYPE_MASK: Word = 0xf;

pub(super) const LINUX_SOCK_NONBLOCK: Word = 0x800;

pub(super) const LINUX_SOCK_CLOEXEC: Word = 0x80000;

pub(super) const LINUX_IPPROTO_TCP: Word = 6;

pub(super) const LINUX_IPPROTO_UDP: Word = 17;

pub(super) const LINUX_IPPROTO_ICMP: Word = 1;

pub(super) const LINUX_NETLINK_ROUTE: Word = 0;

pub(super) const LINUX_SOCKADDR_IN_LEN: Word = 16;

pub(super) const LINUX_SOCKADDR_NL_LEN: Word = 12;

pub(super) const LINUX_ICMP_HEADER_LEN: Word = 8;

pub(super) const LINUX_IPV4_HEADER_LEN: Word = 20;

pub(super) const LINUX_MSGHDR_LEN: Word = 56;

pub(super) const LINUX_IOVEC_LEN: Word = 16;

pub(super) const LINUX_NLMSG_HEADER_LEN: Word = 16;

pub(super) const LINUX_IFINFOMSG_LEN: usize = 16;

pub(super) const LINUX_IFADDRMSG_LEN: usize = 8;

pub(super) const LINUX_NLMSG_DONE: u16 = 3;

pub(super) const LINUX_RTM_NEWLINK: u16 = 16;

pub(super) const LINUX_RTM_GETLINK: u16 = 18;

pub(super) const LINUX_RTM_NEWADDR: u16 = 20;

pub(super) const LINUX_RTM_GETADDR: u16 = 22;

pub(super) const LINUX_NLM_F_MULTI: u16 = 2;

pub(super) const LINUX_IFLA_ADDRESS: u16 = 1;

pub(super) const LINUX_IFLA_BROADCAST: u16 = 2;

pub(super) const LINUX_IFLA_IFNAME: u16 = 3;

pub(super) const LINUX_IFLA_MTU: u16 = 4;

pub(super) const LINUX_IFA_ADDRESS: u16 = 1;

pub(super) const LINUX_IFA_LOCAL: u16 = 2;

pub(super) const LINUX_IFA_LABEL: u16 = 3;

pub(super) const LINUX_IFA_BROADCAST: u16 = 4;

pub(super) const LINUX_ARPHRD_ETHER: u16 = 1;

pub(super) const LINUX_ARPHRD_LOOPBACK: u16 = 772;

pub(super) const LINUX_IFF_UP: u32 = 0x1;

pub(super) const LINUX_IFF_BROADCAST: u32 = 0x2;

pub(super) const LINUX_IFF_LOOPBACK: u32 = 0x8;

pub(super) const LINUX_IFF_RUNNING: u32 = 0x40;

pub(super) const LINUX_IFF_MULTICAST: u32 = 0x1000;

pub(super) const LINUX_RT_SCOPE_UNIVERSE: u8 = 0;

pub(super) const LINUX_RT_SCOPE_HOST: u8 = 254;

pub(super) const LINUX_SOL_SOCKET: Word = 1;

pub(super) const LINUX_SO_TYPE: Word = 3;

pub(super) const LINUX_SO_ACCEPTCONN: Word = 30;

pub(super) const TCP_FLAG_FIN: Word = 0x01;

pub(super) const TCP_FLAG_PSH: Word = 0x08;

pub(super) const TCP_FLAG_ACK: Word = 0x10;

pub(super) const LINUX_SIGCHLD: Word = 17;

pub(super) const LINUX_CLONE_VM: Word = 0x0000_0100;

pub(super) const LINUX_CLONE_PARENT_SETTID: Word = 0x0010_0000;

pub(super) const LINUX_CLONE_SETTLS: Word = 0x0008_0000;

pub(super) const LINUX_CLONE_CHILD_SETTID: Word = 0x0100_0000;

pub(super) const LINUX_O_CREAT: Word = 0o100;

pub(super) const LINUX_O_ACCMODE: Word = 0o3;

pub(super) const LINUX_O_RDONLY: Word = 0;

pub(super) const LINUX_O_WRONLY: Word = 1;

pub(super) const LINUX_O_TRUNC: Word = 0o1000;

pub(super) const LINUX_O_APPEND: Word = 0o2000;

pub(super) const LINUX_O_NONBLOCK: Word = 0o4000;
pub(super) const LINUX_O_DSYNC: Word = 0o10000;
pub(super) const LINUX_O_SYNC: Word = 0o4010000;

pub(super) const LINUX_O_LARGEFILE: Word = 0o100000;

pub(super) const LINUX_O_DIRECTORY: Word = 0o200000;

pub(super) const LINUX_O_CLOEXEC: Word = 0o2000000;

pub(super) const LINUX_MAP_FIXED: Word = 0x10;

pub(super) const LINUX_MAP_ANONYMOUS: Word = 0x20;

pub(super) const LINUX_MREMAP_MAYMOVE: Word = 0x1;

pub(super) const LINUX_MREMAP_FIXED: Word = 0x2;

pub(super) const LINUX_MREMAP_SUPPORTED_FLAGS: Word = LINUX_MREMAP_MAYMOVE | LINUX_MREMAP_FIXED;

pub(super) const LINUX_MADV_NORMAL: Word = 0;

pub(super) const LINUX_MADV_RANDOM: Word = 1;

pub(super) const LINUX_MADV_SEQUENTIAL: Word = 2;

pub(super) const LINUX_MADV_WILLNEED: Word = 3;

pub(super) const LINUX_MADV_DONTNEED: Word = 4;

pub(super) const LINUX_MADV_FREE: Word = 8;

pub(super) const LINUX_MADV_MERGEABLE: Word = 12;

pub(super) const LINUX_MADV_UNMERGEABLE: Word = 13;

pub(super) const LINUX_MADV_HUGEPAGE: Word = 14;

pub(super) const LINUX_MADV_NOHUGEPAGE: Word = 15;

pub(super) const LINUX_MADV_DONTDUMP: Word = 16;

pub(super) const LINUX_MADV_DODUMP: Word = 17;

pub(super) const LINUX_MADV_COLD: Word = 20;

pub(super) const LINUX_MADV_PAGEOUT: Word = 21;

pub(super) const LINUX_MADV_POPULATE_READ: Word = 22;

pub(super) const LINUX_MADV_POPULATE_WRITE: Word = 23;

pub(super) const LINUX_IOV_MAX: Word = 16;

pub(super) const LINUX_POLLFD_BYTES: Word = 8;

pub(super) const LINUX_POLLFD_MAX: Word = 32;

pub(super) const LINUX_POLLIN: i16 = 0x0001;

pub(super) const LINUX_POLLOUT: i16 = 0x0004;

pub(super) const LINUX_POLLNVAL: i16 = 0x0020;

pub(super) const LINUX_TCGETS: Word = 0x5401;

pub(super) const LINUX_TCSETS: Word = 0x5402;

pub(super) const LINUX_TCSETSW: Word = 0x5403;

pub(super) const LINUX_TCSETSF: Word = 0x5404;

pub(super) const LINUX_TIOCGWINSZ: Word = 0x5413;

pub(super) const LINUX_INPUT_EVENT_BYTES: Word = 24;

pub(super) const LINUX_EV_SYN: u16 = 0;

pub(super) const LINUX_EV_KEY: u16 = 1;

pub(super) const LINUX_EV_REL: u16 = 2;

pub(super) const LINUX_SYN_REPORT: u16 = 0;

pub(super) const LINUX_REL_X: u16 = 0;

pub(super) const LINUX_REL_Y: u16 = 1;

pub(super) const LINUX_REL_WHEEL: u16 = 8;

pub(super) const LINUX_FBIOGET_VSCREENINFO: Word = 0x4600;

pub(super) const LINUX_FBIOPUT_VSCREENINFO: Word = 0x4601;

pub(super) const LINUX_FBIOGET_FSCREENINFO: Word = 0x4602;

pub(super) const LINUX_FBIOPAN_DISPLAY: Word = 0x4606;

pub(super) const LINUX_EVIOCGVERSION: Word = 0x8004_4501;

pub(super) const LINUX_EVIOCGID: Word = 0x8008_4502;

pub(super) const LINUX_FB_FIX_SCREENINFO_BYTES: Word = 80;

pub(super) const LINUX_FB_VAR_SCREENINFO_BYTES: Word = 160;

pub(super) const LINUX_F_DUPFD: Word = 0;

pub(super) const LINUX_F_DUPFD_CLOEXEC: Word = 1030;

pub(super) const LINUX_F_GETFD: Word = 1;

pub(super) const LINUX_F_SETFD: Word = 2;

pub(super) const LINUX_F_GETFL: Word = 3;

pub(super) const LINUX_F_SETFL: Word = 4;

pub(super) const LINUX_FD_CLOEXEC: Word = 1;

pub(super) const LINUX_S_IFMT: Word = 0o170000;

pub(super) const LINUX_S_IFIFO: Word = 0o010000;

pub(super) const LINUX_S_IFCHR: Word = 0o020000;

pub(super) const LINUX_S_IFBLK: Word = 0o060000;

pub(super) const LINUX_STATX_BASIC_STATS: u32 = 0x07ff;

pub(super) const POSIX_FILE_TYPE_PIPE: Word = 0xffff_fffe;

pub(super) const POSIX_FILE_TYPE_SOCKET: Word = 0xffff_fffd;

pub(super) const LINUX_S_IFSOCK: Word = 0o140000;

pub(super) const LINUX_TERMIOS_BYTES: Word = 36;

pub(super) const LINUX_ICRNL: u32 = 0x0000_0100;

pub(super) const LINUX_IXON: u32 = 0x0000_0400;

pub(super) const LINUX_OPOST: u32 = 0x0000_0001;

pub(super) const LINUX_ONLCR: u32 = 0x0000_0004;

pub(super) const LINUX_CS8: u32 = 0x0000_0030;

pub(super) const LINUX_CREAD: u32 = 0x0000_0080;

pub(super) const LINUX_ISIG: u32 = 0x0000_0001;

pub(super) const LINUX_ICANON: u32 = 0x0000_0002;

pub(super) const LINUX_ECHO: u32 = 0x0000_0008;

pub(super) const LINUX_ECHOE: u32 = 0x0000_0010;

pub(super) const LINUX_ECHOK: u32 = 0x0000_0020;

pub(super) const LINUX_IEXTEN: u32 = 0x0000_8000;

pub(super) const LINUX_VINTR: Word = 0;

pub(super) const LINUX_VQUIT: Word = 1;

pub(super) const LINUX_VERASE: Word = 2;

pub(super) const LINUX_VKILL: Word = 3;

pub(super) const LINUX_VEOF: Word = 4;

pub(super) const LINUX_VTIME: Word = 5;

pub(super) const LINUX_VMIN: Word = 6;

pub(super) const LINUX_VSTART: Word = 8;

pub(super) const LINUX_VSTOP: Word = 9;

pub(super) const LINUX_VSUSP: Word = 10;

pub(super) const LINUX_VEOL: Word = 11;

pub(super) const LINUX_WNOHANG: Word = 1;

pub(super) const LINUX_STACK_T_BYTES: Word = 24;

pub(super) const LINUX_SS_DISABLE: Word = 2;

pub(super) const LINUX_SS_AUTODISARM: Word = 0x8000_0000;

pub(super) const LINUX_MINSIGSTKSZ: Word = 2048;

pub(super) const LINUX_NSIG: Word = 64;

pub(super) const LINUX_SIGSET_BYTES: Word = 8;

pub(super) const LINUX_KERNEL_SIGACTION_BYTES: usize = 32;

pub(super) const LINUX_TIMESPEC_BYTES: Word = 16;

pub(super) const ALTER_FB_PRESENT_HZ: Word = 60;

pub(super) const ARCH_SET_FS: Word = 0x1002;

pub(super) const ARCH_GET_FS: Word = 0x1003;

pub(super) const AT_NULL: Word = 0;

pub(super) const AT_PHDR: Word = 3;

pub(super) const AT_PHENT: Word = 4;

pub(super) const AT_PHNUM: Word = 5;

pub(super) const AT_PAGESZ: Word = 6;

pub(super) const AT_BASE: Word = 7;

pub(super) const AT_FLAGS: Word = 8;

pub(super) const AT_ENTRY: Word = 9;

pub(super) const AT_UID: Word = 11;

pub(super) const AT_EUID: Word = 12;

pub(super) const AT_GID: Word = 13;

pub(super) const AT_EGID: Word = 14;

pub(super) const AT_PLATFORM: Word = 15;

pub(super) const AT_HWCAP: Word = 16;

pub(super) const AT_CLKTCK: Word = 17;

pub(super) const AT_SECURE: Word = 23;

pub(super) const AT_RANDOM: Word = 25;

pub(super) const AT_EXECFN: Word = 31;

pub(super) const LINUX_DIRENT64_NAME_OFFSET: usize = 19;

pub(super) const LINUX_DT_UNKNOWN: Word = 0;

pub(super) const LINUX_DT_CHR: Word = 2;

pub(super) const LINUX_DT_DIR: Word = 4;

pub(super) const LINUX_DT_BLK: Word = 6;

pub(super) const LINUX_DT_REG: Word = 8;
