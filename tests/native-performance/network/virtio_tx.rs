use super::*;
use std::{
    cmp::min,
    ptr,
    sync::atomic::{fence, Ordering},
};

#[path = "../../../nanami/servers/apps/virtio-net/src/app/virtqueue.rs"]
mod virtqueue;
use virtqueue::*;
#[path = "../../../nanami/servers/apps/virtio-net/src/app/tx_queue.rs"]
mod tx_queue;
use tx_queue::{TxQueue, TX_BUFFER_COUNT};

const NET_MAX_PACKET_BYTES: usize = 1536;
const QUEUE_TX_INDEX: u16 = 1;
struct NetRuntime {
    io_desc: Word,
    io_base: Word,
    tx_queue: TxQueue,
    tx_queue_vaddr: usize,
    tx_buf_vaddr: usize,
    tx_queue_size: u16,
    shared_vaddr: usize,
    shared_size: usize,
}
fn notify_queue(error: Word, _: Word, index: u16) -> Result<(), RequestError> {
    assert_eq!(index, QUEUE_TX_INDEX);
    if error != 0 {
        Err(RequestError::Transport)
    } else {
        Ok(())
    }
}
struct Fixture {
    runtime: NetRuntime,
    _queue: Vec<u64>,
    data: Vec<u8>,
}
impl Fixture {
    fn new(size: u16) -> Self {
        let mut queue = vec![0u64; total_queue_bytes(size).div_ceil(8)];
        let mut data = vec![0u8; TX_BUFFER_COUNT * NET_MAX_PACKET_BYTES];
        Self {
            runtime: NetRuntime {
                io_desc: 0,
                io_base: 0,
                tx_queue: TxQueue::new(size),
                tx_queue_vaddr: queue.as_mut_ptr() as usize,
                tx_buf_vaddr: data.as_mut_ptr() as usize,
                tx_queue_size: size,
                shared_vaddr: 0,
                shared_size: 0,
            },
            _queue: queue,
            data,
        }
    }
    fn complete(&mut self, head: u32) {
        unsafe {
            let base = self.runtime.tx_queue_vaddr as *mut u8;
            let size = self.runtime.tx_queue_size;
            let idx = *used_idx_ptr(base, size);
            *used_ring_ptr(base, size).add(idx as usize % size as usize) =
                VirtqUsedElem { id: head, len: 0 };
            *used_idx_ptr(base, size) = idx.wrapping_add(1);
        }
    }
    fn avail(&self) -> u16 {
        unsafe {
            *avail_idx_ptr(
                self.runtime.tx_queue_vaddr as *mut u8,
                self.runtime.tx_queue_size,
            )
        }
    }
}

#[test]
fn tx_pipelines_packets_without_waiting_for_the_nic() {
    let mut f = Fixture::new(256);
    for slot in 0..TX_BUFFER_COUNT {
        assert_eq!(
            tx_queue::submit_tx_frame(&mut f.runtime, &[slot as u8; 64]),
            Ok(64)
        );
    }
    assert_eq!(f.avail(), 64);
    for slot in 0..TX_BUFFER_COUNT {
        assert_eq!(
            &f.data[slot * NET_MAX_PACKET_BYTES..][..64],
            &[slot as u8; 64]
        );
    }
    // Complete slot 31 first: only that buffer is reusable.
    f.complete(62);
    assert_eq!(
        tx_queue::submit_tx_frame(&mut f.runtime, &[255; 64]),
        Ok(64)
    );
    for slot in 0..TX_BUFFER_COUNT {
        let byte = if slot == 31 { 255 } else { slot as u8 };
        assert_eq!(&f.data[slot * NET_MAX_PACKET_BYTES..][..64], &[byte; 64]);
    }
}

#[test]
fn tx_small_queue_backpressure_never_overwrites_inflight_data() {
    let mut f = Fixture::new(2);
    assert_eq!(tx_queue::submit_tx_frame(&mut f.runtime, &[7; 10]), Ok(10));
    assert_eq!(
        tx_queue::submit_tx_frame(&mut f.runtime, &[9; 10]),
        Err(RequestError::Transport)
    );
    assert_eq!(f.avail(), 1);
    assert_eq!(&f.data[..10], &[7; 10]);
    f.complete(0);
    assert_eq!(tx_queue::submit_tx_frame(&mut f.runtime, &[9; 10]), Ok(10));
}

#[test]
fn tx_ring_indices_wrap_without_losing_completions() {
    let mut f = Fixture::new(2);
    for n in 0..65_538 {
        assert_eq!(tx_queue::submit_tx_frame(&mut f.runtime, &[n as u8]), Ok(1));
        f.complete(0);
    }
    assert_eq!(f.avail(), 2);
}

#[test]
fn tx_invalid_completions_and_notify_errors_do_not_release_owned_buffers() {
    let mut f = Fixture::new(4);
    f.runtime.io_desc = 1;
    assert_eq!(
        tx_queue::submit_tx_frame(&mut f.runtime, &[7]),
        Err(RequestError::Transport)
    );
    f.runtime.io_desc = 0;
    assert_eq!(tx_queue::submit_tx_frame(&mut f.runtime, &[9]), Ok(1));
    assert_eq!(f.data[0], 7);
    assert_eq!(f.data[NET_MAX_PACKET_BYTES], 9);
    f.complete(1); // A tail descriptor is not a valid completion.
    assert_eq!(
        tx_queue::submit_tx_frame(&mut f.runtime, &[11]),
        Err(RequestError::Transport)
    );
    assert_eq!(f.avail(), 2);
}

#[test]
fn tx_shared_memory_bounds_are_validated_before_reserving_a_chain() {
    let mut f = Fixture::new(2);
    let bytes = [7u8; 16];
    f.runtime.shared_vaddr = bytes.as_ptr() as usize;
    f.runtime.shared_size = bytes.len();
    for (offset, len) in [(usize::MAX, 2), (8, 9), (0, 0), (0, 1537)] {
        assert_eq!(
            tx_queue::copy_from_shared_and_submit(&mut f.runtime, offset, len),
            Err(RequestError::InvalidArgument)
        );
    }
    assert_eq!(f.avail(), 0);
    assert_eq!(
        tx_queue::copy_from_shared_and_submit(&mut f.runtime, 0, 16),
        Ok(16)
    );
}
