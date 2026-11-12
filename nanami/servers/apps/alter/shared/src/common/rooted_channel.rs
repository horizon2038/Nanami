//! Read-only rooted queries to the same provider that backs POSIX. Directory
//! FDs, open and mutations still need POSIX's descriptor/session translation.
use nanami_client::vfs::*;
use nanami_runtime::Word;

pub struct Channel {
    attempted: bool,
    port: Word,
    shm: Word,
    bytes: Word,
}

impl Channel {
    pub const fn new() -> Self {
        Self { attempted: false, port: 0, shm: 0, bytes: 0 }
    }

    pub(crate) fn port(&mut self, posix: Word) -> Option<Word> {
        if !self.attempted { self.connect(posix); }
        (self.port != 0).then_some(self.port)
    }

    fn connect(&mut self, posix: Word) {
        self.attempted = true;
        let Ok(expected) = nanami_client::posix::posix_vfs_provider(posix) else { return };
        if expected == 0 { return; }
        let slot = crate::abi::SLOT_VFS_SERVICE;
        let Ok(provider) = nanami_client::registry::connect_service_with_pid(
            nanami_client::registry::VFS_SERVICE, slot,
        ) else { return };
        if provider != expected { return; }
        let port = nanami_runtime::ipc::process_slot_descriptor(slot);
        // A separate control window never aliases the delegated file-data
        // window, whose contents/lifetime belong to the population reader.
        let Ok((shm, bytes)) = vfs_attach_shared_memory(port, 0x10000) else { return };
        self.shm = shm;
        self.bytes = bytes;
        if shm != 0 && bytes >= 0x10000 && shm.checked_add(bytes).is_some() {
            self.port = port;
        }
    }

    /// None requests the original POSIX route. Transport/protocol failures
    /// disable this channel permanently; mapped ownership is retained. These
    /// queries have no descriptor/offset side effects, so retry is safe.
    pub fn request(
        &mut self, posix: Word, source: Word, bytes: Word,
        descriptor: Word, mut request: PathRequest,
    ) -> Option<Result<(Word, Word), PathError>> {
        if !matches!(request.operation, VFS_PATH_RESOLVE | VFS_PATH_STAT | VFS_PATH_READLINK)
            || source == 0 || source.checked_add(bytes).is_none()
        { return None; }
        let extents = [
            (descriptor, core::mem::size_of::<PathRequest>()),
            (request.root_offset, request.root_bytes),
            (request.path_offset, request.path_bytes),
            (request.target_offset, request.target_bytes),
            (request.output_offset, request.output_bytes),
        ];
        if extents.iter().any(|&(offset, count)| offset.checked_add(count).is_none_or(|end| end > bytes)) {
            return None;
        }
        let absolute = request.path_bytes != 0
            && unsafe { *((source + request.path_offset) as *const u8) == b'/' };
        if !absolute && request.directory != VFS_PATH_DIRECTORY_ROOT { return None; }
        if !self.attempted { self.connect(posix); }
        if self.port == 0 { return None; }
        let end = self.shm + self.bytes; // Validated at attach.
        if (source < end && self.shm < source + bytes)
            || extents.iter().any(|&(offset, count)| offset.checked_add(count).is_none_or(|end| end > self.bytes))
        {
            self.port = 0;
            return None;
        }
        // Absolute paths must ignore even an invalid POSIX directory FD, just
        // as the proxy does. Never forward a POSIX FD as a VFS handle.
        request.directory = VFS_PATH_DIRECTORY_ROOT;
        unsafe {
            for &(offset, count) in &extents[1..4] {
                core::ptr::copy_nonoverlapping((source + offset) as *const u8,
                    (self.shm + offset) as *mut u8, count);
            }
            core::ptr::write_unaligned((self.shm + descriptor) as *mut PathRequest, request);
        }
        let result = vfs_path_request(self.port, descriptor);
        if option_env!("NANAMI_ROOTED_CHANNEL_TRACE").is_some() {
            let (status, detail) = match &result {
                Ok(details) => (0, details.0),
                Err(PathError::Filesystem(code)) => (1, *code),
                Err(PathError::Transport(_)) => (2, 0),
            };
            nanami_runtime::print!("[alter.rooted-direct] op={} status={} detail0={}\n",
                request.operation, status, detail);
        }
        match result {
            Ok(details) => {
                if request.operation != VFS_PATH_STAT {
                    if details.0 > request.output_bytes {
                        self.port = 0;
                        return None;
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping((self.shm + request.output_offset) as *const u8,
                            (source + request.output_offset) as *mut u8, details.0);
                    }
                }
                Some(Ok(details))
            }
            Err(error @ PathError::Filesystem(_)) => Some(Err(error)),
            Err(PathError::Transport(_)) => { self.port = 0; None }
        }
    }
}
