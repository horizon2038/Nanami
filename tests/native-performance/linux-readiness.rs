#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as a9n_abi;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
thread_local! {
    static PROTECT_TRACE: std::cell::RefCell<(Option<Vec<(Word, Word, Word, Word)>>, bool)> = const { std::cell::RefCell::new((None, false)) };
    static COPYOUT_FAIL_AT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static COPYOUT_ATTEMPTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RAW_COPYOUT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static OWNED_COPYOUT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FUTEX_DEADLINE_PROCESS_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FUTEX_ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static MEMFD_CLEANUP_TRACE: std::cell::RefCell<Option<MemfdCleanupTrace>> = const { std::cell::RefCell::new(None) };
    static VM_GROUP_QUERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MEMFD_SYNCS: std::cell::RefCell<(Vec<Word>, Option<usize>)> = const { std::cell::RefCell::new((Vec::new(), None)) };
}
struct MemfdCleanupTrace {
    releases: Vec<Word>,
    unmaps: Vec<(Word, Word)>,
    closes: Vec<Word>,
    fail_release: Option<Word>,
    fail_unmap: Option<Word>,
    fail_close: bool,
}
struct CountedAllocator;
fn count_futex_allocation() {
    let _ = FUTEX_ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() { count.set(Some(value + 1)); }
    });
}
unsafe impl std::alloc::GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let result = std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout);
        if !result.is_null() { count_futex_allocation(); }
        result
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, bytes: usize) -> *mut u8 {
        let result = std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, bytes);
        if !result.is_null() { count_futex_allocation(); }
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        std::alloc::GlobalAlloc::dealloc(&std::alloc::System, pointer, layout);
    }
}
#[global_allocator]
static ALLOCATOR: CountedAllocator = CountedAllocator;
// Image selection/serial reporting is cross-built and exercised on KVM.
// This harness isolates wait semantics; the bounded model is tested separately.
#[cfg(feature = "wait-shape")]
mod wait_shape {
    use super::*;
    pub(super) fn note(_: &mut Runtime, _: Word, _: &ReadinessWait, _: u8,
        _: Option<u128>, _: Option<Word>, _: Word, _: bool, _: isize) {}
}
pub mod terminal {
    pub const TERMINAL_NOTIFICATION_INPUT: usize = 1 << 52;
}
pub mod net {
    pub const NET_NOTIFICATION_RX: usize = 1 << 20;
}
pub mod input {
    pub const INPUT_NOTIFICATION_IDENTIFIER: usize = 1 << 19;
}
pub mod posix {
    pub fn posix_fsync(_: usize, fd: usize) -> Result<(), super::RequestError> {
        super::MEMFD_SYNCS.with(|trace| {
            let mut trace = trace.borrow_mut();
            trace.0.push(fd);
            if trace.1 == Some(trace.0.len()) { Err(super::RequestError) } else { Ok(()) }
        })
    }
    pub const POSIX_NOTIFICATION_LOCK_CHANGE: usize = 1 << 21;
    pub fn posix_dup(_: usize, _: usize) -> Result<usize, super::RequestError> {
        Err(super::RequestError)
    }
    pub fn posix_close(_: usize, fd: usize) -> Result<(), super::RequestError> {
        let fail = super::MEMFD_CLEANUP_TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            if let Some(trace) = trace.as_mut() { trace.closes.push(fd); trace.fail_close } else { false }
        });
        if fail { Err(super::RequestError) } else { Ok(()) }
    }
    pub fn posix_map_read_page(_: usize, _: usize, _: usize) -> Result<usize, super::RequestError> {
        Err(super::RequestError)
    }
    pub fn posix_unmap_read_page(_: usize, fd: usize, offset: usize) -> Result<(), super::RequestError> {
        let fail = super::MEMFD_CLEANUP_TRACE.with(|trace| {
            let mut trace = trace.borrow_mut();
            if let Some(trace) = trace.as_mut() { trace.unmaps.push((fd, offset)); trace.fail_unmap == Some(offset) } else { false }
        });
        if fail { Err(super::RequestError) } else { Ok(()) }
    }
}
const SYS_FLOCK: Word = 73;
const SYS_IO_URING_ENTER: Word = 426;
mod flock {
    use super::*;
    pub(super) fn lock(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<Word, i32> {
        Err(EAGAIN)
    }
}
// This harness isolates generic readiness. The actual PTY line discipline is
// tested in linux-pty; complete syscall integration runs in the QEMU fixture.
mod pty {
    use super::*;
    pub(super) fn readiness(runtime: &Runtime, _: LinuxFile) -> Result<i16, i32> {
        runtime.pty_observed_deadline.set(
            runtime.managed[0]
                .readiness_wait
                .as_ref()
                .and_then(|wait| wait.deadline),
        );
        Ok(0)
    }
    pub(super) fn update_deadline(runtime: &Runtime, _: Word, wait: &mut ReadinessWait) {
        if let Some(deadline) = runtime.pty_deadline_override {
            wait.deadline = deadline;
        }
    }
    pub(super) fn retry(
        _: &mut Runtime,
        _: Word,
        _: LinuxSyscallContext,
    ) -> Option<Result<Word, i32>> {
        None
    }
}
// The window-local console codecs are tested separately in linux-console.
mod console {
    use super::*;
    pub(super) fn device(_: LinuxFile) -> Option<()> {
        None
    }
    pub(super) fn ready(_: &Runtime, _: LinuxFile) -> bool {
        false
    }
    pub(super) fn writable(_: &Runtime, _: LinuxFile) -> bool {
        false
    }
}
#[path = "linux-uring.rs"]
mod uring_tests;
use uring_tests::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/io_uring.rs"]
mod io_uring;
pub use std::println;
type Word = usize;
thread_local! {
    static CLOSE_TERMINAL_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CLOSE_RIGHTS_LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
struct UnixSocketMock;
impl UnixSocketMock {
    fn references(&self, _: LinuxFile) -> bool {
        CLOSE_RIGHTS_LOOKUPS.with(|count| count.set(count.get() + 1));
        false
    }
}
mod unix {
    use super::*;
    pub(super) fn retry(
        _: &mut Runtime,
        _: Word,
        _: LinuxSyscallContext,
    ) -> Option<Result<Word, i32>> {
        None
    }
    pub(super) fn readiness(runtime: &Runtime, _: Word) -> Result<i16, i32> {
        runtime.unix_queries.set(runtime.unix_queries.get() + 1);
        Ok(runtime.unix_ready)
    }
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
const LINUX_O_APPEND: usize = 0x400;
const LINUX_O_RDWR: usize = 2;
const LINUX_O_ACCMODE: usize = 3;
const LINUX_O_RDONLY: usize = 0;
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
const ENOSYS: i32 = 38;
const ETIMEDOUT: i32 = 110;
const SYS_FUTEX: Word = 202;
const EBUSY: i32 = 16;
const EACCES: i32 = 13;
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_PROT_NONE: Word = 0;
const LINUX_PROT_WRITE: Word = 2;
const LINUX_MAP_FIXED: Word = 16;
const LINUX_MAP_ANONYMOUS: Word = 32;
const LINUX_MREMAP_MAYMOVE: Word = 1;
const LINUX_MREMAP_FIXED: Word = 2;
const ALTER_PROCESS_MAPPING_MAX: usize = 128;

#[derive(Clone, Copy)]
pub struct ProcessMapping {
    base: Word,
    size: Word,
    prot: Word,
}
impl ProcessMapping {
    const EMPTY: Self = Self {
        base: 0,
        size: 0,
        prot: 0,
    };
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mapping_table.rs"]
mod mapping_table;
use mapping_table::MappingTable;
impl Default for MappingTable {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Default, Clone, Copy)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
}
#[derive(Debug, PartialEq, Clone, Copy)]
enum EmulationAction {
    Return(isize),
    Park,
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/readiness.rs"]
pub mod readiness_state;
pub mod state {
    pub use crate::poll_objects;
    pub use crate::readiness_state as readiness;
    pub use crate::task_sync;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/poll_objects.rs"]
pub mod poll_objects;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/task_sync.rs"]
pub mod task_sync;
use readiness_state::*;
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    PtyMaster,
    PtySlave,
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
    TimerFd,
    SignalFd,
    MemoryFile,
    Epoll,
    IoUring,
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
    vm_group: Word,
    signal_waiting: bool,
    signal_context: LinuxSyscallContext,
    mappings: MappingTable,
    signals: Option<signal_state::Signals>,
    pid: Word,
    pcb: Word,
    personality: Word,
    exited: bool,
    readiness_wait: Option<ReadinessWait>,
    files: Vec<LinuxFile>,
    terminal_id: Word,
}
type ManagedProcess = Process;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/mappings.rs"]
mod mappings;
#[path = "../../nanami/servers/apps/alter/shared/src/common/signals.rs"]
pub mod signal_state;
mod common {
    pub use crate::signal_state as signals;
}
impl Process {
    fn signals(&self) -> Option<&signal_state::Signals> {
        self.signals.as_ref()
    }
    fn signals_mut(&mut self) -> &mut signal_state::Signals {
        self.signals.get_or_insert_with(signal_state::Signals::new)
    }
    fn signal_stopped(&self) -> bool {
        self.signals().is_some_and(|signals| signals.stopped)
    }
}
struct Runtime {
    posix_port: Word,
    tasks: task_sync::Tasks,
    unix_sockets: UnixSocketMock,
    io_calls: usize,
    drain_pipe_on_read: bool,
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
    managed: Vec<Process>,
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    readiness_changes: Word,
    clock_error: bool,
    alarm_error: bool,
    realtime_anchor: Option<(Word, Word)>,
    copy_error: bool,
    posix_shm_size: Word,
    poll_objects: poll_objects::Objects,
    pty_deadline_override: Option<Option<Word>>,
    pty_observed_deadline: std::cell::Cell<Option<Word>>,
    unix_ready: i16,
    unix_queries: std::cell::Cell<usize>,
}
impl Runtime {
    fn vm_group(&self, pid: Word) -> Word {
        VM_GROUP_QUERIES.with(|count| count.set(count.get() + 1));
        self.managed_process(pid).map_or(pid, |process| {
            if process.vm_group == 0 {
                pid
            } else {
                process.vm_group
            }
        })
    }
    fn park_signal_waiter(&mut self, pid: Word, _: Word, context: LinuxSyscallContext) -> bool {
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.signal_waiting = true;
        process.signal_context = context;
        true
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        FUTEX_DEADLINE_PROCESS_LOOKUPS.with(|count| count.set(count.get() + 1));
        self.managed.iter().find(|process| process.pid == pid)
    }
    fn is_signal_stopped(&self, pid: Word) -> bool {
        self.managed_process(pid)
            .is_some_and(Process::signal_stopped)
    }
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        self.files
            .get(fd)
            .copied()
            .filter(|file| file.kind != LinuxFileKind::Empty)
    }
    fn terminal_id(&self, pid: Word) -> Option<Word> {
        CLOSE_TERMINAL_LOOKUPS.with(|count| count.set(count.get() + 1));
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
fn read_target_memory(r: &mut Runtime, pid: Word, ptr: Word, bytes: Word) -> Result<(), i32> {
    if ptr == 0 && bytes != 0 {
        return Err(EFAULT);
    }
    r.reads += usize::from(bytes != 0);
    if bytes != 0 {
        unsafe {
            let source = alias_pointer(pid, ptr, bytes).unwrap_or(ptr);
            std::ptr::copy_nonoverlapping(source as *const u8, r.posix_shm as *mut u8, bytes);
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
fn copyout_owned(r: &mut Runtime, _: Word, ptr: Word, source: &[u8]) -> Result<(), i32> {
    OWNED_COPYOUT_CALLS.with(|count|count.set(count.get()+1));
    if injected_copyout_failure() { return Err(EFAULT); }
    if r.copy_error || (ptr == 0 && !source.is_empty()) { return Err(EFAULT); }
    r.writes += usize::from(!source.is_empty());
    if !source.is_empty() {
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr(),ptr as *mut u8,source.len()); }
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
    pub type HardwareContext = [usize; 35];
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
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/fd_events.rs"]
mod fd_events;
mod realtime {
    use super::*;
    pub(super) fn realtime(runtime: &mut Runtime) -> Result<(Word, Word), i32> {
        Ok(runtime.realtime_anchor.unwrap_or((0, 0)))
    }
}
fn read_shm_u64(runtime: &Runtime, offset: Word) -> Word {
    unsafe { ((runtime.posix_shm + offset) as *const Word).read_unaligned() }
}
fn write_guest_u64(runtime: &mut Runtime, pid: Word, ptr: Word, value: Word) -> Result<(), i32> {
    unsafe {
        write_u64(runtime.posix_shm, value);
    }
    write_target_memory(runtime, pid, ptr, 8)
}
fn write_target_memory_from(
    _: &Runtime,
    _: Word,
    destination: Word,
    source: Word,
    bytes: Word,
) -> Result<(), i32> {
    if destination == 0 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes);
    }
    Ok(())
}
fn copyout_from(
    runtime: &Runtime,
    pid: Word,
    destination: Word,
    source: Word,
    bytes: Word,
) -> Result<(), i32> {
    RAW_COPYOUT_CALLS.with(|count|count.set(count.get()+1));
    if injected_copyout_failure() { return Err(EFAULT); }
    write_target_memory_from(runtime, pid, destination, source, bytes)
}
fn injected_copyout_failure() -> bool {
    let index=COPYOUT_ATTEMPTS.with(|count|count.replace(count.get()+1));
    COPYOUT_FAIL_AT.with(|at|at.get()==Some(index))
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/futex.rs"]
mod futex;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/memfd.rs"]
mod memfd;
#[path = "baseline-private-protect.rs"]
mod baseline_private_protect;
mod guest_views {
    pub(super) fn invalidate(_: &super::Runtime, _: usize) {}
}
fn record_action_result(_: &mut Runtime, _: Word, _: Word, _: EmulationAction) {}
fn read_shm_u32(runtime: &Runtime, offset: Word) -> u32 {
    unsafe { ((runtime.posix_shm + offset) as *const u32).read_unaligned() }
}
fn write_guest_u32(_: &mut Runtime, pid: Word, ptr: Word, value: u32) -> Result<(), i32> {
    let destination = alias_pointer(pid, ptr, 4).unwrap_or(ptr);
    if destination == 0 {
        return Err(EFAULT);
    }
    unsafe {
        (destination as *mut u32).write_unaligned(value);
    }
    Ok(())
}
mod clocks {
    use super::*;
    pub(super) fn sleep_deadline(
        runtime: &mut Runtime,
        pid: Word,
        ptr: Word,
        _: Word,
        absolute: bool,
    ) -> Result<Option<Word>, i32> {
        read_target_memory(runtime, pid, ptr, 16)?;
        let seconds = read_shm_u64(runtime, 0);
        let nanos = read_shm_u64(runtime, 8);
        if nanos >= 1_000_000_000 {
            return Err(EINVAL);
        }
        let ticks = seconds * runtime.monotonic_tick_hz
            + (nanos * runtime.monotonic_tick_hz).div_ceil(1_000_000_000);
        Ok(Some(if absolute {
            ticks
        } else {
            runtime.monotonic_ticks + ticks
        }))
    }
}
fn alias_pointer(pid: Word, pointer: Word, bytes: Word) -> Option<Word> {
    ALIASES.with(|aliases| {
        aliases
            .borrow()
            .iter()
            .find_map(|(&(owner, base), (ram, _))| {
                if owner == pid
                    && pointer >= base
                    && pointer
                        .checked_add(bytes)
                        .is_some_and(|end| end <= base + ram.len())
                {
                    Some(ram.as_ptr() as Word + pointer - base)
                } else {
                    None
                }
            })
    })
}
type Ram = std::rc::Rc<Box<[u8]>>;
thread_local! {
    static OWNED: std::cell::RefCell<std::collections::BTreeMap<Word, Ram>> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
    static ALIASES: std::cell::RefCell<std::collections::BTreeMap<(Word, Word), (Ram, Word)>> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}
#[derive(Debug)]
pub struct RequestError;
fn map_request_error(_: RequestError) -> i32 {
    EIO
}
pub fn request_memory_create(bytes: Word) -> Result<Word, RequestError> {
    let ram = std::rc::Rc::new(vec![0u8; bytes].into_boxed_slice());
    let address = ram.as_ptr() as Word;
    OWNED.with(|owned| owned.borrow_mut().insert(address, ram));
    Ok(address)
}

pub fn request_process_memory_compare_exchange_u32(
    pid: Word,
    address: Word,
    expected: u32,
    desired: u32,
) -> Result<u32, RequestError> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let pointer = alias_pointer(pid, address, 4).unwrap_or(address);
    let atomic = unsafe { &*(pointer as *const AtomicU32) };
    Ok(
        match atomic.compare_exchange(expected, desired, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(previous) | Err(previous) => previous,
        },
    )
}
pub fn request_mapping_share_at(
    pid: Word,
    source: Word,
    _: Word,
    target: Word,
) -> Result<Word, RequestError> {
    let ram = OWNED.with(|owned| owned.borrow().get(&source).unwrap().clone());
    ALIASES.with(|aliases| {
        assert!(aliases
            .borrow_mut()
            .insert((pid, target), (ram, 7))
            .is_none())
    });
    Ok(target)
}
pub fn request_mapping_release(address: Word, _: Word) -> Result<(), RequestError> {
    let fail = MEMFD_CLEANUP_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        if let Some(trace) = trace.as_mut() { trace.releases.push(address); trace.fail_release == Some(address) } else { false }
    });
    if fail { return Err(RequestError); }
    OWNED.with(|owned| {
        owned.borrow_mut().remove(&address);
    });
    Ok(())
}
pub fn request_process_mapping_release_sparse(
    pid: Word,
    base: Word,
    bytes: Word,
) -> Result<(), RequestError> {
    ALIASES.with(|aliases| {
        aliases
            .borrow_mut()
            .retain(|&(owner, va), _| owner != pid || va < base || va >= base + bytes);
    });
    Ok(())
}
pub fn request_process_mapping_protect(
    pid: Word,
    base: Word,
    bytes: Word,
    prot: Word,
) -> Result<(), RequestError> {
    let fail = PROTECT_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        if let Some(calls) = &mut trace.0 { calls.push((pid, base, bytes, prot)); }
        trace.1
    });
    if fail { return Err(RequestError); }
    ALIASES.with(|aliases| {
        for va in (base..base + bytes).step_by(LINUX_PAGE_SIZE) {
            aliases
                .borrow_mut()
                .get_mut(&(pid, va))
                .ok_or(RequestError)?
                .1 = prot;
        }
        Ok(())
    })
}
pub fn request_process_map_anonymous_at(
    pid: Word,
    base: Word,
    bytes: Word,
) -> Result<(Word, Word), RequestError> {
    for va in (base..base + bytes).step_by(LINUX_PAGE_SIZE) {
        let source = request_memory_create(LINUX_PAGE_SIZE)?;
        request_mapping_share_at(pid, source, LINUX_PAGE_SIZE, va)?;
        request_mapping_release(source, LINUX_PAGE_SIZE)?;
    }
    Ok((base, bytes))
}
pub fn request_process_memory_clone(
    parent: Word,
    child: Word,
    base: Word,
    bytes: Word,
) -> Result<(), RequestError> {
    ALIASES.with(|aliases| {
        let aliases = aliases.borrow();
        let source = aliases.get(&(parent, base)).ok_or(RequestError)?.0.as_ptr();
        let destination = aliases.get(&(child, base)).ok_or(RequestError)?.0.as_ptr();
        unsafe {
            std::ptr::copy_nonoverlapping(source, destination as *mut u8, bytes);
        }
        Ok(())
    })
}
fn read_c_string(runtime: &mut Runtime, pid: Word, pointer: Word) -> Result<Word, i32> {
    let len = unsafe {
        std::ffi::CStr::from_ptr(pointer as *const i8)
            .to_bytes()
            .len()
    };
    read_target_memory(runtime, pid, pointer, len)?;
    Ok(len)
}
fn align_up_word(value: Word, alignment: Word) -> Word {
    value.div_ceil(alignment) * alignment
}
fn copy_same_process_range(
    _: &mut Runtime,
    pid: Word,
    source: Word,
    target: Word,
    bytes: Word,
) -> Result<(), i32> {
    for offset in (0..bytes).step_by(LINUX_PAGE_SIZE) {
        let source = alias_pointer(pid, source + offset, LINUX_PAGE_SIZE).ok_or(EFAULT)?;
        let target = alias_pointer(pid, target + offset, LINUX_PAGE_SIZE).ok_or(EFAULT)?;
        unsafe {
            std::ptr::copy_nonoverlapping(source as *const u8, target as *mut u8, LINUX_PAGE_SIZE);
        }
    }
    Ok(())
}
fn reserve_none_mapping(
    runtime: &mut Runtime,
    pid: Word,
    bytes: Word,
) -> Result<(Word, Word), i32> {
    let base = runtime
        .managed_process(pid)
        .unwrap()
        .mappings
        .iter()
        .filter(|mapping| mapping.base != 0)
        .map(|mapping| mapping.base + mapping.size)
        .max()
        .unwrap_or(0x80000000);
    assert!(runtime.add_mapping(pid, base, bytes, 0));
    Ok((base, bytes))
}
fn sys_mmap(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    bytes: Word,
    prot: Word,
    _: Word,
    _: Word,
    _: Word,
) -> Result<Word, i32> {
    assert!(runtime.add_mapping(pid, address, bytes, prot));
    Ok(address)
}
fn sys_munmap(runtime: &mut Runtime, pid: Word, address: Word, bytes: Word) -> Result<Word, i32> {
    memfd::unmap(runtime, pid, address, bytes)?;
    assert!(runtime.remove_mapping(pid, address, bytes));
    Ok(0)
}
fn split_ticks(ticks: Word, hz: Word) -> (Word, Word) {
    (
        ticks / hz,
        ((ticks % hz) as u128 * 1_000_000_000 / hz as u128) as Word,
    )
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/pipe_wait.rs"]
mod pipe_wait;

// Guest-memory I/O is mocked; parking and wake dispatch are production code.
fn simulated_io(runtime: &mut Runtime, _: Word, fd: Word, _: Word, _: Word) -> Result<Word, i32> {
    runtime.io_calls += 1;
    if runtime.drain_pipe_on_read && runtime.files[fd].kind == LinuxFileKind::PipeRead {
        runtime.drain_pipe_on_read = false;
        runtime.pipe.len = 0;
        runtime.readiness_changes |= READY_PIPE;
    }
    runtime.io_result
}
use self::simulated_io as sys_read;
use self::simulated_io as sys_write;
use self::simulated_io as sys_readv;
use self::simulated_io as sys_writev;

fn runtime() -> Runtime {
    PROTECT_TRACE.with(|trace| *trace.borrow_mut() = (None, false));
    COPYOUT_FAIL_AT.with(|at|at.set(None));
    COPYOUT_ATTEMPTS.with(|count|count.set(0));
    RAW_COPYOUT_CALLS.with(|count|count.set(0));
    OWNED_COPYOUT_CALLS.with(|count|count.set(0));
    MEMFD_SYNCS.with(|trace| *trace.borrow_mut() = (Vec::new(), None));
    MEMFD_CLEANUP_TRACE.with(|trace| *trace.borrow_mut() = None);
    RETURNS.with(|v| v.borrow_mut().clear());
    let mut scratch = vec![0; 4096].into_boxed_slice();
    Runtime {
        posix_port: 0,
        tasks: task_sync::Tasks::new(),
        unix_sockets: UnixSocketMock,
        io_calls: 0,
        drain_pipe_on_read: false,
        io_result: Ok(0),
        copy_error: false,
        posix_shm_size: 4096,
        poll_objects: poll_objects::Objects::new(),
        pty_deadline_override: None,
        pty_observed_deadline: std::cell::Cell::new(None),
        unix_ready: 0,
        unix_queries: std::cell::Cell::new(0),
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
        managed: vec![
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
        realtime_anchor: Some((0, 0)),
    }
}

fn private_protect_fixture(pages: usize) -> (Runtime, Word) {
    ALIASES.with_borrow_mut(|aliases| aliases.clear());
    let mut r = runtime();
    let base = 0x8000_0000;
    request_process_map_anonymous_at(1, base, pages * LINUX_PAGE_SIZE).unwrap();
    for page in 0..pages {
        let prot = [1, 3, 5][page % 3];
        assert!(r.add_mapping(1, base + page * LINUX_PAGE_SIZE, LINUX_PAGE_SIZE, prot));
        request_process_mapping_protect(1, base + page * LINUX_PAGE_SIZE, LINUX_PAGE_SIZE, prot).unwrap();
    }
    PROTECT_TRACE.with(|trace| trace.borrow_mut().0 = Some(Vec::new()));
    (r, base)
}

#[test]
fn private_protection_coalesces_fragmented_vmas_with_identical_page_results() {
    for pages in [1, 2, 16, 64] {
        for prot in [0, 1, 3, 5] {
            for old in [true, false] {
                let (r, base) = private_protect_fixture(pages);
                PROTECT_TRACE.with_borrow_mut(|trace| trace.0 = Some(Vec::with_capacity(pages)));
                let apply = if old { baseline_private_protect::previous_protect } else { memfd::protect };
                FUTEX_ALLOCATIONS.with(|count| count.set(Some(0)));
                let result = apply(&r, 1, base, pages * LINUX_PAGE_SIZE, prot);
                let allocations = FUTEX_ALLOCATIONS.with(|count| count.replace(None).unwrap());
                assert_eq!(result, Ok(()));
                if old { assert!(allocations > 0); } else { assert_eq!(allocations, 0); }
                PROTECT_TRACE.with(|trace| {
                    let trace = trace.borrow(); let calls = trace.0.as_ref().unwrap();
                    assert_eq!(calls.len(), if old { pages } else { 1 });
                    if !old { assert_eq!(calls[0], (1, base, pages * LINUX_PAGE_SIZE, prot)); }
                });
                ALIASES.with_borrow(|aliases| {
                    for page in 0..pages {
                        let entry = &aliases[&(1, base + page * LINUX_PAGE_SIZE)];
                        assert_eq!(entry.1, prot);
                        assert!(entry.0.iter().all(|byte| *byte == 0));
                    }
                });
            }
        }
    }
}

#[test]
fn private_protection_holes_and_transport_errors_never_commit_or_retry() {
    for old in [true, false] {
        let (mut r, base) = private_protect_fixture(3);
        assert!(r.remove_mapping(1, base + LINUX_PAGE_SIZE, LINUX_PAGE_SIZE));
        let apply = if old { baseline_private_protect::previous_protect } else { memfd::protect };
        assert_eq!(apply(&r, 1, base, 3 * LINUX_PAGE_SIZE, 1), Err(ENOMEM));
        PROTECT_TRACE.with_borrow(|trace| assert!(trace.0.as_ref().unwrap().is_empty()));
    }
    let (r, base) = private_protect_fixture(16);
    PROTECT_TRACE.with_borrow_mut(|trace| trace.1 = true);
    assert_eq!(memfd::protect(&r, 1, base, 16 * LINUX_PAGE_SIZE, 1), Err(EIO));
    PROTECT_TRACE.with_borrow(|trace| assert_eq!(trace.0.as_ref().unwrap().len(), 1));
    ALIASES.with_borrow(|aliases| {
        for page in 0..16 { assert_eq!(aliases[&(1, base + page * LINUX_PAGE_SIZE)].1, [1, 3, 5][page % 3]); }
    });
}

#[test]
fn shared_eof_holes_keep_previous_plan_and_unrelated_shared_maps_do_not_split_private_ranges() {
    for old in [true, false] {
        ALIASES.with_borrow_mut(|aliases| aliases.clear());
        let mut r = runtime();
        let fd = memfd::create(&mut r, 1, c"protect".as_ptr() as Word, 2).unwrap();
        let file = r.linux_file(1, fd).unwrap();
        memfd::truncate(&mut r, file, LINUX_PAGE_SIZE).unwrap();
        let base = memfd::map(&mut r, 1, 0, 2 * LINUX_PAGE_SIZE, 3, 1, file, 0).unwrap();
        PROTECT_TRACE.with_borrow_mut(|trace| trace.0 = Some(Vec::new()));
        let apply = if old { baseline_private_protect::previous_protect } else { memfd::protect };
        assert_eq!(apply(&r, 1, base, 2 * LINUX_PAGE_SIZE, 1), Ok(()));
        PROTECT_TRACE.with_borrow(|trace| assert_eq!(trace.0.as_ref().unwrap().as_slice(), [(1, base, LINUX_PAGE_SIZE, 1)]));
        assert!(memfd::bus_fault(&r, 1, base + LINUX_PAGE_SIZE));
    }
    let (mut r, base) = private_protect_fixture(16);
    let fd = memfd::create(&mut r, 1, c"elsewhere".as_ptr() as Word, 2).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    memfd::truncate(&mut r, file, LINUX_PAGE_SIZE).unwrap();
    let shared = memfd::map(&mut r, 1, 0, LINUX_PAGE_SIZE, 3, 1, file, 0).unwrap();
    assert!(shared + LINUX_PAGE_SIZE <= base || shared >= base + 16 * LINUX_PAGE_SIZE);
    PROTECT_TRACE.with_borrow_mut(|trace| trace.0 = Some(Vec::new()));
    assert_eq!(memfd::protect(&r, 1, base, 16 * LINUX_PAGE_SIZE, 1), Ok(()));
    PROTECT_TRACE.with_borrow(|trace| assert_eq!(trace.0.as_ref().unwrap().len(), 1));
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
    assert_eq!(runtime.managed[0].readiness_wait.unwrap().sources, READY_PIPE | READY_PIPE_WRITE);
    wake_readiness_waiters(&mut runtime, READY_PIPE_WRITE);
    assert_eq!(runtime.io_calls, 0);
    runtime.pipe.len -= 1;
    runtime.io_result = Err(EAGAIN);
    wake_readiness_waiters(&mut runtime, READY_PIPE_WRITE);
    assert!(runtime.managed[0].readiness_wait.is_some());
    RETURNS.with(|values| assert!(values.borrow().is_empty()));
    runtime.pipe.len = 0;
    runtime.io_result = Ok(1024);
    wake_readiness_waiters(&mut runtime, READY_PIPE_WRITE);
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
fn finite_poll_samples_deadline_once_and_ready_wake_needs_no_clock() {
    let mut r = runtime();
    let mut fds = [fd(4, LINUX_POLLIN)];
    r.pipe.len = 1;
    assert_eq!(poll(&mut r, &mut fds, 25), EmulationAction::Return(1));
    assert_eq!(r.clocks, 1);
    assert!(r.arms.is_empty());
    assert_eq!(fds[0].revents, LINUX_POLLIN);
    r.pipe.len = 0;
    assert_eq!(poll(&mut r, &mut fds, 25), EmulationAction::Park);
    assert_eq!(r.clocks, 2);
    r.pipe.len = 1;
    r.clock_error = true;
    wake_readiness_waiters(&mut r, READY_PIPE);
    assert_eq!(r.clocks, 2);
    assert_eq!(fds[0].revents, LINUX_POLLIN);
    RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
    assert!(r.managed[0].readiness_wait.is_none());
    // Starting a new finite wait still requires a fresh deadline sample.
    assert_eq!(poll(&mut r, &mut fds, 25), EmulationAction::Return(-5));
    assert_eq!(r.clocks, 3);
}

#[test]
fn remaining_time_waits_keep_completion_clock_on_ready_and_wake() {
    for number in [SYS_PPOLL, SYS_SELECT, SYS_PSELECT6] {
        let mut r = runtime();
        let mut fds = [fd(4, LINUX_POLLIN)];
        let mut bits = 1usize << 4;
        let mut timeout = [0i64, 25_000];
        let ctx = LinuxSyscallContext {
            number,
            args: if number == SYS_PPOLL {
                [fds.as_mut_ptr() as Word, 1, timeout.as_mut_ptr() as Word, 0, 0, 0]
            } else {
                [5, &mut bits as *mut _ as Word, 0, 0, timeout.as_mut_ptr() as Word, 0]
            },
        };
        r.pipe.len = 1;
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
        assert_eq!(r.clocks, 2, "syscall {number}");
        r.pipe.len = 0;
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
        assert_eq!(r.clocks, 3);
        r.pipe.len = 1;
        r.clock_error = true;
        wake_readiness_waiters(&mut r, READY_PIPE);
        assert_eq!(r.clocks, 4);
        RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, -5)]));
    }
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
fn pipe_direction_notifications_filter_interests_and_general_close_still_wakes() {
    for (descriptor, events, matching, unrelated) in [
        (4, LINUX_POLLIN, READY_PIPE_READ, READY_PIPE_WRITE),
        (5, LINUX_POLLOUT, READY_PIPE_WRITE, READY_PIPE_READ),
    ] {
        let mut r = runtime();
        r.pipe.len = if descriptor == 4 { 0 } else { LINUX_PIPE_BYTES };
        let mut fds = [fd(descriptor, events)];
        assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
        assert_eq!(r.managed[0].readiness_wait.unwrap().sources, READY_PIPE | matching);
        r.pipe.len = if descriptor == 4 { 1 } else { LINUX_PIPE_BYTES - 1 };
        wake_readiness_waiters(&mut r, unrelated);
        assert!(r.managed[0].readiness_wait.is_some());
        wake_readiness_waiters(&mut r, matching);
        assert!(r.managed[0].readiness_wait.is_none());
        assert_eq!(fds[0].revents, events);
        RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 1)]));
    }
    for (descriptor, expected) in [(4, LINUX_POLLHUP), (5, LINUX_POLLERR)] {
        let mut r = runtime();
        let mut fds = [fd(descriptor, 0)];
        assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
        assert_eq!(r.managed[0].readiness_wait.unwrap().sources, READY_PIPE);
        if descriptor == 4 { r.pipe.writers = 0; } else { r.pipe.readers = 0; }
        wake_readiness_waiters(&mut r, READY_PIPE);
        assert!(r.managed[0].readiness_wait.is_none());
        assert_eq!(fds[0].revents, expected);
    }
}

