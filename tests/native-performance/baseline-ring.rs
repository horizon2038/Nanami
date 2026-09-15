// Test-only baseline: original terminal ByteRing before bulk transfers.
use super::RING_BYTES;

#[derive(Clone, Copy)]
pub struct ByteRing {
    buffer: [u8; RING_BYTES],
    read: usize,
    write: usize,
    len: usize,
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

    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buffer[self.read];
        self.read = (self.read + 1) % self.buffer.len();
        self.len -= 1;
        Some(byte)
    }

    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.len = 0;
    }
}
