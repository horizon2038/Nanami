use super::super::TimerSelection;
use nanami_client::{firmware as acpi, pci};
use nanami_runtime::{RequestError, Word};
#[path = "x86_64/usb.rs"]
mod usb;
pub use usb::prepare as prepare_usb_controller;
const SLOT_PCI_SERVICE: Word = 16;

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
    super::super::start_optional("./bin/ps2-server");
    let port = nanami_runtime::ipc::process_slot_descriptor(SLOT_PCI_SERVICE);
    let mut index = 0;
    let mut intel_graphics = false;
    let mut network_started = false;
    while let Some(device) = pci::enumerate(port, index)? {
        intel_graphics |= nanami_client::uapi::intel_graphics::generation(
            device.vendor_id,
            device.device_id,
            device.class,
        )
        .is_some();
        if !network_started
            && device.vendor_id == 0x1af4
            && matches!(device.device_id, 0x1000 | 0x1041)
        {
            super::super::start_optional("./bin/virtio-net");
            network_started = true;
        }
        index += 1;
    }
    if intel_graphics {
        return Ok(Some("./bin/intel-graphics-server"));
    }
    Ok(None)
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
