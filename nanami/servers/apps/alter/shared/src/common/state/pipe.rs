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
        self.write = (self.write + source.len()) % LINUX_PIPE_BYTES;
        self.len += source.len();
    }
}
