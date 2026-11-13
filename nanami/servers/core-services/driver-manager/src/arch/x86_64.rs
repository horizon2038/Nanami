use super::super::TimerSelection;
use nanami_client::{firmware as acpi, pci};
use nanami_runtime::{RequestError, Word};
#[path = "x86_64/peripherals.rs"]
mod peripherals;
#[path = "x86_64/usb.rs"]
mod usb;
pub use usb::prepare as prepare_usb_controller;
const SLOT_PCI_SERVICE: Word = 16;
const SLOT_PS2_PROBE: Word = 17;

pub fn initialize() -> Result<(), RequestError> {
    let pid = nanami_runtime::request_process_spawn("./bin/pci-server")?;
    nanami_runtime::println!("[driver-manager] spawned ./bin/pci-server pid={}", pid);
    pci::connect(SLOT_PCI_SERVICE)?;
    Ok(())
}

fn find_pci_class(wanted: u32, mut ordinal: usize) -> Result<Option<Word>, RequestError> {
    let port = nanami_runtime::ipc::process_slot_descriptor(SLOT_PCI_SERVICE);
    let mut index = 0;
    while let Some(device) = pci::enumerate(port, index)? {
        if device.class == wanted {
            if ordinal == 0 {
                return Ok(Some(device.address.packed()));
            }
            ordinal -= 1;
        }
        index += 1;
    }
    Ok(None)
}

pub fn select_storage_driver() -> Result<Option<&'static str>, RequestError> {
    if find_pci_class(0x010601, 0)?.is_some() {
        return Ok(Some("./bin/ahci-server"));
    }
    let port = nanami_runtime::ipc::process_slot_descriptor(SLOT_PCI_SERVICE);
    let mut index = 0;
    while let Some(device) = pci::enumerate(port, index)? {
        if device.vendor_id == 0x1af4 && matches!(device.device_id, 0x1001 | 0x1042) {
            return Ok(Some("./bin/virtio-blk-server"));
        }
        index += 1;
    }
    nanami_runtime::println!("[driver-manager] no PCI storage controller; probing USB storage");
    Ok(None)
}

pub fn usb_controller(index: usize) -> Result<Option<Word>, RequestError> {
    find_pci_class(0x0c0330, index)
}

/// Start optional input/network drivers and return the native display candidate.
/// The manager owns display startup and the common framebuffer fallback.
pub fn start_peripherals() -> Result<Option<&'static str>, RequestError> {
    let rsdp = nanami_runtime::request_driver_platform_info(
        nanami_runtime::DRIVER_PLATFORM_INFO_RSDP_ADDRESS,
    )?
    .0;
    let ps2: Result<bool, RequestError> = (|| {
        if !acpi::has_i8042(rsdp)? {
            return Ok(false);
        }
        // ACPI 1.x has no controller-present declaration. A status read is
        // nondestructive and rejects a floating legacy port before spawning a
        // driver or allocating its keyboard/mouse IRQ capabilities.
        nanami_runtime::request_io_port(0x64, 0x64, SLOT_PS2_PROBE)?;
        let status = nanami_runtime::io::io_read(
            nanami_runtime::ipc::process_slot_descriptor(SLOT_PS2_PROBE),
            0x64,
            1,
        )?;
        Ok(status as u8 != 0xff)
    })();
    match ps2 {
        Ok(true) => super::super::start_optional("./bin/ps2-server"),
        Ok(false) => {}
        Err(error) => nanami_runtime::println!(
            "[driver-manager] PS/2 firmware discovery failed: {}; skipping controller",
            error
        ),
    }
    let port = nanami_runtime::ipc::process_slot_descriptor(SLOT_PCI_SERVICE);
    let mut index = 0;
    let mut found = peripherals::Peripherals::default();
    while let Some(device) = pci::enumerate(port, index)? {
        found.observe(device.vendor_id, device.device_id, device.class);
        index += 1;
    }
    if let Some(image) = found.network() {
        super::super::start_optional(image);
    }
    Ok(found.display())
}

pub fn select_timer_driver() -> Result<TimerSelection, RequestError> {
    let (rsdp, _) = nanami_runtime::request_driver_platform_info(
        nanami_runtime::DRIVER_PLATFORM_INFO_RSDP_ADDRESS,
    )?;
    let hpet_mmio_base = match acpi::find_hpet_mmio_base(rsdp) {
        Ok(Some(address)) => address,
        Ok(None) => 0,
        Err(error) => {
            nanami_runtime::println!(
                "[driver-manager] ACPI scan failed: {}; selecting PIT fallback",
                error
            );
            0
        }
    };
    let image = if hpet_mmio_base != 0 {
        "./bin/hpet-server"
    } else {
        "./bin/timer-server"
    };
    Ok(TimerSelection {
        image: Some(image),
        hpet_mmio_base,
    })
}
