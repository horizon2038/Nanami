//! Exercise production NIC initialization, DMA rings and protocol, without I/O.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use nanami_uapi::{RequestError, Word};
pub const OS_RESPONSE_OK: Word = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
pub const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
pub const CLIENT_NOTIFICATION: Word = 24;
pub struct TimerSelection {
    pub image: Option<&'static str>,
    pub hpet_mmio_base: Word,
}
thread_local! { static ARM_MMIO: std::cell::RefCell<Vec<u32>> = std::cell::RefCell::new(vec![0; 0x4000 / 4]); }
pub fn request_mmio(physical: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    assert_eq!((physical, bytes), (0x0a00_0000, 0x4000));
    ARM_MMIO.with(|memory| Ok((physical, memory.borrow_mut().as_mut_ptr() as Word)))
}
mod arm {
    pub mod imp {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../nanami/servers/core-services/driver-manager/src/arch/aarch64.rs"
        ));
    }
}
use arm::imp as arm_discovery;
pub mod device {
    pub struct UsbControllerResource;
}
pub mod uapi {
    pub use nanami_uapi::{intel_graphics, intel_net};
}
pub mod net {
    pub use nanami_uapi::net::*;
}
#[derive(Clone, Copy, Default)]
pub struct ServiceRequest {
    identifier: Word,
    code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
    pub fn notification_notify(_: usize) -> Result<(), crate::RequestError> {
        Ok(())
    }
}
pub fn request_notification_port_copy(
    _: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<(), RequestError> {
    Ok(())
}
#[path = "../../nanami/servers/core-services/intel-net/src/chipset.rs"]
mod chipset;
#[path = "../../nanami/servers/core-services/driver-manager/src/arch/x86_64/peripherals.rs"]
mod peripherals;
#[path = "../../nanami/servers/core-services/intel-net/src/registers.rs"]
mod registers;
#[path = "../../nanami/servers/core-services/intel-net/src/ring.rs"]
mod ring;
#[path = "../../nanami/servers/core-services/intel-net/src/service.rs"]
mod service;
use nanami_uapi::intel_net::{family, Family};
use registers::*;

struct Model {
    regs: Vec<u32>,
    writes: Vec<(usize, u32)>,
    delays: usize,
    stuck: bool,
}
impl Default for Model {
    fn default() -> Self {
        let mut regs = vec![0; 0x20000 / 4];
        regs[RAL / 4] = 0x3322_1102;
        regs[RAH / 4] = 0x8000_5544;
        Self {
            regs,
            writes: Vec::new(),
            delays: 0,
            stuck: false,
        }
    }
}
impl Registers for Model {
    fn read(&self, offset: usize) -> u32 {
        self.regs[offset / 4]
    }
    fn write(&mut self, offset: usize, value: u32) {
        self.writes.push((offset, value));
        let value = if offset == CTRL && !self.stuck {
            value & !CTRL_RESET
        } else {
            value
        };
        let value = if offset == MDIC {
            value | (1 << 28)
        } else {
            value
        };
        self.regs[offset / 4] = value;
    }
    fn delay_ms(&mut self, ms: usize) -> Result<(), RequestError> {
        self.delays += ms;
        Ok(())
    }
}

struct Dma {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}
impl Dma {
    fn new() -> Self {
        let layout = std::alloc::Layout::from_size_align(ring::DMA_BYTES, 4096).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null());
        Self { ptr, layout }
    }
    fn rings(&self) -> ring::Rings {
        unsafe { ring::Rings::new(self.ptr as usize, 0x1_2345_0000, false) }
    }
    fn rx(&self, index: usize, length: u16, status: u8, errors: u8) {
        unsafe {
            let descriptor = self.ptr.cast::<ring::Descriptor>().add(index);
            (*descriptor).length = length;
            (*descriptor).status = status;
            (*descriptor).errors = errors;
            std::ptr::write_bytes(
                self.ptr.add(ring::RX_BUFFERS + index * ring::BUFFER),
                index as u8,
                length as usize,
            );
        }
    }
}
impl Drop for Dma {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(self.ptr, self.layout);
        }
    }
}

#[test]
fn physical_machine_and_emulated_families_match_but_unimplemented_devices_do_not() {
    for (id, kind) in [
        (0x15bb, Family::I219),
        (0x1539, Family::I210),
        (0x10d3, Family::E1000e),
        (0x10c9, Family::Igb),
    ] {
        assert_eq!(family(0x8086, id, 0x020000), Some(kind));
        assert_eq!(family(0x1234, id, 0x020000), None);
        assert_eq!(family(0x8086, id, 0x030000), None);
    }
    assert_eq!(family(0x8086, 0x15f3, 0x020000), None); // I225 != I211
}

