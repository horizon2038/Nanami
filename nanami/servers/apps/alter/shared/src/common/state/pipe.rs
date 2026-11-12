use super::{Word, LINUX_PIPE_BYTES};

#[derive(Clone, Copy)]
pub struct LinuxPipe {
    pub active: bool,
    pub readers: Word,
    pub writers: Word,
    // Runtime owns this allocation; copying metadata does not transfer ownership.
    // Only the `len` bytes beginning at `read` are initialized/readable. Writers
    // initialize their range before advancing write/len; unused capacity is raw.
    pub(super) buffer: Word,
    pub read: usize,
    pub write: usize,
    pub len: usize,
}

impl LinuxPipe {
    pub const EMPTY: Self = Self {
        active: false, readers: 0, writers: 0, buffer: 0, read: 0, write: 0, len: 0,
    };

    /// Only the published queue prefix is initialized. The runtime keeps the
    /// allocation alive throughout this synchronous, non-reentrant copy.
    pub fn contiguous_read(&self, count: usize) -> Option<&[u8]> {
        if !self.active || self.buffer == 0 || count > self.len
            || self.read >= LINUX_PIPE_BYTES || count > LINUX_PIPE_BYTES - self.read
        { return None; }
        Some(unsafe { core::slice::from_raw_parts((self.buffer + self.read) as *const u8, count) })
    }

    /// Free capacity is not readable bytes. Copyin must succeed before the
    /// caller publishes this prefix through commit_write.
    pub fn contiguous_spare(&mut self, count: usize) -> Option<&mut [core::mem::MaybeUninit<u8>]> {
        if !self.active || self.buffer == 0 || self.len > LINUX_PIPE_BYTES
            || count > LINUX_PIPE_BYTES - self.len || self.write >= LINUX_PIPE_BYTES
            || count > LINUX_PIPE_BYTES - self.write
        { return None; }
        Some(unsafe { core::slice::from_raw_parts_mut(
            (self.buffer + self.write) as *mut core::mem::MaybeUninit<u8>, count,
        ) })
    }

    /// Called only after all accepted bytes have been initialized.
    pub fn commit_write(&mut self, count: usize) {
        debug_assert!(count <= LINUX_PIPE_BYTES - self.len);
        self.write = (self.write + count) % LINUX_PIPE_BYTES;
        self.len += count;
    }

    /// Stage current bytes without consuming them: guest copyout can still fail.
    pub fn copy_out(&self, target: &mut [u8]) -> usize {
        debug_assert!(self.active && self.buffer != 0 && self.read < LINUX_PIPE_BYTES);
        debug_assert!(target.len() <= self.len && self.len <= LINUX_PIPE_BYTES);
        let first = target.len().min(LINUX_PIPE_BYTES - self.read);
        unsafe {
            core::ptr::copy_nonoverlapping((self.buffer + self.read) as *const u8,
                target.as_mut_ptr(), first);
            if first < target.len() {
                core::ptr::copy_nonoverlapping(self.buffer as *const u8,
                    target.as_mut_ptr().add(first), target.len() - first);
            }
        }
        (self.read + target.len()) % LINUX_PIPE_BYTES
    }

    /// The source has already passed the fallible guest/service read.
    pub fn copy_in(&mut self, source: &[u8]) {
        debug_assert!(self.active && self.buffer != 0 && self.write < LINUX_PIPE_BYTES);
        debug_assert!(source.len() <= LINUX_PIPE_BYTES - self.len);
        let first = source.len().min(LINUX_PIPE_BYTES - self.write);
        unsafe {
            core::ptr::copy_nonoverlapping(source.as_ptr(),
                (self.buffer + self.write) as *mut u8, first);
            if first < source.len() {
                core::ptr::copy_nonoverlapping(source.as_ptr().add(first),
                    self.buffer as *mut u8, source.len() - first);
            }
        }
        self.commit_write(source.len());
    }
}
