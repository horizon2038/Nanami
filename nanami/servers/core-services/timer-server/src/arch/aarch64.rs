use libnanami::{RequestError, Word};

// AArch64 user-space timer access is intentionally not exposed. A platform
// driver must own a concrete timer device instead of programming CNTV_* from
// EL0; besides being platform policy, this avoids exposing a timing channel.
pub const TICK_HZ: u64 = 100;
pub const MODE: &str = "unsupported";

pub struct PreparedTimer {
    pub resource: Word,
    pub irq_number: Word,
}

pub fn prepare(_timer_resource_slot: Word) -> Result<PreparedTimer, RequestError> {
    Err(RequestError::Unsupported)
}

impl PreparedTimer {
    pub fn start(&mut self) -> Result<(), RequestError> {
        Err(RequestError::Unsupported)
    }
    pub fn now(&mut self) -> u64 {
        0
    }
    pub fn on_interrupt(&mut self) {}
    pub fn arm(&mut self, _deadline: Option<u64>) -> Result<(), RequestError> {
        Err(RequestError::Unsupported)
    }
}