#[test]
fn pci_inventory_selects_only_present_drivers_and_spawn_name_fits_the_wire() {
    let mut inventory = peripherals::Peripherals::default();
    assert_eq!(inventory.network(), None);
    assert_eq!(inventory.display(), None);
    inventory.observe(0x8086, 0x3e91, 0x030000);
    inventory.observe(0x8086, 0x15bb, 0x020000);
    inventory.observe(0x8086, 0x1539, 0x020000);
    assert_eq!(inventory.network(), Some("./bin/intel-net"));
    assert_eq!(inventory.display(), Some("intel-graphics-server"));
    assert!(inventory.display().unwrap().len() <= 24);
    inventory.observe(0x1af4, 0x1000, 0x020000);
    assert_eq!(inventory.network(), Some("./bin/virtio-net"));
    inventory.observe(0x1af4, 0x1050, 0x030000);
    assert_eq!(inventory.display(), Some("./bin/virtio-gpu-server"));
}

#[test]
fn arm_storage_uses_detected_transport_not_architecture_alone() {
    ARM_MMIO.with(|memory| memory.borrow_mut().fill(0));
    assert_eq!(arm_discovery::select_storage_driver(), Ok(None));
    ARM_MMIO.with(|memory| {
        let mut memory = memory.borrow_mut();
        let index = 31 * 0x200 / 4;
        memory[index] = 0x7472_6976;
        memory[index + 1] = 2;
        memory[index + 2] = 1; // network != block
    });
    assert_eq!(arm_discovery::select_storage_driver(), Ok(None));
    ARM_MMIO.with(|memory| memory.borrow_mut()[31 * 0x200 / 4 + 2] = 2);
    assert_eq!(
        arm_discovery::select_storage_driver(),
        Ok(Some("./bin/virtio-blk-server"))
    );
}

#[test]
fn i219_preserves_phy_and_applies_reset_workaround_without_nvm_writes() {
    let mut model = Model::default();
    assert_eq!(
        chipset::reset(&mut model, Family::I219).unwrap(),
        [2, 17, 34, 51, 68, 85]
    );
    assert!(model
        .writes
        .iter()
        .any(|&(r, v)| r == FEXTNVM11 && v & (1 << 13) != 0));
    assert!(!model
        .writes
        .iter()
        .any(|&(r, v)| r == CTRL && v & CTRL_PHY_RESET != 0));
    assert!(!model.writes.iter().any(|&(r, _)| r == MDIC || r == EERD));
}

#[test]
fn reset_timeout_is_bounded_and_i211_restores_warm_reset_packet_buffers() {
    let mut model = Model::default();
    chipset::reset(&mut model, Family::I210).unwrap();
    assert_eq!(model.read(RX_SIZE), 0xa2);
    assert_eq!(model.read(TX_SIZE), 0x0400_0014);
    model.stuck = true;
    assert_eq!(
        chipset::reset(&mut model, Family::I210),
        Err(RequestError::Transport)
    );
    assert!(model.delays < 250);
}

#[test]
fn queue_setup_uses_64bit_dma_and_advanced_igb_descriptors() {
    let mut model = Model::default();
    chipset::configure_queues(
        &mut model,
        Family::I210,
        0x1_2345_0000,
        0x1_2345_1000,
        ring::RING_BYTES,
    )
    .unwrap();
    assert_eq!(model.read(RX_BASE + BASE_HIGH), 1);
    assert_eq!(model.read(TX_BASE + BASE_HIGH), 1);
    assert_eq!(model.read(SRRCTL) & (7 << 25), 1 << 25);
    assert_ne!(model.read(RXDCTL) & QUEUE_ENABLE, 0);
    assert_eq!(model.read(RCTL) & ENABLE, 0); // not enabled before buffers are ready
}

#[test]
fn rx_wraps_and_discards_errors_and_all_fragments_without_copying_oversize_data() {
    let dma = Dma::new();
    let mut rings = dma.rings();
    let mut model = Model::default();
    let mut bytes = [0xab; 1540];
    for index in 0..ring::COUNT {
        dma.rx(index, 64, DONE | END_PACKET, 0);
        assert_eq!(
            unsafe { rings.receive(&mut model, bytes.as_mut_ptr() as usize, 1536) },
            64
        );
        assert_eq!(bytes[0], index as u8);
    }
    dma.rx(0, 2048, DONE, 0);
    dma.rx(1, 64, DONE | END_PACKET, 0);
    dma.rx(2, 64, DONE | END_PACKET, 1);
    dma.rx(3, 64, DONE | END_PACKET, 0);
    assert_eq!(
        unsafe { rings.receive(&mut model, bytes.as_mut_ptr() as usize, 1536) },
        64
    );
    assert_eq!(bytes[0], 3);
    assert_eq!(&bytes[1536..], &[0xab; 4]);
    assert_eq!(model.read(RX_BASE + TAIL), 3);
}

