//! PCI bridge INTx swizzling. Every hop is discovered; no chipset PIRQ guesses.
use nanami_client::uapi::pci::Address;
#[derive(Clone, Copy)]
pub struct Bridge {
    pub address: Address,
    pub primary: u8,
    pub secondary: u8,
    pub subordinate: u8,
}
pub fn resolve(
    mut address: Address,
    mut pin: u8,
    mut bridge_at: impl FnMut(usize) -> Option<Bridge>,
    mut route: impl FnMut(Address, u8) -> Option<usize>,
) -> Option<usize> {
    if !(1..=4).contains(&pin) {
        return None;
    }
    for _ in 0..32 {
        // The ACPI service rejects non-root buses; try it at every discovered
        // hop so nonzero ACPI root buses work without assuming bus zero.
        if let Some(irq) = route(address, pin) {
            return Some(irq);
        }
        let mut parent = None;
        let mut index = 0;
        while let Some(bridge) = bridge_at(index) {
            index += 1;
            if bridge.address.segment == address.segment && bridge.secondary == address.bus {
                if parent.is_some()
                    || bridge.primary != bridge.address.bus
                    || bridge.secondary <= bridge.primary
                    || bridge.subordinate < address.bus
                {
                    return None;
                }
                parent = Some(bridge.address);
            }
        }
        let parent = parent?;
        pin = ((pin - 1 + address.device) & 3) + 1;
        address = parent;
    }
    None
}
