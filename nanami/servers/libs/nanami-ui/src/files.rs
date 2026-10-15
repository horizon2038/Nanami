use crate::Files;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use nanami_client::vfs::*;
use nanami_runtime::{RequestError, Word};
const OFFSET: usize = 512;
pub struct Storage {
    port: Word,
    base: usize,
    chunk: usize,
    max_bytes: usize,
}
impl Storage {
    pub fn connect(max_bytes: usize) -> Result<Self, RequestError> {
        nanami_client::registry::connect_vfs_service(26)?;
        let port = nanami_runtime::ipc::process_slot_descriptor(26);
        let (base, size) = vfs_attach_shared_memory(port, 0x10000)?;
        if base == 0 || size <= OFFSET {
            return Err(RequestError::Protocol);
        }
        Ok(Self {
            port,
            base,
            chunk: size - OFFSET,
            max_bytes,
        })
    }
    fn path(&self, path: &str) -> Result<(), String> {
        if path.is_empty() || path.len() > 255 || path.contains('\0') {
            return Err("Invalid file path".to_string());
        }
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), self.base as *mut u8, path.len());
        }
        Ok(())
    }
    fn open(&self, path: &str, create: bool) -> Result<(Word, usize), String> {
        self.path(path)?;
        let (handle, _, size, kind) = vfs_open_compound(
            self.port,
            0,
            path.len(),
            if create { VFS_OPEN_CREATE } else { 0 },
        )
        .map_err(|e| format!("Cannot open: {}", e))?;
        if kind != VFS_FILE_TYPE_REGULAR {
            let _ = vfs_close(self.port, handle);
            return Err("Not a regular file".to_string());
        }
        Ok((handle, size))
    }
    fn finish<T>(&self, handle: Word, result: Result<T, String>) -> Result<T, String> {
        let closed = vfs_close(self.port, handle);
        match result {
            Err(error) => Err(error),
            Ok(value) => {
                closed.map_err(|e| format!("Close failed: {}", e))?;
                Ok(value)
            }
        }
    }
}
impl Files for Storage {
    fn exists(&mut self, path: &str) -> Result<bool, String> {
        self.path(path)?;
        match vfs_stat(self.port, 0, path.len()) {
            Ok(_) => Ok(true),
            Err(RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)) => Ok(false),
            Err(e) => Err(format!("Cannot inspect file: {}", e)),
        }
    }
    fn read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let (handle, size) = self.open(path, false)?;
        let result = (|| {
            if size > self.max_bytes {
                return Err("File exceeds the supported size".to_string());
            }
            let mut data = Vec::with_capacity(size);
            while data.len() < size {
                let wanted = self.chunk.min(size - data.len());
                let read = vfs_read(self.port, handle, data.len(), wanted, OFFSET)
                    .map_err(|e| format!("Read failed: {}", e))?;
                if read == 0 || read > wanted {
                    return Err("Incomplete file read".to_string());
                }
                data.extend_from_slice(unsafe {
                    core::slice::from_raw_parts((self.base + OFFSET) as *const u8, read)
                });
            }
            Ok(data)
        })();
        let result = self.finish(handle, result);
        if let Ok(data) = &result {
            nanami_runtime::println!("[native-ui] opened path={} bytes={}", path, data.len());
        }
        result
    }
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        if data.len() > self.max_bytes {
            return Err("Image exceeds the supported size".to_string());
        }
        let (handle, _) = self.open(path, true)?;
        let result = (|| {
            let mut offset = 0;
            while offset < data.len() {
                let wanted = self.chunk.min(data.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        data.as_ptr().add(offset),
                        (self.base + OFFSET) as *mut u8,
                        wanted,
                    );
                }
                let written = vfs_write(self.port, handle, offset, wanted, OFFSET)
                    .map_err(|e| format!("Write failed; file may be partial: {}", e))?;
                if written == 0 || written > wanted {
                    return Err("Incomplete write; file may be partial".to_string());
                }
                offset += written;
            }
            vfs_ftruncate(self.port, handle, data.len())
                .map_err(|e| format!("Truncate failed: {}", e))?;
            vfs_fsync(self.port, handle)
                .map_err(|e| format!("Sync failed; image kept in memory: {}", e))?;
            Ok(())
        })();
        let result = self.finish(handle, result);
        if result.is_ok() {
            nanami_runtime::println!("[native-ui] saved path={} bytes={}", path, data.len());
        }
        result
    }
}
