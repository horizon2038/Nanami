//! Production event-ring handshake and completion waits, with W1C MMIO and
//! scripted IRQ/timer delivery. No mouse or hotplug wakeups are supplied.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use std::println;
#[path = "../../../nanami/servers/libs/nanami-runtime/src/diagnostics.rs"]
pub mod diagnostics;
use std::{cell::RefCell, collections::VecDeque};
pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 21;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    Transport,
    Protocol,
    Unsupported,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[path = "../../../nanami/servers/core-services/usb-server/src/clock.rs"]
mod clock;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/events.rs"]
mod events;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/poll.rs"]
mod poll;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/ring.rs"]
mod ring;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/wait.rs"]
mod wait;
#[path = "../../../nanami/servers/core-services/usb-server/src/xhci/wait_trace.rs"]
mod wait_trace;
use ring::Trb;

const PHYSICAL: u64 = 0x2000;
const IRQ: Word = 32;
#[derive(Default)]
struct State {
    iman: u32,
    erdp: u64,
    writes: Vec<(usize, u64)>,
    alarms: Vec<Option<Word>>,
    now: Word,
    clocks: usize,
    acks: usize,
    waits: usize,
    memory: usize,
    actions: VecDeque<(Word, Option<Trb>)>,
    during_arm: Option<Trb>,
    during_clock: Option<(usize, Word, Trb)>,
    polling: bool,
    local_counter: usize,
    cancel_error: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }
mod registers {
    use super::*;
    pub const USBSTS: usize = 4;
    pub const USBCMD: usize = 0;
    pub const MFINDEX: usize = 0;
    pub unsafe fn read(base: usize, offset: usize) -> u32 {
        STATE.with_borrow(|s| match (base, offset) {
            (0x100, 0) => s.iman,
            (0x100, 0x18) => s.erdp as u32,
            (0x200, 0) => 16, // Stopped MFINDEX also bounds active polling.
            _ => 0,
        })
    }
    pub unsafe fn write(_: usize, offset: usize, value: u32) {
        STATE.with_borrow_mut(|s| {
            s.writes.push((offset, value as u64));
            if offset == 0 {
                s.iman = (s.iman & !(value & 1)) | (value & 2);
            }
        });
    }
    pub unsafe fn write64(_: usize, offset: usize, value: u64) {
        assert_eq!(offset, 0x18);
        STATE.with_borrow_mut(|s| {
            s.writes.push((offset, value));
            s.erdp = (value & !15) | (s.erdp & 8 & !value); // EHB is W1C.
        });
    }
}
pub mod ipc {
    use super::*;
    pub fn process_slot_descriptor(slot: Word) -> Word {
        slot
    }
    pub fn interrupt_ack(irq: Word) -> Result<(), RequestError> {
        assert_eq!(irq, IRQ);
        STATE.with_borrow_mut(|s| {
            assert_eq!(s.erdp & 8, 0, "unmask before clearing EHB");
            s.acks += 1;
        });
        Ok(())
    }
    pub fn notification_wait(slot: Word) -> Result<Word, RequestError> {
        assert_eq!(
            slot, 21,
            "a completion must not wait on private delay slot 26"
        );
        STATE.with_borrow_mut(|s| {
            s.waits += 1;
            let (now, event) = s.actions.pop_front().expect("wait without a wakeup source");
            s.now = now;
            if s.local_counter != 0 {
                unsafe {
                    (s.local_counter as *mut u64).write_volatile((now as u64) * 100_000);
                }
            }
            if let Some(event) = event {
                unsafe { (s.memory as *mut Trb).write_volatile(event) };
                if s.polling {
                    // DMA completion without interrupt delivery: only the
                    // one-shot polling alarm wakes initial enumeration.
                    return Ok(1 << 63);
                }
                assert!(s.acks != 0, "completion IRQ was not rearmed");
                s.erdp |= 8;
                s.iman |= 1;
                Ok(1 << 16)
            } else {
                Ok(1 << 63)
            }
        })
    }
}
pub mod timer {
    use super::*;
    pub fn timer_service_monotonic_ticks(_: Word) -> Result<(Word, Word), RequestError> {
        STATE.with_borrow_mut(|s| {
            s.clocks += 1;
            if let Some((query, now, event)) = s.during_clock {
                if s.clocks == query {
                    s.now = now;
                    unsafe { (s.memory as *mut Trb).write_volatile(event) };
                    s.during_clock = None;
                }
            }
            Ok((s.now, 1000))
        })
    }
    pub fn timer_service_set_alarm_ticks(
        _: Word,
        slot: Word,
        deadline: Option<Word>,
    ) -> Result<(), RequestError> {
        assert_eq!(slot, 21);
        STATE.with_borrow_mut(|s| {
            s.alarms.push(deadline);
            if deadline.is_none() && s.cancel_error {
                return Err(RequestError::Transport);
            }
            if deadline.is_some() {
                if let Some(event) = s.during_arm.take() {
                    unsafe { (s.memory as *mut Trb).write_volatile(event) };
                }
            }
            Ok(())
        })
    }
}
pub fn request_mmio(_: Word, _: Word) -> Result<(Word, Word), RequestError> {
    Err(RequestError::Unsupported)
}
pub mod device {
    pub fn hpet_counter_base(_: super::Word) -> Result<super::Word, super::RequestError> {
        Err(super::RequestError::Unsupported)
    }
}
struct Input {
    flushes: usize,
}
impl Input {
    fn flush(&mut self) {
        self.flushes += 1;
    }
}
struct Controller {
    events: events::Events,
    op: usize,
    interrupter: usize,
    runtime: usize,
    timer: Word,
    irq: Option<Word>,
    interrupts_enabled: bool,
    running: bool,
    clock: clock::Clock,
    maintenance_started: bool,
    wait_profile: wait::WaitProfile,
    dispatched: Vec<Trb>,
}
impl Controller {
    fn event(&mut self) -> Option<Trb> {
        self.events.pop(self.interrupter)
    }
    fn fail(&mut self, _: &mut Input) {
        self.running = false;
    }
    fn dispatch(&mut self, event: Trb, _: &mut Input) {
        self.dispatched.push(event);
    }
}
fn fixture() -> (Box<[Trb; 256]>, Controller, Input) {
    let mut page = Box::new([Trb::default(); 256]);
    STATE.with_borrow_mut(|s| {
        *s = State {
            iman: 3,
            erdp: PHYSICAL | 8,
            memory: page.as_mut_ptr() as usize,
            ..State::default()
        }
    });
    let controller = Controller {
        events: unsafe { events::Events::new(PHYSICAL, page.as_mut_ptr() as usize) },
        op: 0,
        interrupter: 0x100,
        runtime: 0x200,
        timer: 24,
        irq: Some(IRQ),
        interrupts_enabled: true,
        running: true,
        clock: clock::Clock::rpc(24),
        maintenance_started: false,
        wait_profile: Default::default(),
        dispatched: Vec::new(),
    };
    (page, controller, Input { flushes: 0 })
}
fn completion() -> Trb {
    Trb {
        parameter: 123,
        status: 1 << 24,
        control: (32 << 10) | 1,
    }
}
fn receive(c: &mut Controller, input: &mut Input) -> Result<u64, RequestError> {
    c.wait_completion(1000, input, |_, _, event| Ok(Some(event.parameter)))
}

