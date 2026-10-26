//! Production signal pump, with synchronous-stop and guest-memory mocks.
#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as a9n_abi;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub type Word = usize;

#[path = "../../nanami/servers/apps/alter/shared/src/common/signals.rs"]
pub mod signals;
mod common {
    pub(crate) use crate::signals;
}
mod state {
    pub mod readiness {
        pub const READY_OBJECT: usize = 32;
    }
    pub const LINUX_FD_MAX: usize = 64;
    pub(crate) use crate::OsPersonality;
}
#[path = "../../nanami/servers/uapi/src/terminal.rs"]
mod terminal_protocol;
const RING_BYTES: usize = 4096;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
#[path = "../../nanami/servers/apps/terminal-service/src/ring.rs"]
mod ring;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/terminal_signals.rs"]
mod terminal_signals;
use constants::*;
#[cfg(target_arch = "x86_64")]
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/arch/x86_64.rs"]
mod syscalls;
#[cfg(target_arch = "aarch64")]
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/arch/aarch64.rs"]
mod syscalls;
use syscalls::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/signals_runtime.rs"]
mod delivery;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/signal_frame.rs"]
mod signal_frame;
use delivery as signals_runtime;
mod futex {
    pub(super) fn pump(_: &mut super::Runtime) {}
}
mod threads {
    pub(super) fn exit_peers(_: &mut super::Runtime, _: usize, _: usize) {}
}
use process::{HardwareContext, LinuxSyscallContext};
use signals::{Action, Info, Signals};