#[test]
fn tx_backpressure_never_overwrites_owned_dma_and_completion_reclaims_it() {
    let dma = Dma::new();
    let mut rings = dma.rings();
    let mut model = Model::default();
    let bytes = [0x55; 42];
    for _ in 0..ring::COUNT - 1 {
        assert_eq!(
            unsafe {
                rings.send(
                    &mut model,
                    0x1_2345_0000,
                    bytes.as_ptr() as usize,
                    bytes.len(),
                )
            }
            .unwrap(),
            42
        );
    }
    assert_eq!(
        unsafe {
            rings.send(
                &mut model,
                0x1_2345_0000,
                bytes.as_ptr() as usize,
                bytes.len(),
            )
        },
        Err(RequestError::Transport)
    );
    unsafe {
        let descriptor = dma.ptr.add(ring::TX_OFFSET).cast::<ring::Descriptor>();
        assert_eq!((*descriptor).length, 60);
        assert_eq!((*descriptor).command, TX_COMMAND);
        assert_eq!(
            std::slice::from_raw_parts(dma.ptr.add(ring::TX_BUFFERS + 42), 18),
            &[0; 18]
        );
        (*descriptor).status = DONE;
    }
    assert!(unsafe {
        rings.send(
            &mut model,
            0x1_2345_0000,
            bytes.as_ptr() as usize,
            bytes.len(),
        )
    }
    .is_ok());
}

#[test]
fn service_checks_batch_bounds_before_consumption_and_restricts_owner() {
    use net::*;
    let dma = Dma::new();
    let mut rings = dma.rings();
    let mut model = Model::default();
    let mut service = service::Service::new(0x1_2345_0000, [2; 6]);
    let mut shared = vec![0; 2 * NET_DEVICE_RX_SLOT_STRIDE];
    let request = |code, arg0, arg1, arg2| ServiceRequest {
        identifier: 10,
        code,
        arg0,
        arg1,
        arg2,
        ..Default::default()
    };
    assert_eq!(
        service
            .request(
                request(
                    NET_DEVICE_REQUEST_CONTROL,
                    NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY,
                    shared.as_mut_ptr() as usize,
                    shared.len()
                ),
                &mut model,
                &mut rings
            )
            .0,
        OS_RESPONSE_OK
    );
    dma.rx(0, 64, DONE | END_PACKET, 0);
    assert_eq!(
        service
            .request(
                request(NET_DEVICE_REQUEST_RECV_BATCH, 0, 3, 0),
                &mut model,
                &mut rings
            )
            .0,
        OS_RESPONSE_INVALID_ARGUMENT
    );
    assert!(rings.pending());
    let batch = request(NET_DEVICE_REQUEST_RECV_BATCH, 0, 2, 0);
    let stranger = ServiceRequest {
        identifier: 11,
        ..batch
    };
    assert_eq!(
        service.request(stranger, &mut model, &mut rings).0,
        OS_RESPONSE_PERMISSION_DENIED
    );
    assert!(rings.pending());
    assert_eq!(service.request(batch, &mut model, &mut rings), (0, 1, 0));
    assert_eq!(u32::from_le_bytes(shared[..4].try_into().unwrap()), 64);
    assert!(!rings.pending());
}

#[test]
fn igb_advanced_writeback_restores_rx_address_and_transmit_sets_payload_length() {
    let dma = Dma::new();
    let mut rings = unsafe { ring::Rings::new(dma.ptr as usize, 0x1_2345_0000, true) };
    let mut model = Model::default();
    let mut bytes = [0; 64];
    unsafe {
        let descriptor = dma.ptr.cast::<ring::AdvancedRx>();
        std::ptr::write_volatile(
            descriptor,
            ring::AdvancedRx {
                packet_info: 0x1234,
                status_error: 3,
                length: 64,
                vlan: 0,
            },
        );
        assert!(rings.pending());
        assert_eq!(
            rings.receive(&mut model, bytes.as_mut_ptr() as usize, 64),
            64
        );
        assert!(!rings.pending());
        assert_eq!(
            (*dma.ptr.cast::<ring::Descriptor>()).address,
            0x1_2345_0000 + ring::RX_BUFFERS as u64
        );
        assert!(rings
            .send(&mut model, 0x1_2345_0000, bytes.as_ptr() as usize, 64)
            .is_ok());
        let tx = &*dma.ptr.add(ring::TX_OFFSET).cast::<ring::AdvancedTx>();
        assert_eq!(tx.command_length & (3 << 20), 3 << 20);
        assert_ne!(tx.command_length & (1 << 29), 0);
        assert_eq!(tx.offload_status, 64 << 14);
    }
}
