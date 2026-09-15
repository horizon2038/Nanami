use super::*;
use net::{NET_DEVICE_RX_BATCH_MAX, NET_DEVICE_RX_SLOT_STRIDE};
use std::sync::atomic::{fence, Ordering};
use std::{cmp::min, ptr, sync::Mutex};

#[path = "../../../nanami/servers/apps/virtio-net/src/app/rx_backlog.rs"]
mod rx_backlog;
#[path = "../../../nanami/servers/apps/virtio-net/src/app/rx_batch.rs"]
mod rx_batch;
#[path = "../../../nanami/servers/apps/virtio-net/src/app/rx_softq.rs"]
mod rx_softq;
use rx_softq::{softq_is_empty, softq_pop_to_shared};

const RX_SOFTQ_CAP: usize = 64;
const NET_MAX_PACKET_BYTES: usize = 1536;
const ENABLE_IRQ_RX_PACKET_DEBUG: bool = false;
const QUEUE_RX_INDEX: u16 = 0;
static LOCK: Mutex<()> = Mutex::new(());
static mut RX_SOFTQ_COUNT: usize = 0;
static mut RX_SOFTQ_TAIL: usize = 0;
static mut RX_SOFTQ_HEAD: usize = 0;
static mut RX_SOFTQ_DATA: [[u8; NET_MAX_PACKET_BYTES]; RX_SOFTQ_CAP] =
    [[0; NET_MAX_PACKET_BYTES]; RX_SOFTQ_CAP];
static mut RX_SOFTQ_LEN: [usize; RX_SOFTQ_CAP] = [0; RX_SOFTQ_CAP];
static mut RX_SNAPSHOT_LEN: usize = 0;
static mut RX_SNAPSHOT: [u8; 64] = [0; 64];

struct NetRuntime {
    io_desc: Word,
    io_base: Word,
    rx_pending_len: usize,
    shared_vaddr: usize,
    shared_size: usize,
    packets: Vec<[u8; NET_MAX_PACKET_BYTES]>,
    next: usize,
    recycled: Vec<usize>,
}
fn rx_buffer_vaddr(runtime: &NetRuntime, chain: usize) -> usize {
    runtime.packets[chain].as_ptr() as usize
}
fn take_rx_chain(runtime: &mut NetRuntime) -> Option<(usize, usize)> {
    let chain = runtime.next;
    if chain == runtime.packets.len() {
        return None;
    }
    runtime.next += 1;
    Some((chain, NET_MAX_PACKET_BYTES))
}
fn recycle_rx_chain(runtime: &mut NetRuntime, chain: usize) {
    runtime.recycled.push(chain);
}
fn notify_queue(_: Word, _: Word, _: u16) -> Result<(), RequestError> {
    Ok(())
}

#[test]
fn full_software_queue_leaves_nic_completions_pending_without_overwriting_syns() {
    let _lock = LOCK.lock().unwrap();
    unsafe {
        RX_SOFTQ_COUNT = 0;
        RX_SOFTQ_TAIL = 0;
    }
    let mut runtime = NetRuntime {
        io_desc: 0,
        io_base: 0,
        rx_pending_len: 0,
        shared_vaddr: 0,
        shared_size: 0,
        next: 0,
        recycled: Vec::new(),
        packets: (0..100).map(|i| [i as u8; NET_MAX_PACKET_BYTES]).collect(),
    };
    assert_eq!(
        rx_backlog::drain_rx_to_softq(&mut runtime, 128),
        RX_SOFTQ_CAP
    );
    assert_eq!(runtime.next, RX_SOFTQ_CAP);
    assert_eq!(runtime.recycled.len(), RX_SOFTQ_CAP);
    assert_eq!(rx_backlog::drain_rx_to_softq(&mut runtime, 128), 0);
    assert_eq!(runtime.next, RX_SOFTQ_CAP);
    unsafe {
        for index in 0..RX_SOFTQ_CAP {
            assert_eq!(RX_SOFTQ_DATA[index], runtime.packets[index]);
        }
        // The consumer has removed the first 10 entries; only these slots may
        // be reused when the next IRQ checks the used ring.
        RX_SOFTQ_COUNT -= 10;
    }
    assert_eq!(rx_backlog::drain_rx_to_softq(&mut runtime, 128), 10);
    assert_eq!(runtime.next, RX_SOFTQ_CAP + 10);
    unsafe {
        for index in 0..10 {
            assert_eq!(RX_SOFTQ_DATA[index], runtime.packets[RX_SOFTQ_CAP + index]);
        }
        for index in 10..RX_SOFTQ_CAP {
            assert_eq!(RX_SOFTQ_DATA[index], runtime.packets[index]);
        }
    }
}

