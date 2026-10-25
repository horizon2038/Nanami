#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as a9n_abi;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
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
struct UnixSocketMock;
impl UnixSocketMock {
    fn references(&self, _: LinuxFile) -> bool { false }
}
mod unix {
    use super::*;
    pub(super) fn retry(_: &mut Runtime, _: Word, _: LinuxSyscallContext) -> Option<Result<Word, i32>> { None }
    pub(super) fn readiness(_: &Runtime, _: Word) -> Result<i16, i32> { Ok(0) }
}
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EBADF: i32 = 9;
const ESRCH: i32 = 3;
const EIO: i32 = 5;
const EAGAIN: i32 = 11;
const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_READV: usize = 19;
const SYS_WRITEV: usize = 20;
const LINUX_O_NONBLOCK: usize = 0x800;
const EOPNOTSUPP: i32 = 95;
const LINUX_FD_MAX: usize = 64;
const LINUX_PIPE_BYTES: usize = 4096;
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
const SYS_EPOLL_WAIT: usize = 232;
const SYS_EPOLL_PWAIT: usize = 281;
const SYS_EPOLL_CTL: usize = 233;
const TEST_EVENT_BYTES: usize = if cfg!(target_arch = "x86_64") { 12 } else { 16 };
const LINUX_O_CLOEXEC: usize = 0x80000;
const LINUX_FD_CLOEXEC: usize = 1;
const EMFILE: i32 = 24;
const ENOMEM: i32 = 12;
const ENOSPC: i32 = 28;
const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const EPERM: i32 = 1;

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
    pub use crate::poll_objects;
    pub use crate::readiness_state as readiness;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/poll_objects.rs"]
pub mod poll_objects;
use readiness_state::*;
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    SocketUnix,
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
    EventFd,
    Epoll,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    flags: Word,
    kind: LinuxFileKind,
    posix_fd: Word,
    resource: Word,
    peer_port: u16,
}
impl LinuxFile {
    const EMPTY: Self = Self {
        kind: LinuxFileKind::Empty,
        flags: 0,
        posix_fd: 0,
        resource: 0,
        peer_port: 0,
    };
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
    files: Vec<LinuxFile>,
    terminal_id: Word,
}
struct Runtime {
    unix_sockets: UnixSocketMock,
    io_calls: usize,
    io_result: Result<Word, i32>,
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
    copy_error: bool,
    posix_shm_size: Word,
    poll_objects: poll_objects::Objects,
}
impl Runtime {
    fn is_signal_stopped(&self, _: Word) -> bool {
        false
    }
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        self.files
            .get(fd)
            .copied()
            .filter(|file| file.kind != LinuxFileKind::Empty)
    }
    fn terminal_id(&self, pid: Word) -> Option<Word> {
        self.managed
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.terminal_id)
    }
    fn allocate_linux_file(&mut self, pid: Word, file: LinuxFile, _: Word) -> Option<Word> {
        let index = self
            .files
            .iter()
            .position(|file| file.kind == LinuxFileKind::Empty)
            .unwrap_or(self.files.len());
        if index >= LINUX_FD_MAX {
            return None;
        }
        if index >= self.files.len() {
            self.files.resize(index + 1, LinuxFile::EMPTY);
        }
        self.files[index] = file;
        let process = self.managed_process_mut(pid)?;
        if index >= process.files.len() {
            process.files.resize(index + 1, LinuxFile::EMPTY);
        }
        process.files[index] = file;
        Some(index)
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
    if r.copy_error || (ptr == 0 && bytes != 0) {
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
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/epoll.rs"]
mod epoll;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/eventfd.rs"]
mod eventfd;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/pipe_wait.rs"]
mod pipe_wait;

// Guest-memory I/O is mocked; parking and wake dispatch are production code.
fn simulated_io(runtime: &mut Runtime, _: Word, _: Word, _: Word, _: Word) -> Result<Word, i32> {
    runtime.io_calls += 1;
    runtime.io_result
}
use self::simulated_io as sys_read;
use self::simulated_io as sys_write;
use self::simulated_io as sys_readv;
use self::simulated_io as sys_writev;

fn runtime() -> Runtime {
    RETURNS.with(|v| v.borrow_mut().clear());
    let mut scratch = vec![0; 4096].into_boxed_slice();
    Runtime {
        unix_sockets: UnixSocketMock,
        io_calls: 0,
        io_result: Ok(0),
        copy_error: false,
        posix_shm_size: 4096,
        poll_objects: poll_objects::Objects::new(),
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
            flags: 0,
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

#[test]
fn blocking_pipe_reads_wait_for_data_or_last_writer_close() {
    let mut runtime = runtime();
    let context = LinuxSyscallContext {
        number: SYS_READ,
        args: [4, 4096, 32, 0, 0, 0],
    };
    assert!(pipe_wait::park(&mut runtime, 1, context));
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    assert_eq!(runtime.io_calls, 0, "an empty live pipe is not EOF");
    runtime.pipe.len = 7;
    runtime.io_result = Ok(7);
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 7)]));
    assert!(runtime.managed[0].readiness_wait.is_none());
    assert!(pipe_wait::park(&mut runtime, 1, context));
    runtime.pipe.len = 0;
    runtime.pipe.writers = 0;
    runtime.io_result = Ok(0);
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 7), (101, 0)]));
}

