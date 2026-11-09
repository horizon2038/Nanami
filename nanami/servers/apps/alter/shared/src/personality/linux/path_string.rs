use super::{Runtime, Word, EFAULT, ENAMETOOLONG, LINUX_PAGE_SIZE};

pub(super) fn read_c_string(runtime: &mut Runtime, pid: Word, user_ptr: Word) -> Result<Word, i32> {
    if user_ptr == 0 {
        return Err(EFAULT);
    }
    let max = ::core::cmp::min(runtime.posix_shm_size as usize, 4096);
    let mut copied = 0usize;
    while copied < max {
        let source = user_ptr.checked_add(copied as Word).ok_or(EFAULT)?;
        let page_remaining = (LINUX_PAGE_SIZE - (source & (LINUX_PAGE_SIZE - 1))) as usize;
        // Most paths terminate near the start: avoid copying an entire page.
        let chunk = ::core::cmp::min(max - copied, page_remaining).min(128.max(copied));
        super::guest_memory::copyin_into(
            runtime,
            pid,
            source,
            runtime.posix_shm + copied as Word,
            chunk as Word,
        )?;

        let bytes = unsafe {
            core::slice::from_raw_parts((runtime.posix_shm + copied) as *const u8, chunk)
        };
        if let Some(index) = crate::common::nul_scan::find_nul(bytes) {
            let len = copied + index;
            #[cfg(feature = "wait-shape")]
            super::wait_shape::path(runtime, pid, len);
            return Ok(len as Word);
        }
        copied += chunk;
    }
    Err(ENAMETOOLONG)
}
