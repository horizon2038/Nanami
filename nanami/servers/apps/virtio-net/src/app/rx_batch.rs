//! Batch only the transport between driver and network server. Packets remain
//! independent and every consumed NIC chain is returned after its data is copied.
use super::*;
use nanami_services::net::{
    NET_DEVICE_RX_BATCH_MAX, NET_DEVICE_RX_FRAME_MAX, NET_DEVICE_RX_SLOT_STRIDE,
};

pub(super) fn recv_batch_to_shared(
    runtime: &mut NetRuntime,
    offset: usize,
    slots: usize,
) -> Result<usize, RequestError> {
    if runtime.shared_vaddr == 0
        || slots == 0
        || slots > NET_DEVICE_RX_BATCH_MAX
        || offset
            .checked_add(slots * NET_DEVICE_RX_SLOT_STRIDE)
            .is_none_or(|end| end > runtime.shared_size)
    {
        return Err(RequestError::InvalidArgument);
    }
    let mut count = 0;
    let mut recycled = false;
    while count < slots {
        let record = offset + count * NET_DEVICE_RX_SLOT_STRIDE;
        let len = if !softq_is_empty() {
            // The complete batch range was validated above.
            softq_pop_to_shared(runtime, record + 4, NET_DEVICE_RX_FRAME_MAX)?
        } else if let Some((chain, len)) = take_rx_chain(runtime) {
            unsafe {
                ptr::copy_nonoverlapping(
                    rx_buffer_vaddr(runtime, chain) as *const u8,
                    (runtime.shared_vaddr + record + 4) as *mut u8,
                    len,
                );
            }
            recycle_rx_chain(runtime, chain);
            recycled = true;
            len
        } else {
            break;
        };
        unsafe {
            ptr::write_unaligned(
                (runtime.shared_vaddr + record) as *mut u32,
                (len as u32).to_le(),
            );
        }
        count += 1;
    }
    if recycled {
        fence(Ordering::SeqCst);
        let _ = notify_queue(runtime.io_desc, runtime.io_base, QUEUE_RX_INDEX);
    }
    Ok(count)
}
