use libnanami::{RequestError, Word};
use nanami_services::posix;

use crate::abi::{ALTER_IO_OFFSET, ALTER_PATH_MAX};
use crate::elf::{parse_elf64_header, ElfError, ElfMetadata};
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

#[derive(Clone, Copy)]
enum ImageBuffer {
    Reusable,
    Cached,
}

pub fn load_linux_elf_image(
    runtime: &mut Runtime,
    path_offset: Word,
    path_len: Word,
) -> Result<LoadedElfImage, LoadError> {
    let (path, path_len) = read_client_path(runtime, path_offset, path_len)?;
    load_elf_image(runtime, &path[..path_len], ImageBuffer::Reusable)
}

pub fn load_cached_fork_linux_elf_image(
    runtime: &mut Runtime,
    path_offset: Word,
    path_len: Word,
) -> Result<LoadedElfImage, LoadError> {
    let (path, path_len) = read_client_path(runtime, path_offset, path_len)?;
    let path = &path[..path_len];
    if let Some(cached) = runtime.cached_fork_image(path) {
        return Ok(LoadedElfImage {
            address: cached.address,
            size: cached.size,
            metadata: cached.metadata,
        });
    }

    let buffer = if runtime.has_fork_image_cache_slot() {
        ImageBuffer::Cached
    } else {
        ImageBuffer::Reusable
    };
    let loaded = load_elf_image(runtime, path, buffer)?;
    if matches!(buffer, ImageBuffer::Cached)
        && !runtime.cache_fork_image(path, loaded.address, loaded.size, loaded.metadata)
    {
        let _ = libnanami::request_mapping_release(loaded.address, loaded.size);
        return Err(LoadError::Io);
    }
    Ok(loaded)
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
    let stat = posix::posix_stat(runtime.posix_port, 0, path.len() as Word)
        .map_err(map_load_request_error)?;
    let size = stat.1;
    if size < 64 {
        return Err(LoadError::InvalidElf);
    }

    let (image, allocation_size) = match buffer {
        ImageBuffer::Reusable => {
            if runtime.exec_image_buffer == 0 || runtime.exec_image_buffer_size < size {
                let (image, image_size) =
                    libnanami::request_heap(size).map_err(|_| LoadError::Io)?;
                if image_size < size {
                    let _ = libnanami::request_mapping_release(image, image_size);
                    return Err(LoadError::Io);
                }
                runtime.exec_image_buffer = image;
                runtime.exec_image_buffer_size = image_size;
            }
            (runtime.exec_image_buffer, runtime.exec_image_buffer_size)
        }
        ImageBuffer::Cached => {
            let (image, image_size) = libnanami::request_heap(size).map_err(|_| LoadError::Io)?;
            if image_size < size {
                let _ = libnanami::request_mapping_release(image, image_size);
                return Err(LoadError::Io);
            }
            (image, image_size)
        }
    };

    let result = read_and_parse_elf(runtime, path, image, size);
    if result.is_err() && matches!(buffer, ImageBuffer::Cached) {
        let _ = libnanami::request_mapping_release(image, allocation_size);
    }
    let metadata = result?;
    Ok(LoadedElfImage {
        address: image,
        size,
        metadata,
    })
}

fn read_and_parse_elf(
    runtime: &mut Runtime,
    path: &[u8],
    image: Word,
    size: Word,
) -> Result<ElfMetadata, LoadError> {
    copy_path_to_posix_shm(runtime, path);
    let fd = posix::posix_open(runtime.posix_port, 0, path.len() as Word, 0)
        .map_err(map_load_request_error)?;
    let chunk_limit = runtime
        .posix_read_buffer_size()
        .saturating_sub(ALTER_IO_OFFSET as Word);
    if chunk_limit == 0 {
        let _ = posix::posix_close(runtime.posix_port, fd);
        return Err(LoadError::InvalidArgument);
    }

    let mut copied = 0;
    while copied < size {
        let chunk = (size - copied).min(chunk_limit);
        let (read, source) = match runtime.read_posix(fd, ALTER_IO_OFFSET as Word, chunk) {
            Ok((read, source)) if read != 0 && read <= chunk => (read, source),
            _ => {
                let _ = posix::posix_close(runtime.posix_port, fd);
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
    posix::posix_close(runtime.posix_port, fd).map_err(|_| LoadError::Io)?;
    let image = unsafe { ::core::slice::from_raw_parts(image as *const u8, size as usize) };
    parse_elf64_header(image).map_err(map_elf_error)
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
        RequestError::Status(libnanami::OS_RESPONSE_INVALID_ARGUMENT)
        | RequestError::Status(libnanami::OS_RESPONSE_INVALID_DESCRIPTOR) => LoadError::NotFound,
        _ => LoadError::Io,
    }
}

pub fn map_load_error_to_status(error: LoadError) -> Word {
    match error {
        LoadError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::NotFound => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::Io => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        LoadError::InvalidElf => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        LoadError::UnsupportedElf => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
    }
}

pub fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport => libnanami::OS_RESPONSE_FATAL,
        RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}