#[derive(Clone, Copy, PartialEq, Eq)]
enum OsPersonality {
    Linux,
    FreeBsd,
}
#[derive(Clone, Copy)]
struct Job {
    session: Word,
    group: Word,
    controlling_terminal: Word,
}
#[derive(Clone, Copy)]
struct TerminalControl {
    id: Word,
    session: Word,
    foreground: Word,
}
#[derive(Clone, Copy, Default)]
struct Rootfs {
    prepared: bool,
}
#[derive(Clone, Copy)]
struct ManagedProcess {
    pid: Word,
    tgid: Word,
    exit_status: Word,
    parent_pid: Word,
    pcb: Word,
    exited: bool,
    signal_state: Word,
    signal_waiting: bool,
    signal_context: LinuxSyscallContext,
    sleep_deadline: Word,
    rootfs: Rootfs,
    personality: OsPersonality,
    job: Job,
    terminal_id: Word,
    terminal_echo: bool,
    terminal_termios: Option<[u8; 36]>,
    terminal_line_len: usize,
    terminal_line_read: usize,
    terminal_line_ready: bool,
}
impl ManagedProcess {
    fn signals(&self) -> Option<&Signals> {
        Some(unsafe { &*(self.signal_state as *const Signals) })
    }
    fn signals_mut(&mut self) -> &mut Signals {
        unsafe { &mut *(self.signal_state as *mut Signals) }
    }
    fn parked_context(&self) -> Option<LinuxSyscallContext> {
        self.signal_waiting.then_some(self.signal_context)
    }
    fn signal_stopped(&self) -> bool {
        self.signals().is_some_and(|state| state.stopped)
    }
    fn cancel_signal_wait(&mut self) {
        self.signal_waiting = false;
    }
}
struct Runtime {
    readiness_changes: Word,
    managed: Vec<ManagedProcess>,
    signals_pending: bool,
    posix_shm: Word,
    posix_shm_size: Word,
    monotonic_ticks: Word,
    monotonic_tick_hz: Word,
    timer_port: Word,
    scratch: Box<[u8; 8192]>,
    terminal_port: Word,
    terminal_controls: Vec<TerminalControl>,
}
impl Runtime {
    fn thread_group(&self, pid: Word) -> Word {
        self.managed_process(pid)
            .map_or(pid, |process| process.tgid)
    }
    fn has_live_threads(&self, pid: Word) -> bool {
        let group = self.thread_group(pid);
        self.managed
            .iter()
            .any(|other| other.pid != pid && !other.exited && other.tgid == group)
    }
    fn new() -> Self {
        BACKEND.with(|state| *state.borrow_mut() = Backend::new());
        let mut scratch = Box::new([0; 8192]);
        let mut runtime = Self {
            readiness_changes: 0,
            managed: Vec::new(),
            signals_pending: false,
            posix_shm: scratch.as_mut_ptr() as Word,
            posix_shm_size: scratch.len(),
            monotonic_ticks: 0,
            monotonic_tick_hz: 1_000_000_000,
            timer_port: 0,
            scratch,
            terminal_port: 1,
            terminal_controls: vec![TerminalControl {
                id: 1,
                session: 10,
                foreground: 10,
            }],
        };
        runtime.managed.push(ManagedProcess {
            pid: 10,
            tgid: 10,
            exit_status: 0,
            parent_pid: 0,
            pcb: 42,
            exited: false,
            signal_state: Box::into_raw(Box::new(Signals::new())) as Word,
            signal_waiting: false,
            signal_context: LinuxSyscallContext::EMPTY,
            sleep_deadline: 0,
            rootfs: Rootfs::default(),
            personality: OsPersonality::Linux,
            job: Job {
                session: 10,
                group: 10,
                controlling_terminal: 1,
            },
            terminal_id: 1,
            terminal_echo: true,
            terminal_termios: None,
            terminal_line_len: 5,
            terminal_line_read: 0,
            terminal_line_ready: false,
        });
        BACKEND.with(|state| state.borrow_mut().input_shm = runtime.posix_shm);
        runtime.managed[0].signals_mut().actions.borrow_mut()[9] = Action {
            handler: 0x2000,
            restorer: 0x3000,
            flags: signals::SA_RESTORER,
            ..Action::default()
        };
        runtime
    }
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        self.managed.iter_mut().find(|p| p.pid == pid)
    }
    fn mark_process_exited(&mut self, pid: Word, _: Word) {
        self.managed_process_mut(pid).unwrap().exited = true;
    }
    fn remove_process(&mut self, _: Word) {}
    fn has_child(&self, _: Word, _: Word) -> bool {
        false
    }
    fn queue(&mut self, signal: Word) {
        delivery::queue(self, 10, Info::new(signal, 0, 20)).unwrap();
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        for process in &self.managed {
            drop(unsafe { Box::from_raw(process.signal_state as *mut Signals) });
        }
    }
}
struct Backend {
    registers: HardwareContext,
    stopped: bool,
    fail_suspend: bool,
    fail_read: bool,
    fail_write: bool,
    fail_resume: bool,
    queued_syscall: bool,
    calls: Vec<&'static str>,
    frame: Vec<u8>,
    input: ring::ByteRing,
    input_shm: Word,
}
impl Backend {
    fn new() -> Self {
        let mut registers = [0; 35];
        for (index, value) in registers.iter_mut().enumerate() {
            *value = 0x1000 + index;
        }
        registers[signal_frame::PC] = 0x4000;
        registers[signal_frame::SP] = 0x800000;
        Self {
            registers,
            stopped: false,
            fail_suspend: false,
            fail_read: false,
            fail_write: false,
            fail_resume: false,
            queued_syscall: false,
            calls: Vec::new(),
            frame: Vec::new(),
            input: ring::ByteRing::EMPTY,
            input_shm: 0,
        }
    }
}
thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::new());
}
fn instruction_bytes() -> Word {
    if cfg!(target_arch = "x86_64") {
        2
    } else {
        4
    }
}
fn calls() -> Vec<&'static str> {
    BACKEND.with(|state| state.borrow().calls.clone())
}
pub mod terminal {
    pub use crate::terminal_protocol::*;
    use crate::*;
    pub fn terminal_read_input(_: Word, _: Word, _: Word, max_len: Word) -> Result<Word, ()> {
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            state.calls.push("tty-read");
            if max_len == 0 {
                return Ok(state.input.len);
            }
            let address = state.input_shm;
            Ok(unsafe { state.input.read_into(address as *mut u8, max_len) })
        })
    }
    pub fn terminal_read_input_filtered(
        _: Word,
        _: Word,
        max_len: Word,
        controls: Word,
    ) -> Result<(Word, Word), ()> {
        let events = BACKEND.with(|state| {
            state.borrow_mut().input.take_controls(
                [
                    controls as u8,
                    (controls >> 8) as u8,
                    (controls >> 16) as u8,
                ],
                controls & TERMINAL_INPUT_FLUSH != 0,
            )
        });
        Ok((terminal_read_input(0, 0, 0, max_len)?, events))
    }
}
pub mod arch {
    pub mod process_control_block {
        use crate::*;
        pub fn suspend(_: Word) -> Result<(), ()> {
            BACKEND.with(|state| {
                let mut state = state.borrow_mut();
                state.calls.push("suspend");
                if state.fail_suspend {
                    return Err(());
                }
                state.stopped = true;
                if state.queued_syscall {
                    state.registers[signal_frame::PC] -= instruction_bytes();
                    state.queued_syscall = false;
                }
                Ok(())
            })
        }
        pub fn resume(_: Word) -> Result<(), ()> {
            BACKEND.with(|state| {
                let mut state = state.borrow_mut();
                assert!(state.stopped, "never resume a target still running");
                state.calls.push("resume");
                if state.fail_resume {
                    return Err(());
                }
                state.stopped = false;
                Ok(())
            })
        }
    }
}
pub mod process {
    use crate::*;
    pub type HardwareContext = [Word; 35];
    #[derive(Clone, Copy)]
    pub struct LinuxSyscallContext {
        pub number: Word,
        pub args: [Word; 6],
        pub program_counter: Word,
    }
    impl LinuxSyscallContext {
        pub const EMPTY: Self = Self {
            number: 0,
            args: [0; 6],
            program_counter: 0,
        };
    }
    pub fn read_hardware_context(_: Word) -> Result<HardwareContext, ()> {
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            assert!(state.stopped, "register reads require a completed stop");
            state.calls.push("read");
            if state.fail_read {
                return Err(());
            }
            Ok(state.registers)
        })
    }
    pub fn write_hardware_context(_: Word, registers: &HardwareContext) -> Result<(), ()> {
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            assert!(state.stopped);
            state.calls.push("write");
            if state.fail_write {
                return Err(());
            }
            state.registers = *registers;
            Ok(())
        })
    }
    pub fn configure_personality_syscall_reply(
        registers: &mut HardwareContext,
        context: LinuxSyscallContext,
        value: isize,
        _: OsPersonality,
    ) -> usize {
        registers[0] = value as Word;
        registers[signal_frame::PC] = context.program_counter;
        1
    }
}
fn map_anonymous_tracked(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<(Word, Word), i32> {
    Err(EFAULT)
}
fn sys_mprotect(_: &mut Runtime, _: Word, _: Word, _: Word, _: Word) -> Result<Word, i32> {
    Err(EFAULT)
}
fn sys_munmap(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<Word, i32> {
    Err(EFAULT)
}
fn write_target_memory_from(_: Word, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Err(EFAULT)
}
fn write_target_memory(runtime: &mut Runtime, _: Word, _: Word, bytes: Word) -> Result<(), i32> {
    BACKEND.with(|state| {
        state.borrow_mut().frame =
            unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes).to_vec() };
    });
    Ok(())
}
fn read_target_memory(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Err(EFAULT)
}
fn refresh_clock(_: &mut Runtime) -> Result<(), i32> {
    Ok(())
}
fn map_request_error(_: ()) -> i32 {
    EIO
}
fn split_ticks(ticks: Word, hz: Word) -> (Word, Word) {
    (ticks / hz, ticks % hz)
}
unsafe fn write_u64(address: Word, value: Word) {
    (address as *mut Word).write_unaligned(value);
}
fn close_process_files(_: &mut Runtime, _: Word) {}
fn notify_terminal_process_exit(_: &mut Runtime, _: Word, _: Word) {}
fn wake_waiter_for_child(_: &mut Runtime, _: Word) {}
fn wake_terminal_readers(_: &mut Runtime) {}
fn wake_network_waiters(_: &mut Runtime) {}
fn wake_device_readers(_: &mut Runtime) {}
fn wake_readiness_waiters(_: &mut Runtime, _: Word) {}
fn arm_clock_timer(_: &mut Runtime) -> Result<(), i32> {
    Ok(())
}
fn write_syscall_return(_: Word, _: LinuxSyscallContext, _: isize) -> Result<(), ()> {
    Ok(())
}
mod signal_syscalls {
    use super::*;
    pub(super) fn consume_wait(_: &mut Runtime, _: Word, _: Word) -> Option<isize> {
        None
    }
}
pub fn request_process_kill(_: Word, _: Word) -> Result<(), ()> {
    BACKEND.with(|state| state.borrow_mut().calls.push("kill"));
    Ok(())
}
pub fn request_process_reap(_: Word) -> Result<(), ()> {
    Ok(())
}
fn saved_context() -> HardwareContext {
    BACKEND.with(|state| {
        let state = state.borrow();
        let mut registers = state.registers;
        assert!(signal_frame::restore(&state.frame, &mut registers));
        registers
    })
}