#[test]
fn erdp_never_acknowledges_an_unexamined_entry() {
    let (mut page, mut c, _) = fixture();
    page[0] = completion();
    page[1] = completion();
    assert!(c.event().is_some());
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.erdp, PHYSICAL));
    assert!(c.events.ready());
    assert!(c.event().is_some());
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.erdp, PHYSICAL + 16));
    assert!(c.event().is_none());
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.erdp, PHYSICAL + 32));
}
#[test]
fn empty_irq_drain_clears_busy_and_pending_but_preserves_enable() {
    let (_page, mut c, _) = fixture();
    c.events.begin(c.op, c.interrupter);
    assert!(c.event().is_none());
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| {
        assert_eq!(s.erdp & 8, 0);
        assert_eq!(s.iman, 2);
    });
}
#[test]
fn enumeration_does_not_enable_irqs_implicitly() {
    let (_page, c, _) = fixture();
    STATE.with_borrow_mut(|s| s.iman = 1);
    c.events.begin(c.op, c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.iman, 0));
}
#[test]
fn erdp_wrap_uses_last_consumed_until_new_cycle_is_checked() {
    let (mut page, mut c, _) = fixture();
    page.fill(completion());
    for _ in 0..256 {
        assert!(c.event().is_some());
    }
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.erdp, PHYSICAL + 255 * 16));
    assert!(!c.events.ready());
    assert!(c.event().is_none());
    c.events.finish(c.interrupter);
    STATE.with_borrow(|s| assert_eq!(s.erdp, PHYSICAL));
    page[0].control &= !1;
    assert!(c.event().is_some());
}
#[test]
fn fast_completion_needs_no_clock_or_timer_rpc() {
    let (mut page, mut c, mut input) = fixture();
    page[0] = completion();
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| {
        assert_eq!(s.clocks, 0);
        assert!(s.alarms.is_empty());
        assert_eq!(s.waits, 0);
        assert_eq!(s.erdp & 8, 0);
    });
}
#[test]
fn completion_irq_wakes_wait_without_mouse_or_timer_expiry() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| s.actions.push_back((2, Some(completion()))));
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| {
        assert_eq!(s.waits, 1);
        assert_eq!(s.alarms, [Some(1000), None]);
    });
    assert_eq!(input.flushes, 1);
}
#[test]
fn completion_arriving_during_alarm_rpc_is_rechecked_before_sleep() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| s.during_arm = Some(completion()));
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| assert_eq!(s.waits, 0));
}
#[test]
fn completion_arriving_during_expiry_query_is_not_discarded() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| {
        s.actions.push_back((999, None));
        s.during_clock = Some((2, 1000, completion()));
    });
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    assert!(c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.waits, 1);
        assert_eq!(s.alarms, [Some(1000), None]);
    });
}
#[test]
fn enumeration_completion_uses_timer_without_irqs_or_hid() {
    let (_page, mut c, mut input) = fixture();
    c.interrupts_enabled = false;
    STATE.with_borrow_mut(|s| {
        s.polling = true;
        s.iman = 0;
        s.actions.extend([(1, None), (2, Some(completion()))]);
    });
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| {
        assert_eq!(s.acks, 0);
        assert_eq!(s.iman, 0);
        assert_eq!(s.alarms, [Some(1), Some(2), None]);
        assert_eq!(s.waits, 2);
    });
}
#[test]
fn expired_polling_wait_also_rechecks_completion_after_clock_rpc() {
    let (_page, mut c, mut input) = fixture();
    c.interrupts_enabled = false;
    STATE.with_borrow_mut(|s| {
        s.iman = 0;
        s.actions.push_back((999, None));
        s.during_clock = Some((2, 1000, completion()));
    });
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    assert!(c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.acks, 0);
        assert_eq!(s.alarms, [Some(1), None]);
    });
}
#[test]
fn unrelated_event_at_expiry_cannot_extend_the_wait() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| {
        s.actions.push_back((999, None));
        s.during_clock = Some((2, 1000, completion()));
    });
    let mut events = 0;
    assert_eq!(
        c.wait_completion(1000, &mut input, |_, _, _| {
            events += 1;
            Ok(None::<()>)
        }),
        Err(RequestError::Transport)
    );
    assert_eq!(events, 1);
    assert!(!c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.waits, 1);
        assert_eq!(s.alarms, [Some(1000), None]);
    });
}
#[test]
fn unrelated_wakeups_do_not_extend_timeout() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| s.actions.extend([(100, None), (900, None), (1000, None)]));
    assert_eq!(receive(&mut c, &mut input), Err(RequestError::Transport));
    assert!(!c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.waits, 3);
        assert_eq!(s.alarms, [Some(1000), None]);
    });
}
#[test]
fn polling_fallback_arms_a_short_alarm_without_extending_deadline() {
    let (_page, _c, _) = fixture();
    let mut wait = wait::CompletionWait::new(clock::Clock::rpc(24), 24, 1000, false);
    wait.prepare(false).unwrap();
    STATE.with_borrow_mut(|s| s.now = 999);
    wait.prepare(false).unwrap();
    STATE.with_borrow_mut(|s| s.now = 1000);
    assert_eq!(wait.prepare(false), Ok(false));
    wait.cancel().unwrap();
    STATE.with_borrow(|s| assert_eq!(s.alarms, [Some(1), Some(1000), None]));
}

