//! Initialization waits must block: PS/2 runs above the filesystem's priority.
use nanami_runtime::{RequestError, Word};

const SLOT_TIMER: Word = 26;
const SLOT_DELAY: Word = 27;

pub fn initialize() -> Result<(), RequestError> {
    nanami_client::registry::connect_timer_service(SLOT_TIMER)?;
    // Keep timer completion separate from keyboard/mouse IRQ notifications.
    nanami_runtime::request_notification_port_create(
        SLOT_DELAY,
        nanami_client::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )
}

pub fn sleep(milliseconds: Word) -> Result<(), RequestError> {
    nanami_client::timer::timer_service_sleep_on_notification_milliseconds(
        nanami_runtime::ipc::process_slot_descriptor(SLOT_TIMER),
        milliseconds,
        SLOT_DELAY,
    )
}

pub struct Wait {
    stage: &'static str,
    milliseconds: Word,
    clock: Option<(Word, Word)>,
}

impl Wait {
    pub fn new(stage: &'static str, milliseconds: Word) -> Self {
        Self {
            stage,
            milliseconds,
            clock: None,
        }
    }

    pub fn pause(&mut self, status: Word) -> Result<(), RequestError> {
        let (now, hz) = nanami_client::timer::timer_service_monotonic_ticks(
            nanami_runtime::ipc::process_slot_descriptor(SLOT_TIMER),
        )?;
        if hz == 0 {
            return Err(RequestError::Protocol);
        }
        let (start, limit) = *self.clock.get_or_insert_with(|| {
            (
                now,
                ((hz as u128 * self.milliseconds as u128).div_ceil(1000)) as Word,
            )
        });
        if now.wrapping_sub(start) >= limit {
            nanami_runtime::println!(
                "[ps2-server] timeout stage={} status={:#x}",
                self.stage,
                status
            );
            return Err(RequestError::Transport);
        }
        sleep(1)
    }
}
