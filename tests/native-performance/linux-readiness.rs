#![allow(dead_code, private_interfaces)]
extern crate self as a9n_abi;
extern crate self as libnanami;
extern crate self as nanami_services;
pub mod terminal {
    pub const TERMINAL_NOTIFICATION_INPUT: usize = 1 << 52;
}
pub mod net {
    pub const NET_NOTIFICATION_RX: usize = 1 << 20;
}
pub mod input {
    pub const INPUT_NOTIFICATION_IDENTIFIER: usize = 1 << 19;
}
pub use std::println;
type Word = usize;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
const ESRCH: i32 = 3;
const EIO: i32 = 5;
const EOPNOTSUPP: i32 = 95;
const LINUX_FD_MAX: usize = 64;
const LINUX_PIPE_BYTES: usize = 1024;
const LINUX_POLLFD_BYTES: usize = 8;
const LINUX_POLLFD_MAX: usize = LINUX_FD_MAX;
const LINUX_TIMESPEC_BYTES: usize = 16;
const LINUX_POLLIN: i16 = 1;
const LINUX_POLLPRI: i16 = 2;
const LINUX_POLLOUT: i16 = 4;
const LINUX_POLLERR: i16 = 8;
const LINUX_POLLHUP: i16 = 16;
const LINUX_POLLNVAL: i16 = 32;
const LINUX_POLLRDNORM: i16 = 64;
const LINUX_POLLWRNORM: i16 = 256;
const LINUX_POLLRDHUP: i16 = 8192;
const SYS_POLL: usize = 7;
const SYS_SELECT: usize = 23;
const SYS_PPOLL: usize = 271;
const SYS_PSELECT6: usize = 270;

