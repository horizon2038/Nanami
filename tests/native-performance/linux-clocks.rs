#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as libnanami;
extern crate self as nanami_services;
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 18;
const ALTER_FB_PRESENT_HZ: Word = 60;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ESRCH: i32 = 3;
const LINUX_CPU_MASK_BYTES: Word = 8;
const LINUX_ITIMERVAL_BYTES: Word = 32;
const LINUX_ITIMER_PROF: Word = 2;
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_TIMESPEC_BYTES: Word = 16;
const SYS_NANOSLEEP: Word = 35;

#[derive(Default, Clone, Copy)]
struct LinuxSyscallContext {
    args: [Word; 6],
}
impl LinuxSyscallContext {
    const EMPTY: Self = Self { args: [0; 6] };
}
#[derive(Debug, PartialEq)]
enum EmulationAction {
    Return(isize),
    Park,
}
#[derive(Default, Clone, Copy)]
struct Process {
    pid: Word,
    pcb: Word,
    personality: Word,
    sleep_waiting: bool,
    sleep_deadline: Word,
    sleep_context: LinuxSyscallContext,
}
#[derive(Default, Clone, Copy)]
struct Graphics {
    active: bool,
    guest_pid: Word,
    guest_framebuffer: Word,
}
#[derive(Default)]
struct Runtime {
    posix_shm: Word,
    timer_port: Word,
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    clock_deadline: Option<Word>,
    framebuffer_deadline: Option<Word>,
    managed: [Process; 4],
    graphics: [Graphics; 4],
}
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut Process> {
        self.managed.iter_mut().find(|p| p.pid == pid)
    }
}
#[derive(Default)]
struct Fake {
    ticks: Word,
    hz: Word,
    clock_reads: usize,
    clock_error: bool,
    alarm_error: bool,
    alarms: Vec<Option<Word>>,
    resumed: Vec<Word>,
    presented: usize,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub mod timer {
    use super::*;
    pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word = 1 << 63;
    pub fn timer_service_monotonic_ticks(_: Word) -> Result<(Word, Word), i32> {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            fake.clock_reads += 1;
            if fake.clock_error {
                Err(EIO)
            } else {
                Ok((fake.ticks, fake.hz))
            }
        })
    }
    pub fn timer_service_set_alarm_ticks(
        _: Word,
        _: Word,
        deadline: Option<Word>,
    ) -> Result<(), i32> {
        FAKE.with(|fake| {
            let mut fake = fake.borrow_mut();
            if fake.alarm_error {
                return Err(EIO);
            }
            fake.alarms.push(deadline);
            Ok(())
        })
    }
}
mod process {
    use super::*;
    pub fn write_personality_syscall_return(
        _: Word,
        _: LinuxSyscallContext,
        _: isize,
        _: Word,
    ) -> Result<(), ()> {
        Ok(())
    }
}
pub mod arch {
    pub mod process_control_block {
        pub fn resume(pcb: usize) -> Result<(), ()> {
            super::super::FAKE.with(|fake| fake.borrow_mut().resumed.push(pcb));
            Ok(())
        }
    }
}
fn map_request_error(error: i32) -> i32 {
    error
}
fn record_syscall_result(_: &mut Runtime, _: Word, _: Word, _: isize) {}
fn read_target_memory(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Ok(())
}
fn write_target_memory(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Ok(())
}
unsafe fn write_u64(address: Word, value: Word) {
    unsafe {
        (address as *mut u64).write_unaligned(value as u64);
    }
}
fn present_mapped_framebuffers(_: &Runtime) {
    FAKE.with(|fake| fake.borrow_mut().presented += 1);
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/clocks.rs"]
mod clocks;
use clocks::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/clock_events.rs"]
mod clock_events;
use clock_events::*;

fn runtime(hz: Word) -> Runtime {
    FAKE.with(|fake| {
        *fake.borrow_mut() = Fake {
            hz,
            ..Fake::default()
        }
    });
    let mut runtime = Runtime::default();
    for index in 0..4 {
        runtime.managed[index].pid = index + 1;
        runtime.managed[index].pcb = index + 1;
    }
    runtime
}
fn sleep(runtime: &mut Runtime, pid: Word, ns: Word) -> EmulationAction {
    let mut request = [ns / 1_000_000_000, ns % 1_000_000_000];
    runtime.posix_shm = request.as_mut_ptr() as Word;
    sys_nanosleep_action(
        runtime,
        pid,
        LinuxSyscallContext {
            args: [1, 0, 0, 0, 0, 0],
        },
    )
}
fn notify(runtime: &mut Runtime, ticks: Word) {
    FAKE.with(|fake| fake.borrow_mut().ticks = ticks);
    handle_timer_notification(runtime, timer::TIMER_NOTIFICATION_IDENTIFIER_BIT);
}

#[test]
fn clock_queries_sample_time_without_creating_periodic_notifications() {
    let mut runtime = runtime(1_000_000_000);
    let mut timespec = [0usize; 2];
    runtime.posix_shm = timespec.as_mut_ptr() as Word;
    for ticks in [123, 1_234_567_890] {
        FAKE.with(|fake| fake.borrow_mut().ticks = ticks);
        sys_clock_gettime(&mut runtime, 1, 1, 1).unwrap();
        assert_eq!(timespec, [ticks / 1_000_000_000, ticks % 1_000_000_000]);
    }
    FAKE.with(|fake| {
        assert!(fake.borrow().alarms.is_empty());
        assert_eq!(fake.borrow().clock_reads, 2);
    });
}

#[test]
fn timespec_fraction_preserves_non_divisor_frequency() {
    let mut runtime = runtime(3);
    let mut timespec = [0usize; 2];
    runtime.posix_shm = timespec.as_mut_ptr() as Word;
    FAKE.with(|fake| fake.borrow_mut().ticks = 2);
    sys_clock_gettime(&mut runtime, 1, 1, 1).unwrap();
    assert_eq!(timespec, [0, 666_666_666]);
}

#[test]
fn nanosleep_uses_backend_units_and_never_wakes_before_deadline() {
    for hz in [100, 1_000_000_000] {
        let mut runtime = runtime(hz);
        FAKE.with(|fake| fake.borrow_mut().ticks = 5 * hz);
        assert_eq!(sleep(&mut runtime, 1, 1_000_000), EmulationAction::Park);
        let deadline = 5 * hz + hz.div_ceil(1000);
        assert_eq!(runtime.managed[0].sleep_deadline, deadline);
        notify(&mut runtime, deadline - 1);
        assert!(runtime.managed[0].sleep_waiting);
        notify(&mut runtime, deadline);
        assert!(!runtime.managed[0].sleep_waiting);
        assert_eq!(runtime.clock_deadline, None);
        FAKE.with(|fake| assert_eq!(fake.borrow().resumed, [1]));
    }
}

#[test]
fn earlier_sleep_replaces_alarm_and_rearms_later_sleep() {
    let mut runtime = runtime(1_000_000_000);
    assert_eq!(sleep(&mut runtime, 1, 500_000_000), EmulationAction::Park);
    assert_eq!(sleep(&mut runtime, 2, 1_000_000), EmulationAction::Park);
    assert_eq!(runtime.clock_deadline, Some(1_000_000));
    notify(&mut runtime, 1_000_000);
    assert_eq!(runtime.clock_deadline, Some(500_000_000));
    // The coalesced old notification must not consume the replacement alarm.
    notify(&mut runtime, 1_000_000);
    assert_eq!(runtime.clock_deadline, Some(500_000_000));
    notify(&mut runtime, 500_000_000);
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().resumed, [2, 1]);
        assert_eq!(
            fake.borrow().alarms,
            [Some(500_000_000), Some(1_000_000), Some(500_000_000)]
        );
    });
}

