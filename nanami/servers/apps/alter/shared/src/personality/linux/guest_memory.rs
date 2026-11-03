use super::{
    map_request_error, Runtime, Word, EFAULT, EINVAL, LINUX_IOVEC_LEN, LINUX_IOV_MAX,
    LINUX_MSGHDR_LEN,
};

pub(super) fn read_linux_msghdr(
    runtime: &mut Runtime,
    pid: Word,
    message: Word,
) -> Result<(Word, Word, Word, Word), i32> {
    if message == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, message, LINUX_MSGHDR_LEN)?;
    let name = read_shm_u64(runtime, 0);
    let name_len = read_shm_u32(runtime, 8) as Word;
    let iov = read_shm_u64(runtime, 16);
    let iov_count = read_shm_u64(runtime, 24);
    if iov_count == 0 || iov_count > LINUX_IOV_MAX || iov == 0 {
        return Err(EINVAL);
    }
    Ok((name, name_len, iov, iov_count))
}

pub(super) fn read_linux_iovecs(
    runtime: &mut Runtime,
    pid: Word,
    iov: Word,
    iov_count: Word,
    bases: &mut [Word; LINUX_IOV_MAX as usize],
    lens: &mut [Word; LINUX_IOV_MAX as usize],
) -> Result<(), i32> {
    if iov_count > LINUX_IOV_MAX {
        return Err(EINVAL);
    }
    if iov_count != 0 && iov == 0 {
        return Err(EFAULT);
    }
    let bytes = iov_count.checked_mul(LINUX_IOVEC_LEN).ok_or(EINVAL)?;
    read_target_memory(runtime, pid, iov, bytes)?;
    let mut index = 0usize;
    while index < iov_count as usize {
        bases[index] = read_shm_u64(runtime, index * LINUX_IOVEC_LEN as usize);
        lens[index] = read_shm_u64(runtime, index * LINUX_IOVEC_LEN as usize + 8);
        if bases[index] == 0 && lens[index] != 0 {
            return Err(EFAULT);
        }
        index += 1;
    }
    Ok(())
}

pub(super) fn read_target_memory(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    len: Word,
) -> Result<(), i32> {
    copyin_into(runtime, pid, user_ptr, runtime.posix_shm, len)
}

/// Copy directly into an owned server buffer without staging through POSIX SHM.
pub(super) fn copyin_into(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    destination: Word,
    len: Word,
) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    if !runtime.memory_access_allowed(pid, user_ptr, len, 1) {
        return Err(EFAULT);
    }
    nanami_runtime::request_process_memory_read_checked(pid, user_ptr, destination, len)
        .map_err(|_| EFAULT)
}

pub(super) fn write_target_memory(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    len: Word,
) -> Result<(), i32> {
    copyout_from(runtime, pid, user_ptr, runtime.posix_shm, len)
}

pub(super) fn copyout_from(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    source: Word,
    len: Word,
) -> Result<(), i32> {
    if !runtime.memory_access_allowed(pid, user_ptr, len, 2) {
        return Err(EFAULT);
    }
    if len == 0 {
        return Ok(());
    }
    nanami_runtime::request_process_memory_write_checked(pid, user_ptr, source, len)
        .map_err(|_| EFAULT)
}

pub(super) fn write_target_memory_from(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    source: Word,
    len: Word,
) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    super::private_memory::with_private_write(runtime, pid, user_ptr, len, || {
        nanami_runtime::request_process_memory_write(pid, user_ptr, source, len)
            .map_err(map_request_error)
    })
}