#[test]
fn unix_read_and_write_notifications_only_scan_matching_interests() {
    for (events, matching, unrelated) in [
        (LINUX_POLLIN, READY_UNIX_READ, READY_UNIX_WRITE),
        (LINUX_POLLOUT, READY_UNIX_WRITE, READY_UNIX_READ),
    ] {
        let mut r = runtime();
        let descriptor = r
            .allocate_linux_file(
                1,
                LinuxFile {
                    kind: LinuxFileKind::SocketUnix,
                    resource: 111,
                    ..LinuxFile::EMPTY
                },
                0,
            )
            .unwrap();
        let mut fds = [fd(descriptor as i32, events)];
        assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
        assert_eq!(
            r.managed[0].readiness_wait.unwrap().sources,
            READY_UNIX | matching
        );
        r.unix_queries.set(0);
        r.unix_ready = events;
        wake_readiness_waiters(&mut r, unrelated);
        assert_eq!(r.unix_queries.get(), 0);
        assert!(r.managed[0].readiness_wait.is_some());
        r.managed[0].signals_mut().stopped = true;
        wake_readiness_waiters(&mut r, matching);
        assert_eq!(r.unix_queries.get(), 0);
        assert!(r.managed[0].readiness_wait.is_some());
        r.managed[0].signals_mut().stopped = false;
        wake_readiness_waiters(&mut r, matching);
        assert!(r.managed[0].readiness_wait.is_none());
        assert_eq!(fds[0].revents, events);
        RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 1)]));
    }
}

