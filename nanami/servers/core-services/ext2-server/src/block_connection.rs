use libnanami::{RequestError, Word, OS_RESPONSE_INVALID_ARGUMENT};

use crate::{log_request_error, SLOT_BLOCK_DEVICE, SLOT_TIMER_SERVICE};

// USB firmware handoff, enumeration and removable-media readiness precede
// root publication. This remains a bounded, sleeping startup wait.
const BLOCK_CONNECT_WAITS: usize = 600;
const BLOCK_CONNECT_RETRY_MS: Word = 100;

pub(super) fn connect_block_device() -> Result<Word, RequestError> {
    let mut timer_port = None;
    let mut completed_waits = 0;
    let mut logged_wait = false;
    loop {
        // Storage may become available before the platform timer does.
        match nanami_services::registry::connect_block_device_with_pid(SLOT_BLOCK_DEVICE) {
            Ok(_) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_BLOCK_DEVICE)),
            Err(error) => {
                if !logged_wait {
                    log_request_error("[ext2-server] waiting block-device: ", error);
                    logged_wait = true;
                }
                // The registry reports an as-yet unregistered service this way.
                // Capability/transport errors are not startup timing failures.
                if !matches!(error, RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT))
                    || completed_waits >= BLOCK_CONNECT_WAITS
                {
                    return Err(error);
                }
            }
        }

        if timer_port.is_none() {
            match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
                Ok(()) => {
                    timer_port = Some(libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE));
                }
                Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT)) => {}
                Err(error) => {
                    log_request_error("[ext2-server] timer connect failed: ", error);
                    return Err(error);
                }
            }
        }
        if let Some(timer) = timer_port {
            if let Err(error) = nanami_services::timer::timer_service_sleep_milliseconds(
                timer,
                BLOCK_CONNECT_RETRY_MS,
            ) {
                log_request_error("[ext2-server] startup delay failed: ", error);
                return Err(error);
            }
            completed_waits += 1;
        } else {
            // Without a timer there is no elapsed-time budget to consume.
            // Yield to the boot drivers, then retry *both* service lookups.
            libnanami::yield_now();
        }
    }
}
