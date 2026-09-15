use super::*;

fn softq_push_from_rx_buffer(runtime: &NetRuntime, chain: usize, packet_len: usize) {
    if packet_len == 0 {
        return;
    }
    unsafe {
        debug_assert!(RX_SOFTQ_COUNT < RX_SOFTQ_CAP);
        let slot = RX_SOFTQ_TAIL;
        let dst = (ptr::addr_of_mut!(RX_SOFTQ_DATA) as *mut u8).add(slot * NET_MAX_PACKET_BYTES);
        let src = rx_buffer_vaddr(runtime, chain) as *const u8;
        ptr::copy_nonoverlapping(src, dst, packet_len);
        RX_SOFTQ_LEN[slot] = packet_len;
        RX_SOFTQ_TAIL = (RX_SOFTQ_TAIL + 1) % RX_SOFTQ_CAP;
        RX_SOFTQ_COUNT += 1;
    }
}

pub(super) fn drain_rx_to_softq(runtime: &mut NetRuntime, burst: usize) -> usize {
    // Do not overwrite packets already handed to the driver. Leave excess
    // completions in the used ring until the client drains the software queue.
    // Only this service loop mutates the queue, so one free-space check suffices.
    let budget = min(burst, unsafe { RX_SOFTQ_CAP - RX_SOFTQ_COUNT });
    let mut drained = 0usize;
    while drained < budget {
        let Some((chain, packet_len)) = take_rx_chain(runtime) else {
            break;
        };
        softq_push_from_rx_buffer(runtime, chain, packet_len);
        recycle_rx_chain(runtime, chain);
        drained += 1;

        if ENABLE_IRQ_RX_PACKET_DEBUG && runtime.rx_pending_len > 0 {
            libnanami::print!("[virtio-net][irq.dbg] rx#");
            libnanami::print!("{}", drained);
            libnanami::print!(" bytes=");
            libnanami::print!("{}", runtime.rx_pending_len);
            libnanami::print!(" head=");
            let mut i = 0usize;
            let snap_len = unsafe { RX_SNAPSHOT_LEN };
            while i < min(snap_len, 16) {
                if i != 0 {
                    libnanami::debug::print_char(' ');
                }
                let b = unsafe { ptr::read((ptr::addr_of!(RX_SNAPSHOT) as *const u8).add(i)) };
                libnanami::print!("{:#x}", b);
                i += 1;
            }
            libnanami::print!("\n");
        }
    }
    if drained != 0 {
        let _ = notify_queue(runtime.io_desc, runtime.io_base, QUEUE_RX_INDEX);
    }
    drained
}
