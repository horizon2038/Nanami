//! Additive Alter/Linux execution-context API; existing launch requests remain unchanged.
use crate::Word;
pub const ALTER_REQUEST_CONTROL: Word = 0xb101;
pub const ALTER_REQUEST_STATUS: Word = 0xb103;
pub const ALTER_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;

pub const ALTER_REQUEST_PREPARE_CONTEXT: Word = 0xb108;
pub const ALTER_REQUEST_START_CONTEXT: Word = 0xb109;
pub const ALTER_REQUEST_DELETE_CONTEXT: Word = 0xb10a;
pub const ALTER_REQUEST_SIGNAL_CONTEXT: Word = 0xb10b;
pub const ALTER_CONTEXT_READONLY: Word = 1;
/// Explicit opt-in to a window-local framebuffer and input session.
pub const ALTER_CONTEXT_GRAPHICS: Word = 1 << 1;
const ALTER_CONTEXT_FLAGS: Word = ALTER_CONTEXT_READONLY | ALTER_CONTEXT_GRAPHICS;
pub const ALTER_CONTEXT_ABI: Word = 1;
pub const ALTER_CONTEXT_HEADER_BYTES: usize = 48;

pub struct ExecutionRequest<'a> {
    pub root: &'a [u8],
    pub cwd: &'a [u8],
    pub executable: &'a [u8],
    /// Existing Alter argc/envc + NUL-terminated argv/envp wire payload.
    pub launch: &'a [u8],
    pub flags: Word,
}

impl<'a> ExecutionRequest<'a> {
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < ALTER_CONTEXT_HEADER_BYTES {
            return None;
        }
        let word = |index: usize| -> Option<usize> {
            Some(u64::from_le_bytes(bytes.get(index * 8..index * 8 + 8)?.try_into().ok()?) as usize)
        };
        if word(0)? != ALTER_CONTEXT_ABI || word(1)? & !ALTER_CONTEXT_FLAGS != 0 {
            return None;
        }
        let mut cursor = ALTER_CONTEXT_HEADER_BYTES;
        let mut take = |len: usize| {
            let end = cursor.checked_add(len)?;
            let result = bytes.get(cursor..end)?;
            cursor = end;
            Some(result)
        };
        let root = take(word(2)?)?;
        let cwd = take(word(3)?)?;
        let executable = take(word(4)?)?;
        let launch = take(word(5)?)?;
        if cursor != bytes.len() {
            return None;
        }
        Some(Self {
            root,
            cwd,
            executable,
            launch,
            flags: word(1)?,
        })
    }

    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let parts = [self.root, self.cwd, self.executable, self.launch];
        let size = parts
            .iter()
            .try_fold(ALTER_CONTEXT_HEADER_BYTES, |total, part| {
                total.checked_add(part.len())
            })?;
        if size > out.len() || self.flags & !ALTER_CONTEXT_FLAGS != 0 {
            return None;
        }
        for (index, value) in [
            ALTER_CONTEXT_ABI,
            self.flags,
            self.root.len(),
            self.cwd.len(),
            self.executable.len(),
            self.launch.len(),
        ]
        .iter()
        .enumerate()
        {
            out[index * 8..index * 8 + 8].copy_from_slice(&(*value as u64).to_le_bytes());
        }
        let mut cursor = ALTER_CONTEXT_HEADER_BYTES;
        for part in parts {
            out[cursor..cursor + part.len()].copy_from_slice(part);
            cursor += part.len();
        }
        Some(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_wire_is_bounded_and_rejects_trailing_data() {
        let request = ExecutionRequest {
            root: b"/bundle/rootfs",
            cwd: b"/",
            executable: b"/bin/busybox",
            launch: b"payload",
            flags: ALTER_CONTEXT_READONLY,
        };
        let mut bytes = [0; 128];
        let size = request.encode(&mut bytes).unwrap();
        let decoded = ExecutionRequest::decode(&bytes[..size]).unwrap();
        assert_eq!(decoded.root, request.root);
        assert_eq!(decoded.launch, request.launch);
        for end in 0..size {
            assert!(ExecutionRequest::decode(&bytes[..end]).is_none());
        }
        assert!(ExecutionRequest::decode(&bytes[..size + 1]).is_none());
        bytes[8] = (ALTER_CONTEXT_READONLY | ALTER_CONTEXT_GRAPHICS) as u8;
        assert!(ExecutionRequest::decode(&bytes[..size]).is_some());
        bytes[8] = 4;
        assert!(ExecutionRequest::decode(&bytes[..size]).is_none());
    }
}
