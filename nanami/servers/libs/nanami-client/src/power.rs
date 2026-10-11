//! Userspace power management. Currently provided by acpi-server on x86_64.
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
pub use nanami_uapi::power::*;

/// Flush and freeze storage, then enter ACPI S5. Success does not return.
pub fn shutdown(port: Word) -> Result<(), RequestError> {
    let (status, _, _) = call_port(port, POWER_REQUEST_SHUTDOWN, 0, 0, 0, 0, 1)?;
    if status == OS_RESPONSE_OK {
        // A completed power-off cannot reply.
        Err(RequestError::Protocol)
    } else {
        Err(RequestError::Status(status))
    }
}
