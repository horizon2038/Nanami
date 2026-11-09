//! Actual signal-return dispatcher and x86 encoder; delivery/task I/O modeled.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::{Cell, RefCell};
pub type Word = usize;
type HardwareContext = [Word; 23];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
    program_counter: Word,
}
mod state {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum OsPersonality { Linux, FreeBsd }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/arch/x86_64/process.rs"]
mod encoder;
use encoder::{configure_personality_syscall_reply, configure_trapped_syscall_reply};
thread_local! {
    static READS: Cell<usize> = const { Cell::new(0) };
    static READ_RESULT: RefCell<Result<HardwareContext, ()>> = const { RefCell::new(Ok([0; 23])) };
}
mod process {
    pub(super) use super::{HardwareContext, LinuxSyscallContext};
    pub(super) fn read_hardware_context(pcb: usize) -> Result<HardwareContext, ()> {
        assert_eq!(pcb, 107);
        super::READS.with(|count| count.set(count.get() + 1));
        super::READ_RESULT.with(|value| *value.borrow())
    }
}
mod linux {
    pub mod signals_runtime {
        #[derive(Clone, Copy)]
        pub enum Delivery { Unchanged, Handler, Stopped, Terminate(usize) }
        pub fn deliver(r: &mut super::super::Runtime, pid: usize,
            registers: &mut super::super::HardwareContext,
            interrupted: Option<super::super::LinuxSyscallContext>) -> Delivery {
            assert_eq!(pid, 7);
            r.calls += 1;
            r.input = Some(*registers);
            r.interrupted = interrupted;
            if matches!(r.delivery, Delivery::Handler) {
                registers[15] = 0x600010;
                registers[18] = 0x900000;
                registers[22] ^= 0x77;
            }
            r.delivery
        }
    }
}
use linux::signals_runtime::Delivery;
struct Runtime {
    delivery: Delivery,
    calls: usize,
    input: Option<HardwareContext>,
    interrupted: Option<LinuxSyscallContext>,
}
#[derive(Debug, PartialEq, Eq)]
enum FaultAction {
    Continue { hardware_context: HardwareContext, hardware_context_count: usize },
    Park,
    Terminate { pid: Word, status: Word },
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/fault_signal.rs"]
mod production;
fn original() -> HardwareContext { core::array::from_fn(|i| 0x1000 + i * 16) }
fn context(pc: usize) -> LinuxSyscallContext {
    LinuxSyscallContext { number: 39, args: [1, 2, 3, 4, 5, 6], program_counter: pc }
}
fn runtime(delivery: Delivery) -> Runtime {
    READS.with(|count| count.set(0));
    READ_RESULT.with(|value| *value.borrow_mut() = Ok(original()));
    Runtime { delivery, calls: 0, input: None, interrupted: None }
}
fn apply(original: HardwareContext, action: FaultAction) -> (HardwareContext, usize) {
    let FaultAction::Continue { hardware_context, hardware_context_count } = action else { panic!("unexpected {action:?}") };
    let mut result = original;
    result[..hardware_context_count].copy_from_slice(&hardware_context[..hardware_context_count]);
    (result, hardware_context_count)
}

#[test]
fn unchanged_delivery_uses_one_word_and_preserves_the_full_kernel_context() {
    for value in [0, 42, -4, isize::MIN, isize::MAX] {
        let before = original();
        let ctx = context(before[15]);
        let mut r = runtime(Delivery::Unchanged);
        let action = production::signal_return(&mut r, 7, 107, ctx, value, before, true, false);
        let (after, count) = apply(before, action);
        let mut expected = before; expected[0] = value as Word;
        assert_eq!(after, expected); assert_eq!(count, 1);
        assert_eq!(r.input, Some(expected)); assert_eq!(r.calls, 1);
        assert_eq!(r.interrupted, None);
        READS.with(|reads| assert_eq!(reads.get(), 0));
    }
}

#[test]
fn changed_continuation_and_slow_context_keep_the_general_reply_prefix() {
    for fast in [false, true] {
        let before = original();
        let ctx = context(if fast { before[15] + 2 } else { before[15] });
        let mut r = runtime(Delivery::Unchanged);
        let supplied = if fast { before } else { [0xdead; 23] };
        let action = production::signal_return(&mut r, 7, 107, ctx, -4, supplied, fast, false);
        let (after, count) = apply(before, action);
        let mut expected = before; expected[0] = (-4isize) as Word; expected[15] = ctx.program_counter;
        assert_eq!(after, expected); assert_eq!(count, 16);
        assert_eq!(r.input, Some(expected));
        READS.with(|reads| assert_eq!(reads.get(), usize::from(!fast)));
    }
}

#[test]
fn actual_handler_still_transmits_stack_pc_and_tail_registers() {
    for parked in [false, true] {
        let before = original(); let ctx = context(before[15]);
        let mut r = runtime(Delivery::Handler);
        let (after, count) = apply(before, production::signal_return(&mut r, 7, 107, ctx, 9, before, true, parked));
        let mut expected = before; expected[0] = 9; expected[15] = 0x600010;
        expected[18] = 0x900000; expected[22] ^= 0x77;
        assert_eq!(after, expected); assert_eq!(count, 23);
        assert_eq!(r.interrupted, parked.then_some(ctx));
    }
}

#[test]
fn parked_unchanged_stopped_and_terminated_delivery_never_resume() {
    let before = original(); let ctx = context(before[15]);
    for (delivery, parked, expected) in [(Delivery::Unchanged, true, FaultAction::Park),
        (Delivery::Stopped, false, FaultAction::Park),
        (Delivery::Terminate(9), false, FaultAction::Terminate { pid: 7, status: 137 })] {
        let mut r = runtime(delivery);
        assert_eq!(production::signal_return(&mut r, 7, 107, ctx, 0, before, true, parked), expected);
        assert_eq!(r.calls, 1); assert_eq!(r.interrupted, parked.then_some(ctx));
    }
}

#[test]
fn failed_slow_context_read_terminates_before_delivery() {
    let before = original(); let mut r = runtime(Delivery::Unchanged);
    READ_RESULT.with(|value| *value.borrow_mut() = Err(()));
    assert_eq!(production::signal_return(&mut r, 7, 107, context(before[15]), 0, before, false, false),
        FaultAction::Terminate { pid: 7, status: 139 });
    assert_eq!(r.calls, 0); assert_eq!(r.input, None);
    READS.with(|reads| assert_eq!(reads.get(), 1));
}