#[test]
fn unix_status_changes_wake_zero_event_interests_and_report_hup_or_invalid_fd() {
    for closed in [false, true] {
        let mut r = runtime();
        let descriptor = r
            .allocate_linux_file(
                1,
                LinuxFile {
                    kind: LinuxFileKind::SocketUnix,
                    resource: 111,
                    ..LinuxFile::EMPTY
                },
                0,
            )
            .unwrap();
        let mut fds = [fd(descriptor as i32, 0)];
        assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
        assert_eq!(r.managed[0].readiness_wait.unwrap().sources, READY_UNIX);
        r.unix_queries.set(0);
        r.unix_ready = LINUX_POLLHUP;
        if closed {
            r.files[descriptor] = LinuxFile::EMPTY;
        }
        wake_readiness_waiters(&mut r, READY_UNIX_READ | READY_UNIX_WRITE);
        assert_eq!(r.unix_queries.get(), 0);
        assert!(r.managed[0].readiness_wait.is_some());
        wake_readiness_waiters(&mut r, READY_UNIX);
        assert!(r.managed[0].readiness_wait.is_none());
        let expected = if closed {
            LINUX_POLLNVAL
        } else {
            LINUX_POLLHUP
        };
        assert_eq!(fds[0].revents, expected);
        RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 1)]));
    }
}

#[test]
fn stopped_readiness_waiter_only_wakes_after_continue() {
    let mut r = runtime();
    let mut fds = [fd(4, LINUX_POLLIN)];
    assert_eq!(poll(&mut r, &mut fds, -1), EmulationAction::Park);
    r.managed[0].signals_mut().stopped = true;
    r.pipe.len = 1;
    wake_readiness_waiters(&mut r, READY_PIPE);
    assert!(r.managed[0].readiness_wait.is_some());
    RETURNS.with(|values| assert!(values.borrow().is_empty()));
    r.managed[0].signals_mut().stopped = false;
    wake_readiness_waiters(&mut r, READY_PIPE);
    assert!(r.managed[0].readiness_wait.is_none());
    assert_eq!(fds[0].revents, LINUX_POLLIN);
    RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(101, 1)]));
}

#[test]
fn pending_poll_scans_preserve_interest_tail_and_nonzero_select_error_results() {
    for select in [false, true] {
      for count in [1, 8, 9, LINUX_FD_MAX] {
        let mut r = runtime();
        let descriptor = if select { 5 } else { 4 };
        if select { r.pipe.readers = 0; }
        let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
        entries[0] = fd(descriptor, if select { LINUX_POLLPRI } else { LINUX_POLLIN });
        if count < LINUX_FD_MAX {
            entries[LINUX_FD_MAX - 1] = PollFd { fd: 47, events: 0x1234, revents: 0x5678 };
        }
        let mut output = [PollFd::EMPTY; LINUX_FD_MAX];
        output[0] = fd(descriptor, LINUX_POLLIN);
        let context = LinuxSyscallContext {
            number: if select { SYS_SELECT } else { SYS_POLL },
            args: if select { [6, 0, 0, 0, 0, 0] }
                else { [output.as_mut_ptr() as Word, count, Word::MAX, 0, 0, 0] },
        };
        r.managed[0].readiness_wait = Some(ReadinessWait {
            context, entries, count, sources: READY_PIPE, deadline: None, select,
        });
        poll_wait::take_pending_wait_bytes();
        for _ in 0..32 {
            wake_readiness_waiters(&mut r, READY_PIPE);
            let wait = r.managed[0].readiness_wait.as_ref().unwrap();
            assert_eq!((wait.context.number, wait.context.args), (context.number, context.args));
            assert_eq!((wait.count, wait.select, wait.deadline), (count, select, None));
            assert_eq!((wait.entries[0].fd, wait.entries[0].events),
                (descriptor, if select { LINUX_POLLPRI } else { LINUX_POLLIN }));
            assert_eq!(wait.entries[0].revents, if select { LINUX_POLLERR } else { 0 });
            assert_eq!(wait.sources, if select { READY_PIPE } else { READY_PIPE | READY_PIPE_READ });
            if count < LINUX_FD_MAX {
                let tail = &wait.entries[LINUX_FD_MAX - 1];
                assert_eq!((tail.fd, tail.events, tail.revents), (47, 0x1234, 0x5678));
            }
        }
        let bytes = poll_wait::take_pending_wait_bytes();
        println!("POLL-PENDING-FIELDS select={select} count={count} scans=32 bytes={bytes}");
        RETURNS.with(|v| assert!(v.borrow().is_empty()));
        // Closing the descriptor must still complete the saved wait with its
        // original syscall semantics; no inactive sentinel becomes an interest.
        r.files[descriptor as usize] = LinuxFile::EMPTY;
        wake_readiness_waiters(&mut r, READY_PIPE);
        if select {
            RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, -(EBADF as isize))]));
        } else {
            RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
            assert_eq!(output[0].revents, LINUX_POLLNVAL);
        }
        assert!(r.managed[0].readiness_wait.is_none());
      }
    }
}

#[test]
fn pty_deadline_is_published_before_scan_and_preserves_the_interest_snapshot() {
    let mut r = runtime();
    let descriptor = r.files.len() as i32;
    r.files.push(LinuxFile {
        kind: LinuxFileKind::PtySlave,
        ..LinuxFile::EMPTY
    });
    let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
    entries[0] = fd(descriptor, LINUX_POLLIN);
    let context = LinuxSyscallContext {
        number: SYS_READ,
        args: [descriptor as Word, 17, 29, 0, 0, 0],
    };
    r.managed[0].readiness_wait = Some(ReadinessWait {
        context,
        entries,
        count: 1,
        sources: READY_PTY,
        deadline: Some(110),
        select: false,
    });
    r.pty_deadline_override = Some(Some(180));
    wake_readiness_waiters(&mut r, READY_PTY);
    assert_eq!(r.pty_observed_deadline.get(), Some(180));
    let wait = r.managed[0].readiness_wait.as_ref().unwrap();
    assert_eq!(wait.deadline, Some(180));
    assert_eq!(wait.context.args, context.args);
    assert_eq!(wait.entries[0].fd, descriptor);
    assert_eq!(
        (wait.count, wait.sources, wait.select),
        (1, READY_PTY, false)
    );
    assert_eq!(r.arms.last(), Some(&Some(180)));
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
fn completion_propagates_to_an_earlier_waiter_before_next_receive() {
    let mut r = runtime();
    r.managed[1].pid = 2;
    r.managed[1].pcb = 102;
    let writer = LinuxSyscallContext {
        number: SYS_WRITE,
        args: [5, 4096, 8, 0, 0, 0],
    };
    let reader = LinuxSyscallContext {
        number: SYS_READ,
        args: [4, 8192, 8, 0, 0, 0],
    };
    assert!(pipe_wait::park(&mut r, 1, writer));
    assert!(pipe_wait::park(&mut r, 2, reader));
    r.pipe.len = LINUX_PIPE_BYTES;
    r.io_result = Ok(8);
    r.drain_pipe_on_read = true;
    r.readiness_changes = READY_PIPE;
    handle_readiness_changes(&mut r);
    assert!(r.managed.iter().all(|p| p.readiness_wait.is_none()));
    RETURNS.with(|values| assert_eq!(&*values.borrow(), &[(102, 8), (101, 8)]));
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
fn ppoll_timespec_validation_remaining_time_and_temporary_mask() {
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
    ts = [0, 1];
    std::hint::black_box(&mut ts);
    std::hint::black_box(mask);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    assert_eq!(r.managed[0].signals().unwrap().blocked, 1);
    r.monotonic_ticks += 1;
    wake_readiness_waiters(&mut r, READY_TIMER);
    assert_eq!(r.managed[0].signals().unwrap().blocked, 0);
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
fn sparse_select_entries_match_scalar_fd_scan_at_every_nfds_boundary() {
    let mut r = runtime();
    let mut seed = 0x13579bdfusize;
    for number in [SYS_SELECT, SYS_PSELECT6] {
        for nfds in 0..=LINUX_FD_MAX {
            for sample in 0..20 {
                let mut sets = [0usize; 3];
                for (index, set) in sets.iter_mut().enumerate() {
                    seed = seed.wrapping_mul(6364136223846793005usize).wrapping_add(1);
                    *set = match sample {
                        0 => 0,
                        1 => usize::MAX,
                        2 => 1usize << 63,
                        3 => 1usize << (nfds.min(63)),
                        4 => (1usize << index) | (1usize << (63 - index)),
                        _ => seed,
                    };
                }
                let mut ctx = LinuxSyscallContext {
                    number,
                    args: [nfds, sets.as_ptr() as Word,
                        unsafe { sets.as_ptr().add(1) } as Word,
                        unsafe { sets.as_ptr().add(2) } as Word, 0, 0],
                };
                // Exercise absent sets as well as overlapping requested bits.
                if sample % 3 == 0 { ctx.args[2] = 0; sets[1] = 0; }
                let expected: Vec<_> = (0..nfds).filter_map(|fd| {
                    let bit = 1usize << fd;
                    let events = (if sets[0] & bit != 0 { LINUX_POLLIN } else { 0 })
                        | (if sets[1] & bit != 0 { LINUX_POLLOUT } else { 0 })
                        | (if sets[2] & bit != 0 { LINUX_POLLPRI } else { 0 });
                    (events != 0).then_some((fd as i32, events, 0i16))
                }).collect();
                let wait = read_poll_set(&mut r, 1, ctx).unwrap();
                let actual: Vec<_> = wait.entries[..wait.count].iter()
                    .map(|entry| (entry.fd, entry.events, entry.revents)).collect();
                assert_eq!(actual, expected);
                assert!(wait.select);
            }
        }
    }
    r.copy_error = true;
    let empty = LinuxSyscallContext { number: SYS_SELECT, args: [0, 1, 2, 3, 0, 0] };
    assert_eq!(read_poll_set(&mut r, 1, empty).unwrap().count, 0);
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
fn finite_epoll_waits_skip_completion_sample_and_restore_pwait_mask() {
    for number in [SYS_EPOLL_WAIT, SYS_EPOLL_PWAIT] {
        let mut r = runtime();
        let fd = eventfd::create(&mut r, 1, 0, 0).unwrap();
        let ep = epoll::create(&mut r, 1, 0).unwrap();
        watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, 123).unwrap();
        let mut output = [0u8; TEST_EVENT_BYTES];
        let mask = 1u64;
        let mut ctx = epoll_context(ep, &mut output, 25);
        ctx.number = number;
        if number == SYS_EPOLL_PWAIT {
            ctx.args[4] = &mask as *const _ as Word;
            ctx.args[5] = 8;
        }
        counter_write(&mut r, fd, 1).unwrap();
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
        assert_eq!(r.clocks, 1);
        assert_eq!(counter_read(&mut r, fd), Ok(1));
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
        assert_eq!(r.clocks, 2);
        counter_write(&mut r, fd, 1).unwrap();
        r.clock_error = true;
        wake_readiness_waiters(&mut r, READY_OBJECT);
        assert_eq!(r.clocks, 2);
        RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
        assert_eq!(r.managed[0].signals().map_or(0, |state| state.blocked), 0);
        assert_eq!(&output[TEST_EVENT_BYTES - 8..], &123u64.to_le_bytes());
    }
}

#[test]
fn epoll_wait_collects_ready_events_once_and_keeps_rotation() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    for value in 1..=8 {
        let fd = eventfd::create(&mut r, 1, 1, 0).unwrap();
        watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, value).unwrap();
    }
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    for value in 1u64..=16 {
        epoll::take_watch_queries();
        assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
        assert_eq!(epoll::take_watch_queries(), 1);
        assert_eq!(&output[TEST_EVENT_BYTES - 8..], &((value - 1) % 8 + 1).to_le_bytes());
    }
}

#[test]
fn parked_epoll_scans_once_on_empty_change_and_ready_wakeup() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    let mut last = 0;
    for value in 1..=8 {
        last = eventfd::create(&mut r, 1, 0, 0).unwrap();
        watch(&mut r, ep, last, 1, LINUX_POLLIN as u32, value).unwrap();
    }
    let mut output = [0xa5u8; TEST_EVENT_BYTES * 8];
    let ctx = epoll_context(ep, &mut output, -1);
    epoll::take_watch_queries();
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    assert_eq!(epoll::take_watch_queries(), 8);
    wake_readiness_waiters(&mut r, READY_OBJECT);
    assert_eq!(epoll::take_watch_queries(), 8);
    assert_eq!(output, [0xa5; TEST_EVENT_BYTES * 8]);
    assert!(r.managed[0].readiness_wait.is_some());
    counter_write(&mut r, last, 1).unwrap();
    wake_readiness_waiters(&mut r, READY_OBJECT);
    assert_eq!(epoll::take_watch_queries(), 8);
    assert!(r.managed[0].readiness_wait.is_none());
    RETURNS.with(|v| assert_eq!(&*v.borrow(), &[(101, 1)]));
    assert_eq!(&output[TEST_EVENT_BYTES - 8..TEST_EVENT_BYTES], &8u64.to_le_bytes());
    assert!(output[TEST_EVENT_BYTES..].iter().all(|byte| *byte == 0xa5));
}

