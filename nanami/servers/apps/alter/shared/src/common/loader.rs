use nanami_client::posix;
use nanami_runtime::{RequestError, Word};

use crate::abi::{ALTER_IO_OFFSET, ALTER_PATH_MAX};
use crate::elf::{parse_elf64_header, parse_elf64_image, ElfError, ElfMetadata};
use crate::state::Runtime;

#[derive(Clone, Copy)]
pub enum LoadError {
    InvalidArgument,
    NotFound,
    Io,
    InvalidElf,
    UnsupportedElf,
}

pub struct LoadedElfImage {
    pub address: Word,
    pub size: Word,
    pub metadata: ElfMetadata,
}

struct FileSnapshot {
    address: Word,
    metadata: Option<ElfMetadata>,
}

/// mmap uses a fresh identity from its own FD. Only fill on a whole-file read,
/// never prefetch a large file for a small mapping. Alter owns the snapshots:
/// guests receive copies, or private readonly aliases of opt-in immutable RAM.
pub fn file_mapping_source(
    runtime: &mut Runtime,
    fd: Word,
    identity: Word,
    size: Word,
    offset: Word,
    bytes: Word,
) -> Result<Option<Word>, LoadError> {
    if offset.checked_add(bytes).is_none_or(|end| end > size) {
        return Err(LoadError::InvalidArgument);
    }
    if let Some(cached) = runtime.image_cache.get(identity, size) {
        return Ok(Some(cached.address + offset));
    }
    if identity >> 32 == 0
        || size == 0
        || size > super::image_cache::MAX_IMAGE_BYTES
        || offset != 0
        || bytes != size
    {
        return Ok(None);
    }
    let file = load_file_snapshot(runtime, fd, identity, size, ImageBuffer::Reusable)?;
    Ok(Some(file.address))
}

#[derive(Clone, Copy)]
enum ImageBuffer {
    Reusable,
    Interpreter { main_address: Word },
}

pub fn load_interpreter_elf_image(
    runtime: &mut Runtime,
    path: &[u8],
    main_address: Word,
) -> Result<LoadedElfImage, LoadError> {
    load_elf_image(runtime, path, ImageBuffer::Interpreter { main_address })
}

pub fn load_linux_elf_image(
    runtime: &mut Runtime,
    path_offset: Word,
    path_len: Word,
) -> Result<LoadedElfImage, LoadError> {
    let (path, path_len) = read_client_path(runtime, path_offset, path_len)?;
    load_elf_image(runtime, &path[..path_len], ImageBuffer::Reusable)
}

pub fn validate_linux_elf(
    runtime: &mut Runtime,
    path_offset: Word,
    path_len: Word,
) -> Result<ElfMetadata, LoadError> {
    let (path, path_len) = read_client_path(runtime, path_offset, path_len)?;
    let path = &path[..path_len];
    if runtime.posix_shm_size < path.len() as Word
        || runtime.posix_read_buffer_size() < (ALTER_IO_OFFSET + 1024) as Word
    {
        return Err(LoadError::InvalidArgument);
    }

    copy_path_to_posix_shm(runtime, path);
    let fd = posix::posix_open(runtime.posix_port, 0, path.len() as Word, 0)
        .map_err(map_load_request_error)?;
    let (read, source) = match runtime.read_posix(fd, ALTER_IO_OFFSET as Word, 1024) {
        Ok(result) => result,
        Err(_) => {
            let _ = posix::posix_close(runtime.posix_port, fd);
            return Err(LoadError::Io);
        }
    };
    posix::posix_close(runtime.posix_port, fd).map_err(|_| LoadError::Io)?;
    if read < 64 {
        return Err(LoadError::InvalidElf);
    }
    let header = unsafe { ::core::slice::from_raw_parts(source as *const u8, read as usize) };
    parse_elf64_header(header).map_err(map_elf_error)
}

fn load_elf_image(
    runtime: &mut Runtime,
    path: &[u8],
    buffer: ImageBuffer,
) -> Result<LoadedElfImage, LoadError> {
    if runtime.posix_shm_size < path.len() as Word
        || runtime.posix_read_buffer_size() <= ALTER_IO_OFFSET as Word
    {
        return Err(LoadError::InvalidArgument);
    }

    copy_path_to_posix_shm(runtime, path);
    // Resolve once. Size and content identity belong to this open file, even
    // if another client renames/replaces the pathname during loading.
    let fd = posix::posix_open(runtime.posix_port, 0, path.len() as Word, 0)
        .map_err(map_load_request_error)?;
    let result = (|| {
        let (identity, size, kind, _, _) =
            posix::posix_fstat_flags(runtime.posix_port, fd, posix::POSIX_STAT_INCLUDE_VERSION)
                .map_err(|_| LoadError::Io)?;
        if kind != posix::POSIX_FILE_TYPE_REGULAR {
            return Err(LoadError::InvalidElf);
        }
        load_opened_image(runtime, fd, identity, size, buffer)
    })();
    let closed = posix::posix_close(runtime.posix_port, fd);
    let loaded = result?;
    closed.map_err(|_| LoadError::Io)?;
    Ok(loaded)
}

