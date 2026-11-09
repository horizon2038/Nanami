//! Production VM-quiescence guard; model only task state and synchronous IPC.
#![allow(dead_code)]
extern crate alloc;
extern crate self as a9n_abi;
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/readonly_mapping.rs"]
mod readonly_mapping;
mod state { pub use super::readonly_mapping::ReadonlyMapping; }
const EIO: i32 = 5;
mod guest_views {
    pub(super) fn invalidate(_: &super::Runtime, _: usize) {}
}
pub use nanami_uapi::os::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY};
#[macro_export]
macro_rules! println {
    ($($args:tt)*) => {};
}
#[derive(Default)]
struct Signals {
    stopped: bool,
    stopped_context: Option<()>,
}
#[derive(Default)]
struct Rootfs {
    prepared: bool,
}
struct Process {
    pid: Word,
    pcb: Word,
    vm_group: Word,
    vm_private_shared: bool,
    exited: bool,
    parked: bool,
    rootfs: Rootfs,
    signals: Signals,
}
impl Process {
    fn parked_context(&self) -> Option<()> {
        self.parked.then_some(())
    }
    fn signals(&self) -> Option<&Signals> {
        Some(&self.signals)
    }
}
struct Runtime {
    readonly_mapping: std::cell::Cell<state::ReadonlyMapping>,
    managed: Vec<Process>,
    current_fault_pid: Word,
}
impl Runtime {
    fn vm_group(&self, pid: Word) -> Word {
        self.managed_process(pid).unwrap().vm_group
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Call {
    Query,
    Suspend(Word),
    Detach,
    Write,
    Resume(Word),
    Kill(Word),
}
struct Backend {
    shared: bool,
    fail_stop: Word,
    fail_resume: Word,
    fail_detach: bool,
    calls: Vec<Call>,
}
thread_local! {
    static BACKEND: RefCell<Backend> = const { RefCell::new(Backend { shared: true, fail_stop: 0, fail_resume: 0, fail_detach: false, calls: Vec::new() }) };
}
fn map_request_error(_: ()) -> i32 {
    EIO
}
pub fn request_process_private_write(_: Word, _: Word, _: Word, mode: Word) -> Result<bool, ()> {
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        if mode == OS_PRIVATE_WRITE_QUERY {
            backend.calls.push(Call::Query);
            return Ok(backend.shared);
        }
        assert_eq!(mode, OS_PRIVATE_WRITE_DETACH);
        backend.calls.push(Call::Detach);
        if backend.fail_detach {
            return Err(());
        }
        Ok(false)
    })
}
pub fn request_process_kill(pid: Word, _: Word) -> Result<(), ()> {
    BACKEND.with(|backend| backend.borrow_mut().calls.push(Call::Kill(pid)));
    Ok(())
}
pub mod arch {
    pub mod process_control_block {
        use super::super::*;
        pub fn suspend(pcb: Word) -> Result<(), ()> {
            BACKEND.with(|backend| {
                let mut backend = backend.borrow_mut();
                backend.calls.push(Call::Suspend(pcb));
                if backend.fail_stop == pcb {
                    return Err(());
                }
                Ok(())
            })
        }
        pub fn resume(pcb: Word) -> Result<(), ()> {
            BACKEND.with(|backend| {
                let mut backend = backend.borrow_mut();
                backend.calls.push(Call::Resume(pcb));
                if backend.fail_resume == pcb {
                    return Err(());
                }
                Ok(())
            })
        }
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/private_memory.rs"]
mod production;
fn setup() -> Runtime {
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Backend {
            shared: true,
            fail_stop: 0,
            fail_resume: 0,
            fail_detach: false,
            calls: Vec::new(),
        }
    });
    Runtime {
        readonly_mapping: std::cell::Cell::new(state::ReadonlyMapping::EMPTY),
        current_fault_pid: 1,
        managed: (1..=8)
            .map(|pid| Process {
                pid,
                pcb: pid + 100,
                vm_group: 1,
                vm_private_shared: true,
                exited: false,
                parked: false,
                rootfs: Rootfs::default(),
                signals: Signals::default(),
            })
            .collect(),
    }
}
fn write() -> Result<Word, i32> {
    BACKEND.with(|backend| backend.borrow_mut().calls.push(Call::Write));
    Ok(91)
}
fn calls(expected: &[Call]) {
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls, expected));
}

#[test]
fn guard_stops_only_active_vm_peers_never_replaying_a_handled_wait() {
    let mut runtime = setup();
    runtime.managed[2].parked = true;
    runtime.managed[3].rootfs.prepared = true;
    runtime.managed[4].signals.stopped = true;
    runtime.managed[5].signals.stopped_context = Some(());
    runtime.managed[6].exited = true;
    runtime.managed[7].vm_group = 8;
    assert_eq!(
        production::with_private_write(&runtime, 1, 4096, 4096, write),
        Ok(91)
    );
    calls(&[
        Call::Query,
        Call::Suspend(102),
        Call::Detach,
        Call::Write,
        Call::Resume(102),
    ]);
}

