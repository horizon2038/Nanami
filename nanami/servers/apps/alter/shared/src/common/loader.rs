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

struct ImmutableSnapshot {
    address: Word,
    capacity: Word,
    copied: Word,
    readonly: bool,
    source_only: bool,
    metadata: Option<ElfMetadata>,
    version_verified: bool,
}

/// mmap uses a fresh identity from its own FD. Writable cold mappings fill
/// only on a whole-file read. Alter owns the snapshots:
/// guests receive copies, or private readonly aliases of immutable RAM.
pub fn file_mapping_source(
    runtime: &mut Runtime,
    fd: Word,
    identity: Word,
    size: Word,
    offset: Word,
    bytes: Word,
) -> Result<Option<Word>, LoadError> {
    mapping_source(runtime, fd, identity, size, offset, bytes, false)
}

/// A readonly mapping can amortize a larger immutable snapshot through aliases.
/// An almost-whole aligned prefix may include one partial trailing file page;
/// writable cold mappings retain whole-file admission and the ordinary cap.
pub fn readonly_file_mapping_source(
    runtime: &mut Runtime, fd: Word, identity: Word, size: Word, offset: Word, bytes: Word,
) -> Result<Option<Word>, LoadError> {
    mapping_source(runtime, fd, identity, size, offset, bytes, true)
}

fn mapping_source(
    runtime: &mut Runtime, fd: Word, identity: Word, size: Word, offset: Word, bytes: Word,
    readonly_alias: bool,
) -> Result<Option<Word>, LoadError> {
    if offset.checked_add(bytes).is_none_or(|end| end > size) {
        return Err(LoadError::InvalidArgument);
    }
    if let Some(cached) = runtime.image_cache.get(identity, size) {
        #[cfg(target_os = "none")]
        if (size >= 2 * 1024 * 1024 && option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("1"))
            || option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("all") {
            nanami_runtime::println!("[loader.cache] reason=hit identity={:#x} size={} offset={} bytes={}", identity, size, offset, bytes);
        }
        return Ok(Some(cached.address + offset));
    }
    // DSO LOAD spans can end a few bytes before nonloaded file metadata.
    // Admit at most one partial trailing page, only for immutable readonly
    // aliases. Keep offset mappings, small probes and writable fills unchanged.
    let readonly_tail = readonly_alias && super::image_cache::IMMUTABLE_STORAGE
        && bytes >= 4096 && bytes & 4095 == 0
        && size.saturating_sub(bytes) < 4096;
    if identity >> 32 == 0
        || size == 0
        || offset != 0
        || (bytes != size && !readonly_tail)
    {
        return Ok(None);
    }
    let Some(slot) = mapping_cache_slot(runtime, size, readonly_alias, identity)? else {
        return Ok(None);
    };
    // All selected owners have retired before filling. Reuse the vacant slot
    // without repeating either the miss lookup or the replacement selection.
    let file = fill_file_snapshot(runtime, fd, identity, size,
        ImageBuffer::Mapping { readonly_alias }, Some(slot))?;
    #[cfg(target_os = "none")]
    if (size >= 2 * 1024 * 1024 && option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("1"))
        || option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("all") {
        nanami_runtime::println!("[loader.cache] reason=filled identity={:#x} size={} retained={}", identity, size, runtime.image_cache.retained_bytes());
    }
    #[cfg(target_os = "none")]
    if bytes != size && option_env!("NANAMI_READONLY_TAIL_DIAG") == Some("1") {
        nanami_runtime::println!("[loader] readonly-tail-snapshot fd={} size={} prefix={} extra={}",
            fd, size, bytes, size - bytes);
    }
    Ok(Some(file.address))
}

