use nanami_runtime::{RequestError, Word};

/// For boot dependencies whose provider runs at least at the caller's priority.
/// Yielding must allow that provider to finish registration; this is not a
/// general wait for an optional or lower-priority service (notably on AArch64).
pub fn connect_boot_service(name: &str, slot: Word) -> Result<(), RequestError> {
    for _ in 0..4096 {
        match nanami_runtime::connect_service_by_name(name, slot) {
            Ok(()) => return Ok(()),
            Err(RequestError::Status(status))
                if status == nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT
                    || status == nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR =>
            {
                nanami_runtime::yield_now();
            }
            Err(error) => return Err(error),
        }
    }
    Err(RequestError::Transport)
}
