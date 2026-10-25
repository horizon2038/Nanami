use super::*;

pub(super) fn softq_is_empty() -> bool {
    unsafe { RX_SOFTQ_COUNT == 0 }
}

pub(super) fn softq_pop_len(requested_len: usize) -> usize {
    unsafe {
        if RX_SOFTQ_COUNT == 0 {
            return 0;
        }
        let slot = RX_SOFTQ_HEAD;
        let n = min(RX_SOFTQ_LEN[slot], requested_len);
        RX_SOFTQ_LEN[slot] = 0;
        RX_SOFTQ_HEAD = (RX_SOFTQ_HEAD + 1) % RX_SOFTQ_CAP;
        RX_SOFTQ_COUNT -= 1;
        n
    }
}

pub(super) fn softq_pop_to_shared(
    runtime: &NetRuntime,
    shared_offset: usize,
    requested_len: usize,
) -> Result<usize, RequestError> {
    if runtime.shared_vaddr == 0 || runtime.shared_size == 0 {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        if RX_SOFTQ_COUNT == 0 {
            return Ok(0);
        }
        let slot = RX_SOFTQ_HEAD;
        let n = min(RX_SOFTQ_LEN[slot], requested_len);
        if shared_offset >= runtime.shared_size || shared_offset + n > runtime.shared_size {
            return Err(RequestError::InvalidArgument);
        }
        let src = (ptr::addr_of!(RX_SOFTQ_DATA) as *const u8).add(slot * NET_MAX_PACKET_BYTES);
        let dst = (runtime.shared_vaddr + shared_offset) as *mut u8;
        ptr::copy_nonoverlapping(src, dst, n);
        RX_SOFTQ_LEN[slot] = 0;
        RX_SOFTQ_HEAD = (RX_SOFTQ_HEAD + 1) % RX_SOFTQ_CAP;
        RX_SOFTQ_COUNT -= 1;
        Ok(n)
    }
}
