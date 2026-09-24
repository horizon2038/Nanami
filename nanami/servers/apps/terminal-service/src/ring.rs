use core::ptr;

use super::RING_BYTES;

#[derive(Clone, Copy)]
pub struct ByteRing {
    buffer: [u8; RING_BYTES],
    read: usize,
    write: usize,
    pub len: usize,
}

impl ByteRing {
    pub const EMPTY: Self = Self {
        buffer: [0; RING_BYTES],
        read: 0,
        write: 0,
        len: 0,
    };

    pub fn push(&mut self, byte: u8) -> bool {
        if self.len >= self.buffer.len() {
            return false;
        }
        self.buffer[self.write] = byte;
        self.write = (self.write + 1) % self.buffer.len();
        self.len += 1;
        true
    }

    /// `source` must be readable for `count` bytes and not overlap this ring.
    /// The caller validates the shared mapping before entering this path.
    pub unsafe fn write_from(&mut self, source: *const u8, count: usize) -> usize {
        let count = count.min(self.buffer.len() - self.len);
        if count == 0 {
            return 0;
        }
        let first = count.min(self.buffer.len() - self.write);
        ptr::copy_nonoverlapping(source, self.buffer.as_mut_ptr().add(self.write), first);
        if first < count {
            ptr::copy_nonoverlapping(source.add(first), self.buffer.as_mut_ptr(), count - first);
        }
        self.write = (self.write + count) % self.buffer.len();
        self.len += count;
        count
    }

    /// Return the number of source bytes consumed, not expanded output bytes.
    /// Never consume a newline unless both CR and LF fit in the ring.
    pub unsafe fn write_crlf(&mut self, source: *const u8, count: usize) -> usize {
        let source = core::slice::from_raw_parts(source, count);
        let mut done = 0;
        while done < count {
            let end = source[done..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(count, |n| done + n);
            let copied = self.write_from(source.as_ptr().add(done), end - done);
            done += copied;
            if done < end || done == count {
                break;
            }
            if RING_BYTES - self.len < 2 {
                break;
            }
            self.push(b'\r');
            self.push(b'\n');
            done += 1;
        }
        done
    }

    /// `destination` must be writable for `count` bytes and not overlap this
    /// ring. Only the returned number of bytes is written.
    pub unsafe fn read_into(&mut self, destination: *mut u8, count: usize) -> usize {
        let count = count.min(self.len);
        if count == 0 {
            return 0;
        }
        let first = count.min(self.buffer.len() - self.read);
        ptr::copy_nonoverlapping(self.buffer.as_ptr().add(self.read), destination, first);
        if first < count {
            ptr::copy_nonoverlapping(self.buffer.as_ptr(), destination.add(first), count - first);
        }
        self.read = (self.read + count) % self.buffer.len();
        self.len -= count;
        count
    }

    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.len = 0;
    }
}