#[test]
fn running_target_is_stopped_before_sampling_and_retains_its_continuation() {
    let mut runtime = Runtime::new();
    let original = BACKEND.with(|state| state.borrow().registers);
    runtime.queue(10);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read", "write", "resume"]);
    let saved = saved_context();
    assert_eq!(&saved[..15], &original[..15]);
    assert_eq!(saved[signal_frame::PC], original[signal_frame::PC]);
    assert_eq!(saved[signal_frame::SP], original[signal_frame::SP]);
    assert!(!runtime.signals_pending);
}

#[test]
fn unaccepted_syscall_replays_from_the_kernel_prepared_pc_without_eintr() {
    let mut runtime = Runtime::new();
    BACKEND.with(|state| state.borrow_mut().queued_syscall = true);
    runtime.queue(10);
    delivery::pump(&mut runtime, 0);
    let saved = saved_context();
    assert_eq!(saved[signal_frame::PC], 0x4000 - instruction_bytes());
    assert_eq!(saved[0], 0x1000);
}

#[test]
fn failed_stop_never_reads_or_consumes_the_pending_signal() {
    let mut runtime = Runtime::new();
    BACKEND.with(|state| state.borrow_mut().fail_suspend = true);
    runtime.queue(10);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend"]);
    assert_eq!(
        runtime.managed[0].signals().unwrap().pending_mask(),
        signals::bit(10)
    );
    BACKEND.with(|state| {
        state.borrow_mut().fail_suspend = false;
    });
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "suspend", "read", "write", "resume"]);
}

