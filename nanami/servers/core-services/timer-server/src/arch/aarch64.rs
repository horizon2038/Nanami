use libnanami::{RequestError, Word};

// AArch64 user-space timer access is intentionally not exposed. A platform
// driver must own a concrete timer device instead of programming CNTV_* from
// EL0; besides being platform policy, this avoids exposing a timing channel.
pub const TICK_HZ: u64 = 100;

pub struct PreparedTimer {
    pub resource: Word,
    pub irq_number: Word,
}

pub fn prepare(_timer_resource_slot: Word) -> Result<PreparedTimer, RequestError> {
    Err(RequestError::Unsupported)
}

pub fn start(_timer_resource: Word) -> Result<(), RequestError> {
    Err(RequestError::Unsupported)
}

pub fn rearm() -> Result<(), RequestError> {
    Err(RequestError::Unsupported)
}
