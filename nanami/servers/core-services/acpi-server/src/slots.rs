//! Host callbacks and shutdown share the same process-local capability space.
pub const IO_SLOT: usize = 16;
pub const HIGH_IO_SLOT: usize = 17;
pub const SERVICE_SLOT: usize = 20;
pub const TIMER_SLOT: usize = 22;
pub const PCI_SLOT: usize = 23;
pub const DEVICE_MANAGER_SLOT: usize = 24;
pub const VFS_SLOT: usize = 25;

// PCI remains connected while shutdown freezes VFS and evaluates _PTS/_GTS.
const _: () = assert!(VFS_SLOT != PCI_SLOT);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_shutdown_slots_do_not_overlap() {
        let slots = [
            IO_SLOT, HIGH_IO_SLOT, SERVICE_SLOT, TIMER_SLOT, PCI_SLOT,
            DEVICE_MANAGER_SLOT, VFS_SLOT,
        ];
        for (index, slot) in slots.iter().enumerate() {
            assert!((16..256).contains(slot));
            assert_ne!(*slot, 21, "slot 21 is the default process notification");
            assert!(!slots[..index].contains(slot), "duplicate capability slot {slot}");
        }
    }
}