#[test]
fn ignored_pending_signal_resumes_without_rewriting_an_unchanged_context() {
    let mut runtime = Runtime::new();
    runtime.queue(10);
    runtime.managed[0].signals_mut().actions.borrow_mut()[9].handler = signals::SIG_IGN;
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read", "resume"]);
    assert!(!runtime.signals_pending);
}

#[test]
fn kill_can_terminate_a_created_bundle_without_starting_it() {
    let mut runtime = Runtime::new();
    runtime.managed[0].rootfs.prepared = true;
    runtime.queue(signals::SIGKILL);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["kill"]);
    assert!(runtime.managed[0].exited);
}

#[test]
fn active_fault_masked_signal_and_unstarted_bundle_do_not_get_suspended() {
    for case in 0..3 {
        let mut runtime = Runtime::new();
        let mut excluded = 0;
        match case {
            0 => excluded = 10,
            1 => runtime.managed[0].signals_mut().blocked = signals::bit(10),
            _ => runtime.managed[0].rootfs.prepared = true,
        }
        runtime.queue(10);
        delivery::pump(&mut runtime, excluded);
        assert!(calls().is_empty());
        assert_eq!(
            runtime.managed[0].signals().unwrap().pending_mask(),
            signals::bit(10)
        );
    }
}

#[test]
fn blocked_group_pending_keeps_delivery_cache_live_until_a_temporary_mask_unblocks_it() {
    let mut runtime = Runtime::new();
    runtime.managed[0].signals_mut().blocked = signals::bit(10);
    delivery::queue_group(&mut runtime, 10, Info::new(10, 0, 10)).unwrap();
    delivery::pump(&mut runtime, 0);
    assert!(calls().is_empty());
    assert!(runtime.signals_pending);
    runtime.managed[0].signals_mut().blocked = 0;
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read", "write", "resume"]);
    assert!(!runtime.signals_pending);
}

