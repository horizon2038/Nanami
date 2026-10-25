//! Device teardown must never touch I/O before a device has been discovered.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub mod block {
    pub use nanami_uapi::block::*;
}
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
const VIRTIO_STATUS_FAILED: u8 = 128;
#[derive(Clone, Copy, Debug)]
pub enum RequestError {
    Transport,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
#[derive(Default)]
struct State {
    reads: Vec<(Word, Word)>,
    writes: Vec<(Word, Word, u8)>,
    fail_read: bool,
    acknowledgements: Vec<(&'static str, Word, Word)>,
    fail_device_ack: bool,
    fail_kernel_ack: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
pub mod arch {
    use super::*;
    pub fn acknowledge_interrupt(io: Word, base: Word) -> Result<(), RequestError> {
        STATE.with_borrow_mut(|state| {
            state.acknowledgements.push(("device", io, base));
            if state.fail_device_ack {
                return Err(RequestError::Transport);
            }
            Ok(())
        })
    }
}
pub mod ipc {
    use super::*;
    pub fn interrupt_ack(port: Word) -> Result<(), RequestError> {
        STATE.with_borrow_mut(|state| {
            state.acknowledgements.push(("kernel", port, 0));
            if state.fail_kernel_ack {
                return Err(RequestError::Transport);
            }
            Ok(())
        })
    }
}
fn read_device_status(io: Word, base: Word) -> Result<u8, RequestError> {
    STATE.with_borrow_mut(|state| {
        state.reads.push((io, base));
        if state.fail_read {
            Err(RequestError::Transport)
        } else {
            Ok(7)
        }
    })
}
fn write_device_status(io: Word, base: Word, status: u8) -> Result<(), RequestError> {
    STATE.with_borrow_mut(|state| state.writes.push((io, base, status)));
    Ok(())
}
#[path = "../../nanami/servers/core-services/virtio-blk-server/src/app/dma.rs"]
mod dma;
#[path = "../../nanami/servers/core-services/virtio-blk-server/src/app/interrupt.rs"]
mod interrupt;
#[path = "../../nanami/servers/core-services/virtio-blk-server/src/app/pci_command.rs"]
mod pci_command;

#[test]
fn polling_masks_pci_intx_without_changing_unrelated_command_flags() {
    for current in [0, 7, 0x400, 0xffff, 0xa8a8] {
        let polling = pci_command::command_flags(current, false);
        assert_eq!(polling & 0x405, 0x405);
        assert_eq!(polling & !0x405, current & !0x405);
        let irq = pci_command::command_flags(polling, true);
        assert_eq!(irq & 0x400, 0);
        assert_eq!(irq & 5, 5);
        assert_eq!(irq & !0x405, current & !0x405);
    }
}

#[test]
fn polling_completion_retires_device_state_without_acknowledging_an_unowned_irq() {
    STATE.with_borrow_mut(|state| *state = State::default());
    interrupt::complete(17, 0xc000, None).unwrap();
    STATE.with_borrow(|state| assert_eq!(state.acknowledgements, [("device", 17, 0xc000)]));
}

#[test]
fn block_completion_clears_device_before_unmasking_irq() {
    STATE.with_borrow_mut(|state| *state = State::default());
    interrupt::acknowledge(17, 0xc000, 19).unwrap();
    STATE.with_borrow(|state| {
        assert_eq!(
            state.acknowledgements,
            [("device", 17, 0xc000), ("kernel", 19, 0)]
        );
    });
}

#[test]
fn failed_device_ack_does_not_unmask_an_asserted_irq() {
    STATE.with_borrow_mut(|state| {
        *state = State {
            fail_device_ack: true,
            ..State::default()
        };
    });
    assert!(interrupt::acknowledge(17, 0xc000, 19).is_err());
    STATE.with_borrow(|state| {
        assert_eq!(state.acknowledgements, [("device", 17, 0xc000)]);
    });
    STATE.with_borrow_mut(|state| {
        *state = State {
            fail_kernel_ack: true,
            ..State::default()
        };
    });
    assert!(interrupt::acknowledge(17, 0xc000, 19).is_err());
    STATE.with_borrow(|state| {
        assert_eq!(state.acknowledgements.len(), 2);
    });
}
#[path = "../../nanami/servers/core-services/virtio-net/src/app/util.rs"]
mod util;

#[test]
fn block_dma_allocation_contains_full_transfer_and_status() {
    use dma::*;
    assert_eq!(MAX_TRANSFER_BYTES, block::BLOCK_DEVICE_DEFAULT_SHM_BYTES);
    assert_eq!(core::mem::size_of::<VirtioBlkReqHeader>(), 16);
    assert!(DMA_QUEUE_OFFSET + QUEUE_MEM_BYTES <= DMA_HEADER_OFFSET);
    assert!(DMA_HEADER_OFFSET + 16 <= DMA_DATA_OFFSET);
    assert!(DMA_DATA_OFFSET + MAX_TRANSFER_BYTES <= DMA_STATUS_OFFSET);
    assert!(DMA_STATUS_OFFSET < DMA_TOTAL_BYTES);
    assert_eq!(DMA_TOTAL_BYTES % 4096, 0);
    assert!(DMA_TOTAL_BYTES - (DMA_STATUS_OFFSET + 1) < 4096);
    // A DMA completion used to land outside the old fixed 0x9000-byte mapping.
    assert!(DMA_STATUS_OFFSET >= 0x9000);
    let mut memory = vec![0xa5u8; DMA_TOTAL_BYTES + 4096];
    memory[DMA_DATA_OFFSET..DMA_DATA_OFFSET + MAX_TRANSFER_BYTES].fill(0x42);
    memory[DMA_STATUS_OFFSET] = 0;
    assert!(memory[DMA_TOTAL_BYTES..].iter().all(|&byte| byte == 0xa5));
    assert_eq!(memory[DMA_DATA_OFFSET + MAX_TRANSFER_BYTES - 1], 0x42);
    assert_eq!(memory[DMA_STATUS_OFFSET], 0);
}

#[test]
fn absent_device_does_not_read_or_write_legacy_ports() {
    STATE.with_borrow_mut(|state| *state = State::default());
    util::fail_device(16, 0); // PCI I/O capability may cover every port.
    STATE.with_borrow(|state| {
        assert!(
            state.reads.is_empty(),
            "no BAR was discovered; status read would hit port 0x12"
        );
        assert!(state.writes.is_empty());
    });
}

#[test]
fn discovered_device_sets_failed_preserving_other_status_bits() {
    STATE.with_borrow_mut(|state| *state = State::default());
    util::fail_device(17, 0xc000);
    STATE.with_borrow(|state| {
        assert_eq!(state.reads, [(17, 0xc000)]);
        assert_eq!(state.writes, [(17, 0xc000, 0x87)]);
    });
}

#[test]
fn failed_status_read_does_not_issue_a_write() {
    STATE.with_borrow_mut(|state| {
        *state = State {
            fail_read: true,
            ..State::default()
        }
    });
    util::fail_device(17, 0xc000);
    STATE.with_borrow(|state| assert!(state.writes.is_empty()));
}
