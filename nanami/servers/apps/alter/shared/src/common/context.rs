//! Per-process backing root. This is an Alter execution mechanism, not OCI policy.
use super::abi::ALTER_PATH_MAX;

#[derive(Clone, Copy)]
pub struct RootfsContext {
    pub leader: usize,
    pub root: [u8; ALTER_PATH_MAX],
    pub root_len: usize,
    pub executable: [u8; ALTER_PATH_MAX],
    pub executable_len: usize,
    pub readonly: bool,
    pub prepared: bool,
}

impl RootfsContext {
    pub const DEFAULT: Self = Self {
        leader: 0,
        root: [0; ALTER_PATH_MAX],
        root_len: 0,
        executable: [0; ALTER_PATH_MAX],
        executable_len: 0,
        readonly: false,
        prepared: false,
    };

    pub fn new(root: &[u8], executable: &[u8], readonly: bool) -> Option<Self> {
        if root == b"/" || !normalized(root) || !normalized(executable) {
            return None;
        }
        let mut result = Self::DEFAULT;
        result.root[..root.len()].copy_from_slice(root);
        result.root_len = root.len();
        result.set_executable(executable)?;
        result.readonly = readonly;
        result.prepared = true;
        Some(result)
    }

    pub fn set_executable(&mut self, path: &[u8]) -> Option<()> {
        if !normalized(path) {
            return None;
        }
        self.executable[..path.len()].copy_from_slice(path);
        self.executable_len = path.len();
        Some(())
    }

    pub fn host_path(&self, guest: &[u8], out: &mut [u8]) -> Option<usize> {
        if !normalized(guest) {
            return None;
        }
        let suffix = if guest == b"/" { &[][..] } else { guest };
        let len = self.root_len.checked_add(suffix.len())?;
        if len > out.len() {
            return None;
        }
        out[..self.root_len].copy_from_slice(&self.root[..self.root_len]);
        out[self.root_len..len].copy_from_slice(suffix);
        Some(len)
    }
}

pub fn normalized(path: &[u8]) -> bool {
    path.first() == Some(&b'/')
        && path.len() < ALTER_PATH_MAX
        && !path.contains(&0)
        && (path.len() == 1 || !path.ends_with(b"/"))
        && (path.len() == 1
            || !path[1..]
                .split(|&byte| byte == b'/')
                .any(|part| part.is_empty() || part == b"." || part == b".."))
}
