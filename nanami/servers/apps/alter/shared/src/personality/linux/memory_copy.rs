use super::{map_request_error, Runtime, Word, LINUX_PAGE_SIZE};


pub(super) fn copy_same_process_range(
    runtime: &mut Runtime,
    pid: Word,
    src_base: Word,
    dst_base: Word,
    size: Word,
) -> Result<(), i32> {
    if size == 0
        || nanami_runtime::request_process_memory_copy_within(pid, src_base, dst_base, size).is_ok()
    {
        return Ok(());
    }

    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(fork_copy_chunk(runtime), size - done);
        let src = src_base + done;
        let dst = dst_base + done;
        nanami_runtime::request_process_memory_read(pid, src, runtime.posix_shm, chunk)
            .map_err(map_request_error)?;
        nanami_runtime::request_process_memory_write(pid, dst, runtime.posix_shm, chunk)
            .map_err(map_request_error)?;
        done += chunk;
    }
    Ok(())
}

pub(super) fn fork_copy_chunk(runtime: &Runtime) -> Word {
    let limit = runtime.posix_shm_size.min(0x10000);
    if limit < LINUX_PAGE_SIZE {
        LINUX_PAGE_SIZE
    } else {
        limit & !(LINUX_PAGE_SIZE - 1)
    }
}
