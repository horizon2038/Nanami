#![allow(dead_code)]
extern crate self as a9n_abi;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub use std::println;
pub type Word = usize;
pub const PROCESS_SLOT_NOTIFICATION: Word = 18;
const ALTER_FB_PRESENT_HZ: Word = 60;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const EOPNOTSUPP: i32 = 95;
const ESRCH: i32 = 3;
const LINUX_CPU_MASK_BYTES: Word = 8;
const LINUX_ITIMERVAL_BYTES: Word = 32;
const LINUX_ITIMER_PROF: Word = 2;
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_TIMESPEC_BYTES: Word = 16;
const SYS_NANOSLEEP: Word = 35;
const SYS_CLOCK_NANOSLEEP: Word = 230;

#[derive(Default, Clone, Copy)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
}
impl LinuxSyscallContext {
    const EMPTY: Self = Self {
        number: 0,
        args: [0; 6],
    };
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
    exited: bool,
    readiness_wait: Option<ReadinessWait>,
}
#[derive(Clone, Copy)]
struct ReadinessWait {
    deadline: Option<Word>,
}
mod state {
    pub mod readiness {
        pub const READY_TIMER: usize = 16;
    }
}
fn wake_readiness_waiters(_: &mut Runtime, _: Word) {}
mod signal_timers {
    pub fn next_deadline(_: &super::Runtime) -> Option<usize> {
        None
    }
    pub fn expire(_: &mut super::Runtime) {}
}
mod futex {
    pub fn next_deadline(_: &super::Runtime) -> Option<usize> {
        None
    }
    pub fn pump(_: &mut super::Runtime) {}
}
#[derive(Default)]
struct PollObjects;
impl PollObjects {
    fn timer_deadline(&self) -> Option<Word> {
        None
    }
    fn expire_timers(&mut self, _: Word) {}
}
mod signals_runtime {
    pub fn pump(_: &mut super::Runtime, _: usize) {}
    pub fn timed_wait_expired(_: &mut super::Runtime, _: usize, _: usize) -> isize {
        0
    }
}
#[derive(Default, Clone, Copy)]
struct Graphics {
    active: bool,
    guest_pid: Word,
    guest_framebuffer: Word,
}
#[derive(Default)]
struct Runtime {
    poll_objects: PollObjects,
    posix_shm: Word,
    timer_port: Word,
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    realtime_anchor: Option<(Word, Word)>,
    clock_deadline: Option<Word>,
    framebuffer_deadline: Option<Word>,
    managed: [Process; 4],
    graphics: [Graphics; 4],
}
impl Runtime {
    fn is_signal_stopped(&self, _: Word) -> bool {
        false
    }
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
    rtc_reads: usize,
    rtc_error: bool,
    write_error: bool,
    memory_writes: Vec<(Word, Word, Word)>,
    completed: Vec<(Word, Word)>,
    read_error: bool,
    timer_available: bool,
    timer_connects: usize,
}
pub mod abi {
    pub const SLOT_RTC_SERVICE: usize = 32;
    pub const SLOT_TIMER_SERVICE: usize = 31;
}
pub mod ipc {
    pub fn process_slot_descriptor(slot: usize) -> usize {
        slot
    }
}
pub mod registry {
    pub fn connect_rtc_service(_: usize) -> Result<(), i32> {
        Ok(())
    }
    pub fn connect_timer_service(slot: usize) -> Result<(), i32> {
        assert_eq!(slot, super::abi::SLOT_TIMER_SERVICE);
        super::FAKE.with_borrow_mut(|fake| {
            fake.timer_connects += 1;
            if fake.timer_available {
                Ok(())
            } else {
                Err(super::EOPNOTSUPP)
            }
        })
    }
}
pub mod rtc {
    use super::*;
    #[derive(Clone, Copy)]
    pub struct RtcDateTime {
        pub year: u16,
        pub month: u8,
        pub day: u8,
        pub hour: u8,
        pub minute: u8,
        pub second: u8,
    }
    pub fn rtc_service_read(_: Word) -> Result<RtcDateTime, i32> {
        FAKE.with(|f| {
            let mut fake = f.borrow_mut();
            fake.rtc_reads += 1;
            if fake.rtc_error {
                return Err(EIO);
            }
            Ok(())
        })?;
        Ok(RtcDateTime {
            year: 2000,
            month: 2,
            day: 29,
            hour: 12,
            minute: 34,
            second: 56,
        })
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/realtime.rs"]
mod realtime;
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
pub mod timer {
    use super::*;
    pub const TIMER_NOTIFICATION_IDENTIFIER_BIT: Word = 1 << 63;
    pub fn timer_service_monotonic_ticks(port: Word) -> Result<(Word, Word), i32> {
        assert_ne!(port, 0, "clock query used a missing capability");
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
fn record_syscall_result(_: &mut Runtime, pid: Word, number: Word, _: isize) {
    FAKE.with(|f| f.borrow_mut().completed.push((pid, number)));
}
fn read_target_memory(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    FAKE.with(|f| {
        if f.borrow().read_error {
            return Err(EINVAL);
        }
        Ok(())
    })
}
fn write_target_memory(_: &mut Runtime, pid: Word, target: Word, bytes: Word) -> Result<(), i32> {
    FAKE.with(|f| {
        let mut fake = f.borrow_mut();
        fake.memory_writes.push((pid, target, bytes));
        if fake.write_error {
            return Err(EINVAL);
        }
        Ok(())
    })
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
            timer_available: true,
            ..Fake::default()
        }
    });
    let mut runtime = Runtime::default();
    runtime.timer_port = abi::SLOT_TIMER_SERVICE;
    for index in 0..4 {
        runtime.managed[index].pid = index + 1;
        runtime.managed[index].pcb = index + 1;
    }
    runtime
}

#[test]
fn absent_timer_does_not_query_zero_or_fabricate_time_and_can_connect_later() {
    let mut runtime = runtime(1_000_000_000);
    runtime.timer_port = 0;
    runtime.monotonic_ticks = 123;
    runtime.monotonic_tick_hz = 456;
    FAKE.with_borrow_mut(|fake| fake.timer_available = false);
    assert_eq!(refresh_clock(&mut runtime), Err(EOPNOTSUPP));
    assert_eq!(runtime.timer_port, 0);
    assert_eq!(runtime.monotonic_ticks, 123);
    assert_eq!(runtime.monotonic_tick_hz, 456);
    FAKE.with_borrow(|fake| {
        assert_eq!(fake.timer_connects, 1);
        assert_eq!(fake.clock_reads, 0);
    });
    FAKE.with_borrow_mut(|fake| {
        fake.timer_available = true;
        fake.ticks = 789;
    });
    refresh_clock(&mut runtime).unwrap();
    refresh_clock(&mut runtime).unwrap();
    assert_eq!(runtime.timer_port, abi::SLOT_TIMER_SERVICE);
    assert_eq!(runtime.monotonic_ticks, 789);
    assert_eq!(runtime.monotonic_tick_hz, 1_000_000_000);
    FAKE.with_borrow(|fake| {
        assert_eq!(fake.timer_connects, 2);
        assert_eq!(fake.clock_reads, 2);
    });
}
fn sleep(runtime: &mut Runtime, pid: Word, ns: Word) -> EmulationAction {
    let mut request = [ns / 1_000_000_000, ns % 1_000_000_000];
    runtime.posix_shm = request.as_mut_ptr() as Word;
    sys_nanosleep_action(
        runtime,
        pid,
        LinuxSyscallContext {
            number: SYS_NANOSLEEP,
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
fn realtime_and_gettimeofday_share_one_rtc_anchor_but_always_sample_current_time() {
    let mut runtime = runtime(1_000_000_000);
    let mut result = [0usize; 2];
    runtime.posix_shm = result.as_mut_ptr() as Word;
    FAKE.with(|fake| fake.borrow_mut().ticks = 10_000_000_000);
    sys_clock_gettime(&mut runtime, 1, 0, 1).unwrap();
    assert_eq!(result, [951827696, 0]); // 2000-02-29 12:34:56 UTC
    FAKE.with(|fake| fake.borrow_mut().ticks += 1_234_567_890);
    sys_gettimeofday(&mut runtime, 1, 1).unwrap();
    assert_eq!(result, [951827697, 234567]);
    sys_clock_gettime(&mut runtime, 1, 5, 1).unwrap();
    assert_eq!(result, [951827697, 234567890]);
    FAKE.with(|fake| {
        assert_eq!(fake.borrow().rtc_reads, 1);
        assert_eq!(fake.borrow().clock_reads, 3);
        assert!(fake.borrow().alarms.is_empty());
    });
}

#[test]
fn time_null_returns_unix_seconds_without_touching_guest_memory() {
    let mut runtime = runtime(1_000_000_000);
    assert_eq!(sys_time(&mut runtime, 1, 0), Ok(951827696));
    FAKE.with(|f| f.borrow_mut().ticks = 1_999_999_999);
    assert_eq!(sys_time(&mut runtime, 1, 0), Ok(951827697));
    FAKE.with(|f| f.borrow_mut().ticks = 2_000_000_000);
    assert_eq!(sys_time(&mut runtime, 1, 0), Ok(951827698));
    FAKE.with(|f| {
        let fake = f.borrow();
        assert_eq!(fake.rtc_reads, 1);
        assert_eq!(fake.clock_reads, 3);
        assert!(fake.memory_writes.is_empty());
        assert!(fake.alarms.is_empty());
    });
}

#[test]
fn time_writes_exactly_eight_bytes_and_shares_the_realtime_anchor() {
    let mut runtime = runtime(3);
    let mut staging = [0usize, Word::MAX];
    runtime.posix_shm = staging.as_mut_ptr() as Word;
    // The ABI uses a 64-bit time_t even after the 2038 boundary.
    runtime.realtime_anchor = Some((2_200_000_000, 0));
    FAKE.with(|f| f.borrow_mut().ticks = 7);
    assert_eq!(sys_time(&mut runtime, 2, 0x1235), Ok(2_200_000_002));
    assert_eq!(staging, [2_200_000_002, Word::MAX]);
    sys_clock_gettime(&mut runtime, 2, 0, 0x1235).unwrap();
    assert_eq!(staging, [2_200_000_002, 333333333]);
    FAKE.with(|f| {
        let fake = f.borrow();
        assert_eq!(fake.rtc_reads, 0);
        assert_eq!(fake.memory_writes, [(2, 0x1235, 8), (2, 0x1235, 16)]);
    });
}

#[test]
fn time_reports_bad_destinations_and_clock_failures() {
    let mut runtime = runtime(1_000_000_000);
    let mut staging = [0usize];
    runtime.posix_shm = staging.as_mut_ptr() as Word;
    FAKE.with(|f| f.borrow_mut().write_error = true);
    assert_eq!(sys_time(&mut runtime, 1, 1), Err(EFAULT));
    FAKE.with(|f| {
        let mut fake = f.borrow_mut();
        fake.memory_writes.clear();
        fake.clock_error = true;
    });
    assert_eq!(sys_time(&mut runtime, 1, 1), Err(EIO));
    runtime.realtime_anchor = None;
    FAKE.with(|f| f.borrow_mut().rtc_error = true);
    assert_eq!(sys_time(&mut runtime, 1, 1), Err(EIO));
    FAKE.with(|f| assert!(f.borrow().memory_writes.is_empty()));
}

fn clock_sleep(
    runtime: &mut Runtime,
    clock: Word,
    flags: Word,
    request: &mut [Word; 2],
) -> EmulationAction {
    runtime.posix_shm = request.as_mut_ptr() as Word;
    sys_clock_nanosleep_action(
        runtime,
        1,
        LinuxSyscallContext {
            number: SYS_CLOCK_NANOSLEEP,
            args: [clock, flags, 1, Word::MAX, 0, 0],
        },
    )
}

#[test]
fn clock_sleep_relative_uses_existing_alarm_and_preserves_syscall_context() {
    for clock in [0, 1, 7] {
        let mut runtime = runtime(1000);
        FAKE.with(|f| f.borrow_mut().ticks = 500);
        let mut request = [0, 1];
        assert_eq!(
            clock_sleep(&mut runtime, clock, 0, &mut request),
            EmulationAction::Park
        );
        assert_eq!(runtime.managed[0].sleep_deadline, 501);
        assert_eq!(
            runtime.managed[0].sleep_context.args,
            [clock, 0, 1, Word::MAX, 0, 0]
        );
        notify(&mut runtime, 500);
        assert!(runtime.managed[0].sleep_waiting);
        notify(&mut runtime, 501);
        assert!(!runtime.managed[0].sleep_waiting);
        FAKE.with(|f| {
            let fake = f.borrow();
            assert_eq!(fake.completed, [(1, SYS_CLOCK_NANOSLEEP)]);
            assert_eq!(fake.resumed, [1]);
            assert_eq!(fake.rtc_reads, 0);
            assert!(fake.memory_writes.is_empty()); // remain is untouched on success.
        });
    }
}

#[test]
fn absolute_clock_sleep_rounds_deadlines_and_past_deadlines_return_immediately() {
    for clock in [1, 7] {
        let mut runtime = runtime(3);
        FAKE.with(|f| f.borrow_mut().ticks = 2);
        assert_eq!(
            clock_sleep(&mut runtime, clock, 1, &mut [0, 999999999]),
            EmulationAction::Park
        );
        assert_eq!(runtime.managed[0].sleep_deadline, 3);
        notify(&mut runtime, 3);
        assert_eq!(
            clock_sleep(&mut runtime, clock, 1, &mut [1, 0]),
            EmulationAction::Return(0)
        );
        assert!(!runtime.managed[0].sleep_waiting);
    }
    let mut runtime = runtime(3);
    runtime.realtime_anchor = Some((951827696, 30));
    FAKE.with(|f| f.borrow_mut().ticks = 31);
    assert_eq!(
        clock_sleep(&mut runtime, 0, 1, &mut [951827697, 0]),
        EmulationAction::Park
    );
    assert_eq!(runtime.managed[0].sleep_deadline, 33);
    notify(&mut runtime, 33);
    assert_eq!(
        clock_sleep(&mut runtime, 0, 1, &mut [951827697, 0]),
        EmulationAction::Return(0)
    );
    assert_eq!(
        clock_sleep(&mut runtime, 0, 1, &mut [0, 0]),
        EmulationAction::Return(0)
    );
}

#[test]
fn clock_sleep_validates_requests_and_rolls_back_failed_alarm() {
    let mut runtime = runtime(1_000_000_000);
    for clock in [3, 10, 12, Word::MAX] {
        assert_eq!(
            clock_sleep(&mut runtime, clock, 0, &mut [0, 1]),
            EmulationAction::Return(-(EINVAL as isize))
        );
    }
    for clock in [2, 4, 5, 6, 8, 9, 11] {
        assert_eq!(
            clock_sleep(&mut runtime, clock, 0, &mut [0, 1]),
            EmulationAction::Return(-(EOPNOTSUPP as isize))
        );
    }
    for mut request in [[Word::MAX, 0], [0, 1_000_000_000]] {
        assert_eq!(
            clock_sleep(&mut runtime, 1, 0, &mut request),
            EmulationAction::Return(-(EINVAL as isize))
        );
    }
    FAKE.with(|f| f.borrow_mut().read_error = true);
    assert_eq!(
        clock_sleep(&mut runtime, 1, 0, &mut [0, 1]),
        EmulationAction::Return(-(EFAULT as isize))
    );
    FAKE.with(|f| {
        let mut fake = f.borrow_mut();
        fake.read_error = false;
        fake.alarm_error = true;
    });
    assert_eq!(
        clock_sleep(&mut runtime, 1, 0, &mut [0, 1]),
        EmulationAction::Return(-(EIO as isize))
    );
    assert!(!runtime.managed[0].sleep_waiting);
    assert_eq!(runtime.clock_deadline, None);
    assert_eq!(runtime.managed[0].sleep_context.number, 0);
}

#[test]
fn unavailable_cpu_and_tai_clocks_fail_without_timer_ipc() {
    let mut runtime = runtime(1_000_000_000);
    for id in [2, 3, 10, 11, Word::MAX] {
        assert_eq!(sys_clock_gettime(&mut runtime, 1, id, 1), Err(EINVAL));
    }
    FAKE.with(|fake| assert_eq!(fake.borrow().clock_reads, 0));
}

#[test]
fn poll_sleep_and_framebuffer_share_the_earliest_deadline() {
    let mut runtime = runtime(1000);
    runtime.managed[0].readiness_wait = Some(ReadinessWait { deadline: Some(50) });
    runtime.managed[1].sleep_waiting = true;
    runtime.managed[1].sleep_deadline = 100;
    arm_clock_timer(&mut runtime).unwrap();
    assert_eq!(runtime.clock_deadline, Some(50));
    runtime.managed[0].readiness_wait = None;
    arm_clock_timer(&mut runtime).unwrap();
    assert_eq!(runtime.clock_deadline, Some(100));
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

#[test]
fn clock_getres_reports_backend_tick_and_accepts_null_destination() {
    let mut runtime = runtime(250);
    let mut scratch = [0u64; 2];
    runtime.posix_shm = scratch.as_mut_ptr() as Word;
    let mut ts = [0u64; 2];
    let pointer = ts.as_mut_ptr() as Word;
    assert_eq!(sys_clock_getres(&mut runtime, 1, 1, pointer), Ok(0));
    assert_eq!(scratch, [0, 4_000_000]);
    assert_eq!(sys_clock_getres(&mut runtime, 1, 1, 0), Ok(0));
    assert_eq!(sys_clock_getres(&mut runtime, 1, 99, 0), Err(EINVAL));
    runtime.monotonic_tick_hz = 1_000_000_000;
    FAKE.with(|fake| fake.borrow_mut().hz = 1_000_000_000);
    assert_eq!(sys_clock_getres(&mut runtime, 1, 0, pointer), Ok(0));
    assert_eq!(scratch, [0, 1]);
}
