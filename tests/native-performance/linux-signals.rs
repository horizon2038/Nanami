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
fn late_signal_membership_preserves_local_group_fifo_and_last_duplicate() {
    let mut counts = Vec::new();
    for group in [false, true] {
        for copies in [1, 2] {
            let mut state = Signals::new();
            for sender in 0..128-copies {
                let info = Info::new(34, -1, sender);
                assert!(if group { state.enqueue_group(info) } else { state.enqueue(info) });
            }
            for sender in 0..copies {
                let info = Info::new(35, -1, 1000 + sender);
                assert!(if group { state.enqueue_group(info) } else { state.enqueue(info) });
            }
            take_signal_reads();
            let selected = state.take_matching_record(bit(35));
            counts.push(take_signal_reads());
            assert_eq!(selected, Some((Info::new(35, -1, 1000), group)));
            assert_eq!(state.pending_mask(), bit(34) | if copies == 2 { bit(35) } else { 0 });
            if copies == 2 {
                assert_eq!(state.take_matching_record(bit(35)), Some((Info::new(35, -1, 1001), group)));
            }
            assert_eq!(state.take_matching(bit(35)), None);
            assert_eq!(state.pending_mask(), bit(34));
            assert_eq!(state.take_matching(bit(34)), Some(Info::new(34, -1, 0)));
        }
    }
    println!("SIGNAL-TAIL local1/local2/group1/group2 reads={counts:?}");
}

#[test]
fn draining_realtime_head_keeps_remaining_records_in_place() {
    let mut state = Signals::new();
    for sender in 0..128 {
        assert!(state.enqueue(Info::new(34, -1, sender)));
    }
    let mut displaced = 0;
    for sender in 0..128 {
        let before = state.pending.record_addresses();
        assert_eq!(state.take_matching(bit(34)), Some(Info::new(34, -1, sender)));
        let after = state.pending.record_addresses();
        for (info, address) in after {
            let original = before.iter().find(|(other, _)| *other == info).unwrap().1;
            if address != original {
                displaced += core::mem::size_of::<Info>();
            }
        }
    }
    assert_eq!(state.pending_mask(), 0);
    println!("SIGNAL-PENDING-DEQUE records=128 displaced-retained-record-bytes={displaced}");
    assert_eq!(displaced, 0);
}

#[test]
fn pending_mask_and_realtime_head_selection_avoid_full_record_scans() {
    let mut state = Signals::new();
    for sender in 0..128 {
        assert!(state.enqueue(Info::new(34, -1, sender)));
    }
    take_signal_reads();
    assert_eq!(state.pending_mask(), bit(34));
    let selected = state.take_matching(bit(34));
    let reads = take_signal_reads();
    assert_eq!(selected, Some(Info::new(34, -1, 0)));
    assert_eq!(state.pending.len(), 127);
    assert_eq!(state.pending_mask(), bit(34));
    println!("SIGNAL-PENDING-MASK records=128 mask-and-take-signal-reads={reads}");
    assert_eq!(reads, 2);
}

#[test]
fn pending_membership_matches_record_oracle_across_shared_thread_queues() {
    fn discard(queue: &mut Vec<Info>, signal: usize) {
        queue.retain(|info| info.signal() != signal);
    }
    fn stops(queue: &mut Vec<Info>) {
        queue.retain(|info| !stop_signal(info.signal()));
    }
    fn enqueue(queue: &mut Vec<Info>, info: Info) -> bool {
        if info.signal() < 32 && queue.iter().any(|other| other.signal() == info.signal()) {
            return true;
        }
        if info.signal() >= 32 && queue.len() >= 128 {
            return false;
        }
        queue.push(info);
        true
    }
    fn candidate(queue: &[Info], mask: u64) -> Option<(usize, usize)> {
        queue.iter().enumerate()
            .filter(|(_, info)| mask & bit(info.signal()) != 0)
            .min_by_key(|(_, info)| info.signal())
            .map(|(index, info)| (index, info.signal()))
    }
    let leader = Signals::new();
    let thread = leader.clone_task(true, true);
    let mut states = [leader, thread];
    let mut local = [Vec::new(), Vec::new()];
    let mut group = Vec::new();
    let mut taken = [None; 2];
    let numbers = [1, 9, 18, 19, 20, 21, 22, 31, 32, 34, 35, 64];
    let mut random = 0x9a31_4527u64;
    for step in 0..20_000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let owner = (random >> 17) as usize & 1;
        let signal = numbers[(random >> 32) as usize % numbers.len()];
        let info = Info::new(signal, -1, step);
        match (random >> 24) % 8 {
            0 | 1 => {
                if signal == SIGCONT {
                    stops(&mut local[owner]);
                    stops(&mut group);
                } else if stop_signal(signal) {
                    discard(&mut local[owner], SIGCONT);
                    discard(&mut group, SIGCONT);
                }
                assert_eq!(states[owner].enqueue(info), enqueue(&mut local[owner], info));
            }
            2 => assert_eq!(states[owner].enqueue_group(info), enqueue(&mut group, info)),
            3 => {
                states[owner].discard(signal);
                discard(&mut local[owner], signal);
                discard(&mut group, signal);
            }
            4 => {
                states[owner].discard_stops();
                stops(&mut local[owner]);
                stops(&mut group);
            }
            5 => {
                let mask = random.rotate_left(11);
                let l = candidate(&local[owner], mask);
                let g = candidate(&group, mask);
                let expected = match (l, g) {
                    (Some((index, s)), other) if other.is_none_or(|(_, gs)| s <= gs) =>
                        Some((local[owner].remove(index), false)),
                    (_, Some((index, _))) => Some((group.remove(index), true)),
                    _ => None,
                };
                assert_eq!(states[owner].take_matching_record(mask), expected);
                taken[owner] = expected;
            }
            6 => {
                if let Some((info, shared)) = taken[owner].take() {
                    states[owner].restore_pending(info, shared);
                    if shared {
                        group.insert(0, info);
                    } else {
                        local[owner].insert(0, info);
                    }
                }
            }
            _ => {
                let child = states[owner].fork();
                assert_eq!(child.pending_mask(), 0);
                assert!(!child.has_pending());
                states[owner].exec();
            }
        }
        for owner in 0..2 {
            let expected = local[owner].iter().chain(group.iter())
                .fold(0, |mask, info| mask | bit(info.signal()));
            assert_eq!(states[owner].pending_mask(), expected, "step {step}");
            assert_eq!(states[owner].pending.len(), local[owner].len());
            assert_eq!(states[owner].has_pending(), !local[owner].is_empty() || !group.is_empty());
        }
    }
}

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
