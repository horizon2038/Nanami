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
    if len == 0 {
        return Ok(());
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.posix_shm, len)
        .map_err(map_request_error)
}

pub(super) fn write_target_memory(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    len: Word,
) -> Result<(), i32> {
    write_target_memory_from(pid, user_ptr, runtime.posix_shm, len)
}

pub(super) fn write_target_memory_from(
    pid: Word,
    user_ptr: Word,
    source: Word,
    len: Word,
) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    libnanami::request_process_memory_write(pid, user_ptr, source, len).map_err(map_request_error)
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