#[test]
fn epoll_syscall_drains_input_once_and_does_not_commit_failed_output() {
    let mut r = runtime();
    r.key_ready = true;
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, 0, 1, LINUX_POLLIN as u32 | (1 << 30), 71).unwrap();
    let mut output = [0xa5u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    r.copy_error = true;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(-(EFAULT as isize)));
    assert_eq!(r.pumps, 1);
    assert_eq!(output, [0xa5; TEST_EVENT_BYTES]);
    r.copy_error = false;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
    assert_eq!(r.pumps, 2);
    assert_eq!(&output[TEST_EVENT_BYTES - 8..], &71u64.to_le_bytes());
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(0));
    assert_eq!(r.pumps, 2);
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
fn epoll_cursor_wraps_non_power_of_two_after_deleting_earlier_registrations() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    let mut descriptors = Vec::new();
    for data in 0..7 {
        let fd = eventfd::create(&mut r, 1, 1, 0).unwrap();
        watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, data).unwrap();
        descriptors.push(fd);
    }
    let mut one = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut one, 0);
    for data in 0u64..5 {
        assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(1));
        assert_eq!(&one[TEST_EVENT_BYTES - 8..], &data.to_le_bytes());
    }
    // The cursor is 5; removing earlier slots leaves three watches. The
    // next batch must normalize that stale cursor without changing order.
    for fd in &descriptors[..4] {
        assert_eq!(epoll::control(&mut r, 1, ep, 2, *fd, 0), Ok(0));
    }
    let mut output = [0xabu8; TEST_EVENT_BYTES * 3];
    let ctx = epoll_context(ep, &mut output, 0);
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(3));
    for (slot, data) in [6u64, 4, 5].into_iter().enumerate() {
        assert_eq!(&output[(slot + 1) * TEST_EVENT_BYTES - 8..(slot + 1) * TEST_EVENT_BYTES], &data.to_le_bytes());
    }
    for fd in &descriptors[4..] {
        assert_eq!(epoll::control(&mut r, 1, ep, 2, *fd, 0), Ok(0));
    }
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(0));
}

#[test]
fn epoll_output_keeps_guest_guards_and_fault_commit_order_without_scratch_dependency() {
    let mut r = runtime();
    let ep = epoll::create(&mut r,1,0).unwrap();
    for i in 0..8 {
        let file = r.files[0];
        let fd = r.allocate_linux_file(1,file,0).unwrap();
        watch(&mut r,ep,fd,1,LINUX_POLLIN as u32 | (1<<30),0x1111_1111_1111_1100+i).unwrap();
    }
    let resource = r.linux_file(1,ep).unwrap().resource;
    let mut output = [0xabu8;TEST_EVENT_BYTES*8+16];
    let ctx = epoll_context(ep,&mut output[8..8+TEST_EVENT_BYTES*8],0);
    r.scratch.fill(0xab);
    r.copy_error=true;
    assert_eq!(epoll::complete(&mut r,1,ctx),Err(EFAULT));
    assert!(output.iter().all(|b|*b==0xab));
    let object = r.poll_objects.epoll_mut(resource).unwrap();
    assert_eq!(object.cursor,0);
    assert!(object.watches.iter().all(|w|w.armed));
    r.copy_error=false;
    r.scratch.fill(0xab);
    assert_eq!(epoll::complete(&mut r,1,ctx),Ok(8));
    let staged = r.scratch.iter().filter(|b|**b!=0xab).count();
    println!("EPOLL-OWNED-OUTPUT staged-changed-bytes={staged}");
    assert!(output[..8].iter().chain(&output[8+TEST_EVENT_BYTES*8..]).all(|b|*b==0xab));
    for i in 0..8 {
        let event=&output[8+i*TEST_EVENT_BYTES..8+(i+1)*TEST_EVENT_BYTES];
        assert_eq!(&event[..4],&(LINUX_POLLIN as u32).to_le_bytes());
        assert_eq!(&event[TEST_EVENT_BYTES-8..],&(0x1111_1111_1111_1100u64+i as u64).to_le_bytes());
    }
    let object = r.poll_objects.epoll_mut(resource).unwrap();
    assert!(object.watches.iter().all(|w|!w.armed));
    assert_eq!(epoll::complete(&mut r,1,ctx),Ok(0));
}

#[test]
fn epoll_input_pump_is_shared_by_watches_and_outer_poll_scan() {
    let mut r = runtime();
    r.key_ready = false;
    r.mouse_ready = false;
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    for i in 0..48 {
        let file = r.files[i % 2];
        let fd = r.allocate_linux_file(1, file, 0).unwrap();
        watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, i as u64).unwrap();
    }
    let file = r.linux_file(1, ep).unwrap();
    let mut pumped = false;
    assert_eq!(readiness::fd_readiness(&mut r, 1, ep, file, LINUX_POLLIN, &mut pumped), Ok(0));
    let ready_pumps = core::mem::take(&mut r.pumps);
    let mut output = [0u8; TEST_EVENT_BYTES * 48];
    let ctx = epoll_context(ep, &mut output, 0);
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(0));
    let complete_pumps = core::mem::take(&mut r.pumps);
    let mut wait = ReadinessWait {
        context: ctx, entries: [PollFd::EMPTY; LINUX_FD_MAX], count: 3,
        sources: 0, deadline: None, select: false,
    };
    for (entry, fd) in wait.entries.iter_mut().zip([ep, 0, 1]) {
        *entry = PollFd { fd: fd as i32, events: LINUX_POLLIN, revents: 0 };
    }
    assert_eq!(readiness::scan_readiness(&mut r, 1, &mut wait), Ok(0));
    assert!(wait.entries[..3].iter().all(|entry| entry.revents == 0));
    assert_eq!(wait.sources, READY_OBJECT | READY_INPUT);
    println!("epoll input pumps ready/complete/mixed-poll: {:?}", [ready_pumps, complete_pumps, r.pumps]);
    assert_eq!([ready_pumps, complete_pumps, r.pumps], [1, 1, 1]);
}

#[test]
fn epoll_input_batch_keeps_payload_order_oneshot_and_copy_failure() {
    for (key, mouse) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut r = runtime();
        r.key_ready = key;
        r.mouse_ready = mouse;
        let ep = epoll::create(&mut r, 1, 0).unwrap();
        for i in 0..8 {
            let file = r.files[i % 2];
            let fd = r.allocate_linux_file(1, file, 0).unwrap();
            watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32 | (1 << 30), i as u64 + 100).unwrap();
        }
        let expected: Vec<_> = (0..8).filter(|i| if i % 2 == 0 { key } else { mouse }).collect();
        let mut output = [0xabu8; TEST_EVENT_BYTES * 8];
        let mut ctx = epoll_context(ep, &mut output, 0);
        ctx.args[1] = 0;
        assert_eq!(epoll::complete(&mut r, 1, ctx), if expected.is_empty() { Ok(0) } else { Err(EFAULT) });
        assert!(output.iter().all(|byte| *byte == 0xab));
        let ctx = epoll_context(ep, &mut output, 0);
        assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(expected.len()));
        for (slot, i) in expected.iter().enumerate() {
            let event = &output[slot * TEST_EVENT_BYTES..(slot + 1) * TEST_EVENT_BYTES];
            assert_eq!(&event[..4], &(LINUX_POLLIN as u32).to_le_bytes());
            assert!(event[4..TEST_EVENT_BYTES - 8].iter().all(|byte| *byte == 0));
            assert_eq!(&event[TEST_EVENT_BYTES - 8..], &(*i as u64 + 100).to_le_bytes());
        }
        assert!(output[expected.len() * TEST_EVENT_BYTES..].iter().all(|byte| *byte == 0xab));
        assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(0));
    }
}

#[test]
fn epoll_input_pump_resets_between_batches_and_skips_disarmed_or_write_only() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, 0, 1, LINUX_POLLOUT as u32, 10).unwrap();
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(0));
    assert_eq!(r.pumps, 0);
    watch(&mut r, ep, 0, 3, LINUX_POLLIN as u32 | (1 << 30), 20).unwrap();
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(1));
    assert_eq!(r.pumps, 1);
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(0));
    assert_eq!(r.pumps, 1); // Disarmed watches do not drain.
    watch(&mut r, ep, 0, 3, LINUX_POLLIN as u32, 30).unwrap();
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(1));
    assert_eq!(r.pumps, 2);
    assert_eq!(epoll::complete(&mut r, 1, ctx), Ok(1));
    assert_eq!(r.pumps, 3); // No state survives a completion batch.
    let second = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, second, 1, 1, LINUX_POLLIN as u32, 40).unwrap();
    r.key_ready = false;
    r.pumps = 0;
    let mut wait = ReadinessWait {
        context: ctx, entries: [PollFd::EMPTY; LINUX_FD_MAX], count: 4,
        sources: 0, deadline: None, select: false,
    };
    for (entry, fd) in wait.entries.iter_mut().zip([0, ep, second, 1]) {
        *entry = PollFd { fd: fd as i32, events: LINUX_POLLIN, revents: 0 };
    }
    assert_eq!(readiness::scan_readiness(&mut r, 1, &mut wait), Ok(0));
    assert_eq!(r.pumps, 1); // Direct input before two epolls also shares it.
}

#[test]
fn epoll_source_mask_tracks_registration_disarm_rearm_and_final_close() {
    fn check(r: &Runtime, ep: Word) -> Word {
        let file = r.linux_file(1, ep).unwrap();
        let object = match r.poll_objects.get(file.resource).unwrap() {
            poll_objects::Object::Epoll(object) => object,
            _ => panic!("expected epoll"),
        };
        let expected = object.watches.iter().filter(|w| w.armed)
            .fold(READY_OBJECT, |mask, w| mask | readiness::file_sources(w.file, w.events as i16));
        for _ in 0..8 {
            assert_eq!(epoll::sources(r, file.resource), expected);
            assert_eq!(object.source_mask.get(), Some(expected));
        }
        expected
    }
    let mut r = runtime();
    let fd = r.allocate_linux_file(1, LinuxFile {
        kind: LinuxFileKind::SocketUnix, resource: 111, ..LinuxFile::EMPTY
    }, 0).unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    assert_eq!(check(&r, ep), READY_OBJECT);
    watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32 | (1 << 30), 5).unwrap();
    assert_eq!(check(&r, ep), READY_OBJECT | READY_UNIX | READY_UNIX_READ);
    assert_eq!(watch(&mut r, ep, fd, 1, 0, 0), Err(EEXIST));
    assert_eq!(check(&r, ep), READY_OBJECT | READY_UNIX | READY_UNIX_READ);
    r.unix_ready = LINUX_POLLIN;
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, 0);
    r.copy_error = true;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(-(EFAULT as isize)));
    assert_eq!(check(&r, ep), READY_OBJECT | READY_UNIX | READY_UNIX_READ);
    r.copy_error = false;
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Return(1));
    assert_eq!(check(&r, ep), READY_OBJECT);
    watch(&mut r, ep, fd, 3, LINUX_POLLOUT as u32, 6).unwrap();
    assert_eq!(check(&r, ep), READY_OBJECT | READY_UNIX | READY_UNIX_WRITE);
    epoll::control(&mut r, 1, ep, 2, fd, 0).unwrap();
    assert_eq!(check(&r, ep), READY_OBJECT);
    watch(&mut r, ep, fd, 1, LINUX_POLLIN as u32, 7).unwrap();
    check(&r, ep);
    let file = r.linux_file(1, fd).unwrap();
    r.poll_objects.forget_file(file, 0);
    assert_eq!(check(&r, ep), READY_OBJECT);
    assert_eq!(epoll::sources(&r, usize::MAX), READY_OBJECT);
}

#[test]
fn unix_epoll_one_shot_rearm_retains_direction_and_failed_copy_preserves_event() {
    let mut r = runtime();
    let descriptor = r
        .allocate_linux_file(
            1,
            LinuxFile {
                kind: LinuxFileKind::SocketUnix,
                resource: 111,
                ..LinuxFile::EMPTY
            },
            0,
        )
        .unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(
        &mut r,
        ep,
        descriptor,
        1,
        LINUX_POLLIN as u32 | (1 << 30),
        11,
    )
    .unwrap();
    r.unix_ready = LINUX_POLLIN;
    let mut output = [0u8; TEST_EVENT_BYTES];
    let immediate = epoll_context(ep, &mut output, 0);
    r.copy_error = true;
    assert_eq!(
        sys_poll_action(&mut r, 1, immediate),
        EmulationAction::Return(-(EFAULT as isize))
    );
    r.copy_error = false;
    assert_eq!(
        sys_poll_action(&mut r, 1, immediate),
        EmulationAction::Return(1)
    );
    assert_eq!(
        sys_poll_action(&mut r, 1, immediate),
        EmulationAction::Return(0)
    );
    r.unix_ready = 0;
    let waiting = epoll_context(ep, &mut output, -1);
    assert_eq!(sys_poll_action(&mut r, 1, waiting), EmulationAction::Park);
    watch(
        &mut r,
        ep,
        descriptor,
        3,
        LINUX_POLLIN as u32 | (1 << 30),
        13,
    )
    .unwrap();
    handle_readiness_changes(&mut r);
    assert_eq!(
        r.managed[0].readiness_wait.unwrap().sources,
        READY_OBJECT | READY_UNIX | READY_UNIX_READ
    );
    r.unix_queries.set(0);
    r.unix_ready = LINUX_POLLIN;
    wake_readiness_waiters(&mut r, READY_UNIX_WRITE);
    assert_eq!(r.unix_queries.get(), 0);
    assert!(r.managed[0].readiness_wait.is_some());
    wake_readiness_waiters(&mut r, READY_UNIX_READ);
    assert!(r.managed[0].readiness_wait.is_none());
    assert_eq!(&output[TEST_EVENT_BYTES - 8..], &13u64.to_le_bytes());
    assert_eq!(
        sys_poll_action(&mut r, 1, immediate),
        EmulationAction::Return(0)
    );
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
fn timer_expiry_bound_skips_repeated_checks_and_keeps_consumed_counts_clear() {
    use poll_objects::{EventCounter, Object, Objects, Timer};
    let mut counts = Vec::new();
    for advance in [false, true] {
        let mut objects = Objects::new();
        let mut timers = Vec::new();
        for slot in 0..256 {
            if slot % 4 == 0 {
                timers.push(objects.insert(Object::Timer(Timer { clock: 1, deadline: Some(50), interval: 10, expirations: 0, nonblocking: false })).unwrap());
            } else {
                objects.insert(Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false })).unwrap();
            }
        }
        assert_eq!(objects.timer_deadline(), Some(50));
        poll_objects::take_timer_inspections();
        for (step, &id) in timers.iter().enumerate() {
            objects.expire_timers(if advance { 100 + step } else { 100 });
            assert!(objects.clear_timer_expirations(id));
        }
        counts.push(poll_objects::take_timer_inspections());
        for (step, id) in timers.into_iter().enumerate() {
            let Some(Object::Timer(t)) = objects.get(id) else { panic!() };
            assert_eq!((t.deadline, t.expirations), if advance { (Some(170), (6 - (step / 10).min(6)) as u64) } else { (Some(110), 0) });
        }
    }
    println!("timer expiry inspections 64 timers, repeated/progressing time: {counts:?}");
    assert_eq!(counts, [64, 448]);
}