fn mapping_cache_slot(
    runtime: &mut Runtime, size: Word, readonly_alias: bool, identity: Word,
) -> Result<Option<usize>, LoadError> {
    let Some(plan) = runtime.image_cache.mapping_replacement(size, readonly_alias, identity) else {
        return Ok(None);
    };
    #[cfg(target_os = "none")]
    if (size >= 2 * 1024 * 1024 && option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("1"))
        || option_env!("NANAMI_MAPPING_CACHE_DIAG") == Some("all") {
        nanami_runtime::println!("[loader.cache] reason=admit identity={:#x} size={} victims={} retained={}", identity, size, plan.victims.count_ones(), runtime.image_cache.retained_bytes());
    }
    let mut victims = plan.victims;
    while victims != 0 {
        let index = victims.trailing_zeros() as usize;
        victims &= victims - 1;
        let owner = runtime.image_cache.entry(index).copied().ok_or(LoadError::Io)?;
        // Release may unmap a prefix before failing. Stop at the first failure,
        // retaining that exact invalid owner and every unattempted allocation.
        runtime.image_cache.invalidate(index);
        nanami_runtime::request_mapping_release(owner.address, owner.capacity)
            .map_err(|_| LoadError::Io)?;
        runtime.image_cache.remove(index);
    }
    Ok(Some(plan.slot))
}

#[derive(Clone, Copy)]
enum ImageBuffer {
    Reusable,
    Interpreter { main_address: Word },
    Mapping { readonly_alias: bool },
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
    if let Some(cached) = runtime.image_cache.get(identity, size).copied() {
        // mmap constructors only pass this backing identity back to Alpha.
        // Exec/interpreter consumers also read bytes on the native CPU.
        if cached.source_only {
            let owner = image_cache_owner(runtime)?;
            nanami_runtime::request_process_mapping_protect(owner, cached.address, cached.capacity, 1)
                .map_err(|_| LoadError::Io)?;
            runtime.image_cache.mark_source_mapped(cached.address);
        }
        return Ok(FileSnapshot {
            address: cached.address,
            metadata: cached.metadata,
        });
    }
    let protected_address = match buffer {
        ImageBuffer::Reusable | ImageBuffer::Mapping { .. } => None,
        ImageBuffer::Interpreter { main_address } => Some(main_address),
    };
    let cache_slot = if identity >> 32 != 0 {
        if let ImageBuffer::Mapping { readonly_alias } = buffer {
            mapping_cache_slot(runtime, size, readonly_alias, identity)?
        } else {
            runtime.image_cache.replacement(size, protected_address, identity)
        }
    } else {
        None
    };
    fill_file_snapshot(runtime, fd, identity, size, buffer, cache_slot)
}

fn fill_file_snapshot(
    runtime: &mut Runtime,
    fd: Word,
    identity: Word,
    size: Word,
    buffer: ImageBuffer,
    cache_slot: Option<usize>,
) -> Result<FileSnapshot, LoadError> {
    let mut filled = 0;
    let mut sealed = false;
    let mut source_only = false;
    let mut source_metadata = None;
    let mut version_verified = false;
    let (image, allocation_size) = if let Some(slot) = cache_slot {
        if super::image_cache::IMMUTABLE_STORAGE {
            let unmapped = super::image_cache::UNMAPPED_SOURCES
                && matches!(buffer, ImageBuffer::Mapping { readonly_alias: true });
            let snapshot = create_immutable_snapshot(runtime, slot, size, fd, identity, unmapped)?;
            filled = snapshot.copied;
            sealed = snapshot.readonly;
            source_only = snapshot.source_only;
            source_metadata = snapshot.metadata;
            version_verified = snapshot.version_verified;
            (snapshot.address, snapshot.capacity)
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
        // Mapping admission was preflighted without yielding. Do not silently
        // add an uncached whole-file copy if that invariant ever changes.
        if matches!(buffer, ImageBuffer::Mapping { .. }) {
            return Err(LoadError::Io);
        }
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
        read_image(runtime, fd, image, size, filled)?;
        if identity >> 32 != 0 && !version_verified {
            let current =
                posix::posix_fstat_flags(runtime.posix_port, fd, posix::POSIX_STAT_INCLUDE_VERSION)
                    .map_err(|_| LoadError::Io)?;
            if current.0 != identity || current.1 != size {
                return Err(LoadError::Io);
            }
        }
        if source_only {
            // Parsed from the stable read window, never from the unmapped
            // backing identity. None also records a non-ELF file accurately.
            Ok(source_metadata)
        } else {
            let bytes = unsafe { core::slice::from_raw_parts(image as *const u8, size) };
            Ok(parse_elf64_image(bytes).ok())
        }
    })();
    let metadata = result?;
    if let Some(slot) = cache_slot {
        if super::image_cache::IMMUTABLE_STORAGE && !sealed {
            // Publish only after a complete fill/version recheck and readonly
            // protection. A failed/partial protection leaves identity zero
            // and its exact allocation owned, but unusable as a cache hit.
            let owner = image_cache_owner(runtime)?;
            nanami_runtime::request_process_mapping_protect(owner, image, allocation_size, 1)
                .map_err(|_| LoadError::Io)?;
        }
        runtime
            .image_cache
            .insert(slot, identity, image, size, allocation_size, metadata);
        if source_only { runtime.image_cache.mark_source_only(slot); }
    }
    Ok(FileSnapshot {
        address: image,
        metadata,
    })
}

