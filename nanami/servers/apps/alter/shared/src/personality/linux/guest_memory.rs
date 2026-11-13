use super::{
    map_request_error, Runtime, Word, EFAULT, EINVAL, LINUX_IOVEC_LEN, LINUX_IOV_MAX,
    LINUX_MSGHDR_LEN, LINUX_PAGE_SIZE,
};
use core::mem::MaybeUninit;

#[path = "owned_gather.rs"]
mod owned_gather;

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

pub(super) fn read_linux_iovecs<const N: usize>(
    runtime: &mut Runtime,
    pid: Word,
    iov: Word,
    iov_count: Word,
    bases: &mut [Word; N],
    lens: &mut [Word; N],
) -> Result<(), i32> {
    if iov_count > LINUX_IOV_MAX || iov_count > N {
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
    if super::guest_views::copy_into_scratch(runtime, pid, user_ptr, destination, len) {
        return Ok(());
    }
    nanami_runtime::request_process_memory_read_checked(pid, user_ptr, destination, len)
        .map_err(|_| EFAULT)
}

/// Optional read lease into an owned POSIX write buffer. Keep the original
/// checked transfer for every ineligible range, including logical-preflight
/// misses; this qualifier does not change the file-I/O fallback's error policy.
pub(super) fn copyin_posix_write(
    runtime: &Runtime, pid: Word, source: Word, destination: Word, bytes: Word,
) -> Result<(), i32> {
    // The bounded view cannot serve larger input. Keep that size test ahead
    // of the optional logical-eligibility lookup on the checked-copy path.
    if bytes != 0 && bytes <= super::guest_views::MAX_CONTIGUOUS_READ_BYTES
        && runtime.memory_access_allowed(pid, source, bytes, 1)
        && super::guest_views::copy_into_scratch(runtime, pid, source, destination, bytes)
    {
        return Ok(());
    }
    nanami_runtime::request_process_memory_read_checked(pid, source, destination, bytes)
        .map_err(|_| EFAULT)
}

/// Initialize an owned buffer directly. Small inputs may use read leases;
/// larger inputs keep the original single checked transfer, without staging
/// through scratch or splitting into page-sized requests.
pub(super) fn copyin_owned(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    destination: &mut [MaybeUninit<u8>],
) -> Result<(), i32> {
    if destination.is_empty() {
        return Ok(());
    }
    if !runtime.memory_access_allowed(pid, user_ptr, destination.len(), 1) {
        return Err(EFAULT);
    }
    if super::guest_views::copy_into_owned(runtime, pid, user_ptr, destination) {
        return Ok(());
    }
    nanami_runtime::request_process_memory_read_checked(
        pid,
        user_ptr,
        destination.as_mut_ptr() as Word,
        destination.len(),
    )
    .map_err(|_| EFAULT)
}

/// Gather bulk input in one existing checked-vector request. The caller owns
/// the unpublished destination and may expose it only after complete success.
/// Ineligible shapes retain scalar reads, including their warm read leases.
pub(super) fn copyin_owned_gather(
    runtime: &Runtime,
    pid: Word,
    bases: &[Word],
    lengths: &[Word],
    destination: &mut [MaybeUninit<u8>],
) -> Option<Result<(), i32>> {
    let bytes = destination.len();
    let plan = owned_gather::plan(
        bases, lengths, destination, runtime.posix_shm_size,
        super::guest_views::MAX_CONTIGUOUS_READ_BYTES,
    )?;
    // Nothing in this private destination has been published. A later denied
    // source can reject the whole gather without promising an earlier prefix.
    if plan.entries().iter().any(|entry| {
        !runtime.memory_access_allowed(pid, entry.target, entry.bytes, 1)
    }) {
        return Some(Err(EFAULT));
    }
    let result = nanami_runtime::request_process_memory_readv_checked(pid, plan.entries())
        .map_err(|_| EFAULT);
    #[cfg(target_os = "none")]
    if result.is_ok() && option_env!("NANAMI_UNIX_GATHER_DIAG") == Some("1") {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static REPORTED: AtomicUsize = AtomicUsize::new(0);
        if REPORTED.fetch_add(1, Ordering::Relaxed) < 16 {
            nanami_runtime::println!("[linux.unix] bulk-gather pid={} bytes={} fields={}",
                pid, bytes, plan.entries().len());
        }
    }
    #[cfg(not(target_os = "none"))]
    let _ = bytes;
    Some(result)
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
    if super::private_memory::needs_checked_write_preparation(runtime, pid) {
        super::private_memory::invalidate_checked_write_origin(runtime, pid, user_ptr, len);
        if super::guest_views::copy_from_scratch_cached(runtime, pid, user_ptr, source, len) {
            return Ok(());
        }
        if super::private_memory::can_try_cold_write_view(runtime, pid) {
            let copied = super::guest_views::copy_from_scratch(runtime, pid, user_ptr, source, len);
            report_cold_write_view(pid, "scratch", len, copied);
            if copied { return Ok(()); }
        }
    }
    super::private_memory::with_checked_private_write(runtime, pid, user_ptr, len, || {
        if super::guest_views::copy_from_scratch(runtime, pid, user_ptr, source, len) {
            return Ok(());
        }
        nanami_runtime::request_process_memory_write_checked(pid, user_ptr, source, len)
            .map_err(|_| EFAULT)
    })
}

/// A typed owned source permits small copyout without exposing an arbitrary
/// raw source VA to the local writer. Larger or ineligible ranges keep the
/// original single permission-checked transfer.
pub(super) fn copyout_owned(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    source: &[u8],
) -> Result<(), i32> {
    if !runtime.memory_access_allowed(pid, user_ptr, source.len(), 2) {
        return Err(EFAULT);
    }
    if source.is_empty() {
        return Ok(());
    }
    if super::private_memory::needs_checked_write_preparation(runtime, pid) {
        super::private_memory::invalidate_checked_write_origin(runtime, pid, user_ptr, source.len());
        if super::guest_views::copy_from_owned_cached(runtime, pid, user_ptr, source) {
            return Ok(());
        }
        if super::private_memory::can_try_cold_write_view(runtime, pid) {
            let copied = super::guest_views::copy_from_owned(runtime, pid, user_ptr, source);
            report_cold_write_view(pid, "owned", source.len(), copied);
            if copied { return Ok(()); }
        }
    }
    super::private_memory::with_checked_private_write(runtime, pid, user_ptr, source.len(), || {
        if super::guest_views::copy_from_owned(runtime, pid, user_ptr, source) {
            return Ok(());
        }
        nanami_runtime::request_process_memory_write_checked(
            pid, user_ptr, source.as_ptr() as Word, source.len(),
        ).map_err(|_| EFAULT)
    })
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

/// Keep typed owned buffers through the optional bounded write view. Prepare
/// every lease before storing; on any miss/failure retain the original single
/// checked vector, including its whole-field preflight and ordered EFAULT prefix.
pub(super) fn copyout_owned_many<const N: usize>(
    runtime: &Runtime,
    pid: Word,
    buffers: &[(Word, &[u8]); N],
) -> Result<(), i32> {
    if super::private_memory::needs_checked_write_preparation(runtime, pid) {
        let cached_permissions_valid = N != 0 && N <= super::guest_views::MAX_WRITE_BUFFERS
            && buffers.iter().all(|(target, source)| !source.is_empty()
                && runtime.memory_access_allowed(pid, *target, source.len(), 2));
        if cached_permissions_valid {
            for &(target, source) in buffers {
                super::private_memory::invalidate_checked_write_origin(runtime, pid, target, source.len());
            }
            if super::guest_views::copy_from_owned_many_cached(runtime, pid, buffers) {
                return Ok(());
            }
            if super::private_memory::can_try_cold_write_view(runtime, pid) {
                let copied = super::guest_views::copy_from_owned_many(runtime, pid, buffers);
                report_cold_write_view(pid, "vector", buffers.iter().fold(0usize,
                    |total, (_, bytes)| total.saturating_add(bytes.len())), copied);
                if copied { return Ok(()); }
            }
        }
        if let Some((base, bytes)) = single_page_copyout_span(buffers) {
            let permissions_valid = cached_permissions_valid || buffers.iter().all(|(target, source)|
                runtime.memory_access_allowed(pid, *target, source.len(), 2));
            if permissions_valid {
                // Every field uses the same backing page: any COW detach
                // failure would already fail the first field. Prepare once,
                // then retain the original ordered view/vector stores.
                return super::private_memory::with_checked_private_write(runtime, pid, base, bytes, ||
                    copyout_owned_many_prepared(runtime, pid, buffers, true));
            }
        }
        // Keep ordered field faults: preparing every field before storing the
        // first would suppress its committed prefix when a later detach fails.
        for &(target, source) in buffers { copyout_owned(runtime, pid, target, source)?; }
        return Ok(());
    }
    let permissions_valid = buffers.iter().all(|(target, source)| {
        !source.is_empty() && runtime.memory_access_allowed(pid, *target, source.len(), 2)
    });
    copyout_owned_many_prepared(runtime, pid, buffers, permissions_valid)
}

fn single_page_copyout_span<const N: usize>(buffers: &[(Word, &[u8]); N]) -> Option<(Word, Word)> {
    if N == 0 || N > nanami_runtime::OS_MEMORY_COPY_VECTOR_MAX { return None; }
    let mut start = Word::MAX;
    let mut end = 0;
    for &(target, source) in buffers {
        if source.is_empty() { return None; }
        let limit = target.checked_add(source.len())?;
        start = start.min(target);
        end = end.max(limit);
    }
    if start / LINUX_PAGE_SIZE != (end - 1) / LINUX_PAGE_SIZE { return None; }
    Some((start, end - start))
}

#[inline]
fn report_cold_write_view(_pid: Word, _kind: &str, _bytes: Word, _copied: bool) {
    #[cfg(target_os = "none")]
    if option_env!("NANAMI_COLD_WRITE_VIEWS_DIAG") == Some("1") {
        nanami_runtime::println!("[linux.cold-write-view] pid={} kind={} bytes={} copied={}",
            _pid, _kind, _bytes, usize::from(_copied));
    }
}

fn copyout_owned_many_prepared<const N: usize>(
    runtime: &Runtime, pid: Word, buffers: &[(Word, &[u8]); N], permissions_valid: bool,
) -> Result<(), i32> {
    if permissions_valid && !buffers.is_empty() {
        for &(target, source) in buffers {
            super::private_memory::invalidate_checked_write_origin(runtime, pid, target, source.len());
        }
    }
    if permissions_valid && super::guest_views::copy_from_owned_many(runtime, pid, buffers)
    {
        return Ok(());
    }
    let transfers = buffers.map(|(target, source)| nanami_runtime::ProcessMemoryTransfer {
        target,
        local: source.as_ptr() as Word,
        bytes: source.len(),
    });
    // View acquisition does not dispatch requests or mutate the personality
    // mappings. Reuse that logical preflight; Alpha still checks every field
    // against current backing/ownership before its ordered vector copy.
    if permissions_valid && !transfers.is_empty()
        && transfers.len() <= nanami_runtime::OS_MEMORY_COPY_VECTOR_MAX
    {
        return nanami_runtime::request_process_memory_writev_checked(pid, &transfers)
            .map_err(|_| EFAULT);
    }
    copyout_many(runtime, pid, &transfers)
}

/// Preserve ordered scalar fault behavior when personality preflight fails.
pub(super) fn copyout_many(
    runtime: &Runtime,
    pid: Word,
    transfers: &[nanami_runtime::ProcessMemoryTransfer],
) -> Result<(), i32> {
    if !super::private_memory::needs_checked_write_preparation(runtime, pid) && !transfers.is_empty()
        && transfers.len() <= nanami_runtime::OS_MEMORY_COPY_VECTOR_MAX
        && transfers.iter().all(|transfer| {
            transfer.bytes != 0
                && runtime.memory_access_allowed(pid, transfer.target, transfer.bytes, 2)
        })
    {
        for transfer in transfers {
            super::private_memory::invalidate_checked_write_origin(runtime, pid, transfer.target, transfer.bytes);
        }
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
        && copyout_owned(
            runtime,
            pid,
            pointers[0],
            // Every u32 byte is initialized and the array has no padding.
            // Keep this borrow tied to the owned values, not an opaque VA.
            unsafe {
                core::slice::from_raw_parts(
                    values.as_ptr().cast::<u8>(),
                    core::mem::size_of_val(&values),
                )
            },
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