#[test]
fn full_or_atomic_pipe_writes_stay_parked_until_retry_can_commit() {
    let mut runtime = runtime();
    let context = LinuxSyscallContext {
        number: SYS_WRITEV,
        args: [5, 4096, 2, 0, 0, 0],
    };
    runtime.pipe.len = LINUX_PIPE_BYTES;
    assert!(pipe_wait::park(&mut runtime, 1, context));
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    assert_eq!(runtime.io_calls, 0);
    runtime.pipe.len -= 1;
    runtime.io_result = Err(EAGAIN);
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    assert!(runtime.managed[0].readiness_wait.is_some());
    RETURNS.with(|values| assert!(values.borrow().is_empty()));
    runtime.pipe.len = 0;
    runtime.io_result = Ok(1024);
    wake_readiness_waiters(&mut runtime, READY_PIPE);
    RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 1024)]));
}

#[test]
fn nonblocking_pipe_io_does_not_install_a_waiter() {
    let mut runtime = runtime();
    runtime.files[4].flags = LINUX_O_NONBLOCK;
    let context = LinuxSyscallContext {
        number: SYS_READV,
        args: [4, 4096, 1, 0, 0, 0],
    };
    assert!(!pipe_wait::park(&mut runtime, 1, context));
    assert!(runtime.managed[0].readiness_wait.is_none());
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

fn counter_file(r: &Runtime, fd: Word) -> LinuxFile {
    r.linux_file(1, fd).unwrap()
}

fn counter_write(r: &mut Runtime, fd: Word, value: u64) -> Result<Word, i32> {
    eventfd::write(r, 1, counter_file(r, fd), &value as *const _ as Word, 8)
}

fn counter_read(r: &mut Runtime, fd: Word) -> Result<u64, i32> {
    let mut value = 0u64;
    eventfd::read(r, 1, counter_file(r, fd), &mut value as *mut _ as Word, 8)?;
    Ok(value)
}

fn watch(
    r: &mut Runtime,
    epfd: Word,
    fd: Word,
    operation: Word,
    events: u32,
    data: u64,
) -> Result<Word, i32> {
    let mut event = [0u8; TEST_EVENT_BYTES];
    event[..4].copy_from_slice(&events.to_le_bytes());
    event[TEST_EVENT_BYTES - 8..].copy_from_slice(&data.to_le_bytes());
    epoll::control(r, 1, epfd, operation, fd, event.as_ptr() as Word)
}

fn epoll_context(epfd: Word, output: &mut [u8], timeout: i32) -> LinuxSyscallContext {
    LinuxSyscallContext {
        number: SYS_EPOLL_WAIT,
        args: [
            epfd,
            output.as_mut_ptr() as Word,
            output.len() / TEST_EVENT_BYTES,
            timeout as Word,
            0,
            0,
        ],
    }
}

#[test]
fn eventfd_counter_semaphore_overflow_and_invalid_copy_are_transactional() {
    let mut r = runtime();
    let fd = eventfd::create(&mut r, 1, 2, 0).unwrap();
    assert_eq!(counter_write(&mut r, fd, 3), Ok(8));
    let file = counter_file(&r, fd);
    assert_eq!(eventfd::read(&mut r, 1, file, 0, 8), Err(EFAULT));
    assert_eq!(counter_read(&mut r, fd), Ok(5));
    assert_eq!(counter_read(&mut r, fd), Err(EAGAIN));
    assert_eq!(counter_write(&mut r, fd, u64::MAX), Err(EINVAL));
    assert_eq!(counter_write(&mut r, fd, u64::MAX - 1), Ok(8));
    assert_eq!(counter_write(&mut r, fd, 1), Err(EAGAIN));
    assert_eq!(eventfd::readiness(&r, file) & LINUX_POLLOUT, 0);
    assert_eq!(counter_read(&mut r, fd), Ok(u64::MAX - 1));
    let sem = eventfd::create(&mut r, 1, 2, 1 | LINUX_O_NONBLOCK | LINUX_O_CLOEXEC).unwrap();
    assert!(eventfd::nonblocking(&r, counter_file(&r, sem)));
    assert_eq!(
        counter_file(&r, sem).flags & LINUX_FD_CLOEXEC,
        LINUX_FD_CLOEXEC
    );
    assert_eq!(counter_read(&mut r, sem), Ok(1));
    assert_eq!(counter_read(&mut r, sem), Ok(1));
    assert_eq!(counter_read(&mut r, sem), Err(EAGAIN));
    assert_eq!(eventfd::create(&mut r, 1, 0, 2), Err(EINVAL));
}

#[test]
fn epoll_delivers_packed_user_data_and_level_triggering_without_consuming_counter() {
    let mut r = runtime();
    let fd = eventfd::create(&mut r, 1, 0, 0).unwrap();
    let ep = epoll::create(&mut r, 1, LINUX_O_CLOEXEC).unwrap();
    assert_eq!(
        watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, 0xfedcba9876543210),
        Ok(0)
    );
    assert_eq!(watch(&mut r, ep, fd, 1, 1, 1), Err(EEXIST));
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(0));
    counter_write(&mut r, fd, 1).unwrap();
    for _ in 0..2 {
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
        assert_eq!(&output[..4], &1u32.to_le_bytes());
        assert_eq!(
            &output[TEST_EVENT_BYTES - 8..],
            &0xfedcba9876543210u64.to_le_bytes()
        );
    }
    assert_eq!(counter_read(&mut r, fd), Ok(1));
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(0));
    assert_eq!(epoll::control(&mut r, 1, ep, 2, fd, 0), Ok(0));
    assert_eq!(epoll::control(&mut r, 1, ep, 2, fd, 0), Err(ENOENT));
}

