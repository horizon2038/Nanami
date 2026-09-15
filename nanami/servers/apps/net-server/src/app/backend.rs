use super::*;
use nanami_services::net::{
    net_device_recv, net_device_recv_batch, NET_DEVICE_RX_BATCH_MAX, NET_DEVICE_RX_FRAME_MAX,
    NET_DEVICE_RX_SLOT_STRIDE,
};

// Keep batch records separate from the TX area: processing an RX frame may
// synchronously construct and transmit its ACK in BACKEND_TX_OFFSET.
const RX_BATCH_OFFSET: Word = 0x2000;
const _: () = assert!(BACKEND_TX_OFFSET + NET_DEVICE_RX_FRAME_MAX <= RX_BATCH_OFFSET);
const _: () = assert!(
    RX_BATCH_OFFSET + NET_DEVICE_RX_BATCH_MAX * NET_DEVICE_RX_SLOT_STRIDE <= BACKEND_SHM_BYTES
);

pub(super) fn pump_backend(runtime: &mut NetRuntime, stats: &mut NetStats) -> Word {
    pump_backend_with_budget(runtime, stats, BACKEND_PUMP_DEFAULT_BURST)
}

pub(super) fn pump_backend_with_budget(
    runtime: &mut NetRuntime,
    stats: &mut NetStats,
    max_frames: usize,
) -> Word {
    let mut processed = 0;
    while processed < max_frames {
        if runtime.backend_rx_batch {
            let slots = min(max_frames - processed, NET_DEVICE_RX_BATCH_MAX);
            let count = match net_device_recv_batch(runtime.net_device_port, RX_BATCH_OFFSET, slots)
            {
                Ok(count) if count > 0 && count <= slots => count,
                _ => break,
            };
            for index in 0..count {
                let offset = RX_BATCH_OFFSET + index * NET_DEVICE_RX_SLOT_STRIDE;
                let len = unsafe {
                    u32::from_le(ptr::read_unaligned(
                        get_backend_shm_ptr(runtime, offset).cast(),
                    )) as usize
                };
                if len > 0 && len <= NET_DEVICE_RX_FRAME_MAX {
                    process_frame(runtime, stats, offset + 4, len);
                }
            }
            processed += count;
        } else {
            // Drivers that do not advertise batching keep the original API.
            let len = match net_device_recv(
                runtime.net_device_port,
                BACKEND_RX_OFFSET,
                NET_DEVICE_RX_FRAME_MAX,
            ) {
                Ok(len) if len > 0 && len <= NET_DEVICE_RX_FRAME_MAX => len,
                _ => break,
            };
            process_frame(runtime, stats, BACKEND_RX_OFFSET, len);
            processed += 1;
        }
    }
    if processed != 0 {
        notify_client_sessions(runtime);
    }
    processed as Word
}

fn process_frame(runtime: &mut NetRuntime, stats: &mut NetStats, offset: Word, len: usize) {
    unsafe {
        let frame =
            core::slice::from_raw_parts(get_backend_shm_ptr(runtime, offset) as *const u8, len);
        let sessions = runtime.sessions;
        for session in sessions {
            if session.active && session.raw_rx_enabled {
                runtime.raw_rx.push(session.caller_id, frame);
            }
        }
        process_ethernet_frame(runtime, stats, frame);
    }
    stats.rx_packets = stats.rx_packets.wrapping_add(1);
    stats.rx_bytes = stats.rx_bytes.wrapping_add(len as Word);
}