#[test]
fn timer_expiry_bound_matches_linear_scan_across_mutations_and_backwards_time() {
    use poll_objects::{EventCounter, Object, Objects, Timer};
    fn timer(deadline: Word, interval: Word) -> Timer { Timer { clock: 1, deadline: Some(deadline), interval, expirations: 0, nonblocking: false } }
    let mut objects = Objects::new();
    let a = objects.insert(Object::Timer(timer(10, 5))).unwrap();
    let b = objects.insert(Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false })).unwrap();
    let c = objects.insert(Object::Timer(timer(20, 0))).unwrap();
    let mut rows = vec![(a, Some(timer(10, 5))), (b, None), (c, Some(timer(20, 0)))];
    assert!(!objects.clear_timer_expirations(b));
    assert!(!objects.clear_timer_expirations(Word::MAX));
    for (step, now) in [0, 9, 10, 10, 11, 20, 5, 20, 30, 100, 100, Word::MAX, 5].into_iter().enumerate() {
        match step {
            3 => { *objects.timer_mut(a).unwrap() = timer(0, 3); rows.iter_mut().find(|r| r.0 == a).unwrap().1 = Some(timer(0, 3)); }
            5 | 10 => { let t = timer(now, if step == 5 { 0 } else { 2 }); let id = objects.insert(Object::Timer(t)).unwrap(); rows.push((id, Some(t))); }
            6 => { objects.remove(c); rows.retain(|r| r.0 != c); }
            7 => { let t = timer(19, 1); *objects.get_mut(b).unwrap() = Object::Timer(t); rows.iter_mut().find(|r| r.0 == b).unwrap().1 = Some(t); }
            8 => { *objects.timer_mut(a).unwrap() = timer(30, 0); rows.iter_mut().find(|r| r.0 == a).unwrap().1 = Some(timer(30, 0)); }
            9 => { objects.remove(b); rows.retain(|r| r.0 != b); }
            _ => {},
        }
        for clear in [false, true] {
            if clear {
                assert!(objects.clear_timer_expirations(a));
                rows.iter_mut().find(|r| r.0 == a).unwrap().1.as_mut().unwrap().expirations = 0;
            }
            objects.expire_timers(now);
            for (_, t) in &mut rows { if let Some(t) = t { t.expire(now); } }
            for &(id, expected) in &rows {
                match (objects.get(id).unwrap(), expected) {
                    (Object::Counter(c), None) => assert_eq!(c.value, 0),
                    (Object::Timer(a), Some(b)) => assert_eq!((a.clock, a.deadline, a.interval, a.expirations, a.nonblocking), (b.clock, b.deadline, b.interval, b.expirations, b.nonblocking), "step={step} clear={clear}"),
                    _ => panic!("kind mismatch"),
                }
            }
            assert_eq!(objects.timer_deadline(), rows.iter().filter_map(|(_, t)| t.and_then(|t| t.deadline)).min());
        }
    }
}

#[test]
fn poll_timer_membership_tracks_creation_and_swap_churn_without_rebuilds() {
    use poll_objects::{EventCounter, Object, Objects, Timer, OBJECT_LIMIT};
    fn counter() -> Object { Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false }) }
    let mut counts = Vec::new();
    let mut objects = Objects::new();
    poll_objects::take_timer_inspections();
    for _ in 0..OBJECT_LIMIT {
        objects.insert(counter()).unwrap();
        assert_eq!(objects.timer_deadline(), None);
    }
    counts.push(poll_objects::take_timer_inspections());
    for has_timer in [false, true] {
        let mut objects = Objects::new();
        let mut counters = Vec::new();
        for slot in 0..OBJECT_LIMIT {
            if has_timer && slot == OBJECT_LIMIT - 1 {
                objects.insert(Object::Timer(Timer { clock: 1, deadline: Some(10000), interval: 0, expirations: 0, nonblocking: false })).unwrap();
            } else { counters.push(objects.insert(counter()).unwrap()); }
        }
        objects.timer_deadline();
        poll_objects::take_timer_inspections();
        for _ in 0..8 {
            objects.remove(counters.swap_remove(0));
            counters.push(objects.insert(counter()).unwrap());
            objects.expire_timers(100);
            assert_eq!(objects.timer_deadline(), if has_timer { Some(10000) } else { None });
        }
        counts.push(poll_objects::take_timer_inspections());
    }
    println!("poll timer inspections creation/churn-no-timer/churn-one-timer: {counts:?}");
    // Include the new insertion/moved-slot kind checks, not just query work.
    assert_eq!(counts, [256, 16, 25]);
}

#[test]
fn poll_timer_membership_stays_invalid_across_structural_changes_after_generic_mutation() {
    use poll_objects::{EventCounter, Object, Objects, Timer};
    fn counter() -> Object { Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false }) }
    fn timer(deadline: Word) -> Object { Object::Timer(Timer { clock: 1, deadline: Some(deadline), interval: 0, expirations: 0, nonblocking: false }) }
    for replace_with_timer in [false, true] {
        let mut objects = Objects::new();
        let first = objects.insert(counter()).unwrap();
        let removed = objects.insert(timer(300)).unwrap();
        let last = objects.insert(counter()).unwrap();
        assert_eq!(objects.timer_deadline(), Some(300));
        *objects.get_mut(first).unwrap() = if replace_with_timer { timer(5) } else { counter() };
        objects.remove(removed);
        let added = objects.insert(timer(1000)).unwrap();
        assert_eq!(objects.timer_deadline(), Some(if replace_with_timer { 5 } else { 1000 }));
        objects.remove(added);
        assert_eq!(objects.timer_deadline(), if replace_with_timer { Some(5) } else { None });
        objects.remove(first);
        assert_eq!(objects.timer_deadline(), None);
        objects.remove(last);
        assert!(objects.is_empty());
        assert_eq!(objects.timer_deadline(), None);
        let id = objects.insert(timer(7)).unwrap();
        assert_eq!(objects.timer_deadline(), Some(7));
        objects.remove(id);
        assert_eq!(objects.timer_deadline(), None);
    }
}

#[test]
fn poll_typed_counter_updates_keep_timer_membership_warm() {
    use poll_objects::{EventCounter, Object, Objects, Timer, OBJECT_LIMIT};
    let mut counts = Vec::new();
    for has_timer in [false, true] {
        let mut objects = Objects::new();
        let mut first = 0;
        for slot in 0..OBJECT_LIMIT {
            let object = if has_timer && slot == OBJECT_LIMIT - 1 {
                Object::Timer(Timer { clock: 1, deadline: Some(10000), interval: 0, expirations: 0, nonblocking: false })
            } else { Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false }) };
            let id = objects.insert(object).unwrap();
            if slot == 0 { first = id; }
        }
        objects.timer_deadline();
        poll_objects::take_timer_inspections();
        for value in 0..8 {
            objects.counter_mut(first).unwrap().value = value;
            objects.expire_timers(100);
            assert_eq!(objects.timer_deadline(), if has_timer { Some(10000) } else { None });
        }
        counts.push(poll_objects::take_timer_inspections());
        assert!(matches!(objects.get(first), Some(Object::Counter(c)) if c.value == 7));
    }
    println!("poll timer inspections across counter updates no-timer/one-timer: {counts:?}");
    assert_eq!(counts, [0, 9]);
}

#[test]
fn poll_typed_accessors_reject_wrong_kinds_and_keep_live_timer_values() {
    use poll_objects::*;
    let mut objects = Objects::new();
    let variants = [
        Object::Counter(EventCounter { value: 1, semaphore: false, nonblocking: false }),
        Object::Timer(Timer { clock: 1, deadline: Some(100), interval: 0, expirations: 0, nonblocking: false }),
        Object::Signals(SignalFd { mask: 0, nonblocking: false }),
        Object::Memory(MemoryFile { sysv: None, anonymous: false, backing: None, pages: Default::default(), size: 0, offset: 0, status_flags: 0, seals: 0, maps: Vec::new() }),
        Object::Epoll(Epoll { watches: Vec::new(), cursor: 0, source_mask: std::cell::Cell::new(None) }),
        Object::Ring(IoUring { memory: 0, sqes: 0, entries: 0, cq_entries: 0, cq_offset: 0, ring_bytes: 0, sqe_bytes: 0, array: false, head: 0, tail: 0, cancel_next: false, overflow: Default::default() }),
    ];
    let ids: Vec<_> = variants.into_iter().map(|o| objects.insert(o).unwrap()).collect();
    assert_eq!(objects.timer_deadline(), Some(100));
    for (kind, &id) in ids.iter().enumerate() {
        assert_eq!(objects.counter_mut(id).is_some(), kind == 0);
        assert_eq!(objects.timer_mut(id).is_some(), kind == 1);
        assert_eq!(objects.signals_mut(id).is_some(), kind == 2);
        assert_eq!(objects.memory_mut(id).is_some(), kind == 3);
        assert_eq!(objects.epoll_mut(id).is_some(), kind == 4);
        assert_eq!(objects.ring_mut(id).is_some(), kind == 5);
        assert_eq!(objects.set_nonblocking(id, true), kind < 3);
    }
    objects.counter_mut(ids[0]).unwrap().value = 42;
    objects.timer_mut(ids[1]).unwrap().deadline = Some(777);
    objects.signals_mut(ids[2]).unwrap().mask = 64;
    objects.memory_mut(ids[3]).unwrap().size = 123;
    objects.epoll_mut(ids[4]).unwrap().cursor = 3;
    objects.ring_mut(ids[5]).unwrap().head = 4;
    assert_eq!(objects.timer_deadline(), Some(777));
    assert!(matches!(objects.get(ids[0]), Some(Object::Counter(c)) if c.value == 42 && c.nonblocking));
    assert!(objects.timer_mut(ids[1]).unwrap().nonblocking);
    assert_eq!((objects.signals_mut(ids[2]).unwrap().mask, objects.memory_mut(ids[3]).unwrap().size, objects.epoll_mut(ids[4]).unwrap().cursor, objects.ring_mut(ids[5]).unwrap().head), (64, 123, 3, 4));
    *objects.get_mut(ids[0]).unwrap() = Object::Timer(Timer { clock: 1, deadline: Some(1), interval: 0, expirations: 0, nonblocking: false });
    assert!(objects.counter_mut(ids[0]).is_none());
    assert_eq!(objects.timer_deadline(), Some(1));
    objects.remove(ids[0]);
    assert!(objects.timer_mut(ids[0]).is_none());
    assert_eq!(objects.timer_deadline(), Some(777));
    assert!(objects.counter_mut(0).is_none() && objects.timer_mut(Word::MAX).is_none());
    assert!(!objects.set_nonblocking(0, true));
}

#[test]
fn poll_timer_index_skips_non_timer_objects_after_warmup() {
    use poll_objects::{EventCounter, Object, Objects, Timer, OBJECT_LIMIT};
    let mut counts = Vec::new();
    for has_timer in [false, true] {
        let mut objects = Objects::new();
        for slot in 0..OBJECT_LIMIT {
            let object = if has_timer && slot == OBJECT_LIMIT - 1 {
                Object::Timer(Timer { clock: 1, deadline: Some(10000), interval: 0, expirations: 0, nonblocking: false })
            } else { Object::Counter(EventCounter { value: 0, semaphore: false, nonblocking: false }) };
            objects.insert(object).unwrap();
        }
        assert_eq!(objects.timer_deadline(), if has_timer { Some(10000) } else { None });
        poll_objects::take_timer_inspections();
        for _ in 0..8 {
            objects.expire_timers(100);
            assert_eq!(objects.timer_deadline(), if has_timer { Some(10000) } else { None });
        }
        counts.push(poll_objects::take_timer_inspections());
    }
    println!("poll timer object inspections no-timer/one-timer: {counts:?}; Objects bytes={}", core::mem::size_of::<Objects>());
    assert_eq!(counts, [0, 9]);
}

#[test]
fn poll_timer_index_matches_linear_state_after_kind_changes_and_swap_removal() {
    use poll_objects::{EventCounter, Object, Objects, Timer, OBJECT_LIMIT};
    #[derive(Clone, Copy)]
    enum Model { Counter(u64), Timer(Timer) }
    fn object(model: Model) -> Object {
        match model {
            Model::Counter(value) => Object::Counter(EventCounter { value, semaphore: false, nonblocking: false }),
            Model::Timer(timer) => Object::Timer(timer),
        }
    }
    fn model(step: usize) -> Model {
        if step % 3 == 0 { Model::Counter(step as u64) } else {
            Model::Timer(Timer { clock: step % 3, deadline: if step % 7 == 0 { None } else { Some(step * 11 + 17) }, interval: step % 5, expirations: step as u64, nonblocking: step % 2 == 0 })
        }
    }
    fn deadline(rows: &[(Word, Model)]) -> Option<Word> {
        rows.iter().filter_map(|(_, m)| match m { Model::Timer(t) => t.deadline, _ => None }).min()
    }
    let mut objects = Objects::new();
    let mut rows = Vec::new();
    for i in 0..OBJECT_LIMIT { let m = model(i); rows.push((objects.insert(object(m)).unwrap(), m)); }
    for step in 0..3000 {
        assert_eq!(objects.timer_deadline(), deadline(&rows));
        assert_eq!(objects.timer_deadline(), deadline(&rows));
        let slot = step * 17 % rows.len();
        let m = model(step + 1000);
        *objects.get_mut(rows[slot].0).unwrap() = object(m);
        rows[slot].1 = m;
        assert_eq!(objects.timer_deadline(), deadline(&rows));
        if step % 2 == 0 {
            let slot = step * 31 % rows.len();
            let (id, _) = rows.swap_remove(slot);
            objects.remove(id);
            assert!(objects.get_mut(id).is_none());
            assert_eq!(objects.timer_deadline(), deadline(&rows));
            let m = model(step + 2000);
            rows.push((objects.insert(object(m)).unwrap(), m));
        }
        assert!(objects.insert(object(Model::Counter(0))).is_none());
        let now = step * 7;
        objects.expire_timers(now);
        for (_, m) in &mut rows { if let Model::Timer(t) = m { t.expire(now); } }
        assert_eq!(objects.timer_deadline(), deadline(&rows));
        for &(id, m) in &rows {
            match (objects.get(id).unwrap(), m) {
                (Object::Counter(c), Model::Counter(value)) => assert_eq!((c.value, c.semaphore, c.nonblocking), (value, false, false)),
                (Object::Timer(a), Model::Timer(b)) => assert_eq!((a.clock, a.deadline, a.interval, a.expirations, a.nonblocking), (b.clock, b.deadline, b.interval, b.expirations, b.nonblocking)),
                _ => panic!("object kind differs"),
            }
        }
    }
}