fn local_clock(c: &mut Controller) -> Box<[u64; 512]> {
    let mut memory = Box::new([0; 512]);
    memory[0] = (10_000_000u64 << 32) | (1 << 13);
    memory[0x10 / 8] = 1;
    c.clock = unsafe { clock::Clock::test_counter(memory.as_ptr() as usize, 24) }.unwrap();
    STATE.with_borrow_mut(|s| s.local_counter = memory.as_mut_ptr() as usize + 0xf0);
    memory
}

#[test]
fn normal_transfer_uses_maintenance_timeout_checks_without_any_timer_rpc() {
    let (_page, mut c, mut input) = fixture();
    let _counter = local_clock(&mut c);
    c.start_maintenance();
    STATE.with_borrow_mut(|s| s.actions.extend([(10, None), (20, Some(completion()))]));
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| {
        assert_eq!(s.clocks, 0);
        assert!(s.alarms.is_empty());
        assert_eq!(s.waits, 2);
        assert!(s.acks >= 2);
    });
}

#[test]
fn periodic_wakes_bound_timeout_without_usb_interrupt_or_mouse_input() {
    let (_page, mut c, mut input) = fixture();
    let _counter = local_clock(&mut c);
    c.start_maintenance();
    c.interrupts_enabled = false;
    STATE.with_borrow_mut(|s| s.actions.extend([(10, None), (999, None), (1000, None)]));
    assert_eq!(receive(&mut c, &mut input), Err(RequestError::Transport));
    assert!(!c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.clocks, 0);
        assert!(s.alarms.is_empty());
        assert_eq!(s.waits, 3);
    });
}

