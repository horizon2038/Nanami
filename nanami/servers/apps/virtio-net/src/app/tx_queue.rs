//! Each outstanding descriptor chain owns its packet until the used ring
//! returns it. IPC completion means accepted by the driver, not sent by the NIC.
use super::*;

pub(super) const TX_BUFFER_COUNT: usize = 64;

pub(super) struct TxQueue {
    free: u64,
    capacity: usize,
    used_idx: u16,
}

impl TxQueue {
    pub(super) fn new(queue_size: u16) -> Self {
        let capacity = min(TX_BUFFER_COUNT, queue_size as usize / 2);
        assert!(capacity != 0);
        Self {
            free: u64::MAX >> (64 - capacity),
            capacity,
            used_idx: 0,
        }
    }

    unsafe fn reclaim(&mut self, base: *mut u8, size: u16) -> Result<(), RequestError> {
        let used = ptr::read_volatile(used_idx_ptr(base, size));
        if used == self.used_idx {
            return Ok(());
        }
        fence(Ordering::Acquire);
        if used.wrapping_sub(self.used_idx) as usize > self.capacity {
            return Err(RequestError::Transport);
        }
        while self.used_idx != used {
            let elem = ptr::read_volatile(
                used_ring_ptr(base, size).add(self.used_idx as usize % size as usize),
            );
            let slot = elem.id as usize / 2;
            if elem.id & 1 != 0 || slot >= self.capacity || self.free & (1 << slot) != 0 {
                return Err(RequestError::Transport);
            }
            self.free |= 1 << slot;
            self.used_idx = self.used_idx.wrapping_add(1);
        }
        Ok(())
    }

    unsafe fn reserve(&mut self, base: *mut u8, size: u16) -> Result<usize, RequestError> {
        // Normally this is one used-index load. Wait only under backpressure,
        // never for the completion of the packet we have just submitted.
        self.reclaim(base, size)?;
        for _ in 0..5_000_000 {
            if self.free != 0 {
                let slot = self.free.trailing_zeros() as usize;
                self.free &= !(1 << slot);
                return Ok(slot);
            }
            core::hint::spin_loop();
            self.reclaim(base, size)?;
        }
        Err(RequestError::Transport)
    }
}

pub(super) fn submit_tx_frame(
    runtime: &mut NetRuntime,
    frame: &[u8],
) -> Result<usize, RequestError> {
    if frame.is_empty() || frame.len() > NET_MAX_PACKET_BYTES {
        return Err(RequestError::InvalidArgument);
    }
    unsafe {
        let base = runtime.tx_queue_vaddr as *mut u8;
        let size = runtime.tx_queue_size;
        let slot = runtime.tx_queue.reserve(base, size)?;
        let head = slot * 2;
        ptr::copy_nonoverlapping(
            frame.as_ptr(),
            (runtime.tx_buf_vaddr + slot * NET_MAX_PACKET_BYTES) as *mut u8,
            frame.len(),
        );
        (*desc_ptr(base).add(head + 1)).len = frame.len() as u32;
        let avail = ptr::read_volatile(avail_idx_ptr(base, size));
        ptr::write_volatile(
            avail_ring_ptr(base, size).add(avail as usize % size as usize),
            head as u16,
        );
        // Payload, descriptors and ring entry must precede publishing the head.
        fence(Ordering::Release);
        ptr::write_volatile(avail_idx_ptr(base, size), avail.wrapping_add(1));
        fence(Ordering::SeqCst);
    }
    // On an I/O error the chain still belongs to the device: don't release it.
    notify_queue(runtime.io_desc, runtime.io_base, QUEUE_TX_INDEX)?;
    Ok(frame.len())
}

pub(super) fn submit_tx(
    runtime: &mut NetRuntime,
    requested_len: usize,
) -> Result<usize, RequestError> {
    let zero = [0u8; NET_MAX_PACKET_BYTES];
    submit_tx_frame(runtime, &zero[..min(requested_len, NET_MAX_PACKET_BYTES)])
}

pub(super) fn copy_from_shared_and_submit(
    runtime: &mut NetRuntime,
    shared_offset: usize,
    requested_len: usize,
) -> Result<usize, RequestError> {
    if runtime.shared_vaddr == 0
        || requested_len == 0
        || requested_len > NET_MAX_PACKET_BYTES
        || shared_offset
            .checked_add(requested_len)
            .is_none_or(|end| end > runtime.shared_size)
    {
        return Err(RequestError::InvalidArgument);
    }
    let frame = unsafe {
        core::slice::from_raw_parts(
            (runtime.shared_vaddr + shared_offset) as *const u8,
            requested_len,
        )
    };
    submit_tx_frame(runtime, frame)
}
