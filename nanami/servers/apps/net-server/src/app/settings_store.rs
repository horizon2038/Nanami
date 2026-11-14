use super::*;
use nanami_client::vfs::*;
use settings_record::{encode, latest, FILE_BYTES, RECORD_BYTES};
const PATH: &[u8] = b"/nanami/network.settings";
const DATA: usize = 512;
pub struct Store {
    port: Word,
    base: Word,
}
impl Store {
    pub fn connect() -> Result<Self, RequestError> {
        nanami_client::registry::connect_vfs_service(24)?;
        let port = nanami_runtime::ipc::process_slot_descriptor(24);
        let (base, bytes) = vfs_attach_shared_memory(port, 4096)?;
        if base == 0 || bytes < DATA + FILE_BYTES {
            return Err(RequestError::Protocol);
        }
        Ok(Self { port, base })
    }
    fn open(&self, create: bool) -> Result<(Word, usize), RequestError> {
        unsafe {
            ptr::copy_nonoverlapping(PATH.as_ptr(), self.base as *mut u8, PATH.len());
        }
        let (handle, _, size, kind) = vfs_open_compound(
            self.port,
            0,
            PATH.len(),
            if create { VFS_OPEN_CREATE } else { 0 },
        )?;
        if kind != VFS_FILE_TYPE_REGULAR || size > FILE_BYTES {
            let _ = vfs_close(self.port, handle);
            return Err(RequestError::Protocol);
        }
        Ok((handle, size))
    }
    pub fn load(&self) -> Result<(nanami_client::net::Ipv4Settings, Word), RequestError> {
        let (handle, size) = match self.open(false) {
            Ok(value) => value,
            Err(RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR)) => {
                return Ok((nanami_client::net::Ipv4Settings::DEFAULT, 0))
            }
            Err(error) => return Err(error),
        };
        let result = (|| {
            let mut offset = 0;
            while offset < size {
                let count = vfs_read(self.port, handle, offset, size - offset, DATA + offset)?;
                if count == 0 || count > size - offset {
                    return Err(RequestError::Protocol);
                }
                offset += count;
            }
            let bytes =
                unsafe { core::slice::from_raw_parts((self.base + DATA) as *const u8, size) };
            latest(bytes).ok_or(RequestError::Protocol)
        })();
        let closed = vfs_close(self.port, handle);
        let value = result?;
        closed?;
        Ok(value)
    }
    pub fn save(
        &self,
        settings: nanami_client::net::Ipv4Settings,
        revision: Word,
    ) -> Result<(), RequestError> {
        let (handle, _) = self.open(true)?;
        let record = encode(settings, revision);
        unsafe {
            ptr::copy_nonoverlapping(record.as_ptr(), (self.base + DATA) as *mut u8, RECORD_BYTES);
        }
        let result = (|| {
            let offset = (revision & 1) * RECORD_BYTES;
            let written = vfs_write(self.port, handle, offset, RECORD_BYTES, DATA)?;
            if written != RECORD_BYTES {
                return Err(RequestError::Transport);
            }
            vfs_fsync(self.port, handle)?;
            let read = vfs_read(self.port, handle, offset, RECORD_BYTES, DATA)?;
            if read != RECORD_BYTES {
                return Err(RequestError::Protocol);
            }
            let bytes = unsafe {
                core::slice::from_raw_parts((self.base + DATA) as *const u8, read.min(RECORD_BYTES))
            };
            if bytes != record {
                return Err(RequestError::Protocol);
            }
            Ok(())
        })();
        let closed = vfs_close(self.port, handle);
        result?;
        closed
    }
}