fn load_opened_image(
    runtime: &mut Runtime,
    fd: Word,
    identity: Word,
    size: Word,
    buffer: ImageBuffer,
) -> Result<LoadedElfImage, LoadError> {
    if size < 64 {
        return Err(LoadError::InvalidElf);
    }
    let file = load_file_snapshot(runtime, fd, identity, size, buffer)?;
    let metadata = match file.metadata {
        Some(metadata) => metadata,
        None => {
            let bytes = unsafe { core::slice::from_raw_parts(file.address as *const u8, size) };
            return parse_elf64_image(bytes)
                .map(|metadata| LoadedElfImage {
                    address: file.address,
                    size,
                    metadata,
                })
                .map_err(map_elf_error);
        }
    };
    Ok(LoadedElfImage {
        address: file.address,
        size,
        metadata,
    })
}

fn load_file_snapshot(
    runtime: &mut Runtime,
    fd: Word,
    identity: Word,
    size: Word,
    buffer: ImageBuffer,
) -> Result<FileSnapshot, LoadError> {
    if let Some(cached) = runtime.image_cache.get(identity, size) {
        return Ok(FileSnapshot {
            address: cached.address,
            metadata: cached.metadata,
        });
    }
    let protected_address = match buffer {
        ImageBuffer::Reusable => None,
        ImageBuffer::Interpreter { main_address } => Some(main_address),
    };
    let cache_slot = if identity >> 32 != 0 {
        runtime.image_cache.replacement(size, protected_address)
    } else {
        None
    };
    let (image, allocation_size) = if let Some(slot) = cache_slot {
        if super::image_cache::IMMUTABLE_STORAGE {
            create_immutable_snapshot(runtime, slot, size)?
        } else {
            let previous = runtime.image_cache.entry(slot);
            let allocation = match previous.filter(|entry| entry.capacity >= size) {
                Some(entry) => (entry.address, entry.capacity),
                None => {
                    // Alpha may defer physical-page reuse after mapping release.
                    // Reuse each cache buffer in place, and grow geometrically so
                    // eviction and failed reads cannot accumulate unbounded RAM.
                    let capacity = runtime.image_cache.allocation_capacity(slot, size);
                    let allocation =
                        nanami_runtime::request_heap(capacity).map_err(|_| LoadError::Io)?;
                    if allocation.1 < size {
                        let _ = nanami_runtime::request_mapping_release(allocation.0, allocation.1);
                        return Err(LoadError::Io);
                    }
                    if let Some(entry) = previous {
                        if nanami_runtime::request_mapping_release(entry.address, entry.capacity)
                            .is_err()
                        {
                            let _ =
                                nanami_runtime::request_mapping_release(allocation.0, allocation.1);
                            return Err(LoadError::Io);
                        }
                    }
                    allocation
                }
            };
            // Retain allocation ownership on I/O/parse failure without allowing
            // the partially overwritten buffer to be used as a content hit.
            runtime
                .image_cache
                .insert(slot, 0, allocation.0, 0, allocation.1, None);
            allocation
        }
    } else {
        let (address, capacity) = if matches!(buffer, ImageBuffer::Interpreter { .. }) {
            (
                &mut runtime.interpreter_image_buffer,
                &mut runtime.interpreter_image_buffer_size,
            )
        } else {
            (
                &mut runtime.exec_image_buffer,
                &mut runtime.exec_image_buffer_size,
            )
        };
        if *address == 0 || *capacity < size {
            let (image, image_size) =
                nanami_runtime::request_heap(size).map_err(|_| LoadError::Io)?;
            if image_size < size {
                let _ = nanami_runtime::request_mapping_release(image, image_size);
                return Err(LoadError::Io);
            }
            if *address != 0 {
                if nanami_runtime::request_mapping_release(*address, *capacity).is_err() {
                    let _ = nanami_runtime::request_mapping_release(image, image_size);
                    return Err(LoadError::Io);
                }
            }
            *address = image;
            *capacity = image_size;
        }
        (*address, *capacity)
    };
    let result = (|| {
        read_image(runtime, fd, image, size)?;
        if identity >> 32 != 0 {
            let current =
                posix::posix_fstat_flags(runtime.posix_port, fd, posix::POSIX_STAT_INCLUDE_VERSION)
                    .map_err(|_| LoadError::Io)?;
            if current.0 != identity || current.1 != size {
                return Err(LoadError::Io);
            }
        }
        let bytes = unsafe { core::slice::from_raw_parts(image as *const u8, size) };
        Ok(parse_elf64_image(bytes).ok())
    })();
    let metadata = result?;
    if let Some(slot) = cache_slot {
        if super::image_cache::IMMUTABLE_STORAGE {
            // Publish only after a complete fill/version recheck and readonly
            // protection. A failed/partial protection leaves identity zero
            // and its exact allocation owned, but unusable as a cache hit.
            let owner = nanami_runtime::get_self_pid().map_err(|_| LoadError::Io)?;
            nanami_runtime::request_process_mapping_protect(owner, image, allocation_size, 1)
                .map_err(|_| LoadError::Io)?;
        }
        runtime
            .image_cache
            .insert(slot, identity, image, size, allocation_size, metadata);
    }
    Ok(FileSnapshot {
        address: image,
        metadata,
    })
}