#[test]
fn poll_object_slot_hints_preserve_identity_and_order_through_collisions() {
    use poll_objects::{EventCounter, Object, Objects, OBJECT_LIMIT};
    fn counter(value: u64) -> Object {
        Object::Counter(EventCounter { value, semaphore: false, nonblocking: false })
    }
    let mut objects = Objects::new();
    let mut oracle = Vec::new();
    // Retain old IDs while creating colliding new generations and repeatedly
    // moving the last entry into a removed slot. The oracle has no hints.
    for step in 0..6000usize {
        if oracle.len() == OBJECT_LIMIT || (step % 3 == 0 && !oracle.is_empty()) {
            let slot = (step * 17) % oracle.len();
            let (id, _) = oracle.swap_remove(slot);
            objects.remove(id);
            assert!(objects.get(id).is_none());
            assert!(objects.get_mut(id).is_none());
        }
        let value = step as u64;
        let id = objects.insert(counter(value)).unwrap();
        oracle.push((id, value));
        for &(id, expected) in &oracle {
            match objects.get(id).unwrap() {
                Object::Counter(c) => assert_eq!(c.value, expected),
                _ => panic!("wrong object kind"),
            }
        }
        let slot = step % oracle.len();
        let (id, value) = &mut oracle[slot];
        *value ^= 0x1234;
        match objects.get_mut(*id).unwrap() {
            Object::Counter(c) => c.value ^= 0x1234,
            _ => panic!("wrong object kind"),
        }
        assert_eq!(objects.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                   oracle.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        assert!(objects.get(0).is_none());
        assert!(objects.get(usize::MAX).is_none());
    }
    assert_eq!(oracle.len(), OBJECT_LIMIT);
    assert!(objects.insert(counter(0)).is_none());
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

#[test]
fn unwatchable_closes_skip_reference_discovery_but_watched_descriptions_keep_aliases() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    let watched = eventfd::create(&mut r, 1, 1, 0).unwrap();
    watch(&mut r, ep, watched, 1, 1, 99).unwrap();
    let epfile = r.linux_file(1, ep).unwrap();
    for kind in [LinuxFileKind::Posix, LinuxFileKind::MemoryFile,
        LinuxFileKind::VirtualFile, LinuxFileKind::VirtualDirectory,
        LinuxFileKind::Framebuffer] {
        let fd = r.allocate_linux_file(1, LinuxFile {
            kind, posix_fd: 123, resource: 456, ..LinuxFile::EMPTY
        }, 0).unwrap();
        assert_eq!(watch(&mut r, ep, fd, 1, 1, 0), Err(EPERM));
        let file = r.linux_file(1, fd).unwrap();
        CLOSE_TERMINAL_LOOKUPS.with(|c| c.set(0));
        CLOSE_RIGHTS_LOOKUPS.with(|c| c.set(0));
        epoll::forget_closed(&mut r, 1, file);
        assert_eq!(CLOSE_TERMINAL_LOOKUPS.with(|c| c.get()), 0);
        assert_eq!(CLOSE_RIGHTS_LOOKUPS.with(|c| c.get()), 0);
    }
    CLOSE_TERMINAL_LOOKUPS.with(|c| c.set(0));
    epoll::forget_closed(&mut r, 1, epfile);
    epoll::forget_closed(&mut r, 1, LinuxFile::EMPTY);
    assert_eq!(CLOSE_TERMINAL_LOOKUPS.with(|c| c.get()), 0);
    let file = r.linux_file(1, watched).unwrap();
    // A live alias must keep the watch, including its one-shot/metadata state.
    CLOSE_TERMINAL_LOOKUPS.with(|c| c.set(0));
    epoll::forget_closed(&mut r, 1, file);
    assert_eq!(CLOSE_TERMINAL_LOOKUPS.with(|c| c.get()), 1);
    let count = |r: &Runtime| match r.poll_objects.get(epfile.resource).unwrap() {
        poll_objects::Object::Epoll(object) => object.watches.len(),
        _ => panic!("expected epoll"),
    };
    assert_eq!(count(&r), 1);
    r.files[watched] = LinuxFile::EMPTY;
    r.managed[0].files[watched] = LinuxFile::EMPTY;
    CLOSE_RIGHTS_LOOKUPS.with(|c| c.set(0));
    epoll::forget_closed(&mut r, 1, file);
    assert_eq!(CLOSE_RIGHTS_LOOKUPS.with(|c| c.get()), 1);
    assert_eq!(count(&r), 0);
}

#[test]
fn unwatched_pollable_closes_preserve_disabled_watches_without_reference_work() {
    let mut r = runtime();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    let watched = eventfd::create(&mut r, 1, 1, 0).unwrap();
    watch(&mut r, ep, watched, 1, (1 << 30) | 1, 99).unwrap();
    let file = r.linux_file(1, watched).unwrap();
    let epfile = r.linux_file(1, ep).unwrap();
    r.poll_objects.epoll_mut(epfile.resource).unwrap().watches[0].armed = false;
    CLOSE_RIGHTS_LOOKUPS.with(|count| count.set(0));
    for candidate in [
        LinuxFile { resource: file.resource + 1000, ..file },
        LinuxFile { posix_fd: file.posix_fd + 1000, ..file },
        LinuxFile { kind: LinuxFileKind::PipeRead, ..file },
        LinuxFile { kind: LinuxFileKind::SocketUnix, ..file },
    ] {
        epoll::forget_closed(&mut r, 1, candidate);
    }
    let lookups = CLOSE_RIGHTS_LOOKUPS.with(|count| count.get());
    let object = r.poll_objects.epoll_mut(epfile.resource).unwrap();
    assert_eq!(object.watches.len(), 1);
    assert!(!object.watches[0].armed);
    assert_eq!(object.watches[0].data, 99);
    assert_eq!(object.watches[0].events, (1 << 30) | 1);
    // Disabled one-shot registrations still require cleanup on final close.
    r.files[watched] = LinuxFile::EMPTY;
    r.managed[0].files[watched] = LinuxFile::EMPTY;
    CLOSE_RIGHTS_LOOKUPS.with(|count| count.set(0));
    epoll::forget_closed(&mut r, 1, file);
    assert_eq!(CLOSE_RIGHTS_LOOKUPS.with(|count| count.get()), 1);
    assert!(r.poll_objects.epoll_mut(epfile.resource).unwrap().watches.is_empty());
    println!("unwatched-close rights-reference-queries={lookups}");
}

#[test]
fn timerfd_periodic_expirations_epoll_and_failed_read_preserve_count() {
    let mut r = runtime();
    let fd = fd_events::timer_create(&mut r, 1, 1, LINUX_O_NONBLOCK).unwrap();
    let input = [0usize, 10_000_000, 0, 20_000_000];
    fd_events::timer_set(&mut r, 1, fd, 0, input.as_ptr() as Word, 0).unwrap();
    let ep = epoll::create(&mut r, 1, 0).unwrap();
    watch(&mut r, ep, fd, 1, 1, 123).unwrap();
    let mut output = [0u8; TEST_EVENT_BYTES];
    let ctx = epoll_context(ep, &mut output, -1);
    assert_eq!(sys_poll_action(&mut r, 1, ctx), EmulationAction::Park);
    r.monotonic_ticks = 145;
    wake_readiness_waiters(&mut r, READY_TIMER);
    assert!(r.managed[0].readiness_wait.is_none());
    let file = r.linux_file(1, fd).unwrap();
    assert_eq!(fd_events::read(&mut r, 1, file, 0, 8), Err(EFAULT));
    let mut count = 0u64;
    assert_eq!(
        fd_events::read(&mut r, 1, file, &mut count as *mut _ as Word, 8),
        Ok(8)
    );
    assert_eq!(count, 3);
    assert_eq!(
        fd_events::read(&mut r, 1, file, &mut count as *mut _ as Word, 8),
        Err(EAGAIN)
    );
    assert_eq!(r.poll_objects.timer_deadline(), Some(150));
}

#[test]
fn signalfd_second_record_fault_keeps_prefix_and_restores_group_membership() {
    let mut r=runtime();
    let mask=1u64<<33;
    let fd=fd_events::signal_create(&mut r,1,Word::MAX,&mask as *const _ as Word,8,0).unwrap();
    let file=r.linux_file(1,fd).unwrap();
    let state=r.managed[0].signals_mut();
    state.blocked=mask;
    state.enqueue(signal_state::Info::new(34,-6,111));
    state.enqueue_group(signal_state::Info::new(34,-6,222));
    let mut output=[0xabu8;272];
    COPYOUT_FAIL_AT.with(|at|at.set(Some(1)));
    assert_eq!(fd_events::read(&mut r,1,file,output.as_mut_ptr() as Word+8,256),Ok(128));
    let mut expected=[0u8;128];
    expected[..4].copy_from_slice(&34u32.to_le_bytes());
    expected[8..12].copy_from_slice(&(-6i32).to_le_bytes());
    expected[12..16].copy_from_slice(&111u32.to_le_bytes());
    assert_eq!(&output[8..136],&expected);
    assert!(output[..8].iter().chain(&output[136..]).all(|b|*b==0xab));
    let state=r.managed[0].signals_mut();
    assert!(state.pending.is_empty());
    assert_eq!(state.pending_mask(),mask);
    let (info,group)=state.take_matching_record(mask).unwrap();
    assert!(group);
    assert_eq!(&info.bytes[16..20],&222u32.to_le_bytes());
    state.restore_pending(info,group);
    COPYOUT_FAIL_AT.with(|at|at.set(None));
    assert_eq!(fd_events::read(&mut r,1,file,output.as_mut_ptr() as Word+136,128),Ok(128));
    expected[12..16].copy_from_slice(&222u32.to_le_bytes());
    assert_eq!(&output[136..264],&expected);
    assert!(output[..8].iter().chain(&output[264..]).all(|b|*b==0xab));
    assert_eq!(r.managed[0].signals().unwrap().pending_mask(),0);
    assert_eq!(r.managed[0].signals().unwrap().blocked,mask);
    assert_eq!(fd_events::read(&mut r,1,file,output.as_mut_ptr() as Word+8,128),Err(EAGAIN));
    println!("SIGNALFD-OWNED raw={} owned={}",RAW_COPYOUT_CALLS.with(|c|c.get()),OWNED_COPYOUT_CALLS.with(|c|c.get()));
}

#[test]
fn signalfd_consumes_pending_records_and_reports_poll_readability() {
    let mut r = runtime();
    let mask = 1u64 << 9;
    let fd =
        fd_events::signal_create(&mut r, 1, Word::MAX, &mask as *const _ as Word, 8, 0).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    let state = r.managed[0].signals_mut();
    state.blocked = mask;
    state.enqueue(signal_state::Info::new(10, -6, 777));
    assert_ne!(
        fd_events::readiness(&mut r, 1, file).unwrap() & LINUX_POLLIN,
        0
    );
    assert_eq!(fd_events::read(&mut r, 1, file, 0, 128), Err(EFAULT));
    let mut record = [0u8; 128];
    assert_eq!(
        fd_events::read(&mut r, 1, file, record.as_mut_ptr() as Word, 128),
        Ok(128)
    );
    assert_eq!(&record[..4], &10u32.to_le_bytes());
    assert_eq!(&record[12..16], &777u32.to_le_bytes());
    assert_eq!(fd_events::readiness(&mut r, 1, file), Ok(0));
    assert_eq!(r.managed[0].signals().unwrap().blocked, mask);
}

#[test]
fn memfd_io_shares_offsets_but_positioned_io_does_not_move_them() {
    let mut r = runtime();
    let fd = memfd::create(&mut r, 1, c"memory".as_ptr() as Word, 3).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    assert_eq!(file.flags & LINUX_FD_CLOEXEC, LINUX_FD_CLOEXEC);
    let data = *b"shared";
    assert_eq!(
        memfd::io(
            &mut r,
            1,
            file,
            data.as_ptr() as Word,
            data.len(),
            None,
            true
        ),
        Ok(6)
    );
    assert_eq!(memfd::size(&r, file), Ok(6));
    assert_eq!(memfd::seek(&mut r, file, 0, 0), Ok(0));
    let duplicate = r.allocate_linux_file(1, file, 0).unwrap();
    let mut output = [0u8; 6];
    assert_eq!(
        memfd::io(&mut r, 1, file, output.as_mut_ptr() as Word, 3, None, false),
        Ok(3)
    );
    let alias = r.linux_file(1, duplicate).unwrap();
    assert_eq!(
        memfd::io(
            &mut r,
            1,
            alias,
            output.as_mut_ptr() as Word + 3,
            3,
            None,
            false
        ),
        Ok(3)
    );
    assert_eq!(&output, b"shared");
    assert_eq!(
        memfd::io(
            &mut r,
            1,
            file,
            output.as_mut_ptr() as Word,
            6,
            Some(0),
            false
        ),
        Ok(6)
    );
    assert_eq!(memfd::seek(&mut r, file, 0, 1), Ok(6));
    memfd::seals(&mut r, file, Some(2 | 4 | 8)).unwrap();
    assert_eq!(memfd::truncate(&mut r, file, 0), Err(EPERM));
    assert_eq!(
        memfd::io(&mut r, 1, file, data.as_ptr() as Word, 1, Some(0), true),
        Err(EPERM)
    );
}

#[test]
fn memfd_shared_aliases_survive_close_protection_and_truncate_then_regrow() {
    let mut r = runtime();
    r.managed[1].pid = 2;
    let fd = memfd::create(&mut r, 1, c"buffer".as_ptr() as Word, 2).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    memfd::truncate(&mut r, file, 8192).unwrap();
    let first = memfd::map(&mut r, 1, 0, 8192, 3, 1, file, 0).unwrap();
    let second = memfd::map(&mut r, 2, 0, 8192, 3, 1, file, 0).unwrap();
    ALIASES.with(|aliases| {
        let aliases = aliases.borrow();
        let a = &aliases[&(1, first)].0;
        let b = &aliases[&(2, second)].0;
        assert!(
            std::rc::Rc::ptr_eq(a, b),
            "MAP_SHARED must reference identical backing"
        );
        unsafe {
            (a.as_ptr() as *mut u8).write(42);
        }
        assert_eq!(b[0], 42);
    });
    memfd::protect(&r, 1, first, 4096, 0).unwrap();
    memfd::protect(&r, 1, first, 4096, 3).unwrap();
    ALIASES.with(|aliases| assert_eq!(aliases.borrow()[&(1, first)].0[0], 42));
    assert_eq!(memfd::seals(&mut r, file, Some(8)), Err(EBUSY));
    memfd::truncate(&mut r, file, 17).unwrap();
    assert!(memfd::bus_fault(&r, 1, first + 4096));
    assert!(memfd::bus_fault(&r, 2, second + 4096));
    memfd::truncate(&mut r, file, 8192).unwrap();
    assert!(!memfd::bus_fault(&r, 1, first + 4096));
    r.files[fd] = LinuxFile::EMPTY;
    r.managed[0].files[fd] = LinuxFile::EMPTY;
    memfd::collect(&mut r, file.resource);
    assert!(r.poll_objects.get(file.resource).is_some());
    sys_munmap(&mut r, 1, first, 8192).unwrap();
    assert!(r.poll_objects.get(file.resource).is_some());
    sys_munmap(&mut r, 2, second, 8192).unwrap();
    assert!(r.poll_objects.get(file.resource).is_none());
}

#[test]
fn memfd_truncate_keeps_mapping_records_without_snapshot_allocation() {
    let mut counts = Vec::new();
    for count in [0, 1, 8, 128, 1024] {
        let (mut r, id, _) = collectible_memfd(0);
        let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
        memory.size = LINUX_PAGE_SIZE;
        memory.maps = (0..count).map(|i| lookup_mapping(1 + i % 2, 0x10000 + i * 8192)).collect();
        let signature = |r: &Runtime| match r.poll_objects.get(id).unwrap() {
            poll_objects::Object::Memory(memory) => memory.maps.iter().map(|m| (m.pid, m.base, m.bytes, m.offset, m.prot, m.shared, m.attachment, m.readonly_attachment)).collect::<Vec<_>>(),
            _ => panic!(),
        };
        let before = signature(&r);
        let file = LinuxFile { kind: LinuxFileKind::MemoryFile, resource: id, posix_fd: id, ..LinuxFile::EMPTY };
        FUTEX_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = memfd::truncate(&mut r, file, LINUX_PAGE_SIZE);
        counts.push(FUTEX_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert_eq!(result, Ok(0));
        assert_eq!(signature(&r), before);
    }
    println!("memfd truncate mapping counts 0/1/8/128/1024 allocations: {counts:?}");
    assert_eq!(counts, [0; 5]);
}

#[test]
fn memfd_truncate_sparse_pages_matches_byte_reference_and_regrowth() {
    for length in [0, 1, 4095, 4096, 4097, 3 * 4096, 3 * 4096 + 13, 21 * 4096 + 31, 24 * 4096] {
        let (mut r, id, addresses) = collectible_memfd(8);
        let old = 24 * LINUX_PAGE_SIZE;
        let file = LinuxFile { kind: LinuxFileKind::MemoryFile, resource: id, posix_fd: id, ..LinuxFile::EMPTY };
        let mut expected = Vec::new();
        for (i, &address) in addresses.iter().enumerate() {
            let page = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, LINUX_PAGE_SIZE) };
            for (offset, byte) in page.iter_mut().enumerate() { *byte = ((i * 17 + offset) % 251 + 1) as u8; }
            expected.push(page.iter().enumerate().map(|(offset, &byte)| if i * 3 * LINUX_PAGE_SIZE + offset < length { byte } else { 0 }).collect::<Vec<_>>());
        }
        assert_eq!(memfd::truncate(&mut r, file, length), Ok(0));
        assert_eq!(memfd::size(&r, file), Ok(length));
        for (&address, expected) in addresses.iter().zip(&expected) {
            assert_eq!(unsafe { core::slice::from_raw_parts(address as *const u8, LINUX_PAGE_SIZE) }, expected);
        }
        assert_eq!(memfd::truncate(&mut r, file, old), Ok(0));
        for (&address, expected) in addresses.iter().zip(&expected) {
            assert_eq!(unsafe { core::slice::from_raw_parts(address as *const u8, LINUX_PAGE_SIZE) }, expected);
        }
    }
}