#[test]
fn rx_batch_preserves_soft_queue_then_dma_order_and_respects_requested_capacity() {
    let _lock = LOCK.lock().unwrap();
    unsafe {
        RX_SOFTQ_HEAD = 0;
        RX_SOFTQ_TAIL = 0;
        RX_SOFTQ_COUNT = 0;
    }
    let mut shm = vec![0xa5; NET_DEVICE_RX_SLOT_STRIDE * NET_DEVICE_RX_BATCH_MAX + 32];
    let mut runtime = NetRuntime {
        io_desc: 0,
        io_base: 0,
        rx_pending_len: 0,
        shared_vaddr: shm.as_mut_ptr() as usize,
        shared_size: shm.len(),
        packets: (0..80).map(|i| [i as u8; NET_MAX_PACKET_BYTES]).collect(),
        next: 0,
        recycled: Vec::new(),
    };
    assert_eq!(rx_backlog::drain_rx_to_softq(&mut runtime, 20), 20);
    assert_eq!(rx_batch::recv_batch_to_shared(&mut runtime, 16, 32), Ok(32));
    assert_eq!(runtime.next, 32);
    assert_eq!(runtime.recycled, (0..32).collect::<Vec<_>>());
    for slot in 0..32 {
        let record = &shm[16 + slot * NET_DEVICE_RX_SLOT_STRIDE..][..NET_DEVICE_RX_SLOT_STRIDE];
        assert_eq!(
            u32::from_le_bytes(record[..4].try_into().unwrap()),
            NET_MAX_PACKET_BYTES as u32
        );
        assert_eq!(&record[4..], &[slot as u8; NET_MAX_PACKET_BYTES]);
    }
    assert_eq!(&shm[..16], &[0xa5; 16]);
    assert_eq!(&shm[shm.len() - 16..], &[0xa5; 16]);
    assert_eq!(rx_batch::recv_batch_to_shared(&mut runtime, 16, 32), Ok(32));
    assert_eq!(rx_batch::recv_batch_to_shared(&mut runtime, 16, 32), Ok(16));
    assert_eq!(rx_batch::recv_batch_to_shared(&mut runtime, 16, 32), Ok(0));
    assert_eq!(runtime.recycled, (0..80).collect::<Vec<_>>());
}

#[test]
fn rx_batch_rejects_invalid_ranges_before_consuming_any_packet() {
    let _lock = LOCK.lock().unwrap();
    unsafe {
        RX_SOFTQ_HEAD = 0;
        RX_SOFTQ_TAIL = 0;
        RX_SOFTQ_COUNT = 0;
    }
    let mut shm = vec![0xa5; NET_DEVICE_RX_SLOT_STRIDE];
    let mut runtime = NetRuntime {
        io_desc: 0,
        io_base: 0,
        rx_pending_len: 0,
        shared_vaddr: shm.as_mut_ptr() as usize,
        shared_size: shm.len(),
        packets: vec![[1; NET_MAX_PACKET_BYTES]; 2],
        next: 0,
        recycled: Vec::new(),
    };
    rx_backlog::drain_rx_to_softq(&mut runtime, 1);
    for (offset, count) in [(0, 0), (0, 33), (usize::MAX, 1), (1, 1), (0, usize::MAX)] {
        assert_eq!(
            rx_batch::recv_batch_to_shared(&mut runtime, offset, count),
            Err(RequestError::InvalidArgument)
        );
    }
    assert_eq!(runtime.next, 1);
    assert!(!softq_is_empty());
    assert!(shm.iter().all(|byte| *byte == 0xa5));
    assert_eq!(rx_batch::recv_batch_to_shared(&mut runtime, 0, 1), Ok(1));
    assert!(softq_is_empty());
    assert_eq!(runtime.next, 1);
}
