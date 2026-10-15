//! Slow completion waits use IRQ-or-deadline, not a minimum-duration sleep.
use super::wait_trace::{Snapshot, Target};
use super::{poll::CompletionPoll, registers::*, ring, Controller, Input, Trb};
use crate::clock::Clock;
use nanami_client::timer::timer_service_set_alarm_ticks;
use nanami_runtime::{ipc, RequestError, Word, PROCESS_SLOT_NOTIFICATION};

impl Controller {
    /// The same completion/interrupt handshake serves command, control and bulk
    /// rings. Minimum hardware delays remain on the private delay notification.
    pub(super) fn wait_completion<T>(
        &mut self,
        milliseconds: Word,
        input: &mut Input,
        handle: impl FnMut(&mut Self, &mut Input, Trb) -> Result<Option<T>, RequestError>,
    ) -> Result<T, RequestError> {
        self.wait_completion_recovering(milliseconds, input, handle, |this, input| {
            this.fail(input);
            Ok(())
        })
    }

    pub fn start_maintenance(&mut self) {
        self.maintenance_started = true;
    }

    pub fn profile_clock(&self) -> Option<Word> {
        self.clock.local_nanoseconds()
    }

    pub fn print_wait_profile(&mut self) {
        let p = core::mem::take(&mut self.wait_profile);
        nanami_runtime::println!(
            "[usb.wait] transfers={} slow={} irq-wakes={} timer-wakes={} empty-wakes={} total-us={} max-us={} sleep-us={} prepare-us={} local-clock={}",
            p.transfers, p.slow, p.irqs, p.timers, p.empty,
            p.total_ns / 1000, p.max_ns / 1000, p.sleep_ns / 1000,
            p.prepare_ns / 1000, self.clock.local_nanoseconds().is_some()
        );
    }

    pub(super) fn wait_completion_recovering<T>(
        &mut self,
        milliseconds: Word,
        input: &mut Input,
        handle: impl FnMut(&mut Self, &mut Input, Trb) -> Result<Option<T>, RequestError>,
        recover_timeout: impl FnMut(&mut Self, &mut Input) -> Result<(), RequestError>,
    ) -> Result<T, RequestError> {
        self.wait_completion_traced(milliseconds, input, handle, recover_timeout, None)
    }

    pub(super) fn wait_completion_target<T>(
        &mut self,
        milliseconds: Word,
        input: &mut Input,
        target: Target,
        handle: impl FnMut(&mut Self, &mut Input, Trb) -> Result<Option<T>, RequestError>,
    ) -> Result<T, RequestError> {
        self.wait_completion_traced(
            milliseconds,
            input,
            handle,
            |this, input| {
                this.fail(input);
                Ok(())
            },
            Some(target),
        )
    }

