use super::{map_request_error, Runtime, Word, LINUX_DIRECT_COPY_CHUNK, LINUX_PAGE_SIZE};

pub(super) fn copy_present_process_range(
    runtime: &mut Runtime,
    src_pid: Word,
    dst_pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(LINUX_DIRECT_COPY_CHUNK, size - done);
        let src = base + done;
        if libnanami::request_process_memory_clone(src_pid, dst_pid, src, chunk).is_err() {
            copy_present_process_range_via_shm(runtime, src_pid, dst_pid, src, chunk)?;
        }
        done += chunk;
    }
    Ok(())
}

pub(super) fn copy_present_process_range_via_shm(
    runtime: &mut Runtime,
    src_pid: Word,
    dst_pid: Word,
    base: Word,
    size: Word,
) -> Result<(), i32> {
    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(fork_copy_chunk(runtime), size - done);
        let src = base + done;
        if libnanami::request_process_memory_read(src_pid, src, runtime.posix_shm, chunk).is_ok() {
            if let Err(error) =
                libnanami::request_process_memory_write(dst_pid, src, runtime.posix_shm, chunk)
            {
                let errno = map_request_error(error);
                libnanami::println!(
                    "[alter/linux] fork copy failed src_pid={} dst_pid={} va=0x{:x} bytes=0x{:x} errno={}",
                    src_pid,
                    dst_pid,
                    src,
                    chunk,
                    errno
                );
                return Err(errno);
            }
        }
        done += chunk;
    }
    Ok(())
}

pub(super) fn copy_same_process_range(
    runtime: &mut Runtime,
    pid: Word,
    src_base: Word,
    dst_base: Word,
    size: Word,
) -> Result<(), i32> {
    if size == 0
        || libnanami::request_process_memory_copy_within(pid, src_base, dst_base, size).is_ok()
    {
        return Ok(());
    }

    let mut done = 0;
    while done < size {
        let chunk = ::core::cmp::min(fork_copy_chunk(runtime), size - done);
        let src = src_base + done;
        let dst = dst_base + done;
        libnanami::request_process_memory_read(pid, src, runtime.posix_shm, chunk)
            .map_err(map_request_error)?;
        libnanami::request_process_memory_write(pid, dst, runtime.posix_shm, chunk)
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
