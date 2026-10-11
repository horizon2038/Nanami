//! Slow completion waits use IRQ-or-deadline, not a minimum-duration sleep.
use super::{poll::CompletionPoll, registers::*, ring, Controller, Input, Trb};
use libnanami::{ipc, RequestError, Word, PROCESS_SLOT_NOTIFICATION};
use nanami_services::timer::{timer_service_monotonic_ticks, timer_service_set_alarm_ticks};

impl Controller {
    /// The same completion/interrupt handshake serves command, control and bulk
    /// rings. Minimum hardware delays remain on the private delay notification.
    pub(super) fn wait_completion<T>(
        &mut self,
        milliseconds: Word,
        input: &mut Input,
        mut handle: impl FnMut(&mut Self, &mut Input, Trb) -> Result<Option<T>, RequestError>,
    ) -> Result<T, RequestError> {
        let mut poll = CompletionPoll::new(unsafe { read(self.runtime, MFINDEX) });
        let mut wait = CompletionWait::new(self.timer, milliseconds);
        let mut expired = false;
        self.events.begin(self.op, self.interrupter);
        let result = (|| {
            loop {
                for _ in 0..ring::ENTRIES {
                    let Some(event) = self.event() else { break };
                    if let Some(value) = handle(self, input, event)? {
                        return Ok(value);
                    }
                    if !self.running {
                        return Err(RequestError::Transport);
                    }
                }
                if expired {
                    // The clock RPC can be delayed while the HC completes the TD.
                    // Always drain once after observing expiry before declaring a
                    // timeout. Keep this bounded even under continuous HID input.
                    libnanami::println!(
                        "[usb-server] completion timeout after {} ms",
                        milliseconds
                    );
                    self.fail(input); // Quiesce before the alarm-cancellation RPC.
                    return Err(RequestError::Transport);
                }
                if poll.keep_polling(unsafe { read(self.runtime, MFINDEX) }) {
                    core::hint::spin_loop();
                    continue;
                }
                self.events.finish(self.interrupter);
                input.flush();
                let wake = (|| {
                    if self.interrupts_enabled {
                        if let Some(irq) = self.irq {
                            ipc::interrupt_ack(irq).inspect_err(|error| {
                                libnanami::println!(
                                    "[usb-server] completion IRQ ack failed: {}",
                                    error
                                );
                            })?;
                        }
                    }
                    expired = !wait.prepare(self.interrupts_enabled)?;
                    // Recheck after unmasking and timer RPCs. A completion in this
                    // window must not need a subsequent HID event to wake the server.
                    if !expired && !self.events.ready() {
                        wait.wait()?;
                    }
                    Ok(())
                })();
                if let Err(error) = wake {
                    self.fail(input); // No completion: hardware still owns the TD.
                    return Err(error);
                }
                self.events.begin(self.op, self.interrupter);
            }
        })();
        self.events.finish(self.interrupter);
        let cancel = wait.cancel();
        let result = result.and_then(|value| cancel.map(|()| value));
        if matches!(result, Err(RequestError::Transport)) && self.running {
            // Timeout is not DMA completion. Retain mappings and stop the HC.
            self.fail(input);
        }
        result
    }
}

pub(super) struct CompletionWait {
    timer: Word,
    milliseconds: Word,
    deadline: Option<Word>,
    alarm: Option<Word>,
}

impl CompletionWait {
    pub fn new(timer: Word, milliseconds: Word) -> Self {
        Self {
            timer,
            milliseconds,
            deadline: None,
            alarm: None,
        }
    }

    /// False means the deadline expired; the caller must recheck the event ring.
    pub fn prepare(&mut self, interrupt_driven: bool) -> Result<bool, RequestError> {
        let (now, hz) = timer_service_monotonic_ticks(self.timer).inspect_err(|error| {
            libnanami::println!("[usb-server] completion clock query failed: {}", error);
        })?;
        if hz == 0 {
            return Err(RequestError::Protocol);
        }
        let ticks =
            |ms: Word| ((ms as u128 * hz as u128).div_ceil(1000)).min(Word::MAX as u128) as Word;
        let deadline = *self
            .deadline
            .get_or_insert_with(|| now.saturating_add(ticks(self.milliseconds)));
        if now >= deadline {
            return Ok(false);
        }
        // Without an IRQ (including initial enumeration), recheck at 1 ms.
        // With an IRQ, the alarm is only a timeout; a completion wakes us early.
        let alarm = if interrupt_driven {
            deadline
        } else {
            now.saturating_add(ticks(1)).min(deadline)
        };
        if self.alarm != Some(alarm) {
            timer_service_set_alarm_ticks(self.timer, PROCESS_SLOT_NOTIFICATION, Some(alarm))
                .inspect_err(|error| {
                    libnanami::println!("[usb-server] completion alarm failed: {}", error);
                })?;
            self.alarm = Some(alarm);
        }
        Ok(true)
    }

    pub fn wait(&self) -> Result<(), RequestError> {
        // USB IRQs and maintenance timers share this notification. Every wake
        // means recheck the event ring, never assume the transfer has completed.
        ipc::notification_wait(ipc::process_slot_descriptor(PROCESS_SLOT_NOTIFICATION))
            .inspect_err(|error| {
                libnanami::println!(
                    "[usb-server] completion notification wait failed: {}",
                    error
                );
            })?;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), RequestError> {
        if self.alarm.take().is_some() {
            timer_service_set_alarm_ticks(self.timer, PROCESS_SLOT_NOTIFICATION, None)
                .inspect_err(|error| {
                    libnanami::println!("[usb-server] completion alarm cancel failed: {}", error);
                })?;
        }
        Ok(())
    }
}