fn image_cache_owner(runtime: &mut Runtime) -> Result<Word, LoadError> {
    if let Some(owner) = runtime.image_cache.owner_pid { return Ok(owner); }
    let owner = nanami_runtime::get_self_pid().map_err(|_| LoadError::Io)?;
    runtime.image_cache.owner_pid = Some(owner);
    Ok(owner)
}

fn create_immutable_snapshot(
    runtime: &mut Runtime,
    slot: usize,
    size: Word,
    fd: Word,
    identity: Word,
    source_only: bool,
) -> Result<ImmutableSnapshot, LoadError> {
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
    // A complete positioned read fits in one stable service window. Build
    // owned readonly RAM directly from it instead of zeroing the whole image,
    // copying in Alter, then changing every leaf's permissions. A short read
    // keeps its prefix and continues through the ordinary owned-RAM path.
    let verified = runtime.pread_versioned_image(fd, size, identity).map_err(|_| LoadError::Io)?;
    let prefix = if let Some(source) = verified {
        Some((size, source))
    } else if size <= runtime.posix_image_read_buffer_size() {
        let (read, source) = runtime.pread_posix(fd, 0, size, 0).map_err(|_| LoadError::Io)?;
        if read == 0 || read > size { return Err(LoadError::Io); }
        Some((read, source))
    } else { None };
    if let Some((read, source)) = prefix.filter(|&(read, _)| read == size) {
        let constructor = if source_only { nanami_runtime::request_memory_create_source }
            else { nanami_runtime::request_memory_create_readonly };
        let address = constructor(capacity, source, read)
            .map_err(|_| LoadError::Io)?;
        // Retain exact ownership before version checking/parsing can fail.
        runtime.image_cache.insert(slot, 0, address, 0, capacity, None);
        let metadata = if source_only {
            runtime.image_cache.mark_source_only(slot);
            let bytes = unsafe { core::slice::from_raw_parts(source as *const u8, read) };
            parse_elf64_image(bytes).ok()
        } else { None };
        return Ok(ImmutableSnapshot { address, capacity, copied: read, readonly: true, source_only,
            metadata, version_verified: verified.is_some() });
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
    let filled = if let Some((read, source)) = prefix {
        copy_image_prefix(address, source, read);
        read
    } else { 0 };
    Ok(ImmutableSnapshot { address, capacity, copied: filled, readonly: false,
        source_only: false, metadata: None, version_verified: false })
}

fn copy_image_prefix(destination: Word, source: Word, bytes: Word) {
    unsafe {
        ::core::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes);
    }
}

fn read_image(runtime: &mut Runtime, fd: Word, image: Word, size: Word, mut copied: Word) -> Result<(), LoadError> {
    if copied == size { return Ok(()); }
    // The FD is already open and the caller owns its path bytes separately.
    // No pathname needs to survive in the read window during a snapshot fill.
    let chunk_limit = runtime.posix_image_read_buffer_size();
    if chunk_limit == 0 {
        return Err(LoadError::InvalidArgument);
    }

    while copied < size {
        let chunk = (size - copied).min(chunk_limit);
        // Positioned reads make direct-buffer fallback idempotent: a failed
        // transport cannot silently advance the descriptor's sequential offset.
        let (read, source) = match runtime.pread_posix(fd, 0, chunk, copied) {
            Ok((read, source)) if read != 0 && read <= chunk => (read, source),
            _ => {
                return Err(LoadError::Io);
            }
        };
        copy_image_prefix(image + copied, source, read);
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