#[test]
fn memfd_sync_stream_allocation_and_group_queries() {
    let mut results = Vec::new();
    for owner in [1, 2] {
        let (mut r, id, _) = collectible_memfd(0);
        r.managed[0].vm_group = 7;
        r.managed[1].pid = 2;
        r.managed[1].vm_group = 7;
        let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
        memory.maps = (0..1024).map(|i| lookup_mapping(owner, 0x10000 + i * 8192)).collect();
        MEMFD_SYNCS.with(|trace| trace.borrow_mut().0.reserve(1024));
        VM_GROUP_QUERIES.with(|count| count.set(0));
        FUTEX_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = memfd::sync_files(&r, 1, 0x10000, 1024 * 8192);
        let allocations = FUTEX_ALLOCATIONS.with(|count| count.replace(None).unwrap());
        let groups = VM_GROUP_QUERIES.with(|count| count.get());
        assert_eq!(result, Ok(()));
        MEMFD_SYNCS.with(|trace| assert_eq!(trace.borrow().0, vec![77; 1024]));
        results.push((allocations, groups));
    }
    println!("memfd sync own/shared-owner (allocations,group-queries): {results:?}");
    assert_eq!(results, [(0, 0), (0, 1025)]);
}

#[test]
fn memfd_sync_keeps_reference_order_boundaries_and_first_failure() {
    let (mut r, first, _) = collectible_memfd(0);
    r.managed[0].vm_group = 7;
    r.managed[1].pid = 2;
    let mut ids = vec![first];
    for i in 0..3 {
        let fd = memfd::create(&mut r, 1, c"sync".as_ptr() as Word, 2).unwrap();
        let id = r.linux_file(1, fd).unwrap().resource;
        let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
        memory.backing = if i == 1 { None } else { Some(80 + i) };
        ids.push(id);
    }
    for (n, id) in ids.into_iter().enumerate() {
        let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
        for (i, owner) in [2, 1, 99, 1].into_iter().enumerate() {
            memory.maps.push(lookup_mapping(owner, 0x10000 + i * 4096 + n * 1024));
        }
    }
    for shared in [false, true] {
        r.managed[1].vm_group = if shared { 7 } else { 9 };
        for pid in [1, 2, 99, 123] {
            for (base, bytes) in [(0, 1), (0x10000, 0), (0x10800, 0), (0x10000, 4096), (0x11000, 12288), (0, 0x20000), (Word::MAX, 1)] {
                let mut expected = Vec::new();
                let overflow = base.checked_add(bytes).is_none();
                if let Some(end) = base.checked_add(bytes) {
                    for (_, object) in r.poll_objects.iter() {
                        if let poll_objects::Object::Memory(memory) = object {
                            for m in &memory.maps {
                                if r.vm_group(m.pid) == r.vm_group(pid) && m.base < end && base < m.base + m.bytes {
                                    if let Some(fd) = memory.backing { expected.push(fd); }
                                }
                            }
                        }
                    }
                }
                for failure in 0..=expected.len() {
                    MEMFD_SYNCS.with(|trace| *trace.borrow_mut() = (Vec::new(), if failure == 0 { None } else { Some(failure) }));
                    let result = memfd::sync_files(&r, pid, base, bytes);
                    assert_eq!(result, if overflow { Err(EINVAL) } else if failure != 0 { Err(map_request_error(RequestError)) } else { Ok(()) });
                    let count = if failure == 0 { expected.len() } else { failure };
                    MEMFD_SYNCS.with(|trace| assert_eq!(trace.borrow().0, expected[..count]));
                }
            }
        }
    }
}

fn lookup_mapping(pid: Word, base: Word) -> poll_objects::MemoryMapping {
    poll_objects::MemoryMapping {
        pid, base, bytes: LINUX_PAGE_SIZE, offset: base / 2, prot: 3, shared: true,
        attachment: Some(base), readonly_attachment: false,
    }
}

#[test]
fn memfd_lookup_filters_unrelated_ranges_before_vm_group_queries() {
    let (mut r, id, _) = collectible_memfd(0);
    let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
    memory.maps = (0..1024).map(|i| lookup_mapping(1, 0x10000 + i * 8192)).collect();
    let mut counts = Vec::new();
    for (address, expected) in [(0x10000 + 1023 * 8192, true), (Word::MAX, false)] {
        VM_GROUP_QUERIES.with(|count| count.set(0));
        assert_eq!(memfd::mapping_at(&r, 1, address).is_some(), expected);
        counts.push(VM_GROUP_QUERIES.with(|count| count.get()));
    }
    println!("memfd lookup VM-group queries late-hit/miss: {counts:?}");
    assert_eq!(counts, [0, 0]);
    r.managed[0].vm_group = 7;
    r.managed[1].pid = 2;
    r.managed[1].vm_group = 7;
    VM_GROUP_QUERIES.with(|count| count.set(0));
    let (_, mapping) = memfd::mapping_at(&r, 2, 0x10000 + 1023 * 8192).unwrap();
    assert_eq!(mapping.pid, 2);
    assert_eq!(VM_GROUP_QUERIES.with(|count| count.get()), 2);
}

#[test]
fn memfd_lookup_matches_group_first_reference_with_overlapping_address_spaces() {
    fn signature(result: Option<(Word, poll_objects::MemoryMapping)>) -> Option<(Word, Word, Word, Word, Word, Word, bool, Option<Word>, bool)> {
        result.map(|(id, m)| (id, m.pid, m.base, m.bytes, m.offset, m.prot, m.shared, m.attachment, m.readonly_attachment))
    }
    fn reference(r: &Runtime, pid: Word, address: Word) -> Option<(Word, poll_objects::MemoryMapping)> {
        r.poll_objects.iter().find_map(|(id, object)| match object {
            poll_objects::Object::Memory(memory) => memory.maps.iter().copied().find(|m| {
                r.vm_group(m.pid) == r.vm_group(pid) && address >= m.base && address - m.base < m.bytes
            }).map(|mut m| { m.pid = pid; (id, m) }),
            _ => None,
        })
    }
    let (mut r, first, _) = collectible_memfd(0);
    r.managed[1].pid = 2;
    let fd = memfd::create(&mut r, 1, c"lookup".as_ptr() as Word, 2).unwrap();
    let second = r.linux_file(1, fd).unwrap().resource;
    for id in [first, second] {
        let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
        for i in 0..64 {
            for owner in [2, 99, 1] {
                let mut m = lookup_mapping(owner, 0x10000 + i * 8192);
                m.offset += id + owner;
                m.shared = owner != 99;
                m.readonly_attachment = owner == 2;
                memory.maps.push(m);
            }
        }
        memory.maps.push(lookup_mapping(1, Word::MAX - LINUX_PAGE_SIZE + 1));
    }
    for shared in [false, true] {
        r.managed[0].vm_group = 7;
        r.managed[1].vm_group = if shared { 7 } else { 9 };
        for pid in [1, 2, 99, 123] {
            for i in 0..64 {
                let base = 0x10000 + i * 8192;
                for address in [base - 1, base, base + 4095, base + 4096, Word::MAX] {
                    assert_eq!(signature(memfd::mapping_at(&r, pid, address)), signature(reference(&r, pid, address)), "shared={shared} pid={pid} address={address:#x}");
                }
            }
        }
    }
}

fn collectible_memfd(pages: usize) -> (Runtime, Word, Vec<Word>) {
    let mut r = runtime();
    let fd = memfd::create(&mut r, 1, c"cleanup".as_ptr() as Word, 2).unwrap();
    let id = r.linux_file(1, fd).unwrap().resource;
    let addresses: Vec<_> = (0..pages).map(|_| request_memory_create(LINUX_PAGE_SIZE).unwrap()).collect();
    let Some(poll_objects::Object::Memory(memory)) = r.poll_objects.get_mut(id) else { panic!() };
    memory.backing = Some(77);
    memory.size = pages * 3 * LINUX_PAGE_SIZE;
    for (i, &address) in addresses.iter().enumerate() { memory.pages.insert(i * 3, address); }
    r.files[fd] = LinuxFile::EMPTY;
    r.managed[0].files[fd] = LinuxFile::EMPTY;
    MEMFD_CLEANUP_TRACE.with(|trace| *trace.borrow_mut() = Some(MemfdCleanupTrace {
        releases: Vec::with_capacity(pages + 4), unmaps: Vec::with_capacity(pages + 4),
        closes: Vec::with_capacity(4), fail_release: None, fail_unmap: None, fail_close: false,
    }));
    (r, id, addresses)
}

#[test]
fn memfd_forget_process_only_queries_vm_group_for_memory_cleanup() {
    let mut counts = Vec::new();
    for mode in 0..3 {
        let mut r = runtime();
        r.managed[0].vm_group = 7;
        r.managed.truncate(1);
        for pid in 2..=256 { r.managed.push(Process { pid, vm_group: 9, ..Process::default() }); }
        if mode == 1 {
            r.poll_objects.insert(poll_objects::Object::Counter(poll_objects::EventCounter {
                value: 0, semaphore: false, nonblocking: false,
            })).unwrap();
        }
        let id = if mode == 2 {
            let fd = memfd::create(&mut r, 1, c"exit".as_ptr() as Word, 0).unwrap();
            let id = r.linux_file(1, fd).unwrap().resource;
            r.poll_objects.memory_mut(id).unwrap().maps.push(lookup_mapping(1, 0x10000));
            Some(id)
        } else { None };
        VM_GROUP_QUERIES.with(|count| count.set(0));
        memfd::forget_process(&mut r, 1);
        counts.push(VM_GROUP_QUERIES.with(|count| count.get()));
        if let Some(id) = id { assert!(r.poll_objects.memory_mut(id).unwrap().maps.is_empty()); }
    }
    println!("memfd forget VM group queries empty/counter-only/memory with255 peers: {counts:?}");
    assert_eq!(counts, [0, 0, 1]);
}

#[test]
fn memfd_forget_process_keeps_first_live_replacement_and_mapping_order() {
    for replacement in [None, Some(2), Some(9)] {
        let mut r = runtime();
        r.managed = vec![Process { pid: 1, vm_group: 7, ..Process::default() }];
        for pid in 2..=9 {
            r.managed.push(Process { pid, vm_group: if Some(pid) == replacement { 7 } else { 9 }, ..Process::default() });
        }
        // Empty and exited slots must not become replacements even with matching group.
        r.managed.insert(1, Process { pid: 20, vm_group: 7, exited: true, ..Process::default() });
        r.managed.insert(1, Process { pid: 0, vm_group: 7, ..Process::default() });
        if replacement == Some(2) { r.managed.push(Process { pid: 21, vm_group: 7, ..Process::default() }); }
        let fd = memfd::create(&mut r, 1, c"exit".as_ptr() as Word, 0).unwrap();
        let id = r.linux_file(1, fd).unwrap().resource;
        let original = [lookup_mapping(1, 0x10000), lookup_mapping(30, 0x20000), lookup_mapping(1, 0x30000)];
        r.poll_objects.memory_mut(id).unwrap().maps.extend(original);
        let signature = |m: &poll_objects::MemoryMapping| (m.pid, m.base, m.bytes, m.offset, m.prot, m.shared, m.attachment, m.readonly_attachment);
        let expected: Vec<_> = original.into_iter().filter_map(|mut m| {
            if m.pid == 1 { m.pid = replacement?; }
            Some(signature(&m))
        }).collect();
        memfd::forget_process(&mut r, 1);
        assert_eq!(r.poll_objects.memory_mut(id).unwrap().maps.iter().map(signature).collect::<Vec<_>>(), expected);
    }
}

#[test]
fn memfd_collection_skips_global_ring_scan_for_retained_or_missing_memory() {
    let mut counts = Vec::new();
    for mode in 0..3 {
        let (mut r, id, _) = collectible_memfd(0);
        let memory = r.poll_objects.memory_mut(id).unwrap();
        if mode == 0 { memory.maps.push(lookup_mapping(1, 0x10000)); }
        if mode == 1 {
            memory.sysv = Some(poll_objects::SysvSegment {
                scope: 1, key: 7, mode: 0o600, creator: 1, last_pid: 1,
                created: 0, attached: 0, detached: 0, removed: false,
            });
        }
        for _ in 1..poll_objects::OBJECT_LIMIT {
            r.poll_objects.insert(poll_objects::Object::Counter(poll_objects::EventCounter {
                value: 0, semaphore: false, nonblocking: false,
            })).unwrap();
        }
        memfd::take_collect_ring_inspections();
        for _ in 0..64 { memfd::collect(&mut r, if mode == 2 { Word::MAX } else { id }); }
        counts.push(memfd::take_collect_ring_inspections());
        assert!(r.poll_objects.get(id).is_some());
        let memory = r.poll_objects.memory_mut(id).unwrap();
        assert_eq!(memory.maps.len(), usize::from(mode == 0));
        assert_eq!(memory.sysv.as_ref().map(|s| s.removed), (mode == 1).then_some(false));
        MEMFD_CLEANUP_TRACE.with(|trace| {
            let trace = trace.borrow(); let trace = trace.as_ref().unwrap();
            assert!(trace.releases.is_empty() && trace.unmaps.is_empty() && trace.closes.is_empty());
        });
    }
    println!("memfd collect ring inspections mapped/live-sysv/missing x64: {counts:?}");
    assert_eq!(counts, [0, 0, 0]);
}

#[test]
fn memfd_collection_keeps_ring_pins_after_mapping_and_sysv_guards_clear() {
    for sqes in [false, true] {
        let (mut r, id, _) = collectible_memfd(0);
        let memory = r.poll_objects.memory_mut(id).unwrap();
        memory.maps.push(lookup_mapping(1, 0x10000));
        memory.sysv = Some(poll_objects::SysvSegment {
            scope: 1, key: 7, mode: 0o600, creator: 1, last_pid: 1,
            created: 0, attached: 0, detached: 0, removed: false,
        });
        let ring = r.poll_objects.insert(poll_objects::Object::Ring(poll_objects::IoUring {
            memory: if sqes { Word::MAX } else { id }, sqes: if sqes { id } else { Word::MAX },
            entries: 1, cq_entries: 1, cq_offset: 0, ring_bytes: 4096, sqe_bytes: 4096,
            array: false, head: 0, tail: 0, cancel_next: false, overflow: alloc::collections::VecDeque::new(),
        })).unwrap();
        memfd::collect(&mut r, id);
        r.poll_objects.memory_mut(id).unwrap().maps.clear();
        memfd::collect(&mut r, id); // Live SysV segment still retains it.
        r.poll_objects.memory_mut(id).unwrap().sysv.as_mut().unwrap().removed = true;
        memfd::collect(&mut r, id); // Ring memory/SQE pin now retains it.
        assert!(r.poll_objects.get(id).is_some());
        MEMFD_CLEANUP_TRACE.with(|trace| assert!(trace.borrow().as_ref().unwrap().closes.is_empty()));
        let pin = r.poll_objects.ring_mut(ring).unwrap();
        pin.memory = Word::MAX; pin.sqes = Word::MAX;
        memfd::collect(&mut r, id);
        assert!(r.poll_objects.get(id).is_none());
        assert!(r.poll_objects.ring_mut(ring).is_some());
        MEMFD_CLEANUP_TRACE.with(|trace| assert_eq!(trace.borrow().as_ref().unwrap().closes, [77]));
    }
}