#[test]
fn long_wait_diagnostics_preserve_completion_and_do_not_add_timer_rpc() {
    let (_page, mut c, mut input) = fixture();
    let _counter = local_clock(&mut c);
    c.start_maintenance();
    c.interrupts_enabled = true;
    STATE.with_borrow_mut(|s| {
        s.actions
            .extend([(10, None), (150, None), (200, Some(completion()))])
    });
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    assert!(c.running);
    STATE.with_borrow(|s| {
        assert_eq!(s.clocks, 0);
        assert!(s.alarms.is_empty());
        assert_eq!(s.waits, 3);
    });
}

#[test]
fn endpoint_snapshot_is_read_only_and_does_not_pop_or_ack() {
    let (mut page, c, _input) = fixture();
    page[0] = completion();
    let context = [1u32, 0, 0x12345671, 0];
    let original = context;
    let position = c.events.diagnostic_position();
    let writes = STATE.with_borrow(|s| s.writes.len());
    let target = wait_trace::Target {
        phase: "data",
        slot: 1,
        dci: 3,
        td: 0x1234,
        bytes: 4096,
        context: context.as_ptr() as usize,
    };
    let _snapshot =
        wait_trace::Snapshot::capture(c.op, c.interrupter, c.runtime, &c.events, Some(target));
    assert_eq!(context, original);
    assert_eq!(c.events.diagnostic_position(), position);
    assert_eq!(page[0], completion());
    STATE.with_borrow(|s| {
        assert_eq!(s.writes.len(), writes);
        assert_eq!(s.acks, 0);
        assert_eq!(s.clocks, 0);
    });
}

#[test]
fn fast_completion_rearms_irq_before_returning_to_block_service() {
    let (mut page, mut c, mut input) = fixture();
    page[0] = completion();
    assert_eq!(receive(&mut c, &mut input), Ok(123));
    STATE.with_borrow(|s| assert_eq!(s.acks, 1));
}

#[test]
fn fast_return_acknowledges_new_ip_and_drains_already_queued_hid_events() {
    let (mut page, mut c, mut input) = fixture();
    page[0] = completion();
    let mut hid = completion();
    hid.parameter = 777;
    page[1] = hid;
    // Simulate an IP asserted after the initial drain begins, while the TD
    // completion handler processes its result.
    assert_eq!(
        c.wait_completion(1000, &mut input, |_, _, event| {
            STATE.with_borrow_mut(|s| s.iman |= 1);
            Ok(Some(event.parameter))
        }),
        Ok(123)
    );
    assert_eq!(c.dispatched, [hid]);
    STATE.with_borrow(|s| {
        assert_eq!(s.iman & 1, 0);
        assert_eq!(s.erdp, PHYSICAL + 32);
        assert_eq!(s.acks, 1);
        assert_eq!(s.waits, 0);
    });
}

#[test]
fn pipe_timeout_recovery_preserves_other_devices_and_failure_stops_controller() {
    for recovery_fails in [false, true] {
        let (_page, mut c, mut input) = fixture();
        STATE.with_borrow_mut(|s| s.actions.push_back((1000, None)));
        let mut recovered = 0;
        assert!(c
            .wait_completion_recovering(
                1000,
                &mut input,
                |_, _, _| Ok(None::<()>),
                |this, _| {
                    assert!(
                        this.running,
                        "stopped controller before attempting pipe recovery"
                    );
                    recovered += 1;
                    if recovery_fails {
                        Err(RequestError::Protocol)
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err());
        assert_eq!(recovered, 1);
        assert_eq!(c.running, !recovery_fails);
    }
}

#[test]
fn alarm_cancel_failure_after_completion_does_not_revoke_dma_ownership() {
    let (_page, mut c, mut input) = fixture();
    STATE.with_borrow_mut(|s| {
        s.actions.push_back((2, Some(completion())));
        s.cancel_error = true;
    });
    assert_eq!(receive(&mut c, &mut input), Err(RequestError::Transport));
    assert!(c.running);
}
