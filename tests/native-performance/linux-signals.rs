//! Signal policy and both Linux frame ABIs, including untrusted sigreturn state.
#![allow(dead_code)]
extern crate alloc;
pub mod process {
    pub type HardwareContext = [usize; 35];
    #[derive(Clone, Copy)]
    pub struct LinuxSyscallContext {
        pub number: usize,
        pub args: [usize; 6],
        pub program_counter: usize,
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/signals.rs"]
pub mod signals;
mod common {
    pub(crate) use crate::signals;
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/signal_frame.rs"]
mod frame;
// Also compile the non-host codec: wire layouts must not depend on the host CPU.
pub use frame::{get, put, user_address};
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/arch/aarch64_signal.rs"]
mod arm;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/arch/x86_64_signal.rs"]
mod x86;
use signals::*;

#[test]
fn masks_cannot_block_kill_or_stop() {
    assert_eq!(blockable(u64::MAX) & (bit(SIGKILL) | bit(SIGSTOP)), 0);
    assert_ne!(blockable(u64::MAX) & bit(SIGCONT), 0);
}

#[test]
fn threads_share_dispositions_group_pending_and_timer_but_not_masks_or_targeted_signals() {
    let mut leader = Signals::new();
    leader.blocked = bit(10);
    leader.altstack = AltStack {
        sp: 0x1000,
        flags: 0,
        size: 8192,
    };
    let mut thread = leader.clone_task(true, true);
    assert_eq!(thread.altstack, AltStack::DISABLED);
    thread.blocked = bit(12);
    thread.set_action(
        10,
        Action {
            handler: 0x4000,
            ..Action::default()
        },
    );
    assert_eq!(leader.action(10).handler, 0x4000);
    thread.timer.set(Timer {
        deadline: Some(55),
        interval: 10,
    });
    assert_eq!(leader.timer.get().deadline, Some(55));
    leader.enqueue_group(Info::new(10, 0, 123));
    assert!(leader.next().is_none());
    assert_eq!(thread.next(), Some(Info::new(10, 0, 123)));
    assert_eq!(leader.pending_mask(), 0);
    leader.enqueue(Info::new(12, -6, 7));
    assert!(thread.next().is_none());
    assert_eq!(leader.next(), Some(Info::new(12, -6, 7)));
}

#[test]
fn standard_signals_coalesce_but_realtime_preserves_fifo_and_priority() {
    let mut state = Signals::new();
    assert!(state.enqueue(Info::new(10, 0, 1)));
    assert!(state.enqueue(Info::new(10, 0, 2)));
    for (signal, sender) in [(35, 10), (34, 20), (34, 21)] {
        assert!(state.enqueue(Info::new(signal, -1, sender)));
    }
    assert_eq!(state.pending.len(), 4);
    assert_eq!(state.next().unwrap(), Info::new(10, 0, 1));
    assert_eq!(state.next().unwrap(), Info::new(34, -1, 20));
    assert_eq!(state.next().unwrap(), Info::new(34, -1, 21));
    assert_eq!(state.next().unwrap().signal(), 35);
}

#[test]
fn blocked_signals_remain_pending_and_unblock_in_signal_order() {
    let mut state = Signals::new();
    state.blocked = bit(10);
    state.enqueue(Info::new(10, 0, 0));
    state.enqueue(Info::new(12, 0, 0));
    assert_eq!(state.next().unwrap().signal(), 12);
    assert!(state.next().is_none());
    assert_eq!(state.pending_mask(), bit(10));
    state.blocked = 0;
    assert_eq!(state.next().unwrap().signal(), 10);
}

#[test]
fn stop_and_continue_cancel_each_others_pending_requests() {
    let mut state = Signals::new();
    state.enqueue(Info::new(20, 0, 0));
    state.enqueue(Info::new(21, 0, 0));
    state.enqueue(Info::new(SIGCONT, 0, 0));
    assert_eq!(state.pending_mask(), bit(SIGCONT));
    state.enqueue(Info::new(SIGSTOP, 0, 0));
    assert_eq!(state.pending_mask(), bit(SIGSTOP));
}

#[test]
fn queue_is_bounded_but_kill_and_stop_cannot_be_starved() {
    let mut state = Signals::new();
    for _ in 0..128 {
        assert!(state.enqueue(Info::new(34, -1, 0)));
    }
    assert!(!state.enqueue(Info::new(34, -1, 0)));
    assert!(state.enqueue(Info::new(SIGKILL, 0, 0)));
    assert_eq!(state.next().unwrap().signal(), SIGKILL);
    assert_eq!(state.pending.len(), 128);
}

#[test]
fn fork_copies_dispositions_and_mask_not_pending_timers_or_stop_state() {
    let mut parent = Signals::new();
    parent.actions.borrow_mut()[9] = Action {
        handler: 0x1234,
        flags: SA_RESTART,
        ..Action::default()
    };
    parent.blocked = bit(10);
    parent.enqueue(Info::new(12, 0, 0));
    parent.timer.set(Timer {
        deadline: Some(20),
        interval: 0,
    });
    parent.stopped = true;
    let mut child = parent.fork();
    assert_eq!(child.actions, parent.actions);
    assert_eq!(child.blocked, parent.blocked);
    assert!(child.pending.is_empty());
    assert!(!child.stopped);
    assert!(child.timer.get().deadline.is_none());
    child.actions.borrow_mut()[9].handler = 0x5678;
    assert_eq!(parent.action(10).handler, 0x1234);
}

#[test]
fn exec_preserves_ignored_actions_pending_signals_mask_and_real_timer() {
    let mut state = Signals::new();
    state.actions.borrow_mut()[9].handler = 0x1234;
    state.actions.borrow_mut()[11].handler = SIG_IGN;
    state.blocked = bit(10);
    state.enqueue(Info::new(10, 0, 0));
    state.timer.set(Timer {
        deadline: Some(100),
        interval: 0,
    });
    state.trampoline = 0x1000;
    state.altstack = AltStack {
        sp: 0x1000,
        flags: 0,
        size: 8192,
    };
    state.exec();
    assert_eq!(state.action(10).handler, SIG_DFL);
    assert_eq!(state.action(12).handler, SIG_IGN);
    assert_eq!(state.pending_mask(), bit(10));
    assert_eq!(state.blocked, bit(10));
    assert_eq!(state.timer.get().deadline, Some(100));
    assert_eq!(state.trampoline, 0);
    assert_eq!(state.altstack, AltStack::DISABLED);
}

#[test]
fn x86_linux_layout_roundtrips_all_gprs_without_restoring_privileged_state() {
    use x86 as abi;
    let mut registers = [0; 23];
    for (index, value) in registers.iter_mut().enumerate() {
        *value = 0x1000 + index;
    }
    registers[abi::SP] = 0x8000;
    registers[abi::PC] = 0x4000;
    let original = registers;
    let mut bytes = [0; abi::FRAME_BYTES];
    abi::save(&mut bytes, &registers, 0x3000);
    assert_eq!(abi::FRAME_BYTES, 440);
    assert_eq!(abi::MASK_OFFSET, 304);
    assert_eq!(abi::INFO_OFFSET, 312);
    assert_eq!(frame::get(&bytes, abi::MCONTEXT_OFFSET + 184), 0); // fpstate
    abi::enter(&mut registers, 0x7008, 0x5000, 10, 0x3000);
    assert_eq!(registers[4], 10);
    assert_eq!(registers[5], 0x7008 + abi::INFO_OFFSET);
    assert_eq!(registers[3], 0x7010);
    frame::put(&mut bytes, abi::MCONTEXT_OFFSET + 144, 0); // malicious CS/SS
    frame::put(&mut bytes, abi::MCONTEXT_OFFSET + 17 * 8, usize::MAX);
    assert!(abi::restore(&bytes, &mut registers));
    for index in [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 18, 16, 19, 20, 21,
    ] {
        assert_eq!(registers[index], original[index], "register {index}");
    }
    assert_eq!(registers[17] & (0x3000 | 0x4000 | 0x20000), 0);
    assert_eq!(registers[22], 2); // IRET, not SYSRET clobbering RCX/R11
    assert_eq!(abi::frame_address(0x8000).unwrap() & 15, 8);
    frame::put(
        &mut bytes,
        abi::MCONTEXT_OFFSET + 16 * 8,
        0xffff800000000000,
    );
    assert!(!abi::restore(&bytes, &mut registers));
}

#[test]
fn arm_linux_layout_keeps_reserved_fp_records_empty_and_el0_state_safe() {
    use arm as abi;
    let mut registers = [0; 35];
    for (index, value) in registers.iter_mut().enumerate() {
        *value = 0x1000 + index;
    }
    registers[abi::SP] = 0x8000;
    registers[abi::PC] = 0x4000;
    let original = registers;
    let mut bytes = [0; abi::FRAME_BYTES];
    abi::save(&mut bytes, &registers, 0);
    assert_eq!(abi::MCONTEXT_OFFSET, 304);
    assert_eq!(abi::FRAME_BYTES, 4688);
    assert!(bytes[592..].iter().all(|byte| *byte == 0));
    abi::enter(&mut registers, 0x6000, 0x5000, 12, 0x3000);
    assert_eq!(&registers[..3], &[12, 0x6000, 0x6080]);
    assert_eq!(registers[30], 0x3000);
    frame::put(&mut bytes, abi::MCONTEXT_OFFSET + 8 + 33 * 8, usize::MAX);
    assert!(abi::restore(&bytes, &mut registers));
    assert_eq!(&registers[..33], &original[..33]);
    assert_eq!(registers[33], 0xf0000000);
    assert_eq!(registers[34], original[34]);
    frame::put(&mut bytes, abi::MCONTEXT_OFFSET + 8 + 31 * 8, 0x8001);
    assert!(!abi::restore(&bytes, &mut registers));
}