#[test]
fn framebuffer_only_arms_when_mapped_and_stops_after_unmapping() {
    let mut runtime = runtime(1_000_000_000);
    refresh_clock(&mut runtime).unwrap();
    arm_clock_timer(&mut runtime).unwrap();
    assert_eq!(runtime.clock_deadline, None);
    runtime.graphics[0] = Graphics {
        active: true,
        guest_pid: 1,
        guest_framebuffer: 0x1000,
    };
    arm_clock_timer(&mut runtime).unwrap();
    assert_eq!(runtime.clock_deadline, Some(16_666_667));
    // A clock query cannot consume the presentation boundary.
    FAKE.with(|fake| fake.borrow_mut().ticks = 20_000_000);
    refresh_clock(&mut runtime).unwrap();
    notify(&mut runtime, 20_000_000);
    assert_eq!(runtime.clock_deadline, Some(33_333_334));
    FAKE.with(|fake| assert_eq!(fake.borrow().presented, 1));
    runtime.graphics[0].guest_framebuffer = 0;
    arm_clock_timer(&mut runtime).unwrap();
    assert_eq!(runtime.clock_deadline, None);
    FAKE.with(|fake| assert_eq!(fake.borrow().alarms.last(), Some(&None)));
}

#[test]
fn zero_sleep_is_immediate_and_arm_error_does_not_park_guest() {
    let mut runtime = runtime(1_000_000_000);
    assert_eq!(sleep(&mut runtime, 1, 0), EmulationAction::Return(0));
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().clock_reads, 0);
        fake.borrow_mut().alarm_error = true;
    });
    assert_eq!(sleep(&mut runtime, 1, 1), EmulationAction::Return(-5));
    assert!(!runtime.managed[0].sleep_waiting);
}
