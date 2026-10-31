//! Streaming initialization for private file mmap, including short reads.
use super::{map_request_error, LinuxFile, Runtime, Word, EIO, LINUX_PAGE_SIZE};

pub(super) fn initialize(
    runtime: &mut Runtime,
    pid: Word,
    base: Word,
    mapped: Word,
    file: LinuxFile,
    offset: Word,
    bytes: Word,
) -> Result<(), i32> {
    // Full pages go directly from the POSIX read window to new backing.
    // Only a short read's partial page needs bounded local buffering.
    let mut tail = [0u8; LINUX_PAGE_SIZE as usize];
    let mut buffered = 0usize;
    let mut initialized = 0;
    let mut copied = 0;
    while copied < bytes {
        let chunk = (bytes - copied).min(runtime.posix_read_buffer_size());
        if chunk == 0 {
            return Err(EIO);
        }
        let (read, source) = runtime
            .pread_posix(file.posix_fd, 0, chunk, offset + copied)
            .map_err(map_request_error)?;
        if read == 0 {
            break;
        }
        if read > chunk {
            return Err(EIO);
        }
        let mut consumed = 0;
        if buffered != 0 {
            let chunk = (read as usize).min(tail.len() - buffered);
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    source as *const u8,
                    tail.as_mut_ptr().add(buffered),
                    chunk,
                );
            }
            buffered += chunk;
            consumed += chunk as Word;
            if buffered == tail.len() {
                nanami_runtime::request_process_map_initialized_at(
                    pid,
                    base + initialized,
                    LINUX_PAGE_SIZE,
                    tail.as_ptr() as Word,
                    LINUX_PAGE_SIZE,
                )
                .map_err(map_request_error)?;
                initialized += LINUX_PAGE_SIZE;
                buffered = 0;
            }
        }
        let full = (read - consumed) & !(LINUX_PAGE_SIZE - 1);
        if full != 0 {
            nanami_runtime::request_process_map_initialized_at(
                pid,
                base + initialized,
                full,
                source + consumed,
                full,
            )
            .map_err(map_request_error)?;
            initialized += full;
            consumed += full;
        }
        if consumed < read {
            let remaining = (read - consumed) as usize;
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    (source + consumed) as *const u8,
                    tail.as_mut_ptr().add(buffered),
                    remaining,
                );
            }
            buffered += remaining;
        }
        copied += read;
    }
    if buffered != 0 {
        nanami_runtime::request_process_map_initialized_at(
            pid,
            base + initialized,
            LINUX_PAGE_SIZE,
            tail.as_ptr() as Word,
            buffered as Word,
        )
        .map_err(map_request_error)?;
        initialized += LINUX_PAGE_SIZE;
    }
    if initialized < mapped {
        nanami_runtime::request_process_map_initialized_at(
            pid,
            base + initialized,
            mapped - initialized,
            0,
            0,
        )
        .map_err(map_request_error)?;
    }
    Ok(())
}
