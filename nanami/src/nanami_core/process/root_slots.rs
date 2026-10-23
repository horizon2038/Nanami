//! Provenance for the flat Native root owned by Alpha. Never discover an
//! unknown capability's type by sending it a type-specific operation number.
//! Foreign revocation may empty a tracked slot without changing it to another
//! object type. Thus type provenance is not an exact occupancy oracle.
use nun::CapabilityError;

pub fn install_notification_alias(
    known_notification: bool,
    mut copy: impl FnMut() -> Result<(), CapabilityError>,
    mut remove: impl FnMut() -> Result<(), CapabilityError>,
) -> Result<(), CapabilityError> {
    if copy().is_ok() {
        return Ok(());
    }
    if !known_notification {
        return Err(CapabilityError::PermissionDenied);
    }
    remove()?;
    copy()
}

#[derive(Clone, Copy)]
pub struct RootSlotTypes {
    slots: [u64; 8],
}

impl RootSlotTypes {
    pub const fn empty() -> Self {
        Self { slots: [0; 8] }
    }
    pub fn native() -> Self {
        let mut types = Self::empty();
        for slot in 1..8 {
            types.other(slot);
        }
        types.notification(21);
        types.ipc(20);
        types
    }
    pub fn clear(&mut self, slot: usize) {
        self.set(slot, 0);
    }
    fn set(&mut self, slot: usize, kind: u64) {
        if slot < 256 {
            let shift = 2 * (slot % 32);
            self.slots[slot / 32] = (self.slots[slot / 32] & !(3 << shift)) | (kind << shift);
        }
    }
    fn kind(&self, slot: usize) -> u64 {
        if slot >= 256 {
            return 0;
        }
        (self.slots[slot / 32] >> (2 * (slot % 32))) & 3
    }
    pub fn notification(&mut self, slot: usize) {
        self.set(slot, 1);
    }
    pub fn ipc(&mut self, slot: usize) {
        self.set(slot, 2);
    }
    pub fn other(&mut self, slot: usize) {
        self.set(slot, 3);
    }
    pub fn is_notification(&self, slot: usize) -> bool {
        self.kind(slot) == 1
    }
    pub fn is_ipc(&self, slot: usize) -> bool {
        self.kind(slot) == 2
    }
    pub fn occupied(&self, slot: usize) -> bool {
        self.kind(slot) != 0
    }
}