#[test]
fn parked_syscalls_keep_existing_eintr_and_sa_restart_without_another_stop() {
    for restart in [false, true] {
        let mut runtime = Runtime::new();
        runtime.managed[0].signal_waiting = true;
        runtime.managed[0].signal_context = LinuxSyscallContext {
            number: SYS_READ,
            args: [7, 0x5000, 16, 0, 0, 0],
            program_counter: 0x4000,
        };
        if restart {
            runtime.managed[0].signals_mut().actions.borrow_mut()[9].flags |= signals::SA_RESTART;
        }
        BACKEND.with(|state| state.borrow_mut().stopped = true);
        runtime.queue(10);
        delivery::pump(&mut runtime, 0);
        assert_eq!(calls(), ["read", "write", "resume"]);
        let saved = saved_context();
        if restart {
            assert_eq!(saved[signal_frame::PC], 0x4000 - instruction_bytes());
        } else {
            assert_eq!(saved[signal_frame::PC], 0x4000);
            assert_eq!(saved[0] as isize, -(EINTR as isize));
        }
        assert!(!runtime.managed[0].signal_waiting);
    }
}

#[test]
fn stop_and_continue_preserve_a_running_context_and_defer_handlers_until_continue() {
    let mut runtime = Runtime::new();
    runtime.queue(signals::SIGSTOP);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read"]);
    assert!(runtime.managed[0].signals().unwrap().stopped);
    runtime.queue(10);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read"]);
    runtime.queue(signals::SIGCONT);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read", "write", "resume"]);
    assert!(!runtime.managed[0].signals().unwrap().stopped);
    assert!(runtime.managed[0]
        .signals()
        .unwrap()
        .stopped_context
        .is_none());
    assert_eq!(saved_context()[signal_frame::PC], 0x4000);
}

#[test]
fn default_continue_resumes_saved_context_even_when_continue_is_blocked() {
    let mut runtime = Runtime::new();
    let original = BACKEND.with(|state| state.borrow().registers);
    runtime.managed[0].signals_mut().blocked = signals::bit(signals::SIGCONT);
    runtime.queue(signals::SIGSTOP);
    delivery::pump(&mut runtime, 0);
    runtime.queue(signals::SIGCONT);
    delivery::pump(&mut runtime, 0);
    assert_eq!(calls(), ["suspend", "read", "write", "resume"]);
    BACKEND.with(|state| assert_eq!(state.borrow().registers, original));
}

#[test]
fn context_failures_terminate_instead_of_resuming_partial_handler_state() {
    for failure in 0..3 {
        let mut runtime = Runtime::new();
        BACKEND.with(|state| {
            let mut state = state.borrow_mut();
            state.fail_read = failure == 0;
            state.fail_write = failure == 1;
            state.fail_resume = failure == 2;
        });
        runtime.queue(10);
        delivery::pump(&mut runtime, 0);
        assert_eq!(calls().last(), Some(&"kill"));
        assert!(runtime.managed[0].exited);
    }
}