#[test]
fn known_snapshot_skips_query_but_still_detaches_under_vm_quiescence() {
    for marker in [false, true] {
        let mut runtime = setup(); runtime.managed.truncate(2);
        runtime.managed[0].vm_private_shared = marker;
        runtime.readonly_mapping.set(state::ReadonlyMapping::new(3, 1 << 32 | 7, 4096, 4096, 4096, 0));
        BACKEND.with(|b| b.borrow_mut().shared = false);
        assert_eq!(production::with_snapshot_write(&runtime, 1, 4096, 4096, write), Ok(91));
        calls(&[Call::Suspend(102), Call::Detach, Call::Write, Call::Resume(102)]);
        assert_eq!(runtime.readonly_mapping.get().generation, 0);
    }
}

#[test]
fn known_snapshot_stop_detach_and_operation_failures_keep_barrier_order() {
    for stage in 0..3 {
        let mut runtime = setup(); runtime.managed.truncate(2);
        runtime.readonly_mapping.set(state::ReadonlyMapping::new(3, 1 << 32 | 7, 4096, 4096, 4096, 0));
        BACKEND.with(|b| {
            let mut b = b.borrow_mut(); b.fail_stop = if stage == 0 { 102 } else { 0 };
            b.fail_detach = stage == 1;
        });
        assert_eq!(production::with_snapshot_write(&runtime, 1, 4096, 4096, || {
            write()?; Err::<Word, _>(EIO)
        }), Err(EIO));
        assert_eq!(runtime.readonly_mapping.get().generation, 0);
        match stage {
            0 => calls(&[Call::Suspend(102)]),
            1 => calls(&[Call::Suspend(102), Call::Detach, Call::Resume(102)]),
            _ => calls(&[Call::Suspend(102), Call::Detach, Call::Write, Call::Resume(102)]),
        }
    }
}

#[test]
fn ordinary_copyout_has_no_prepare_or_stop_and_unshared_range_only_queries() {
    let mut runtime = setup();
    runtime.managed[0].vm_private_shared = false;
    assert_eq!(
        production::with_private_write(&runtime, 1, 4096, 4096, write),
        Ok(91)
    );
    calls(&[Call::Write]);
    BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        backend.calls.clear();
        backend.shared = false;
    });
    runtime.managed[0].vm_private_shared = true;
    assert_eq!(
        production::with_private_write(&runtime, 1, 4096, 4096, write),
        Ok(91)
    );
    calls(&[Call::Query, Call::Write]);
}

#[test]
fn stop_failure_resumes_only_acknowledged_prefix_without_detach_or_write() {
    let runtime = setup();
    BACKEND.with(|backend| backend.borrow_mut().fail_stop = 103);
    assert_eq!(
        production::with_private_write(&runtime, 1, 4096, 4096, write),
        Err(EIO)
    );
    calls(&[
        Call::Query,
        Call::Suspend(102),
        Call::Suspend(103),
        Call::Resume(102),
    ]);
}

#[test]
fn detach_and_operation_failures_restore_only_previously_running_tasks() {
    for fail_detach in [true, false] {
        let mut runtime = setup();
        runtime.managed.truncate(3);
        BACKEND.with(|backend| backend.borrow_mut().fail_detach = fail_detach);
        assert_eq!(
            production::with_private_write(&runtime, 1, 4096, 4096, || Err::<Word, _>(EIO)),
            Err(EIO)
        );
        calls(&[
            Call::Query,
            Call::Suspend(102),
            Call::Suspend(103),
            Call::Detach,
            Call::Resume(103),
            Call::Resume(102),
        ]);
    }
}

#[test]
fn resume_failure_kills_failed_peer_without_reversing_a_committed_write() {
    let mut runtime = setup();
    runtime.managed.truncate(2);
    BACKEND.with(|backend| backend.borrow_mut().fail_resume = 102);
    assert_eq!(
        production::with_private_write(&runtime, 1, 4096, 4096, write),
        Ok(91)
    );
    calls(&[
        Call::Query,
        Call::Suspend(102),
        Call::Detach,
        Call::Write,
        Call::Resume(102),
        Call::Kill(2),
    ]);
}

#[test]
fn marking_sharing_follows_vm_group_not_process_group_and_reaches_all_peers() {
    let mut runtime = setup();
    runtime.managed[7].vm_group = 8;
    for process in &mut runtime.managed {
        process.vm_private_shared = false;
    }
    production::mark_shared(&mut runtime, 1);
    assert!(runtime.managed[..7].iter().all(|p| p.vm_private_shared));
    assert!(!runtime.managed[7].vm_private_shared);
}

#[test]
fn fork_barrier_quiesces_running_peers_even_without_shared_markers() {
    let mut runtime = setup();
    runtime.managed.truncate(2);
    runtime.managed[0].vm_private_shared = false;
    assert_eq!(production::with_vm_quiescence(&runtime, 1, write), Ok(91));
    calls(&[Call::Suspend(102), Call::Write, Call::Resume(102)]);
}

#[test]
fn privileged_writes_clear_immutable_origin_even_without_detach_or_guest_views() {
    for shared_marker in [false, true] {
        let mut runtime = setup();
        runtime.managed[0].vm_private_shared = shared_marker;
        BACKEND.with(|backend| backend.borrow_mut().shared = false);
        runtime.readonly_mapping.set(state::ReadonlyMapping::new(1, 1 << 32, 4096, 4096, 4096, 0));
        assert_eq!(production::with_private_write(&runtime, 1, 4096, 1, || {
            assert_eq!(runtime.readonly_mapping.get().generation, 0);
            write()
        }), Ok(91));
    }
}
