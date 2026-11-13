//! PCI identity policy, independent of spawning, MMIO and firmware access.
use nanami_client::uapi::{intel_graphics, intel_net};

#[derive(Default)]
pub struct Peripherals {
    intel_graphics: bool,
    virtio_graphics: bool,
    intel_network: bool,
    virtio_network: bool,
}

impl Peripherals {
    pub fn observe(&mut self, vendor: u16, device: u16, class: u32) {
        self.intel_graphics |= intel_graphics::generation(vendor, device, class).is_some();
        self.virtio_graphics |= vendor == 0x1af4 && device == 0x1050 && class >> 16 == 3;
        self.intel_network |= intel_net::family(vendor, device, class).is_some();
        self.virtio_network |=
            vendor == 0x1af4 && matches!(device, 0x1000 | 0x1041) && class == 0x020000;
    }

    pub fn display(&self) -> Option<&'static str> {
        if self.virtio_graphics {
            return Some("./bin/virtio-gpu-server");
        }
        if self.intel_graphics {
            // Alpha's 24-byte spawn request accepts an unambiguous basename.
            // It applies policy using the resolved, full archive-entry path.
            return Some("intel-graphics-server");
        }
        None
    }

    pub fn network(&self) -> Option<&'static str> {
        if self.virtio_network {
            return Some("./bin/virtio-net");
        }
        if self.intel_network {
            return Some("./bin/intel-net");
        }
        None
    }
}
