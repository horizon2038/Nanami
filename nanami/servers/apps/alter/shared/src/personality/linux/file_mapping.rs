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
    permissions: Option<Word>,
) -> Result<(), i32> {
    // Full pages go directly from the POSIX read window to new backing.
    // Only a short read's partial page needs bounded local buffering.
    // Only [..buffered] is initialized or published. A full-page publish
    // requires buffered == len; a partial publish passes exactly buffered
    // source bytes and Alpha zeros the destination suffix. No eager clear
    // is needed when reads bypass this tail buffer altogether.
    let mut tail = [core::mem::MaybeUninit::<u8>::uninit(); LINUX_PAGE_SIZE as usize];
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
        // No later read can overwrite this final source window. Publish its
        // full prefix, partial page and remaining zero pages in one constructor
        // instead of staging a tail and issuing separate suffix requests.
        if buffered == 0 && read == bytes - copied
            && read <= mapped.saturating_sub(initialized)
        {
            publish(
                pid,
                base + initialized,
                mapped - initialized,
                source,
                read,
                permissions,
            )
            .map_err(map_request_error)?;
            return Ok(());
        }
        let mut consumed = 0;
        if buffered != 0 {
            let chunk = (read as usize).min(tail.len() - buffered);
            unsafe {
                ::core::ptr::copy_nonoverlapping(
                    source as *const u8,
                    tail.as_mut_ptr().cast::<u8>().add(buffered),
                    chunk,
                );
            }
            buffered += chunk;
            consumed += chunk as Word;
            if buffered == tail.len() {
                publish(
                    pid,
                    base + initialized,
                    LINUX_PAGE_SIZE,
                    tail.as_ptr() as Word,
                    LINUX_PAGE_SIZE,
                    permissions,
                )
                .map_err(map_request_error)?;
                initialized += LINUX_PAGE_SIZE;
                buffered = 0;
            }
        }
        let full = (read - consumed) & !(LINUX_PAGE_SIZE - 1);
        if full != 0 {
            publish(
                pid,
                base + initialized,
                full,
                source + consumed,
                full,
                permissions,
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
                    tail.as_mut_ptr().cast::<u8>().add(buffered),
                    remaining,
                );
            }
            buffered += remaining;
        }
        copied += read;
    }
    if buffered != 0 {
        // This owned prefix survives EOF and short reads. The constructor can
        // initialize its partial page and every remaining zero page together.
        let remaining = mapped.saturating_sub(initialized).max(LINUX_PAGE_SIZE);
        publish(
            pid,
            base + initialized,
            remaining,
            tail.as_ptr() as Word,
            buffered as Word,
            permissions,
        )
        .map_err(map_request_error)?;
        initialized += remaining;
    }
    if initialized < mapped {
        publish(
            pid,
            base + initialized,
            mapped - initialized,
            0,
            0,
            permissions,
        )
        .map_err(map_request_error)?;
    }
    Ok(())
}

fn publish(
    pid: Word, base: Word, mapped: Word, source: Word, bytes: Word,
    permissions: Option<Word>,
) -> Result<(), nanami_runtime::RequestError> {
    if let Some(permissions) = permissions {
        nanami_runtime::request_process_map_initialized_protected_at(
            pid, base, mapped, source, bytes, permissions,
        )
    } else {
        nanami_runtime::request_process_map_initialized_at(pid, base, mapped, source, bytes)
    }
}