/// Preserve ordered scalar fault behavior when personality preflight fails.
pub(super) fn copyout_many(
    runtime: &Runtime,
    pid: Word,
    transfers: &[nanami_runtime::ProcessMemoryTransfer],
) -> Result<(), i32> {
    if !transfers.is_empty()
        && transfers.len() <= nanami_runtime::OS_MEMORY_COPY_VECTOR_MAX
        && transfers.iter().all(|transfer| {
            transfer.bytes != 0
                && runtime.memory_access_allowed(pid, transfer.target, transfer.bytes, 2)
        })
    {
        return nanami_runtime::request_process_memory_writev_checked(pid, transfers)
            .map_err(|_| EFAULT);
    }
    for transfer in transfers {
        copyout_from(
            runtime,
            pid,
            transfer.target,
            transfer.local,
            transfer.bytes,
        )?;
    }
    Ok(())
}

pub(super) fn write_u32_to_target(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: u32,
) -> Result<(), i32> {
    unsafe {
        write_u32(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 4)
}

pub(super) fn write_guest_u32(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: u32,
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u32(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 4)
}

pub(super) fn write_guest_u64(
    runtime: &mut Runtime,
    pid: Word,
    user_ptr: Word,
    value: Word,
) -> Result<(), i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    unsafe {
        write_u64(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, user_ptr, 8)
}

/// Coalesce adjacent outputs without changing ordered stores on a bad pointer.
pub(super) fn write_guest_u32s<const N: usize>(
    runtime: &mut Runtime,
    pid: Word,
    pointers: [Word; N],
    values: [u32; N],
) -> Result<(), i32> {
    let contiguous = N > 1
        && pointers[0] != 0
        && pointers
            .windows(2)
            .all(|pair| pair[0].checked_add(core::mem::size_of::<u32>()) == Some(pair[1]));
    if contiguous
        && copyout_from(
            runtime,
            pid,
            pointers[0],
            values.as_ptr() as Word,
            core::mem::size_of_val(&values),
        )
        .is_ok()
    {
        return Ok(());
    }
    // A combined copy may reject a later page before writing its valid prefix.
    // Retry scalar stores so EFAULT preserves the original argument order.
    for (pointer, value) in pointers.into_iter().zip(values) {
        write_guest_u32(runtime, pid, pointer, value)?;
    }
    Ok(())
}

pub(super) fn align_up_word(value: Word, align: Word) -> Word {
    (value + align - 1) & !(align - 1)
}

pub(super) fn align_down_word(value: Word, align: Word) -> Word {
    value & !(align - 1)
}

pub(super) fn align_down_usize(value: usize, align: usize) -> usize {
    value & !(align - 1)
}

pub(super) fn read_shm_u8(runtime: &Runtime, offset: usize) -> u8 {
    unsafe { ::core::ptr::read((runtime.posix_shm as usize + offset) as *const u8) }
}

pub(super) fn move_shm_bytes(runtime: &Runtime, src_offset: Word, dst_offset: Word, len: Word) {
    if len == 0 {
        return;
    }
    unsafe {
        ::core::ptr::copy(
            (runtime.posix_shm + src_offset) as *const u8,
            (runtime.posix_shm + dst_offset) as *mut u8,
            len as usize,
        );
    }
}

pub(super) fn read_shm_u16(runtime: &Runtime, offset: usize) -> u16 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u16) }
}

pub(super) fn read_shm_u32(runtime: &Runtime, offset: usize) -> u32 {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const u32) }
}

pub(super) fn read_shm_u64(runtime: &Runtime, offset: usize) -> Word {
    unsafe { ::core::ptr::read_unaligned((runtime.posix_shm as usize + offset) as *const Word) }
}

pub(super) unsafe fn write_u32(address: Word, value: u32) {
    ::core::ptr::write_unaligned(address as *mut u32, value);
}

pub(super) unsafe fn write_u16(address: Word, value: u16) {
    ::core::ptr::write_unaligned(address as *mut u16, value);
}

pub(super) unsafe fn write_u8(address: Word, value: u8) {
    ::core::ptr::write(address as *mut u8, value);
}

pub(super) unsafe fn write_u64(address: Word, value: Word) {
    ::core::ptr::write_unaligned(address as *mut Word, value);
}