fn input(bytes: &[u8]) {
    BACKEND.with(|state| unsafe {
        assert_eq!(
            state
                .borrow_mut()
                .input
                .write_from(bytes.as_ptr(), bytes.len()),
            bytes.len()
        );
    });
}

fn termios(flags: u32, interrupt: u8) -> [u8; 36] {
    let mut value = [0; 36];
    value[12..16].copy_from_slice(&flags.to_ne_bytes());
    value[17 + LINUX_VINTR] = interrupt;
    value[17 + LINUX_VQUIT] = 28;
    value[17 + LINUX_VSUSP] = 26;
    value
}

#[test]
fn terminal_interrupt_reaches_only_the_foreground_group_in_its_controlling_session() {
    let mut runtime = Runtime::new();
    runtime.terminal_controls[0].foreground = 11;
    for pid in [11, 12, 20] {
        let mut child = runtime.managed[0];
        child.pid = pid;
        child.tgid = pid;
        child.job.group = 11;
        if pid == 20 {
            child.job.session = 20;
        }
        child.signal_state = Box::into_raw(Box::new(Signals::new())) as Word;
        runtime.managed.push(child);
    }
    input(b"pending\x03after");
    terminal_signals::handle_terminal_input(&mut runtime);
    for process in &runtime.managed {
        let mask = process.signals().unwrap().pending_mask();
        if process.pid == 11 || process.pid == 12 {
            assert_eq!(mask, signals::bit(2));
            let mut snapshot = process.signals().unwrap().clone();
            assert_eq!(snapshot.take_matching(mask), Some(Info::new(2, 128, 0)));
        } else {
            assert_eq!(mask, 0);
        }
        assert_eq!(process.terminal_line_len, 0);
    }
    BACKEND.with(|state| assert_eq!(state.borrow().input.len, 5));
}

#[test]
fn isig_disabled_and_disabled_vintr_return_ctrl_c_as_an_ordinary_byte() {
    for attributes in [termios(0, 3), termios(LINUX_ISIG, 0)] {
        let mut runtime = Runtime::new();
        runtime.managed[0].terminal_termios = Some(attributes);
        input(b"\x03");
        assert_eq!(
            terminal_signals::read_terminal_input(&mut runtime, 10, 1),
            Ok(1)
        );
        assert_eq!(runtime.scratch[0], 3);
        assert!(!runtime.signals_pending);
        assert_eq!(runtime.managed[0].terminal_line_len, 5);
    }
}

#[test]
fn remapped_vintr_and_noflsh_preserve_ordinary_queued_and_partial_line_bytes() {
    let mut runtime = Runtime::new();
    runtime.managed[0].terminal_termios = Some(termios(LINUX_ISIG | LINUX_NOFLSH, b'x'));
    input(b"abx\x03");
    assert_eq!(
        terminal_signals::read_terminal_input(&mut runtime, 10, 0),
        Ok(3)
    );
    assert_eq!(
        runtime.managed[0].signals().unwrap().pending_mask(),
        signals::bit(2)
    );
    assert_eq!(runtime.managed[0].terminal_line_len, 5);
    assert_eq!(
        terminal_signals::read_terminal_input(&mut runtime, 10, 3),
        Ok(3)
    );
    assert_eq!(&runtime.scratch[..3], b"ab\x03");
}

#[test]
fn quit_and_suspend_characters_queue_their_linux_signals() {
    let mut runtime = Runtime::new();
    input(b"\x1c\x1a");
    terminal_signals::handle_terminal_input(&mut runtime);
    assert_eq!(
        runtime.managed[0].signals().unwrap().pending_mask(),
        signals::bit(3) | signals::bit(20)
    );
}
