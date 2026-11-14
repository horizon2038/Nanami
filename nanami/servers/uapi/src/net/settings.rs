//! Versioned IPv4 settings shared by the service, CLI and native GUI.
use crate::Word;

pub const IPV4_MODE_DHCP: Word = 0;
pub const IPV4_MODE_MANUAL: Word = 1;
pub const IPV4_STATE_UNCONFIGURED: Word = 0;
pub const IPV4_STATE_ACQUIRING: Word = 1;
pub const IPV4_STATE_READY: Word = 2;
pub const IPV4_STATE_FAILED: Word = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Settings {
    pub mode: Word,
    pub address: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
}
impl Ipv4Settings {
    pub const DEFAULT: Self = Self {
        mode: IPV4_MODE_DHCP,
        address: [0; 4],
        netmask: [0; 4],
        gateway: [0; 4],
        dns: [0; 4],
    };
    /// Draft fields may be empty until Reload/Apply; malformed fields may not.
    pub fn validate_draft(&self) -> Result<(), &'static str> {
        if self.mode > IPV4_MODE_MANUAL {
            return Err("Unknown IP mode");
        }
        for address in [self.address, self.gateway, self.dns] {
            if address != [0; 4] && !unicast(address) {
                return Err("Use a unicast IPv4 address (or 0.0.0.0 for an optional field)");
            }
        }
        if self.netmask != [0; 4] && !valid_netmask(self.netmask) {
            return Err("Subnet mask must contain contiguous leading one bits");
        }
        Ok(())
    }
    pub fn validate_apply(&self) -> Result<(), &'static str> {
        self.validate_draft()?;
        if self.mode == IPV4_MODE_DHCP {
            return Ok(());
        }
        if !unicast(self.address) || !valid_netmask(self.netmask) {
            return Err("Manual mode requires an IP address and subnet mask");
        }
        let host = pack_ipv4(self.address) & !pack_ipv4(self.netmask);
        let host_mask = !pack_ipv4(self.netmask);
        if host_mask > 1 && (host == 0 || host == host_mask) {
            return Err("IP address must not be the subnet or broadcast address");
        }
        if self.gateway != [0; 4] {
            let gateway = pack_ipv4(self.gateway);
            let mask = pack_ipv4(self.netmask);
            if self.gateway == self.address || gateway & mask != pack_ipv4(self.address) & mask {
                return Err("Default gateway must be a different address on the same subnet");
            }
            let host = gateway & !mask;
            if host_mask > 1 && (host == 0 || host == host_mask) {
                return Err("Default gateway must not be the subnet or broadcast address");
            }
        }
        Ok(())
    }
}
pub const fn pack_ipv4(address: [u8; 4]) -> u32 {
    u32::from_be_bytes(address)
}
pub const fn unpack_ipv4(address: Word) -> [u8; 4] {
    (address as u32).to_be_bytes()
}
pub fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let mut result = [0; 4];
    let mut parts = text.split('.');
    for byte in &mut result {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *byte = part.parse::<u8>().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(result)
}
pub fn valid_netmask(mask: [u8; 4]) -> bool {
    let mask = pack_ipv4(mask);
    mask != 0 && (!mask).wrapping_add(1).is_power_of_two()
}
fn unicast(address: [u8; 4]) -> bool {
    address[0] != 0 && address[0] != 127 && address[0] < 224
}
