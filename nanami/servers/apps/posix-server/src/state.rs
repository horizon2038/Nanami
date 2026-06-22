use libnanami::Word;
use nanami_services::posix::*;

pub(crate) const SLOT_SERVICE_PORT: Word = 20;
pub(crate) const SLOT_VFS_SERVICE: Word = 23;
pub(crate) const VFS_SHM_BYTES: Word = 0x4000;
pub(crate) const MAX_SESSIONS: usize = 16;
pub(crate) const MAX_FDS: usize = 32;
pub(crate) const MAX_OPEN_FILES: usize = 128;
pub(crate) const MAX_ENV_VARS: usize = 16;
pub(crate) const PATH_MAX: usize = POSIX_PATH_MAX;
pub(crate) const ENV_NAME_MAX: usize = POSIX_ENV_NAME_MAX;
pub(crate) const ENV_VALUE_MAX: usize = POSIX_ENV_VALUE_MAX;
pub(crate) const MAX_COMPONENTS: usize = 16;
pub(crate) const VFS_PATH_OFFSET: usize = 0;
pub(crate) const VFS_PATH2_OFFSET: usize = 256;
pub(crate) const VFS_IO_OFFSET: usize = 512;

#[derive(Clone, Copy)]
pub(crate) struct Session {
    pub(crate) active: bool,
    pub(crate) owner_pid: Word,
    pub(crate) shm_local: Word,
    pub(crate) shm_size: Word,
    pub(crate) posix_pid: Word,
    pub(crate) posix_ppid: Word,
    pub(crate) posix_pgid: Word,
    pub(crate) posix_sid: Word,
    pub(crate) uid: Word,
    pub(crate) euid: Word,
    pub(crate) gid: Word,
    pub(crate) egid: Word,
    pub(crate) cwd: [u8; PATH_MAX],
    pub(crate) cwd_len: usize,
    pub(crate) env: [EnvironmentVariable; MAX_ENV_VARS],
    pub(crate) fds: [FileDescriptor; MAX_FDS],
}

impl Session {
    pub(crate) const EMPTY: Self = Self {
        active: false,
        owner_pid: 0,
        shm_local: 0,
        shm_size: 0,
        posix_pid: 0,
        posix_ppid: 0,
        posix_pgid: 0,
        posix_sid: 0,
        uid: POSIX_ROOT_UID,
        euid: POSIX_ROOT_UID,
        gid: POSIX_ROOT_GID,
        egid: POSIX_ROOT_GID,
        cwd: [0; PATH_MAX],
        cwd_len: 0,
        env: [EnvironmentVariable::EMPTY; MAX_ENV_VARS],
        fds: [FileDescriptor::EMPTY; MAX_FDS],
    };
}

#[derive(Clone, Copy)]
pub(crate) struct EnvironmentVariable {
    pub(crate) active: bool,
    pub(crate) name: [u8; ENV_NAME_MAX],
    pub(crate) name_len: usize,
    pub(crate) value: [u8; ENV_VALUE_MAX],
    pub(crate) value_len: usize,
}

impl EnvironmentVariable {
    pub(crate) const EMPTY: Self = Self {
        active: false,
        name: [0; ENV_NAME_MAX],
        name_len: 0,
        value: [0; ENV_VALUE_MAX],
        value_len: 0,
    };
}

#[derive(Clone, Copy)]
pub(crate) struct FileDescriptor {
    pub(crate) active: bool,
    pub(crate) open_file: usize,
    pub(crate) flags: Word,
}

impl FileDescriptor {
    pub(crate) const EMPTY: Self = Self {
        active: false,
        open_file: 0,
        flags: 0,
    };
}

#[derive(Clone, Copy)]
pub(crate) struct OpenFile {
    pub(crate) active: bool,
    pub(crate) kind: FdKind,
    pub(crate) offset: Word,
    pub(crate) vfs_handle: Word,
    pub(crate) ref_count: Word,
}

impl OpenFile {
    pub(crate) const EMPTY: Self = Self {
        active: false,
        kind: FdKind::Empty,
        offset: 0,
        vfs_handle: 0,
        ref_count: 0,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FdKind {
    Empty,
    Regular,
    Directory,
    DevNull,
    DevZero,
}

pub(crate) struct Runtime {
    pub(crate) vfs_port: Word,
    pub(crate) vfs_shm: Word,
    pub(crate) vfs_shm_size: Word,
    pub(crate) sessions: [Session; MAX_SESSIONS],
    pub(crate) open_files: [OpenFile; MAX_OPEN_FILES],
    pub(crate) next_posix_pid: Word,
}
