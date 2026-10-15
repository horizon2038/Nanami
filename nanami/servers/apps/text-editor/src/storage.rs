use crate::{app::Files, document::MAX_BYTES};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use nanami_client::vfs::*;
use nanami_runtime::{RequestError, Word};

const DATA_OFFSET: usize = 512;

pub struct Storage {
    port: Word,
    base: usize,
    chunk: usize,
}

impl Storage {
    pub fn connect(slot: Word) -> Result<Self, RequestError> {
        nanami_client::registry::connect_vfs_service(slot)?;
        let port = nanami_runtime::ipc::process_slot_descriptor(slot);
        let (base, bytes) = vfs_attach_shared_memory(port, 0x4000)?;
        if base == 0 || bytes as usize <= DATA_OFFSET {
            return Err(RequestError::Protocol);
        }
        Ok(Self {
            port,
            base: base as usize,
            chunk: bytes as usize - DATA_OFFSET,
        })
    }

    fn path(&mut self, path: &str) -> Result<(), String> {
        if path.is_empty() || path.len() > 255 || path.as_bytes().contains(&0) {
            return Err("Invalid file path".to_string());
        }
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), self.base as *mut u8, path.len());
        }
        Ok(())
    }

    fn open(&mut self, path: &str, create: bool) -> Result<(Word, usize), String> {
        self.path(path)?;
        let flags = if create { VFS_OPEN_CREATE } else { 0 };
        let (handle, _, size, kind) = vfs_open_compound(self.port, 0, path.len() as Word, flags)
            .map_err(|e| format!("Cannot open file: {:?}", e))?;
        if kind != VFS_FILE_TYPE_REGULAR {
            let _ = vfs_close(self.port, handle);
            return Err("This path is not a regular file".to_string());
        }
        Ok((handle, size as usize))
    }

    fn close<T>(&mut self, handle: Word, result: Result<T, String>) -> Result<T, String> {
        // Always release handles, including every read/write/error path.
        let closed = vfs_close(self.port, handle);
        match result {
            Err(error) => Err(error),
            Ok(value) => {
                closed.map_err(|e| format!("Cannot close file: {:?}", e))?;
                Ok(value)
            }
        }
    }
}

impl Files for Storage {
    fn exists(&mut self, path: &str) -> Result<bool, String> {
        self.path(path)?;
        match vfs_stat(self.port, 0, path.len() as Word) {
            Ok(_) => Ok(true),
            Err(RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)) => Ok(false),
            Err(error) => Err(format!("Cannot inspect destination: {:?}", error)),
        }
    }

    fn read(&mut self, path: &str) -> Result<String, String> {
        let (handle, size) = self.open(path, false)?;
        let result = (|| {
            if size > MAX_BYTES {
                return Err("File exceeds the 256 KiB editor limit".to_string());
            }
            let mut bytes = Vec::with_capacity(size);
            while bytes.len() < size {
                let wanted = self.chunk.min(size - bytes.len());
                let count = vfs_read(
                    self.port,
                    handle,
                    bytes.len() as Word,
                    wanted as Word,
                    DATA_OFFSET as Word,
                )
                .map_err(|e| format!("Read failed: {:?}", e))? as usize;
                if count == 0 || count > wanted {
                    return Err("File changed or returned a short read".to_string());
                }
                let data = unsafe {
                    core::slice::from_raw_parts((self.base + DATA_OFFSET) as *const u8, count)
                };
                bytes.extend_from_slice(data);
            }
            let text =
                String::from_utf8(bytes).map_err(|_| "File is not valid UTF-8".to_string())?;
            if text.contains('\0') {
                return Err("Binary files cannot be edited".to_string());
            }
            Ok(text)
        })();
        let result = self.close(handle, result);
        if let Ok(text) = &result {
            nanami_runtime::println!("[text-editor] opened path={} bytes={}", path, text.len());
        }
        result
    }

    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        let (handle, _) = self.open(path, true)?;
        let result = (|| {
            let mut offset = 0;
            while offset < text.len() {
                let wanted = self.chunk.min(text.len() - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        text.as_ptr().add(offset),
                        (self.base + DATA_OFFSET) as *mut u8,
                        wanted,
                    );
                }
                let count = vfs_write(
                    self.port,
                    handle,
                    offset as Word,
                    wanted as Word,
                    DATA_OFFSET as Word,
                )
                .map_err(|e| format!("Write failed (file may be partial): {:?}", e))?
                    as usize;
                if count == 0 || count > wanted {
                    return Err("Write incomplete; the file may be partial".to_string());
                }
                offset += count;
            }
            vfs_ftruncate(self.port, handle, text.len() as Word)
                .map_err(|e| format!("Truncate failed: {:?}", e))?;
            vfs_fsync(self.port, handle)
                .map_err(|e| format!("Disk sync failed; text kept in editor: {:?}", e))?;
            Ok(())
        })();
        let result = self.close(handle, result);
        if result.is_ok() {
            nanami_runtime::println!("[text-editor] saved path={} bytes={}", path, text.len());
        }
        result
    }
}