#[test]
fn epoll_one_shot_rearms_and_fault_does_not_consume_the_event() {
    let mut r = runtime();
    let fd = eventfd::create(&mut r, 1, 1, 0).unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, fd, 1, 1 | (1 << 30), 7).unwrap();
    let bad = LinuxSyscallContext {
        number: SYS_EPOLL_WAIT,
        args: [ep, 0, 1, 0, 0, 0],
    };
    assert_eq!(
        sys_poll_action(&mut r, 1, bad),
        EmulationAction::Return(-(EFAULT as isize))
    );
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    r.copy_error = true;
    assert_eq!(
        sys_poll_action(&mut r, 1, ctx),
        EmulationAction::Return(-(EFAULT as isize))
    );
    r.copy_error = false;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(0));
    watch(&mut r, ep, fd, 3, 1 | (1 << 30), 8).unwrap();
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
    assert_eq!(&output[TEST_EVENT_BYTES - 8..], &8u64.to_le_bytes());
}

#[test]
fn eventfd_wakes_parked_epoll_and_timer_expiry_returns_zero() {
    let mut r = runtime();
    let fd = eventfd::create(&mut r, 1, 0, 0).unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, fd, 1, 1, 9).unwrap();
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, -1);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    counter_write(&mut r, fd, 1).unwrap();
    handle_readiness_changes(&mut r);
    assert!(r.managed[0].readiness_wait.is_none());
    RETURNS.with(|returns| assert_eq!(*returns.borrow(), [(101, 1)]));
    counter_read(&mut r, fd).unwrap();
    let ctx = epoll_context(ep, &mut output, 10);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    r.monotonic_ticks += 10;
    wake_readiness_waiters(&mut r, READY_TIMER);
    RETURNS.with(|returns| assert_eq!(returns.borrow().last(), Some(&(101, 0))));
}

#[test]
fn changing_a_parked_epoll_interest_set_retains_new_wakeup_sources() {
    let mut r = runtime();
    r.terminal_ready = false;
    let terminal = r
        .allocate_linux_file(
            1,
            LinuxFile {
                kind: LinuxFileKind::Terminal,
                ..LinuxFile::EMPTY
            },
            0,
        )
        .unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, -1);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    watch(&mut r, ep, terminal, 1, 1, 13).unwrap();
    handle_readiness_changes(&mut r);
    assert_ne!(
        r.managed[0].readiness_wait.unwrap().sources & READY_TERMINAL,
        0
    );
    r.terminal_ready = true;
    wake_readiness_waiters(&mut r, READY_TERMINAL);
    assert!(r.managed[0].readiness_wait.is_none());
    RETURNS.with(|returns| assert_eq!(*returns.borrow(), [(101, 1)]));
    assert_eq!(&output[TEST_EVENT_BYTES - 8..], &13u64.to_le_bytes());
}

#[test]
fn final_close_removes_registration_and_ids_cannot_alias_recycled_fds() {
    let mut r = runtime();
    let fd = eventfd::create(&mut r, 1, 1, 0).unwrap();
    let old = counter_file(&r, fd);
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, fd, 1, 1, 9).unwrap();
    r.files[fd] = LinuxFile::EMPTY;
    r.managed[0].files[fd] = LinuxFile::EMPTY;
    epoll::forget_closed(&mut r, 1, old);
    eventfd::release(&mut r, old);
    assert!(r.poll_objects.get(old.resource).is_none());
    let replacement = eventfd::create(&mut r, 1, 1, 0).unwrap();
    assert_eq!(replacement, fd);
    assert_ne!(counter_file(&r, replacement).resource, old.resource);
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(0));
}
