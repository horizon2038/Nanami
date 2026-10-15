use core::ptr;
use nanami_runtime::diagnostics::{measure, RpcStage};
use nanami_runtime::{RequestError, Word};

use super::{log_spawn_stage, VfsClient, FILE_CHUNK_OFFSET, PATH_OFFSET};

/// Scratch space shared by the serialized boot and exec requests.
/// Alpha copies the source image before replying, including suspended spawns.
pub(super) struct ImageBuffer {
    base: Word,
    capacity: Word,
}

impl ImageBuffer {
    pub(super) const fn new() -> Self {
        Self {
            base: 0,
            capacity: 0,
        }
    }

    fn reserve(&mut self, size: Word) -> Result<Word, RequestError> {
        if size <= self.capacity {
            return Ok(self.base);
        }
        let (base, capacity) = measure(RpcStage::LoadHeap, size, || {
            nanami_runtime::request_heap(size)
        })
        .map_err(|error| log_spawn_stage("heap", error))?;
        if base == 0 || capacity < size || base.checked_add(capacity).is_none() {
            return Err(RequestError::Protocol);
        }
        self.base = base;
        self.capacity = capacity;
        Ok(base)
    }
}

// Both spawn paths must close the VFS handle on read failures and reject bad
// lengths before copying out of the shared transfer buffer.
pub(super) fn load<const BOOT_TRACE: bool>(
    vfs: &mut VfsClient,
    path_len: usize,
) -> Result<(Word, Word), RequestError> {
    if BOOT_TRACE {
        nanami_runtime::println!("[system-manager] load stage=stat");
    }
    let (_, size, kind) = measure(RpcStage::LoadStat, 0, || {
        nanami_client::vfs::vfs_stat(vfs.port, PATH_OFFSET as Word, path_len as Word)
    })
    .map_err(|error| log_spawn_stage("stat", error))?;
    if kind != nanami_client::vfs::VFS_FILE_TYPE_REGULAR || size == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if BOOT_TRACE {
        nanami_runtime::println!("[system-manager] load stage=heap bytes={:#x}", size);
    }
    let image_base = vfs.image.reserve(size)?;

    if BOOT_TRACE {
        nanami_runtime::println!("[system-manager] load stage=open");
    }
    let handle = measure(RpcStage::LoadOpen, 0, || {
        nanami_client::vfs::vfs_open(vfs.port, PATH_OFFSET as Word, path_len as Word)
    })
    .map_err(|error| log_spawn_stage("open", error))?;
    let result = read_image::<BOOT_TRACE>(vfs, handle, image_base, size);
    if BOOT_TRACE {
        nanami_runtime::println!("[system-manager] load stage=close");
    }
    let _ = measure(RpcStage::LoadClose, 0, || {
        nanami_client::vfs::vfs_close(vfs.port, handle)
    });
    result?;
    Ok((image_base, size))
}

fn read_image<const BOOT_TRACE: bool>(
    vfs: &VfsClient,
    handle: Word,
    image_base: Word,
    size: Word,
) -> Result<(), RequestError> {
    let chunk_capacity = (vfs.shm_size as usize)
        .checked_sub(FILE_CHUNK_OFFSET)
        .filter(|&bytes| bytes != 0)
        .ok_or(RequestError::Protocol)?;
    let mut offset = 0usize;
    while offset < size as usize {
        let chunk = (size as usize - offset).min(chunk_capacity);
        if BOOT_TRACE {
            nanami_runtime::println!(
                "[system-manager] load stage=read offset={:#x} bytes={:#x}",
                offset,
                chunk
            );
        }
        let read = measure(RpcStage::LoadRead, chunk, || {
            nanami_client::vfs::vfs_read(
                vfs.port,
                handle,
                offset as Word,
                chunk as Word,
                FILE_CHUNK_OFFSET as Word,
            )
        })
        .map_err(|error| log_spawn_stage("read", error))? as usize;
        if read == 0 || read > chunk {
            return Err(RequestError::Protocol);
        }
        let copy_before = nanami_runtime::diagnostics::stamp();
        unsafe {
            ptr::copy_nonoverlapping(
                (vfs.shm as usize + FILE_CHUNK_OFFSET) as *const u8,
                (image_base as usize + offset) as *mut u8,
                read,
            );
        }
        nanami_runtime::diagnostics::rpc(
            RpcStage::LoadCopy,
            copy_before,
            nanami_runtime::diagnostics::stamp(),
            read,
            false,
        );
        offset += read;
    }
    Ok(())
}
