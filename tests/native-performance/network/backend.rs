use super::*;
use net::{NET_DEVICE_RX_FRAME_MAX, NET_DEVICE_RX_SLOT_STRIDE};
use std::{cmp::min, ptr};

#[path = "../../../nanami/servers/apps/net-server/src/app/backend.rs"]
mod backend;
const BACKEND_SHM_BYTES: Word = 0x20000;
const BACKEND_TX_OFFSET: Word = 0x1000;
const BACKEND_RX_OFFSET: Word = 0;
const BACKEND_PUMP_DEFAULT_BURST: usize = 128;
#[derive(Clone, Copy)]
struct Session {
    active: bool,
    raw_rx_enabled: bool,
    caller_id: Word,
}
struct RawRx(Vec<Vec<u8>>);
impl RawRx {
    fn push(&mut self, _: Word, data: &[u8]) {
        self.0.push(data.to_vec());
    }
}
struct NetRuntime {
    net_device_port: Word,
    backend_rx_batch: bool,
    backend_shm_local: Word,
    sessions: [Session; 1],
    raw_rx: RawRx,
    packets: Vec<Vec<u8>>,
}
#[derive(Default)]
struct NetStats {
    rx_packets: Word,
    rx_bytes: Word,
}
#[derive(Default)]
struct Device {
    shm: usize,
    frames: VecDeque<Vec<u8>>,
    calls: Vec<(usize, bool)>,
    notifications: usize,
    invalid_count: bool,
}
thread_local! { static DEVICE: RefCell<Device> = RefCell::default(); }
fn get_backend_shm_ptr(runtime: &NetRuntime, offset: Word) -> *mut u8 {
    (runtime.backend_shm_local + offset) as *mut u8
}
fn notify_client_sessions(_: &NetRuntime) {
    DEVICE.with(|device| device.borrow_mut().notifications += 1);
}
fn process_ethernet_frame(runtime: &mut NetRuntime, _: &mut NetStats, frame: &[u8]) {
    // Simulate an immediate ACK overwriting the entire separate TX workspace.
    unsafe {
        ptr::write_bytes(
            get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET),
            0xef,
            NET_DEVICE_RX_FRAME_MAX,
        );
    }
    runtime.packets.push(frame.to_vec());
}
pub(super) fn receive(offset: usize, slots: usize, batch: bool) -> Result<Word, RequestError> {
    DEVICE.with(|device| {
        let mut device = device.borrow_mut();
        device.calls.push((slots, batch));
        if device.invalid_count {
            return Ok(slots + 1);
        }
        let count = min(slots, device.frames.len());
        for index in 0..count {
            let frame = device.frames.pop_front().unwrap();
            unsafe {
                let mut dst = (device.shm + offset) as *mut u8;
                if batch {
                    dst = dst.add(index * NET_DEVICE_RX_SLOT_STRIDE);
                    ptr::write_unaligned(dst.cast::<u32>(), (frame.len() as u32).to_le());
                    dst = dst.add(4);
                }
                if frame.len() <= NET_DEVICE_RX_FRAME_MAX {
                    ptr::copy_nonoverlapping(frame.as_ptr(), dst, frame.len());
                }
            }
            if !batch {
                return Ok(frame.len());
            }
        }
        Ok(count)
    })
}
fn setup(batch: bool, count: usize) -> (Vec<u8>, NetRuntime, NetStats) {
    let mut shm = vec![0; BACKEND_SHM_BYTES];
    DEVICE.with(|device| {
        *device.borrow_mut() = Device {
            shm: shm.as_mut_ptr() as usize,
            frames: (0..count).map(|i| vec![i as u8; 100]).collect(),
            ..Device::default()
        }
    });
    let runtime = NetRuntime {
        net_device_port: 1,
        backend_rx_batch: batch,
        backend_shm_local: shm.as_mut_ptr() as usize,
        sessions: [Session {
            active: true,
            raw_rx_enabled: true,
            caller_id: 1,
        }],
        raw_rx: RawRx(Vec::new()),
        packets: Vec::new(),
    };
    (shm, runtime, NetStats::default())
}

#[test]
fn batched_backend_respects_budget_order_and_separate_tx_storage() {
    let (_shm, mut runtime, mut stats) = setup(true, 81);
    assert_eq!(
        backend::pump_backend_with_budget(&mut runtime, &mut stats, 33),
        33
    );
    DEVICE.with(|device| {
        let d = device.borrow();
        assert_eq!(d.calls, [(32, true), (1, true)]);
        assert_eq!(d.notifications, 1);
        assert_eq!(d.frames.len(), 48);
    });
    assert_eq!(backend::pump_backend(&mut runtime, &mut stats), 48);
    assert_eq!(stats.rx_packets, 81);
    assert_eq!(stats.rx_bytes, 8100);
    for index in 0..81 {
        assert_eq!(runtime.packets[index], [index as u8; 100]);
    }
    assert_eq!(runtime.raw_rx.0, runtime.packets);
}

#[test]
fn legacy_backend_uses_single_frame_api_with_identical_packet_order() {
    let (_shm, mut runtime, mut stats) = setup(false, 81);
    assert_eq!(backend::pump_backend(&mut runtime, &mut stats), 81);
    DEVICE.with(|device| assert_eq!(device.borrow().calls, vec![(1, false); 82]));
    assert_eq!(stats.rx_packets, 81);
    assert_eq!(runtime.raw_rx.0, runtime.packets);
}

#[test]
fn malformed_batch_metadata_cannot_read_beyond_slot_or_batch_bounds() {
    let (_shm, mut runtime, mut stats) = setup(true, 0);
    DEVICE.with(|device| device.borrow_mut().frames = [vec![1; 1537], vec![2; 100], vec![]].into());
    assert_eq!(backend::pump_backend(&mut runtime, &mut stats), 3);
    assert_eq!(stats.rx_packets, 1);
    assert_eq!(runtime.packets, [vec![2; 100]]);
    DEVICE.with(|device| device.borrow_mut().invalid_count = true);
    assert_eq!(backend::pump_backend(&mut runtime, &mut stats), 0);
    assert_eq!(stats.rx_packets, 1);
}