    fn wait_completion_traced<T>(
        &mut self,
        milliseconds: Word,
        input: &mut Input,
        mut handle: impl FnMut(&mut Self, &mut Input, Trb) -> Result<Option<T>, RequestError>,
        mut recover_timeout: impl FnMut(&mut Self, &mut Input) -> Result<(), RequestError>,
        target: Option<Target>,
    ) -> Result<T, RequestError> {
        let profile = option_env!("NANAMI_USB_WAIT_TRACE") == Some("1");
        let diagnostic = nanami_runtime::diagnostics::ENABLED;
        let started = (profile || diagnostic)
            .then(|| self.clock.local_nanoseconds())
            .flatten();
        let mut stalled = None;
        if profile {
            self.wait_profile.transfers += 1;
        }
        let mut poll = CompletionPoll::new(unsafe { read(self.runtime, MFINDEX) });
        let mut wait = CompletionWait::new(
            self.clock,
            self.timer,
            milliseconds,
            self.maintenance_started,
        );
        let mut expired = false;
        let mut recovered = false;
        let mut completed = false;
        let mut slow = false;
        let mut woke = false;
        self.events.begin(self.op, self.interrupter);
        let result = (|| {
            loop {
                let mut events = 0;
                for _ in 0..ring::ENTRIES {
                    let Some(event) = self.event() else { break };
                    events += 1;
                    if let Some(value) = handle(self, input, event)? {
                        completed = true;
                        return Ok(value);
                    }
                    if !self.running {
                        return Err(RequestError::Transport);
                    }
                }
                if profile && woke && events == 0 {
                    self.wait_profile.empty += 1;
                }
                woke = false;
                if expired {
                    // The clock RPC can be delayed while the HC completes the TD.
                    // Always drain once after observing expiry before declaring a
                    // timeout. Keep this bounded even under continuous HID input.
                    nanami_runtime::println!(
                        "[usb-server] completion timeout after {} ms",
                        milliseconds
                    );
                    // A control pipe can be stopped independently. Commands and
                    // bulk transfers retain the conservative controller stop.
                    if let Err(error) = recover_timeout(self, input) {
                        self.fail(input);
                        return Err(error);
                    }
                    recovered = true;
                    return Err(RequestError::Transport);
                }
                if poll.keep_polling(unsafe { read(self.runtime, MFINDEX) }) {
                    core::hint::spin_loop();
                    continue;
                }
                if diagnostic && stalled.is_none() {
                    if let Some(elapsed) = started
                        .zip(self.clock.local_nanoseconds())
                        .map(|(before, after)| after.saturating_sub(before))
                        .filter(|elapsed| *elapsed >= 100_000_000)
                    {
                        stalled = Some((
                            elapsed / 1000,
                            Snapshot::capture(
                                self.op,
                                self.interrupter,
                                self.runtime,
                                &self.events,
                                target,
                            ),
                        ));
                    }
                }
                // Events can assert IP while the ring is being drained. Clear
                // the device cause before EHB/unmask; the post-unmask ready()
                // check below closes a completion racing this acknowledgement.
                self.events.begin(self.op, self.interrupter);
                self.events.finish(self.interrupter);
                input.flush();
                if profile && !slow {
                    self.wait_profile.slow += 1;
                }
                slow = true;
                let wake = (|| {
                    if self.interrupts_enabled {
                        if let Some(irq) = self.irq {
                            ipc::interrupt_ack(irq).inspect_err(|error| {
                                nanami_runtime::println!(
                                    "[usb-server] completion IRQ ack failed: {}",
                                    error
                                );
                            })?;
                        }
                    }
                    let before = profile.then(|| self.clock.local_nanoseconds()).flatten();
                    expired = !wait.prepare(self.interrupts_enabled)?;
                    if let (Some(before), Some(after)) = (
                        before,
                        profile.then(|| self.clock.local_nanoseconds()).flatten(),
                    ) {
                        self.wait_profile.prepare_ns += after.saturating_sub(before) as u64;
                    }
                    // Recheck after unmasking and timer RPCs. A completion in this
                    // window must not need a subsequent HID event to wake the server.
                    if !expired && !self.events.ready() {
                        let before = profile.then(|| self.clock.local_nanoseconds()).flatten();
                        let badge = wait.wait()?;
                        woke = true;
                        if profile {
                            self.wait_profile.irqs += u64::from(badge & !(1 << 63) != 0);
                            self.wait_profile.timers += u64::from(badge & (1 << 63) != 0);
                            if let (Some(before), Some(after)) =
                                (before, self.clock.local_nanoseconds())
                            {
                                self.wait_profile.sleep_ns += after.saturating_sub(before) as u64;
                            }
                        }
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
        if self.running {
            // A fast completion can arrive AFTER the initial IP acknowledgement.
            // Finish the device handshake too, not just the kernel mask. Drain
            // already queued HID/port events so clearing IP cannot strand them.
            self.events.begin(self.op, self.interrupter);
            for _ in 0..ring::ENTRIES {
                let Some(event) = self.event() else { break };
                self.dispatch(event, input);
                if !self.running {
                    break;
                }
            }
        }
        self.events.finish(self.interrupter);
        // A fast completion may consume the event before the IRQ notification
        // reaches the main loop. Do not leave the source masked until unrelated
        // input or a maintenance tick happens to rearm it.
        let ack = if self.running && self.interrupts_enabled {
            self.irq.map_or(Ok(()), ipc::interrupt_ack)
        } else {
            Ok(())
        };
        let cancel = wait.cancel();
        let result = result.and_then(|value| {
            ack.and(cancel)?;
            if self.running {
                Ok(value)
            } else {
                Err(RequestError::Transport)
            }
        });
        if matches!(result, Err(RequestError::Transport))
            && self.running
            && !completed
            && !recovered
        {
            // Timeout is not DMA completion. Retain mappings and stop the HC.
            self.fail(input);
        }
        if let (Some(before), Some(after)) = (
            started,
            profile.then(|| self.clock.local_nanoseconds()).flatten(),
        ) {
            let elapsed = after.saturating_sub(before) as u64;
            self.wait_profile.total_ns += elapsed;
            self.wait_profile.max_ns = self.wait_profile.max_ns.max(elapsed);
        }
        let duration = diagnostic
            .then(|| {
                started
                    .zip(self.clock.local_nanoseconds())
                    .map_or(0, |(before, after)| after.saturating_sub(before) / 1000)
            })
            .unwrap_or(0);
        if duration >= 100_000 {
            nanami_runtime::println!("[pipe.td] phase={} slot={} ep={} td={:#x} bytes={} wait-us={} sampled-at-us={} success={}",
                target.map_or("command/control", |t| t.phase), target.map_or(0, |t| t.slot),
                target.map_or(0, |t| t.dci), target.map_or(0, |t| t.td),
                target.map_or(0, |t| t.bytes), duration, stalled.map_or(0, |(at, _)| at), result.is_ok());
            if let Some((_, snapshot)) = stalled {
                snapshot.report("after-100ms");
            }
            Snapshot::capture(
                self.op,
                self.interrupter,
                self.runtime,
                &self.events,
                target,
            )
            .report("finished");
        }
        result
    }
}

pub(super) struct CompletionWait {
    clock: Clock,
    timer: Word,
    milliseconds: Word,
    periodic_wake: bool,
    deadline: Option<Word>,
    alarm: Option<Word>,
}

impl CompletionWait {
    pub fn new(clock: Clock, timer: Word, milliseconds: Word, periodic_wake: bool) -> Self {
        Self {
            clock,
            timer,
            milliseconds,
            periodic_wake,
            deadline: None,
            alarm: None,
        }
    }

    /// False means the deadline expired; the caller must recheck the event ring.
    pub fn prepare(&mut self, interrupt_driven: bool) -> Result<bool, RequestError> {
        let (now, hz) = self.clock.now().inspect_err(|error| {
            nanami_runtime::println!("[usb-server] completion clock query failed: {}", error);
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
        // The permanent 10-ms maintenance notification is independent of all
        // alarms and wakes this exact bound notification. Once registered, it
        // bounds timeout checks even if an IRQ is lost. No per-TD timer arm or
        // cancellation RPC is necessary. Enumeration still uses 1-ms alarms.
        if self.periodic_wake {
            return Ok(true);
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
                    nanami_runtime::println!("[usb-server] completion alarm failed: {}", error);
                })?;
            self.alarm = Some(alarm);
        }
        Ok(true)
    }

    pub fn wait(&self) -> Result<Word, RequestError> {
        // USB IRQs and maintenance timers share this notification. Every wake
        // means recheck the event ring, never assume the transfer has completed.
        ipc::notification_wait(ipc::process_slot_descriptor(PROCESS_SLOT_NOTIFICATION)).inspect_err(
            |error| {
                nanami_runtime::println!(
                    "[usb-server] completion notification wait failed: {}",
                    error
                );
            },
        )
    }

    pub fn cancel(&mut self) -> Result<(), RequestError> {
        if self.alarm.take().is_some() {
            timer_service_set_alarm_ticks(self.timer, PROCESS_SLOT_NOTIFICATION, None)
                .inspect_err(|error| {
                    nanami_runtime::println!(
                        "[usb-server] completion alarm cancel failed: {}",
                        error
                    );
                })?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct WaitProfile {
    transfers: u64,
    slow: u64,
    irqs: u64,
    timers: u64,
    empty: u64,
    total_ns: u64,
    max_ns: u64,
    sleep_ns: u64,
    prepare_ns: u64,
}
