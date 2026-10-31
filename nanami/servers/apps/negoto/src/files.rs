use alloc::{format, vec, vec::Vec};
use nanami_client::{registry, vfs};
use nanami_runtime::{ipc, Word};

pub struct Files {
    port: Word,
    shm: Word,
    bytes: Word,
}
impl Files {
    pub fn connect() -> Result<Self, alloc::string::String> {
        registry::connect_vfs_service(24).map_err(|e| format!("VFS: {e}"))?;
        let port = ipc::process_slot_descriptor(24);
        let (shm, bytes) = vfs::vfs_attach_shared_memory(port, 64 * 1024)
            .map_err(|e| format!("VFS memory: {e}"))?;
        Ok(Self { port, shm, bytes })
    }
    fn path(&self, path: &str) -> Result<usize, alloc::string::String> {
        if !path.starts_with('/') || path.len() >= 512 || path.contains('\0') {
            return Err("invalid VFS pathname".into());
        }
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), self.shm as *mut u8, path.len());
        }
        Ok(path.len())
    }
    pub fn stat(&self, path: &str) -> Result<(Word, Word, Word), alloc::string::String> {
        let len = self.path(path)?;
        vfs::vfs_stat(self.port, 0, len).map_err(|e| format!("{path}: {e}"))
    }
    pub fn remove(&self, path: &str) -> Result<(), alloc::string::String> {
        let len = self.path(path)?;
        vfs::vfs_remove(self.port, 0, len).map_err(|e| format!("remove {path}: {e}"))
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>, alloc::string::String> {
        let (_, len, kind) = self.stat(path)?;
        if kind != vfs::VFS_FILE_TYPE_REGULAR || len > negoto_oci::CONFIG_MAX_BYTES {
            return Err("config.json must be a regular file <=32 KiB".into());
        }
        let path_len = self.path(path)?;
        let handle =
            vfs::vfs_open(self.port, 0, path_len).map_err(|e| format!("open {path}: {e}"))?;
        let result: Result<Vec<u8>, alloc::string::String> = (|| {
            let mut data = vec![0; len];
            let mut done = 0;
            while done < len {
                let wanted = (len - done).min(self.bytes);
                let count = vfs::vfs_read(self.port, handle, done, wanted, 0)
                    .map_err(|e| format!("read {path}: {e}"))?;
                if count == 0 || count > wanted {
                    return Err("config.json was truncated during read".into());
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        self.shm as *const u8,
                        data[done..].as_mut_ptr(),
                        count,
                    );
                }
                done += count;
            }
            Ok(data)
        })();
        let close = vfs::vfs_close(self.port, handle).map_err(|e| format!("close {path}: {e}"));
        let data = result?;
        close?;
        Ok(data)
    }
}
