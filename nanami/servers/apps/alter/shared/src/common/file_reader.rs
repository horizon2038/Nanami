//! Request-local direct reader, never a cache of FD metadata or file bytes.
//! Only a live LinuxFile's readonly-open grant may enter this synchronous read
//! scope. Negotiated cursor grants share the VFS-owned sequential position;
//! all other descriptions retain POSIX offset translation.
use nanami_runtime::{RequestError, Word};
use nanami_client::{posix, vfs};

#[derive(Clone, Copy)]
pub struct Reader { fd: Word, token: Word, port: Word, cursor: bool }
impl Reader {
    pub const EMPTY: Self = Self { fd: Word::MAX, token: 0, port: 0, cursor: false };
    pub fn new(fd: Word, token: Word, port: Word) -> Self {
        if token > Word::MAX >> 8 { return Self::EMPTY; }
        Self { fd, token, port, cursor: false }
    }
    pub fn with_cursor(mut self, cursor: bool) -> Self { self.cursor = cursor && self.token != 0; self }
    pub fn cursor_for(&self, fd: Word) -> bool { self.cursor && self.token != 0 && self.fd == fd }
    fn matches(&self, fd: Word) -> bool { self.token != 0 && self.port != 0 && self.fd == fd }
    fn trace(&self, operation: &str, success: bool) {
        if option_env!("NANAMI_FD_DIRECT_TRACE").is_some() {
            nanami_runtime::print!("[alter.fd-direct] op={} fd={} ok={}\n", operation, self.fd, usize::from(success));
        }
    }
    pub fn read(&self, fd: Word, output: Word, bytes: Word) -> Option<Result<Word, RequestError>> {
        (self.matches(fd) && self.cursor).then(|| {
            let result = vfs::vfs_read_cursor_granted(self.port, self.token, bytes, output);
            self.trace("read-cursor", result.is_ok()); result
        })
    }
    pub fn seek(&self, fd: Word, offset: Word, whence: Word) -> Option<Result<Word, RequestError>> {
        (self.matches(fd) && self.cursor).then(|| {
            let result = vfs::vfs_seek_cursor_granted(self.port, self.token, offset, whence);
            self.trace("seek-cursor", result.is_ok()); result
        })
    }
    pub fn pread(&self, fd: Word, output: Word, bytes: Word, offset: Word) -> Option<Result<Word, RequestError>> {
        self.matches(fd).then(|| {
            let result = vfs::vfs_pread_granted(self.port, self.token, offset, bytes, output);
            self.trace("pread", result.is_ok()); result
        })
    }
    pub fn versioned(&self, fd: Word, bytes: Word, identity: Word) -> Option<Result<(Word, Word), RequestError>> {
        self.matches(fd).then(|| {
            let result = vfs::vfs_read_versioned_granted(self.port, self.token, identity, bytes);
            self.trace("versioned", result.is_ok()); result
        })
    }
    pub fn fstat(&self, posix_port: Word, fd: Word, flags: Word) -> Result<(Word, Word, Word, Word, Word), RequestError> {
        if self.matches(fd) {
            let result = vfs::vfs_fstat_granted(self.port, self.token, flags);
            self.trace("fstat", result.is_ok());
            if let Ok((inode, size, kind)) = result {
                // Grants are issued only for regular files. Do not publish a
                // malformed provider type as POSIX device/socket metadata.
                if kind == vfs::VFS_FILE_TYPE_REGULAR {
                    return Ok((inode, size, posix::POSIX_FILE_TYPE_REGULAR, 0, 0));
                }
            }
        }
        posix::posix_fstat_flags(posix_port, fd, flags)
    }
}