#[test]
fn memfd_collection_allocation_and_release_order() {
    let mut counts = Vec::new();
    for pages in [0, 1, 8, 128, 1024] {
        let (mut r, id, addresses) = collectible_memfd(pages);
        // The historical counter name also covers this isolated collector scope.
        FUTEX_ALLOCATIONS.with(|count| count.set(Some(0)));
        memfd::collect(&mut r, id);
        counts.push(FUTEX_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert!(r.poll_objects.get(id).is_none());
        MEMFD_CLEANUP_TRACE.with(|trace| {
            let trace = trace.borrow(); let trace = trace.as_ref().unwrap();
            assert_eq!(trace.releases, addresses);
            assert_eq!(trace.unmaps, (0..pages).map(|i| (77, i * 3 * LINUX_PAGE_SIZE)).collect::<Vec<_>>());
            assert_eq!(trace.closes, [77]);
        });
        OWNED.with(|owned| assert!(addresses.iter().all(|a| !owned.borrow().contains_key(a))));
    }
    println!("memfd collector allocation counts: {counts:?}");
    assert_eq!(counts, [0; 5]);
}

#[test]
fn memfd_collection_retries_local_release_lease_release_and_close() {
    let (mut r, id, a) = collectible_memfd(3);
    let pages = |r: &Runtime| match r.poll_objects.get(id).unwrap() {
        poll_objects::Object::Memory(memory) => memory.pages.iter().map(|(&i, &a)| (i, a)).collect::<Vec<_>>(),
        _ => panic!(),
    };
    MEMFD_CLEANUP_TRACE.with(|t| t.borrow_mut().as_mut().unwrap().fail_release = Some(a[1]));
    memfd::collect(&mut r, id);
    assert_eq!(pages(&r), [(3, a[1]), (6, a[2])]);
    OWNED.with(|o| { let o = o.borrow(); assert!(!o.contains_key(&a[0])); assert!(o.contains_key(&a[1]) && o.contains_key(&a[2])); });
    MEMFD_CLEANUP_TRACE.with(|t| { let mut t = t.borrow_mut(); let t = t.as_mut().unwrap(); t.fail_release = None; t.fail_unmap = Some(3 * LINUX_PAGE_SIZE); });
    memfd::collect(&mut r, id);
    assert_eq!(pages(&r), [(3, 0), (6, a[2])]);
    OWNED.with(|o| { let o = o.borrow(); assert!(!o.contains_key(&a[1])); assert!(o.contains_key(&a[2])); });
    MEMFD_CLEANUP_TRACE.with(|t| { let mut t = t.borrow_mut(); let t = t.as_mut().unwrap(); t.fail_unmap = None; t.fail_close = true; });
    memfd::collect(&mut r, id);
    assert!(pages(&r).is_empty());
    MEMFD_CLEANUP_TRACE.with(|t| t.borrow_mut().as_mut().unwrap().fail_close = false);
    memfd::collect(&mut r, id);
    assert!(r.poll_objects.get(id).is_none());
    MEMFD_CLEANUP_TRACE.with(|t| {
        let t = t.borrow(); let t = t.as_ref().unwrap();
        assert_eq!(t.releases, [a[0], a[1], a[1], a[2]]);
        assert_eq!(t.unmaps, [(77, 0), (77, 3 * LINUX_PAGE_SIZE), (77, 3 * LINUX_PAGE_SIZE), (77, 6 * LINUX_PAGE_SIZE)]);
        assert_eq!(t.closes, [77, 77]);
    });
}

#[test]
fn a_read_only_shared_memfd_mapping_still_prevents_a_new_write_seal() {
    let mut r = runtime();
    let fd = memfd::create(&mut r, 1, c"sealed".as_ptr() as Word, 2).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    memfd::truncate(&mut r, file, 4096).unwrap();
    let base = memfd::map(&mut r, 1, 0, 4096, 1, 1, file, 0).unwrap();
    assert_eq!(memfd::seals(&mut r, file, Some(8)), Err(EBUSY));
    sys_munmap(&mut r, 1, base, 4096).unwrap();
    memfd::seals(&mut r, file, Some(8)).unwrap();
    let base = memfd::map(&mut r, 1, 0, 4096, 1, 1, file, 0).unwrap();
    assert_eq!(memfd::seals(&mut r, file, Some(8)), Ok(0));
    assert_eq!(memfd::protect(&r, 1, base, 4096, 3), Err(EACCES));
}

#[test]
fn futex_shared_alias_keys_wake_the_waiter_not_just_the_same_virtual_address() {
    let mut r = runtime();
    r.managed[1].pid = 2;
    r.managed[1].pcb = 102;
    let fd = memfd::create(&mut r, 1, c"futex".as_ptr() as Word, 0).unwrap();
    let file = r.linux_file(1, fd).unwrap();
    memfd::truncate(&mut r, file, 4096).unwrap();
    let first = memfd::map(&mut r, 1, 0, 4096, 3, 1, file, 0).unwrap();
    let second = memfd::map(&mut r, 2, 0x80010000, 4096, 3, 1 | LINUX_MAP_FIXED, file, 0).unwrap();
    write_guest_u32(&mut r, 1, first, 42).unwrap();
    let wait = LinuxSyscallContext {
        number: SYS_FUTEX,
        args: [first, 0, 41, 0, 0, 0],
    };
    assert_eq!(
        futex::dispatch(&mut r, 1, wait),
        EmulationAction::Return(-(EAGAIN as isize))
    );
    let wait = LinuxSyscallContext {
        args: [first, 0, 42, 0, 0, 0],
        ..wait
    };
    assert_eq!(futex::dispatch(&mut r, 1, wait), EmulationAction::Park);
    let wake = LinuxSyscallContext {
        number: SYS_FUTEX,
        args: [second, 1 | 128, 1, 0, 0, 0],
    };
    assert_eq!(futex::dispatch(&mut r, 2, wake), EmulationAction::Return(0));
    assert!(r.managed[0].signal_waiting);
    let wake = LinuxSyscallContext {
        args: [second, 1, 1, 0, 0, 0],
        ..wake
    };
    assert_eq!(futex::dispatch(&mut r, 2, wake), EmulationAction::Return(1));
    assert!(!r.managed[0].signal_waiting);
    RETURNS.with(|returns| assert_eq!(*returns.borrow(), [(101, 0)]));
}

#[test]
fn futex_deadline_ignores_untimed_completed_stale_and_nonfutex_waits() {
    let mut r = runtime();
    r.managed = (1..=64).map(|pid| Process {
        pid, signal_waiting: true,
        signal_context: LinuxSyscallContext { number: SYS_FUTEX, args: [0; 6] },
        ..Process::default()
    }).collect();
    for pid in 1..=64 {
        r.tasks.ensure(pid).unwrap().wait = Some(task_sync::FutexWait {
            key: task_sync::FutexKey { scope: 1, address: pid * 4096, shared: false },
            bitset: u32::MAX, deadline: None, result: None,
        });
    }
    FUTEX_DEADLINE_PROCESS_LOOKUPS.with(|count| count.set(0));
    assert_eq!(futex::next_deadline(&r), None);
    let untimed = FUTEX_DEADLINE_PROCESS_LOOKUPS.with(|count| count.replace(0));
    for (pid, deadline) in [(2, 110), (3, 90), (4, 80), (5, 70), (6, 60)] {
        r.tasks.ensure(pid).unwrap().wait.as_mut().unwrap().deadline = Some(deadline);
    }
    r.managed[2].exited = true;
    r.managed[3].signal_context.number = SYS_READ;
    r.managed[4].signal_waiting = false;
    r.tasks.ensure(6).unwrap().wait.as_mut().unwrap().result = Some(0);
    r.tasks.ensure(99).unwrap().wait = Some(task_sync::FutexWait {
        key: task_sync::FutexKey { scope: 1, address: 0, shared: false },
        bitset: 1, deadline: Some(50), result: None,
    });
    // Only pid 2 is both timed and currently blocked in this syscall.
    assert_eq!(futex::next_deadline(&r), Some(110));
    let mixed = FUTEX_DEADLINE_PROCESS_LOOKUPS.with(|count| count.replace(0));
    println!("FUTEX-DEADLINE-GATE process-lookups untimed={untimed} mixed={mixed}");
    r.tasks.ensure(2).unwrap().wait.as_mut().unwrap().deadline = Some(Word::MAX);
    assert_eq!(futex::next_deadline(&r), Some(Word::MAX));
    r.tasks.ensure(2).unwrap().wait.as_mut().unwrap().result = Some(-(ETIMEDOUT as isize));
    assert_eq!(futex::next_deadline(&r), None);
}

#[test]
fn futex_pump_reuses_walk_slots_when_clearing_stale_waiters() {
    for rows in [1usize, 2, 8, 64] {
        let mut r = runtime();
        for index in 0..rows {
            let pid = index + 100;
            let task = r.tasks.ensure(pid).unwrap();
            task.clear_tid = pid * 8;
            task.robust_list = pid * 16;
            task.wait = Some(task_sync::FutexWait {
                key: task_sync::FutexKey { scope: pid, address: pid * 4096, shared: false },
                bitset: u32::MAX, deadline: None, result: None,
            });
        }
        r.tasks.take_pid_checks();
        futex::pump(&mut r);
        let checks = r.tasks.take_pid_checks();
        for index in 0..rows {
            let pid = index + 100;
            let task = r.tasks.get(pid).unwrap();
            assert!(task.wait.is_none());
            assert_eq!((task.clear_tid, task.robust_list), (pid * 8, pid * 16));
        }
        RETURNS.with(|values| assert!(values.borrow().is_empty()));
        println!("FUTEX-TASK-SLOT rows={rows} pid-checks={checks}");
        assert_eq!(checks, rows);
    }
}

#[test]
fn futex_pump_and_wake_keep_wait_state_without_temporary_allocations() {
    fn allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
        FUTEX_ALLOCATIONS.with(|count| count.set(Some(0)));
        let result = operation();
        let count = FUTEX_ALLOCATIONS.with(|count| count.replace(None).unwrap());
        (result, count)
    }
    let mut r = runtime();
    r.managed[1].pid = 2;
    r.managed[1].pcb = 202;
    r.managed[1].vm_group = 1;
    r.arms.reserve(16);
    RETURNS.with(|values| values.borrow_mut().reserve(16));
    let value = 42u32;
    let context = LinuxSyscallContext { number: SYS_FUTEX,
        args: [&value as *const _ as Word, 128, 42, 0, 0, 0] };
    let stale = r.tasks.ensure(99).unwrap();
    stale.clear_tid = 1234;
    stale.wait = Some(task_sync::FutexWait {
        key: task_sync::FutexKey { scope: 1, address: &value as *const _ as Word, shared: false },
        bitset: u32::MAX, deadline: None, result: None,
    });
    assert_eq!(futex::dispatch(&mut r, 1, context), EmulationAction::Park);
    let (_, pump) = allocations(|| futex::pump(&mut r));
    assert!(r.tasks.get(99).unwrap().wait.is_none());
    assert_eq!(r.tasks.get(99).unwrap().clear_tid, 1234);
    assert!(r.managed[0].signal_waiting);
    let wake = LinuxSyscallContext { number: SYS_FUTEX,
        args: [&value as *const _ as Word, 1 | 128, 0, 0, 0, 0] };
    let (result, zero) = allocations(|| futex::dispatch(&mut r, 2, wake));
    assert_eq!(result, EmulationAction::Return(0));
    assert!(r.managed[0].signal_waiting);
    let wake = LinuxSyscallContext { args: [wake.args[0], wake.args[1], 1, 0, 0, 0], ..wake };
    let (result, woken) = allocations(|| futex::dispatch(&mut r, 2, wake));
    assert_eq!(result, EmulationAction::Return(1));
    assert!(!r.managed[0].signal_waiting);
    assert!(r.tasks.get(1).unwrap().wait.is_none());
    RETURNS.with(|values| assert_eq!(*values.borrow(), [(101, 0)]));
    assert_eq!(futex::dispatch(&mut r, 1, context), EmulationAction::Park);
    r.managed[0].signals_mut().stopped = true;
    let (result, stopped) = allocations(|| futex::dispatch(&mut r, 2, wake));
    assert_eq!(result, EmulationAction::Return(1));
    assert_eq!(r.tasks.get(1).unwrap().wait.unwrap().result, Some(0));
    assert!(r.managed[0].signal_waiting);
    RETURNS.with(|values| assert_eq!(values.borrow().len(), 1));
    r.managed[0].signals_mut().stopped = false;
    let (_, continued) = allocations(|| futex::pump(&mut r));
    assert!(!r.managed[0].signal_waiting);
    assert!(r.tasks.get(1).unwrap().wait.is_none());
    RETURNS.with(|values| assert_eq!(*values.borrow(), [(101, 0), (101, 0)]));
    println!("FUTEX-WAITER-CURSOR pump={pump} wake-zero={zero} wake={woken} stopped-wake={stopped} continue={continued}");
    assert_eq!([pump, zero, woken, stopped, continued], [0; 5]);
}

#[test]
fn futex_timeout_and_exit_clear_tid_use_real_wait_queue() {
    let mut r = runtime();
    let value = 7u32;
    let timeout = [0usize, 1_000_000];
    let context = LinuxSyscallContext {
        number: SYS_FUTEX,
        args: [
            &value as *const _ as Word,
            128,
            7,
            timeout.as_ptr() as Word,
            0,
            0,
        ],
    };
    assert_eq!(futex::dispatch(&mut r, 1, context), EmulationAction::Park);
    assert_eq!(futex::next_deadline(&r), Some(101));
    r.monotonic_ticks = 101;
    futex::pump(&mut r);
    assert!(!r.managed[0].signal_waiting);
    RETURNS.with(|returns| assert_eq!(*returns.borrow(), [(101, -(ETIMEDOUT as isize))]));
    let mut tid = 1u32;
    assert_eq!(futex::set_tid(&mut r, 1, &mut tid as *mut _ as Word), Ok(1));
    futex::exit(&mut r, 1);
    assert_eq!(tid, 0);
    assert!(r.tasks.get(1).is_none());
}

#[test]
fn robust_mutex_owner_death_is_atomic_and_does_not_overwrite_other_owners() {
    let mut r = runtime();
    let mut head = [0usize; 3];
    let mut node = [head.as_mut_ptr() as Word, (1u32 | (1 << 31)) as Word];
    head[0] = node.as_mut_ptr() as Word;
    head[1] = 8;
    assert_eq!(
        futex::set_robust(&mut r, 1, head.as_mut_ptr() as Word, 23),
        Err(EINVAL)
    );
    assert_eq!(
        futex::set_robust(&mut r, 1, head.as_mut_ptr() as Word, 24),
        Ok(0)
    );
    let (mut pointer, mut length) = (0usize, 0usize);
    futex::get_robust(
        &mut r,
        1,
        0,
        &mut pointer as *mut _ as Word,
        &mut length as *mut _ as Word,
    )
    .unwrap();
    assert_eq!((pointer, length), (head.as_ptr() as Word, 24));
    futex::exit(&mut r, 1);
    assert_eq!(node[1], (1u32 << 30 | 1u32 << 31) as Word);
    node[1] = 2;
    futex::set_robust(&mut r, 1, head.as_mut_ptr() as Word, 24).unwrap();
    futex::exit(&mut r, 1);
    assert_eq!(node[1], 2);
}