#[derive(Default, Clone, Copy)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
}
#[derive(Debug, PartialEq)]
enum EmulationAction {
    Return(isize),
    Park,
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/readiness.rs"]
pub mod readiness_state;
pub mod state {
    pub use crate::readiness_state as readiness;
}
use readiness_state::*;
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    Terminal,
    Posix,
    PipeRead,
    PipeWrite,
    SocketUdp,
    SocketTcp,
    SocketTcpListener,
    SocketIcmp,
    SocketNetlink,
    VirtualDirectory,
    VirtualFile,
    EvdevKeyboard,
    EvdevMouse,
    Framebuffer,
    Empty,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    resource: Word,
    peer_port: u16,
}
#[derive(Default)]
struct Pipe {
    len: usize,
    readers: usize,
    writers: usize,
}
#[derive(Default)]
struct Process {
    pid: Word,
    pcb: Word,
    personality: Word,
    exited: bool,
    readiness_wait: Option<ReadinessWait>,
}
struct Runtime {
    posix_shm: Word,
    scratch: Box<[u8]>,
    files: Vec<LinuxFile>,
    pipe: Pipe,
    pumps: usize,
    reads: usize,
    writes: usize,
    clocks: usize,
    arms: Vec<Option<Word>>,
    key_ready: bool,
    mouse_ready: bool,
    terminal_ready: bool,
    network_ready: i16,
    managed: [Process; 2],
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    readiness_changes: Word,
    clock_error: bool,
    alarm_error: bool,
}
impl Runtime {
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        self.files.get(fd).copied()
    }
    fn pipe(&self, _: Word) -> Option<&Pipe> {
        Some(&self.pipe)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut Process> {
        self.managed.iter_mut().find(|process| process.pid == pid)
    }
}
fn pump_input_events(r: &mut Runtime) {
    r.pumps += 1;
}
fn keyboard_event_ready(r: &Runtime, _: Word) -> bool {
    r.key_ready
}
fn mouse_event_ready(r: &Runtime, _: Word) -> bool {
    r.mouse_ready
}
fn terminal_readable(r: &mut Runtime, _: Word) -> Result<bool, i32> {
    r.scratch.fill(0xcd); // Services may overwrite scratch while checking readiness.
    Ok(r.terminal_ready)
}
fn socket_readiness(r: &mut Runtime, _: Word, _: Word, _: LinuxFile) -> Result<i16, i32> {
    Ok(r.network_ready)
}
fn read_target_memory(r: &mut Runtime, _: Word, ptr: Word, bytes: Word) -> Result<(), i32> {
    if ptr == 0 && bytes != 0 {
        return Err(EFAULT);
    }
    r.reads += usize::from(bytes != 0);
    if bytes != 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(ptr as *const u8, r.posix_shm as *mut u8, bytes);
        }
    }
    Ok(())
}
fn write_target_memory(r: &mut Runtime, _: Word, ptr: Word, bytes: Word) -> Result<(), i32> {
    if ptr == 0 && bytes != 0 {
        return Err(EFAULT);
    }
    r.writes += usize::from(bytes != 0);
    if bytes != 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(r.posix_shm as *const u8, ptr as *mut u8, bytes);
        }
    }
    Ok(())
}
unsafe fn write_u64(p: Word, v: Word) {
    (p as *mut u64).write_unaligned(v as u64);
}
fn refresh_clock(r: &mut Runtime) -> Result<(), i32> {
    r.clocks += 1;
    if r.clock_error {
        Err(EIO)
    } else {
        Ok(())
    }
}
fn arm_clock_timer(r: &mut Runtime) -> Result<(), i32> {
    r.arms.push(
        r.managed
            .iter()
            .filter_map(|p| p.readiness_wait.as_ref().and_then(|w| w.deadline))
            .min(),
    );
    if r.alarm_error {
        Err(EIO)
    } else {
        Ok(())
    }
}
fn record_syscall_result(_: &mut Runtime, _: Word, _: Word, _: isize) {}
thread_local! { static RETURNS: std::cell::RefCell<Vec<(Word,isize)>> = const { std::cell::RefCell::new(Vec::new()) }; }
mod process {
    use super::*;
    pub fn write_personality_syscall_return(
        pcb: Word,
        _: LinuxSyscallContext,
        value: isize,
        _: Word,
    ) -> Result<(), ()> {
        RETURNS.with(|values| values.borrow_mut().push((pcb, value)));
        Ok(())
    }
}
pub mod arch {
    pub mod process_control_block {
        pub fn resume(_: usize) -> Result<(), ()> {
            Ok(())
        }
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/readiness.rs"]
mod readiness;
use readiness::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/poll_set.rs"]
mod poll_set;
use poll_set::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/poll_timeout.rs"]
mod poll_timeout;
use poll_timeout::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/poll_wait.rs"]
mod poll_wait;
use poll_wait::*;

fn runtime() -> Runtime {
    RETURNS.with(|v| v.borrow_mut().clear());
    let mut scratch = vec![0; 4096].into_boxed_slice();
    Runtime {
        posix_shm: scratch.as_mut_ptr() as Word,
        scratch,
        files: [
            LinuxFileKind::EvdevKeyboard,
            LinuxFileKind::EvdevMouse,
            LinuxFileKind::Posix,
            LinuxFileKind::Terminal,
            LinuxFileKind::PipeRead,
            LinuxFileKind::PipeWrite,
            LinuxFileKind::SocketTcp,
            LinuxFileKind::SocketNetlink,
        ]
        .into_iter()
        .map(|kind| LinuxFile {
            kind,
            posix_fd: 0,
            resource: 0,
            peer_port: 0,
        })
        .collect(),
        pipe: Pipe {
            len: 0,
            readers: 1,
            writers: 1,
        },
        pumps: 0,
        reads: 0,
        writes: 0,
        clocks: 0,
        arms: Vec::new(),
        key_ready: true,
        mouse_ready: false,
        terminal_ready: false,
        network_ready: 0,
        managed: [
            Process {
                pid: 1,
                pcb: 101,
                ..Process::default()
            },
            Process::default(),
        ],
        monotonic_ticks: 100,
        monotonic_tick_hz: 1000,
        readiness_changes: 0,
        clock_error: false,
        alarm_error: false,
    }
}
fn fd(fd: i32, events: i16) -> PollFd {
    PollFd {
        fd,
        events,
        revents: -1,
    }
}
fn poll(r: &mut Runtime, fds: &mut [PollFd], timeout: i32) -> EmulationAction {
    sys_poll_action(
        r,
        1,
        LinuxSyscallContext {
            number: SYS_POLL,
            args: [
                fds.as_mut_ptr() as Word,
                fds.len(),
                timeout as Word,
                0,
                0,
                0,
            ],
        },
    )
}
#[test]
fn zero_timeout_drains_input_once_without_clock_or_alarm_ipc() {
    let mut r = runtime();
    let mut fds = [
        fd(0, LINUX_POLLIN),
        fd(1, LINUX_POLLIN),
        fd(0, LINUX_POLLIN),
    ];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(2));
    assert_eq!(
        (r.pumps, r.reads, r.writes, r.clocks, r.arms.len()),
        (1, 1, 1, 0, 0)
    );
    assert_eq!(fds.map(|f| f.revents), [1, 0, 1]);
}
#[test]
fn ignored_negative_fd_regular_file_and_invalid_fd() {
    let mut r = runtime();
    let mut fds = [fd(-1, -1), fd(2, LINUX_POLLIN | LINUX_POLLOUT), fd(999, 0)];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(2));
    assert_eq!(fds.map(|f| f.revents), [0, 5, LINUX_POLLNVAL]);
    assert_eq!(r.pumps, 0);
}
#[test]
fn pipes_report_hup_and_error_even_without_requested_events() {
    let mut r = runtime();
    r.pipe.writers = 0;
    r.pipe.len = 2;
    let mut fds = [fd(4, LINUX_POLLIN), fd(5, LINUX_POLLOUT)];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(2));
    assert_eq!(
        fds.map(|f| f.revents),
        [LINUX_POLLIN | LINUX_POLLHUP, LINUX_POLLOUT]
    );
    r.pipe.readers = 0;
    fds = [fd(4, 0), fd(5, 0)];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(2));
    assert_eq!(fds.map(|f| f.revents), [LINUX_POLLHUP, LINUX_POLLERR]);
}
#[test]
fn empty_terminals_and_sockets_are_not_readable_and_scratch_is_not_retained() {
    let mut r = runtime();
    let mut fds = [
        fd(3, LINUX_POLLIN),
        fd(6, LINUX_POLLIN),
        fd(7, LINUX_POLLIN),
        fd(2, LINUX_POLLOUT),
    ];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(1));
    assert_eq!(fds.map(|f| f.revents), [0, 0, 0, LINUX_POLLOUT]);
    r.terminal_ready = true;
    r.network_ready = LINUX_POLLIN;
    r.files[7].peer_port = 1;
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(4));
}
#[test]
fn infinite_wait_only_rechecks_its_sources_and_snapshots_guest_interest() {
    let mut r = runtime();
    let mut fds = [fd(1, LINUX_POLLIN)];
    assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
    assert_eq!(r.clocks, 0);
    let pumps = r.pumps;
    wake_readiness_waiters(&mut r, READY_NETWORK | READY_TIMER);
    assert_eq!(r.pumps, pumps);
    // Guest shared memory changes must not change the already parked interest.
    fds[0].fd = 999;
    r.mouse_ready = true;
    wake_readiness_waiters(&mut r, READY_INPUT);
    assert!(r.managed[0].readiness_wait.is_none());
    assert_eq!(fds[0].revents, LINUX_POLLIN);
    RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
}
#[test]
fn finite_wait_is_one_shot_and_not_rescanned_by_early_timer() {
    let mut r = runtime();
    let mut fds = [fd(1, LINUX_POLLIN)];
    assert_eq!(poll(&mut r, &mut fds, 25), EmulationAction::Park);
    assert_eq!(r.arms, [Some(125)]);
    r.monotonic_ticks = 124;
    wake_readiness_waiters(&mut r, READY_TIMER);
    assert_eq!(r.pumps, 1);
    r.monotonic_ticks = 125;
    wake_readiness_waiters(&mut r, READY_TIMER);
    assert_eq!(fds[0].revents, 0);
    assert!(r.managed[0].readiness_wait.is_none());
    RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 0)]));
}
#[test]
fn ready_at_park_boundary_and_pipe_changes_wake_once() {
    let mut r = runtime();
    let mut fds = [fd(4, LINUX_POLLIN)];
    assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
    r.pipe.len = 1;
    r.readiness_changes = READY_PIPE;
    handle_readiness_changes(&mut r);
    handle_readiness_changes(&mut r);
    RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
    assert_eq!(r.readiness_changes, 0);
}
#[test]
fn alarm_failure_rolls_back_wait_and_invalid_arguments_do_not_park() {
    let mut r = runtime();
    r.alarm_error = true;
    assert_eq!(poll(&mut r, &mut [], 1), EmulationAction::Return(-5));
    assert!(r.managed[0].readiness_wait.is_none());
    let mut fds = [fd(-1, 0); 65];
    assert_eq!(poll(&mut r, &mut fds, 0), EmulationAction::Return(-22));
    assert_eq!(
        sys_poll_action(
            &mut r,
            1,
            LinuxSyscallContext {
                number: SYS_POLL,
                args: [0, 1, 0, 0, 0, 0]
            }
        ),
        EmulationAction::Return(-14)
    );
}
#[test]
fn ppoll_timespec_validation_remaining_time_and_mask_rejection() {
    let mut r = runtime();
    let mut ts = [0i64, 25_000_000];
    let mut mask = 0u64;
    let mut ctx = LinuxSyscallContext {
        number: SYS_PPOLL,
        args: [
            0,
            0,
            ts.as_mut_ptr() as Word,
            &mut mask as *mut _ as Word,
            8,
            0,
        ],
    };
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    r.monotonic_ticks = 125;
    wake_readiness_waiters(&mut r, READY_TIMER);
    assert_eq!(ts, [0, 0]);
    ts[1] = -1;
    assert_eq!(
        sys_poll_action(&mut r, 1, ctx),
        EmulationAction::Return(-22)
    );
    ts[1] = 1_000_000_000;
    assert_eq!(
        sys_poll_action(&mut r, 1, ctx),
        EmulationAction::Return(-22)
    );
    mask = 1;
    std::hint::black_box(mask);
    assert_eq!(
        sys_poll_action(&mut r, 1, ctx),
        EmulationAction::Return(-95)
    );
    ctx.args[4] = 16;
    assert_eq!(
        sys_poll_action(&mut r, 1, ctx),
        EmulationAction::Return(-22)
    );
}
#[test]
fn select_counts_ready_bits_masks_nfds_and_checks_exception_set() {
    let mut r = runtime();
    let mut sets = [1usize << 2 | 1 << 50, 1usize << 2, 1usize << 2];
    let mut ts = [0i64; 2];
    let mut ctx = LinuxSyscallContext {
        number: SYS_SELECT,
        args: [
            3,
            sets.as_mut_ptr() as Word,
            unsafe { sets.as_mut_ptr().add(1) } as Word,
            unsafe { sets.as_mut_ptr().add(2) } as Word,
            ts.as_mut_ptr() as Word,
            0,
        ],
    };
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(2));
    assert_eq!(sets, [4, 4, 0]);
    sets[2] = 1 << 30;
    std::hint::black_box(&sets);
    ctx.args[0] = 31;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(-9));
}
#[test]
fn zero_fds_still_sleeps_and_subtick_timeout_rounds_up() {
    let mut r = runtime();
    let mut ts = [0i64, 1];
    let ctx = LinuxSyscallContext {
        number: SYS_PSELECT6,
        args: [0, 0, 0, 0, ts.as_mut_ptr() as Word, 0],
    };
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    assert_eq!(r.arms, [Some(101)]);
    assert_eq!(
        poll_deadline(1, 1_000_000_000, i64::MAX as u128 * 1_000_000_000),
        Word::MAX
    );
}