fn create_immutable_snapshot(
    runtime: &mut Runtime,
    slot: usize,
    size: Word,
) -> Result<(Word, Word), LoadError> {
    let capacity = runtime.image_cache.allocation_capacity(slot, size);
    if let Some(previous) = runtime.image_cache.entry(slot).copied() {
        // Release can fail after unmapping a prefix. Invalidate before that
        // call, retain the exact owner record on failure, and retry retirement
        // before allocating/refilling anything. PT_INTERP's live main entry
        // was excluded by replacement(), so it cannot be retired here.
        runtime.image_cache.invalidate(slot);
        nanami_runtime::request_mapping_release(previous.address, previous.capacity)
            .map_err(|_| LoadError::Io)?;
        runtime.image_cache.remove(slot);
    }
    // Unlike request_heap, this RAM object's release drops our physical
    // reference immediately. Future readonly aliases must hold independent
    // references; they may outlive this cache entry without permitting refill
    // of their backing. No guest alias is introduced by this constructor.
    let address = nanami_runtime::request_memory_create(capacity).map_err(|_| LoadError::Io)?;
    // request_memory_create returns zeroed, page-aligned storage. In particular
    // the partial final file page cannot expose an earlier snapshot's suffix.
    runtime
        .image_cache
        .insert(slot, 0, address, 0, capacity, None);
    Ok((address, capacity))
}

fn read_image(runtime: &mut Runtime, fd: Word, image: Word, size: Word) -> Result<(), LoadError> {
    let chunk_limit = runtime
        .posix_read_buffer_size()
        .saturating_sub(ALTER_IO_OFFSET as Word);
    if chunk_limit == 0 {
        return Err(LoadError::InvalidArgument);
    }

    let mut copied = 0;
    while copied < size {
        let chunk = (size - copied).min(chunk_limit);
        // Positioned reads make direct-buffer fallback idempotent: a failed
        // transport cannot silently advance the descriptor's sequential offset.
        let (read, source) = match runtime.pread_posix(fd, ALTER_IO_OFFSET as Word, chunk, copied) {
            Ok((read, source)) if read != 0 && read <= chunk => (read, source),
            _ => {
                return Err(LoadError::Io);
            }
        };
        unsafe {
            ::core::ptr::copy_nonoverlapping(
                source as *const u8,
                (image + copied) as *mut u8,
                read as usize,
            );
        }
        copied += read;
    }
    Ok(())
}

fn read_client_path(
    runtime: &Runtime,
    path_offset: Word,
    path_len: Word,
) -> Result<([u8; ALTER_PATH_MAX], usize), LoadError> {
    if path_len == 0 || path_len as usize > ALTER_PATH_MAX {
        return Err(LoadError::InvalidArgument);
    }
    if path_offset
        .checked_add(path_len)
        .filter(|end| *end <= runtime.client_shm_size)
        .is_none()
    {
        return Err(LoadError::InvalidArgument);
    }
    let path_len = path_len as usize;
    let mut path = [0; ALTER_PATH_MAX];
    unsafe {
        ::core::ptr::copy_nonoverlapping(
            (runtime.client_shm + path_offset) as *const u8,
            path.as_mut_ptr(),
            path_len,
        );
    }
    Ok((path, path_len))
}

fn copy_path_to_posix_shm(runtime: &Runtime, path: &[u8]) {
    unsafe {
        ::core::ptr::copy_nonoverlapping(path.as_ptr(), runtime.posix_shm as *mut u8, path.len());
    }
}

fn map_elf_error(error: ElfError) -> LoadError {
    match error {
        ElfError::Invalid => LoadError::InvalidElf,
        ElfError::Unsupported => LoadError::UnsupportedElf,
    }
}

fn map_load_request_error(error: RequestError) -> LoadError {
    match error {
        RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT)
        | RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR) => {
            LoadError::NotFound
        }
        _ => LoadError::Io,
    }
}

pub fn map_load_error_to_status(error: LoadError) -> Word {
    match error {
        LoadError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::NotFound => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::Io => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        LoadError::InvalidElf => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::UnsupportedElf => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
    }
}

pub fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}
