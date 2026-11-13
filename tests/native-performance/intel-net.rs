//! Exercise production NIC initialization, DMA rings and protocol, without I/O.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use nanami_uapi::{RequestError, Word};
pub use std::println;
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
#[path = "../../nanami/servers/core-services/intel-net/src/ports.rs"]
mod ports;
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
    bus_master: std::rc::Rc<std::cell::Cell<bool>>,
    coherence_before_master: bool,
    strict_i211: bool,
    check_pch_reset: bool,
    pch_reset_delay: std::cell::Cell<bool>,
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
            bus_master: std::rc::Rc::new(std::cell::Cell::new(true)),
            coherence_before_master: false,
            strict_i211: false,
            check_pch_reset: false,
            pch_reset_delay: std::cell::Cell::new(false),
        }
    }
}
impl Registers for Model {
    fn read(&self, offset: usize) -> u32 {
        assert!(
            !self.pch_reset_delay.get(),
            "PCH MMIO read before reset delay"
        );
        self.regs[offset / 4]
    }
    fn write(&mut self, offset: usize, value: u32) {
        let resetting = self.check_pch_reset && offset == CTRL && value & CTRL_RESET != 0;
        if self.strict_i211 {
            assert!(
                !matches!(offset, 0x2810 | 0x3810),
                "I211 heads are read-only"
            );
            if matches!(offset, RXDCTL | TXDCTL) {
                assert_eq!(value & 0x01e0_e0e0, 0, "I211 reserved threshold/GRAN bits");
                if value & QUEUE_ENABLE != 0 && self.read(offset) & QUEUE_ENABLE == 0 {
                    let base = offset - DCTL;
                    self.regs[(base + HEAD) / 4] = 0;
                    self.regs[(base + TAIL) / 4] = 0;
                }
            }
            if matches!(offset, 0x2818 | 0x3818) {
                let base = offset - TAIL;
                assert_ne!(self.read(base + DCTL) & QUEUE_ENABLE, 0);
            }
        }
        if offset == GCR || (offset == CTRL_EXT && value & (1 << 16) != 0) {
            self.coherence_before_master = !self.bus_master.get();
        }
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
        // Unlike QEMU's permissive model, I211 ignores queue enable while
        // PCI Bus Master Enable is clear (datasheet 4.3.1.1).
        let value = if matches!(offset, RXDCTL | TXDCTL) && !self.bus_master.get() {
            value & !QUEUE_ENABLE
        } else {
            value
        };
        self.regs[offset / 4] = value;
        if resetting {
            self.pch_reset_delay.set(true);
        }
    }
    fn delay_ms(&mut self, ms: usize) -> Result<(), RequestError> {
        self.pch_reset_delay.set(false);
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
    model.check_pch_reset = true;
    model.regs[CTRL / 4] = CTRL_MASTER_DISABLE;
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
    assert_eq!(model.read(CTRL) & CTRL_MASTER_DISABLE, 0);
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
fn i211_internal_phy_controls_mac_speed_not_clock_auto_detection() {
    // Actual failing trace: CTRL=0x081c0261 (ASDE set), STATUS=0x00380703
    // (10 Mb/s MAC). A link-up indication/GPTC is not evidence of matching
    // PHY and MAC rates. Intel 333017 3.5.3.1.2.2 explicitly requires ASDE=0.
    for linked in [false, true] {
        let mut model = Model::default();
        model.regs[CTRL / 4] = 0x081c0261 | CTRL_FORCE;
        model.regs[CTRL_EXT / 4] = SPEED_BYPASS;
        if linked {
            model.regs[STATUS / 4] = 0x00380703;
        }
        chipset::reset(&mut model, Family::I210).unwrap();
        assert_eq!(model.read(CTRL) & (CTRL_AUTO_SPEED | CTRL_FORCE), 0);
        assert_eq!(model.read(CTRL_EXT) & SPEED_BYPASS, 0);
        for &(register, value) in &model.writes {
            if register == CTRL {
                assert_eq!(value & (CTRL_AUTO_SPEED | CTRL_FORCE), 0);
            }
        }
        // PHY BMCR.AN_ENABLE/AN_RESTART remain set: clearing MAC ASDE
        // must not disable IEEE 802.3 auto-negotiation at the PHY.
        assert_eq!(model.read(MDIC) & 0x1200, 0x1200);
    }
}

#[test]
fn i211_dma_coherence_overrides_per_queue_no_snoop_before_mastering() {
    let mut model = Model::default();
    model.bus_master.set(false);
    let master = model.bus_master.clone();
    chipset::start_queues(
        &mut model,
        Family::I210,
        0x1000,
        0x2000,
        ring::RING_BYTES,
        127,
        || {
            master.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(model.read(CTRL_EXT) & 0x30000, 0x30000);
    assert!(model.coherence_before_master);
    let policy = model
        .writes
        .iter()
        .position(|(r, _)| *r == CTRL_EXT)
        .unwrap();
    let queue = model
        .writes
        .iter()
        .position(|(r, _)| *r == RX_BASE)
        .unwrap();
    assert!(policy < queue);
    assert!(!model.writes.iter().any(|(r, _)| *r == GCR));
}

#[test]
fn mac_speed_reporting_does_not_mistake_carrier_for_gigabit() {
    assert_eq!(link_mbps(0x00380703), 10); // failing physical I211 trace
    assert_eq!(link_mbps(0x00380743), 100);
    assert_eq!(link_mbps(0x00380783), 1000);
    assert_eq!(link_mbps(0x003807c3), 1000);
    assert_eq!(link_mbps(0x00380780), 0);
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
fn i211_queue_enable_requires_pci_master_before_programming_and_tail() {
    let mut old = Model::default();
    old.bus_master.set(false);
    assert_eq!(
        chipset::configure_queues(&mut old, Family::I210, 0x1000, 0x2000, ring::RING_BYTES),
        Err(RequestError::Transport)
    );
    assert_eq!(old.delays, 100);

    let mut model = Model::default();
    model.strict_i211 = true;
    model.bus_master.set(false);
    let master = model.bus_master.clone();
    chipset::start_queues(
        &mut model,
        Family::I210,
        0x1000,
        0x2000,
        ring::RING_BYTES,
        127,
        || {
            master.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(model.delays, 0);
    assert_eq!(model.read(RXDCTL) & QUEUE_ENABLE, QUEUE_ENABLE);
    assert_eq!(model.read(TXDCTL) & QUEUE_ENABLE, QUEUE_ENABLE);
    assert_eq!(model.writes.last(), Some(&(RX_BASE + TAIL, 127)));
    assert_eq!(model.read(RCTL) & ENABLE, 0);
    assert_eq!(model.read(TCTL) & ENABLE, 0);
}

#[test]
fn pch_queue_setup_keeps_required_tx_policy_and_coherent_dma_before_mastering() {
    // These post-reset PCH requirements are not modeled by QEMU's 82574/82576.
    const GCR: usize = 0x5b00;
    const TARC0: usize = 0x3840;
    const TARC1: usize = 0x3940;
    let mut model = Model::default();
    let master = model.bus_master.clone();
    master.set(false);
    model.regs[GCR / 4] = 0xa500003f; // firmware no-snoop choices must not leak
    model.regs[TXDCTL / 4] = 1 << 24; // preserve non-threshold policy
    chipset::start_queues(
        &mut model,
        Family::I219,
        0x1000,
        0x2000,
        ring::RING_BYTES,
        127,
        || {
            master.set(true);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(model.read(GCR), 0xa5000000);
    assert!(model.coherence_before_master);
    assert_eq!(model.read(CTRL_EXT) & 0x420000, 0x420000);
    assert_eq!(
        model.read(TXDCTL) & ((1 << 22) | (1 << 24)),
        (1 << 22) | (1 << 24)
    );
    assert_ne!(model.read(TX_BASE + 0x100 + DCTL) & (1 << 22), 0);
    assert_eq!(model.read(TX_BASE + 0x100 + DCTL), model.read(TXDCTL));
    assert_eq!(model.read(TXDCTL) & 0xfe00_0000, 0); // no legacy LWTHRESH
    assert_eq!(model.read(RXDCTL) & 0xfe00_0000, 0); // reserved, not queue enable
    assert_eq!(model.read(TARC0) & 0x0d800000, 0x0d800000);
    assert_eq!(model.read(TARC1) & 0x55000000, 0x55000000);
    assert_eq!(model.read(RFCTL) & 0x80c0, 0xc0); // legacy, NFS filters disabled
    let first_base = model
        .writes
        .iter()
        .position(|(r, _)| *r == TX_BASE + BASE_LOW)
        .unwrap();
    let coherence = model.writes.iter().position(|(r, _)| *r == GCR).unwrap();
    assert!(coherence < first_base);
}

#[test]
fn pch_filter_initialization_does_not_write_discrete_only_register_ranges() {
    for kind in [Family::I219, Family::E1000e, Family::Igb, Family::I210] {
        let mut model = Model::default();
        chipset::configure_queues(&mut model, kind, 0x1000, 0x2000, ring::RING_BYTES).unwrap();
        let multicast: Vec<_> = model
            .writes
            .iter()
            .filter(|(r, _)| (0x5200..0x5400).contains(r))
            .collect();
        let vlan: Vec<_> = model
            .writes
            .iter()
            .filter(|(r, _)| (0x5600..0x5800).contains(r))
            .collect();
        if kind == Family::I219 {
            assert_eq!(multicast.len(), 32);
            assert!(multicast.iter().all(|(r, _)| *r < 0x5280));
            assert!(vlan.is_empty()); // PCH has no discrete-controller VFTA
        } else {
            assert_eq!(multicast.len(), 128);
            assert_eq!(vlan.len(), 128);
        }
    }
}

#[test]
fn an_isolated_discover_and_offer_do_not_require_other_packets_or_irqs() {
    for kind in [Family::I219, Family::E1000e, Family::Igb, Family::I210] {
        let mut model = Model::default(); // firmware GRAN=0 is not our policy
        chipset::configure_queues(&mut model, kind, 0x1000, 0x2000, ring::RING_BYTES).unwrap();
        let tx = model.read(TXDCTL);
        let rx = model.read(RXDCTL);
        // Legacy MAC thresholds can be cache-line (4 descriptors) units.
        // Keep the prefetch policy independent of how many further packets
        // arrive; a cache-empty fetch may bypass HTHRESH on some MACs.
        let legacy = matches!(kind, Family::I219 | Family::E1000e);
        let tx_unit = if legacy && tx & (1 << 24) == 0 { 4 } else { 1 };
        let rx_unit = if legacy && rx & (1 << 24) == 0 { 4 } else { 1 };
        assert!(
            ((tx >> 8) & 63) * tx_unit <= 1,
            "TX waits for a second packet: {kind:?} {tx:#x}"
        );
        assert!(
            ((rx >> 16) & 63) * rx_unit <= 1,
            "RX withholds an isolated OFFER: {kind:?} {rx:#x}"
        );
        let writeback = (tx >> 16) & 63;
        if legacy {
            assert_eq!(writeback, 1); // full descriptor, not the advanced RS-only policy
            assert_eq!(tx & (1 << 22), 1 << 22);
            assert_eq!(model.read(TXDCTL + 0x100), tx);
        } else {
            assert_eq!(writeback, 0); // RS reports each TX without a batching timer
            assert_eq!((tx | rx) & (1 << 24), 0); // not GRAN on I211
        }
    }
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
    let report = request(
        NET_DEVICE_REQUEST_CONTROL,
        NET_DEVICE_CONTROL_REPORT_STATUS,
        0,
        0,
    );
    let unauthorized_report = ServiceRequest {
        identifier: 11,
        ..report
    };
    let writes = model.writes.clone();
    let before = unsafe { std::slice::from_raw_parts(dma.ptr, ring::DMA_BYTES) }.to_vec();
    assert_eq!(
        service
            .request(unauthorized_report, &mut model, &mut rings)
            .0,
        OS_RESPONSE_PERMISSION_DENIED
    );
    assert_eq!(service.request(report, &mut model, &mut rings), (0, 0, 1));
    assert_eq!(model.writes, writes);
    assert_eq!(
        unsafe { std::slice::from_raw_parts(dma.ptr, ring::DMA_BYTES) },
        before
    );
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

#[test]
fn port_selection_retains_independent_dma_and_updates_carrier_and_mac() {
    use net::*;
    let a = Dma::new();
    let b = Dma::new();
    let mut devices = [
        ports::Port {
            regs: Model::default(),
            rings: a.rings(),
            physical: 0x1_2345_0000,
            mac: [2; 6],
            bdf: 0x1f06,
        },
        ports::Port {
            regs: Model::default(),
            rings: unsafe { ring::Rings::new(b.ptr as usize, 0x2_0000_0000, true) },
            physical: 0x2_0000_0000,
            mac: [4; 6],
            bdf: 0x010000,
        },
    ];
    let mut active = 0;
    let mut service = service::Service::new(devices[0].physical, devices[0].mac);
    assert!(!service.authorized(0));
    assert!(!service.authorized(10));
    assert_eq!(
        ports::control(
            &mut devices,
            &mut active,
            &mut service,
            NET_DEVICE_CONTROL_PORT_COUNT,
            0
        )
        .unwrap(),
        (2, 0)
    );
    assert!(!devices[1].carrier());
    devices[1].regs.regs[STATUS / 4] |= LINK_UP; // Late carrier, after publication.
    assert_eq!(
        ports::control(
            &mut devices,
            &mut active,
            &mut service,
            NET_DEVICE_CONTROL_PORT_INFO,
            1
        )
        .unwrap(),
        (0x010000, NET_DEVICE_PORT_CARRIER)
    );
    let bytes = [0xa5; 64];
    let old = &mut devices[0];
    unsafe {
        old.rings
            .send(
                &mut old.regs,
                old.physical,
                bytes.as_ptr() as usize,
                bytes.len(),
            )
            .unwrap();
    }
    let snapshot = unsafe { std::slice::from_raw_parts(a.ptr, ring::DMA_BYTES) }.to_vec();
    assert_eq!(
        ports::control(
            &mut devices,
            &mut active,
            &mut service,
            NET_DEVICE_CONTROL_SELECT_PORT,
            1
        )
        .unwrap()
        .0,
        0x040404040404
    );
    assert_eq!(active, 1);
    assert_eq!(devices[0].regs.read(RCTL), 0);
    assert_eq!(devices[0].regs.read(TCTL), 0);
    assert_eq!(devices[1].regs.read(RCTL), RX_CONTROL);
    assert_eq!(
        unsafe { std::slice::from_raw_parts(a.ptr, ring::DMA_BYTES) },
        snapshot
    );
    assert!(ports::select(&mut devices, &mut active, &mut service, 2).is_err());
    assert_eq!(active, 1);
}

#[test]
fn discrete_cold_phy_reset_is_skipped_for_an_existing_carrier() {
    for linked in [false, true] {
        let mut model = Model::default();
        if linked {
            model.regs[STATUS / 4] = LINK_UP;
        }
        chipset::reset(&mut model, Family::I210).unwrap();
        assert_eq!(
            model
                .writes
                .iter()
                .any(|&(r, v)| r == CTRL && v & CTRL_PHY_RESET != 0),
            !linked
        );
    }
}
