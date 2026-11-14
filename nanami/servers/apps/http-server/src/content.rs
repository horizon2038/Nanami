use alloc::{boxed::Box, vec::Vec};
use nanami_client::vfs::*;
use nanami_runtime::{ipc, RequestError};
pub const MAX_CONTENT: usize = 4 * 1024 * 1024;
pub fn load(index: &str, css: &str) -> Result<(&'static [u8], &'static [u8]), RequestError> {
    nanami_client::registry::connect_vfs_service(24)?;
    let port = ipc::process_slot_descriptor(24);
    let (base, bytes) = vfs_attach_shared_memory(port, 0x10000)?;
    if base == 0 || bytes <= 512 {
        return Err(RequestError::Protocol);
    }
    let read = |path: &str, limit: usize| -> Result<Vec<u8>, RequestError> {
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), base as *mut u8, path.len());
        }
        let (handle, _, size, kind) = vfs_open_compound(port, 0, path.len(), 0)?;
        let result = (|| {
            if kind != VFS_FILE_TYPE_REGULAR || size > limit {
                return Err(RequestError::InvalidArgument);
            }
            let mut data = Vec::new();
            data.try_reserve_exact(size)
                .map_err(|_| RequestError::Transport)?;
            while data.len() < size {
                let wanted = (bytes - 512).min(size - data.len());
                let count = vfs_read(port, handle, data.len(), wanted, 512)?;
                if count == 0 || count > wanted {
                    return Err(RequestError::Protocol);
                }
                data.extend_from_slice(unsafe {
                    core::slice::from_raw_parts((base + 512) as *const u8, count)
                });
            }
            Ok(data)
        })();
        let closed = vfs_close(port, handle);
        let data = result?;
        closed?;
        Ok(data)
    };
    let index = read(index, MAX_CONTENT)?;
    let css = read(css, MAX_CONTENT - index.len())?;
    // Immutable startup cache, retained for the server's entire lifetime.
    // No per-request filesystem access or allocation; restart to reload files.
    Ok((
        Box::leak(index.into_boxed_slice()),
        Box::leak(css.into_boxed_slice()),
    ))
}
