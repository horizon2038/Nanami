//! Preserve unrelated PCI command flags while selecting IRQ or polling mode.
pub(super) fn command_flags(current: u16, interrupt_enabled: bool) -> u16 {
    let mut command = current | (1 << 0) | (1 << 2); // I/O and bus mastering.
    if interrupt_enabled {
        command &= !(1 << 10);
    } else {
        command |= 1 << 10; // Mask INTx when no exclusive IRQ port is available.
    }
    command
}
