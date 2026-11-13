//! Coherent x86 DMA rings. No per-packet allocation or intermediate RX copy.
use crate::registers::*;
use core::{
    ptr,
    sync::atomic::{fence, Ordering},
};
use nanami_runtime::RequestError;

pub const COUNT: usize = 128;
pub const BUFFER: usize = 2048;
pub const DESCRIPTOR_BYTES: usize = 16;
pub const RING_BYTES: usize = COUNT * DESCRIPTOR_BYTES;
pub const TX_OFFSET: usize = 4096;
pub const RX_BUFFERS: usize = 8192;
pub const TX_BUFFERS: usize = RX_BUFFERS + COUNT * BUFFER;
pub const DMA_BYTES: usize = TX_BUFFERS + COUNT * BUFFER;
pub const MAX_FRAME: usize = 1536;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Descriptor {
    pub address: u64,
    pub length: u16,
    pub checksum: u8,
    pub command: u8,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}
const _: () = assert!(core::mem::size_of::<Descriptor>() == DESCRIPTOR_BYTES);

#[repr(C)]
pub struct AdvancedRx {
    pub packet_info: u64,
    pub status_error: u32,
    pub length: u16,
    pub vlan: u16,
}
#[repr(C)]
pub struct AdvancedTx {
    pub address: u64,
    pub command_length: u32,
    pub offload_status: u32,
}
const _: () = assert!(core::mem::size_of::<AdvancedRx>() == DESCRIPTOR_BYTES);
const _: () = assert!(core::mem::size_of::<AdvancedTx>() == DESCRIPTOR_BYTES);

pub struct Rings {
    memory: usize,
    physical: u64,
    advanced: bool,
    rx_next: usize,
    rx_fragment: bool,
    tx_next: usize,
    tx_clean: usize,
    tx_used: usize,
}

impl Rings {
    /// Caller owns a pinned, coherent DMA_BYTES allocation, aligned to 4 KiB.
    /// It must outlive the device; reset/quiesce before releasing it.
    pub unsafe fn new(memory: usize, physical: u64, advanced: bool) -> Self {
        assert_eq!(memory & 4095, 0);
        assert_eq!(physical & 4095, 0);
        ptr::write_bytes(memory as *mut u8, 0, DMA_BYTES);
        for index in 0..COUNT {
            ptr::write_volatile(
                (memory as *mut Descriptor).add(index),
                Descriptor {
                    address: physical + (RX_BUFFERS + index * BUFFER) as u64,
                    ..Descriptor::default()
                },
            );
        }
        fence(Ordering::Release);
        Self {
            memory,
            physical,
            advanced,
            rx_next: 0,
            rx_fragment: false,
            tx_next: 0,
            tx_clean: 0,
            tx_used: 0,
        }
    }

    fn descriptor(&self, offset: usize, index: usize) -> *mut Descriptor {
        (self.memory + offset + index * DESCRIPTOR_BYTES) as *mut Descriptor
    }

    pub fn pending(&self) -> bool {
        let offset = if self.advanced { 8 } else { 12 };
        let pointer = self.descriptor(0, self.rx_next).cast::<u8>();
        unsafe { ptr::read_volatile(pointer.add(offset)) & DONE != 0 }
    }

    /// Destination was range-checked against the client's shared mapping.
    pub unsafe fn receive(
        &mut self,
        regs: &mut impl Registers,
        destination: usize,
        capacity: usize,
    ) -> usize {
        // Bound work even when a malformed/oversize burst fills the entire ring.
        for _ in 0..COUNT {
            let index = self.rx_next;
            let descriptor = self.descriptor(0, index);
            let status_offset = if self.advanced { 8 } else { 12 };
            let status = ptr::read_volatile(descriptor.cast::<u8>().add(status_offset));
            if status & DONE == 0 {
                return 0;
            }
            fence(Ordering::Acquire);
            let (length, errors) = if self.advanced {
                let advanced = descriptor.cast::<AdvancedRx>();
                (
                    ptr::read_volatile(ptr::addr_of!((*advanced).length)) as usize,
                    (ptr::read_volatile(ptr::addr_of!((*advanced).status_error)) >> 24) as u8,
                )
            } else {
                (
                    ptr::read_volatile(ptr::addr_of!((*descriptor).length)) as usize,
                    ptr::read_volatile(ptr::addr_of!((*descriptor).errors)),
                )
            };
            let end = status & END_PACKET != 0;
            let valid = !self.rx_fragment
                && end
                && errors & 0x97 == 0
                && (14..=MAX_FRAME).contains(&length);
            self.rx_fragment = !end;
            let copied = if valid { length.min(capacity) } else { 0 };
            if copied != 0 {
                ptr::copy_nonoverlapping(
                    (self.memory + RX_BUFFERS + index * BUFFER) as *const u8,
                    destination as *mut u8,
                    copied,
                );
            }
            // Advanced writeback replaces the buffer address with RSS metadata.
            // Restore the read-format descriptor, not just its done bit.
            ptr::write_volatile(
                descriptor,
                Descriptor {
                    address: self.physical + (RX_BUFFERS + index * BUFFER) as u64,
                    ..Descriptor::default()
                },
            );
            fence(Ordering::Release);
            regs.write(RX_BASE + TAIL, index as u32);
            self.rx_next = (index + 1) % COUNT;
            if copied != 0 {
                return copied;
            }
        }
        0
    }

    pub fn reclaim_tx(&mut self) {
        while self.tx_used != 0 {
            let descriptor = self.descriptor(TX_OFFSET, self.tx_clean);
            let status = unsafe { ptr::read_volatile(ptr::addr_of!((*descriptor).status)) };
            if status & DONE == 0 {
                break;
            }
            fence(Ordering::Acquire);
            self.tx_used -= 1;
            self.tx_clean = (self.tx_clean + 1) % COUNT;
        }
    }

    pub unsafe fn send(
        &mut self,
        regs: &mut impl Registers,
        physical: u64,
        source: usize,
        length: usize,
    ) -> Result<usize, RequestError> {
        if !(14..=MAX_FRAME).contains(&length) {
            return Err(RequestError::InvalidArgument);
        }
        self.reclaim_tx();
        if self.tx_used == COUNT - 1 {
            return Err(RequestError::Transport);
        }
        let buffer = TX_BUFFERS + self.tx_next * BUFFER;
        ptr::copy_nonoverlapping(
            source as *const u8,
            (self.memory + buffer) as *mut u8,
            length,
        );
        // Pad in software as well: no stale bytes may leave through a short frame.
        let wire_length = length.max(60);
        ptr::write_bytes(
            (self.memory + buffer + length) as *mut u8,
            0,
            wire_length - length,
        );
        let descriptor = self.descriptor(TX_OFFSET, self.tx_next);
        if self.advanced {
            ptr::write_volatile(
                descriptor.cast::<AdvancedTx>(),
                AdvancedTx {
                    address: physical + buffer as u64,
                    command_length: wire_length as u32
                        | (3 << 20)
                        | ((TX_COMMAND as u32 | 0x20) << 24),
                    offload_status: (wire_length as u32) << 14, // PAYLEN; no checksum/TSO offload
                },
            );
        } else {
            ptr::write_volatile(
                descriptor,
                Descriptor {
                    address: physical + buffer as u64,
                    length: wire_length as u16,
                    command: TX_COMMAND,
                    ..Descriptor::default()
                },
            );
        }
        fence(Ordering::Release);
        self.tx_used += 1;
        self.tx_next = (self.tx_next + 1) % COUNT;
        regs.write(TX_BASE + TAIL, self.tx_next as u32);
        Ok(length)
    }
}
