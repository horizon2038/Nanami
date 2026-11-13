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
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_MAP_FIXED: Word = 16;
mod guest_views {
    pub(super) fn invalidate(_: &super::Runtime, _: usize) {}
    pub(super) fn has_cached_owner(runtime: &super::Runtime, _: usize) -> bool { runtime.cached_owner }
    pub(super) fn invalidate_range(_: &super::Runtime, _: usize, base: usize, bytes: usize) {
        super::RANGE_INVALIDATIONS.with_borrow_mut(|rows| rows.push((base, bytes,
            super::BACKEND.with_borrow(|b| b.calls.clone()))));
    }
}
thread_local! { static RANGE_INVALIDATIONS: RefCell<Vec<(Word, Word, Vec<Call>)>> = const { RefCell::new(Vec::new()) }; }
pub use nanami_uapi::os::{OS_PRIVATE_WRITE_DETACH, OS_PRIVATE_WRITE_QUERY, OS_PRIVATE_WRITE_DETACH_DEFERRED, OS_PRIVATE_WRITE_DETACH_POPULATE};
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
    mappings: Mappings,
    pid: Word,
    pcb: Word,
    vm_group: Word,
    vm_private_shared: bool,
    vm_writable_private_shared: bool,
    exited: bool,
    parked: bool,
    rootfs: Rootfs,
    signals: Signals,
}
struct Mapping { base: Word, size: Word, prot: Word }
struct Mappings(Word, Vec<Mapping>);
impl Mappings {
    fn generation(&self) -> Word { self.0 }
    fn iter(&self) -> impl Iterator<Item=&Mapping> { self.1.iter() }
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
    fault_permissions: Option<Word>,
    fault_access: bool,
    cached_owner: bool,
    readonly_mapping: std::cell::Cell<state::ReadonlyMapping>,
    managed: Vec<Process>,
    current_fault_pid: Word,
}
impl Runtime {
    fn can_protect_mapping(&self, _: Word, _: Word, _: Word) -> bool { self.fault_access }
    fn mapping_protection_needed(&self, _: Word, _: Word, _: Word, _: Word) -> Option<bool> { Some(true) }
    fn mapping_prot(&self, _: Word, _: Word, _: Word) -> Option<Word> { self.fault_permissions }
    fn memory_access_allowed(&self, _: Word, _: Word, _: Word, _: Word) -> bool { self.fault_access }
    fn vm_group(&self, pid: Word) -> Word {
        self.managed_process(pid).unwrap().vm_group
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Query,
    Suspend(Word),
    Detach,
    Protect,
    Write,
    Resume(Word),
    Kill(Word),
    Release,
}
struct Backend {
    shared: bool,
    fail_stop: Word,
    fail_resume: Word,
    fail_detach: bool,
    fail_protect: bool,
    calls: Vec<Call>,
    detach_modes: Vec<Word>,
}
thread_local! {
    static BACKEND: RefCell<Backend> = const { RefCell::new(Backend { shared: true, fail_stop: 0, fail_resume: 0, fail_detach: false, fail_protect: false, calls: Vec::new(), detach_modes: Vec::new() }) };
}
pub fn request_process_mapping_protect(_: Word, page: Word, bytes: Word, permissions: Word) -> Result<(), ()> {
    assert_eq!(page & 4095, 0);
    assert_eq!(bytes, 4096);
    assert!(permissions & 2 != 0);
    BACKEND.with(|backend| {
        let mut b = backend.borrow_mut();
        b.calls.push(Call::Protect);
        if b.fail_protect { Err(()) } else { Ok(()) }
    })
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
        assert!(matches!(mode, OS_PRIVATE_WRITE_DETACH | OS_PRIVATE_WRITE_DETACH_DEFERRED | OS_PRIVATE_WRITE_DETACH_POPULATE));
        backend.detach_modes.push(mode);
        backend.calls.push(Call::Detach);
        if backend.fail_detach {
            return Err(());
        }
        Ok(backend.shared)
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
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_remap.rs"]
mod remap;

// Model the mprotect adapter's hardware/metadata effects, using the production
// private-write guard. This checks that writable remap admission still enters
// its stop/DETACH/protect protocol rather than returning after a rights update.
fn sys_mprotect(runtime: &mut Runtime, pid: Word, base: Word, bytes: Word, prot: Word) -> Result<Word,i32> {
    let origin=remap::before_protect(runtime,pid,prot);
    production::with_private_protect(runtime,pid,base,bytes,|| {
        request_process_mapping_protect(pid,base,bytes,prot).map_err(map_request_error)
    })?;
    let group=runtime.vm_group(pid);
    for process in &mut runtime.managed {
        if process.vm_group==group { process.mappings.0+=1; }
    }
    remap::after_protect(runtime,pid,origin);
    Ok(0)
}
fn sys_munmap(_: &mut Runtime, _: Word, _: Word, _: Word) -> Result<Word,i32> {
    BACKEND.with_borrow_mut(|b| b.calls.push(Call::Release));
    Ok(0)
}
fn setup() -> Runtime {
    RANGE_INVALIDATIONS.with_borrow_mut(Vec::clear);
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Backend {
            shared: true,
            fail_stop: 0,
            fail_resume: 0,
            fail_detach: false,
            fail_protect: false,
            calls: Vec::new(),
            detach_modes: Vec::new(),
        }
    });
    Runtime {
        fault_permissions: Some(3),
        fault_access: true,
        cached_owner: false,
        readonly_mapping: std::cell::Cell::new(state::ReadonlyMapping::EMPTY),
        current_fault_pid: 1,
        managed: (1..=8)
            .map(|pid| Process {
                mappings: Mappings(1, Vec::new()),
                pid,
                pcb: pid + 100,
                vm_group: 1,
                vm_private_shared: true,
                vm_writable_private_shared: true,
                exited: false,
                parked: false,
                rootfs: Rootfs::default(),
                signals: Signals::default(),
            })
            .collect(),
    }
}

#[test]
fn confined_mapping_invalidation_keeps_stop_ack_order_and_does_not_run_after_failed_stop() {
    let r = setup();
    assert_eq!(production::with_mapping_mutation(&r, 1, 0x400000, 1 << 21, true, || {
        RANGE_INVALIDATIONS.with_borrow(|rows| {
            assert_eq!(rows.len(), 1);
            assert_eq!((rows[0].0, rows[0].1), (0x400000, 1 << 21));
            assert_eq!(rows[0].2, (2..=8).map(|p| Call::Suspend(p + 100)).collect::<Vec<_>>());
        });
        Err::<(), _>(EIO)
    }), Err(EIO));
    BACKEND.with_borrow(|b| assert!(b.calls.ends_with(
        &(2..=8).rev().map(|p| Call::Resume(p + 100)).collect::<Vec<_>>())));
    let r = setup();
    BACKEND.with_borrow_mut(|b| b.fail_stop = 104);
    assert_eq!(production::with_mapping_mutation(&r, 1, 0x400000, 1 << 21, true,
        || panic!("unacknowledged peer cannot reach mutation")), Err::<(), _>(EIO));
    RANGE_INVALIDATIONS.with_borrow(|rows| assert!(rows.is_empty()));
}
fn write() -> Result<Word, i32> {
    BACKEND.with(|backend| backend.borrow_mut().calls.push(Call::Write));
    Ok(91)
}
fn calls(expected: &[Call]) {
    BACKEND.with(|backend| assert_eq!(backend.borrow().calls, expected));
}

#[test]
fn protection_writes_and_faults_select_their_distinct_detach_contracts() {
    for kind in 0..5 {
        let r = setup();
        let result = match kind {
            0 => production::with_private_protect(&r, 1, 4096, 4096, write),
            1 => production::with_snapshot_write(&r, 1, 4096, 4096, write),
            2 => production::with_private_write(&r, 1, 4096, 4096, write),
            3 => production::with_checked_private_write(&r, 1, 4096, 4096, write),
            _ => { assert!(production::materialize_private_write_fault(&r, 1, 4099, 3)); Ok(91) },
        };
        assert_eq!(result, Ok(91));
        BACKEND.with_borrow(|b| assert_eq!(b.detach_modes,
            [if kind < 2 { OS_PRIVATE_WRITE_DETACH_DEFERRED }
                else if kind == 4 { OS_PRIVATE_WRITE_DETACH_POPULATE } else { OS_PRIVATE_WRITE_DETACH }]));
    }
}

#[test]
fn writable_fixed_reuse_keeps_cow_peer_stop_and_cleanup_ack_failures_authoritative() {
    for copied in [false,true] {
        for failure in 0..4 {
            let mut r=setup();
            for p in &mut r.managed[2..] { p.parked=true; }
            let constructor=if copied { state::ReadonlyMapping::copied } else { state::ReadonlyMapping::new };
            r.readonly_mapping.set(constructor(1,1<<32|7,8192,4096,8192,0).in_vm(1));
            BACKEND.with_borrow_mut(|b| match failure {
                1 => b.fail_stop=102,
                2 => b.fail_detach=true,
                3 => b.fail_protect=true,
                _ => {},
            });
            assert_eq!(remap::reuse(&mut r,1,1<<32|7,8192,4096,4096,0,3,18),
                if failure==0 {Ok(true)} else {Err(EIO)});
            assert_eq!(r.readonly_mapping.get().generation,0);
            match failure {
                0 => calls(&[Call::Query,Call::Suspend(102),Call::Detach,Call::Protect,Call::Resume(102)]),
                1 => calls(&[Call::Query,Call::Suspend(102),Call::Release]),
                2 => calls(&[Call::Query,Call::Suspend(102),Call::Detach,Call::Resume(102),Call::Release]),
                _ => calls(&[Call::Query,Call::Suspend(102),Call::Detach,Call::Protect,Call::Resume(102),Call::Release]),
            }
        }
    }
    let mut r=setup();
    for p in &mut r.managed { p.vm_private_shared=false; }
    r.readonly_mapping.set(state::ReadonlyMapping::copied(1,1<<32|7,8192,4096,8192,0).in_vm(1));
    assert_eq!(remap::reuse(&mut r,1,1<<32|7,8192,4096,4096,0,3,18),Ok(true));
    calls(&[Call::Protect]); // sole-owner copied backing needs no detach
}

#[test]
fn protect_with_quiescent_peers_and_no_views_detaches_without_query() {
    for shared in [false, true] {
        let mut runtime = setup();
        runtime.managed[1].parked = true;
        runtime.managed[2].rootfs.prepared = true;
        runtime.managed[3].signals.stopped = true;
        runtime.managed[4].signals.stopped_context = Some(());
        runtime.managed[5].exited = true;
        runtime.managed[6].vm_group = 9;
        runtime.managed[7].pid = 0;
        BACKEND.with(|b| b.borrow_mut().shared = shared);
        assert_eq!(production::with_private_protect(&runtime, 1, 4096, 4096, write), Ok(91));
        calls(&[Call::Detach, Call::Write]);
    }
}

#[test]
fn quiescent_copyout_preparation_counts_shared_empty_and_failed_detach_requests() {
    for mode in 0..3 {
        let mut runtime = setup(); runtime.managed.truncate(1);
        runtime.readonly_mapping.set(state::ReadonlyMapping::new(3, 1 << 32 | 7, 4096, 4096, 4096, 0));
        BACKEND.with(|b| { let mut b=b.borrow_mut(); b.shared=mode!=0; b.fail_detach=mode==2; });
        assert_eq!(production::with_private_write(&runtime, 1, 4096, 4096, write), if mode==2 { Err(EIO) } else { Ok(91) });
        assert_eq!(runtime.readonly_mapping.get().generation, 0);
        BACKEND.with(|b| {
            let b=b.borrow();
            let requests=b.calls.iter().filter(|c| matches!(c, Call::Query | Call::Detach)).count();
            assert_eq!(requests, 1);
            assert!(!b.calls.iter().any(|c| matches!(c, Call::Suspend(_) | Call::Resume(_))));
            assert_eq!(b.calls.contains(&Call::Write), mode!=2);
            std::println!("COPYOUT-PREP mode={mode} requests={requests}");
        });
    }
}

#[test]
fn copyout_keeps_query_and_acknowledged_barrier_for_cached_views_or_running_peers() {
    for cached in [false,true] {
        for shared in [false,true] {
            let mut r=setup();r.managed.truncate(if cached { 1 } else { 2 });r.cached_owner=cached;
            BACKEND.with(|b| b.borrow_mut().shared=shared);
            production::with_private_write(&r,1,4096,4,write).unwrap();
            let expected=if !shared { vec![Call::Query,Call::Write] }
                else if cached { vec![Call::Query,Call::Detach,Call::Write] }
                else { vec![Call::Query,Call::Suspend(102),Call::Detach,Call::Write,Call::Resume(102)] };
            calls(&expected);
        }
    }
}

#[test]
fn protect_preserves_query_for_active_peers_or_any_cached_alias() {
    for cached in [false, true] {
        for shared in [false, true] {
            let mut runtime = setup(); runtime.managed.truncate(if cached { 1 } else { 2 });
            runtime.cached_owner = cached;
            BACKEND.with(|b| b.borrow_mut().shared = shared);
            assert_eq!(production::with_private_protect(&runtime, 1, 4096, 4096, write), Ok(91));
            let expected = if !shared { vec![Call::Query, Call::Write] }
                else if cached { vec![Call::Query, Call::Detach, Call::Write] }
                else { vec![Call::Query, Call::Suspend(102), Call::Detach, Call::Write, Call::Resume(102)] };
            calls(&expected);
        }
    }
}

#[test]
fn direct_protect_detach_failure_clears_proof_and_never_writes() {
    let mut runtime = setup(); runtime.managed.truncate(1);
    runtime.readonly_mapping.set(state::ReadonlyMapping::new(3, 1 << 32 | 7, 4096, 4096, 4096, 0));
    BACKEND.with(|b| b.borrow_mut().fail_detach = true);
    assert_eq!(production::with_private_protect(&runtime, 1, 4096, 4096, write), Err(EIO));
    calls(&[Call::Detach]);
    assert_eq!(runtime.readonly_mapping.get().generation, 0);
}

#[test]
fn unshared_or_zero_byte_protect_keeps_no_request_fast_path() {
    for bytes in [0, 4096] {
        let mut runtime = setup(); runtime.managed[0].vm_private_shared = false;
        assert_eq!(production::with_private_protect(&runtime, 1, 4096, bytes, write), Ok(91));
        calls(&[Call::Write]);
    }
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
fn cold_write_view_requires_the_current_fault_and_no_runnable_vm_user() {
    let enabled=option_env!("NANAMI_COLD_WRITE_VIEWS")!=Some("0");
    for mode in 0..8 {
        let mut r=setup();r.managed.truncate(2);
        match mode {
            0=>{}, // A running peer forbids a cold attempt.
            1=>r.managed[1].parked=true,
            2=>r.managed[1].rootfs.prepared=true,
            3=>r.managed[1].signals.stopped=true,
            4=>r.managed[1].signals.stopped_context=Some(()),
            5=>r.managed[1].exited=true,
            6=>r.managed[1].vm_group=2,
            _=>r.managed[1].pid=0,
        }
        assert_eq!(production::can_try_cold_write_view(&r,1),enabled && mode!=0);
        BACKEND.with_borrow(|b|assert!(b.calls.is_empty()));
    }
    for invalid in 0..5 {
        let mut r=setup();r.managed.truncate(1);
        match invalid { 0=>r.current_fault_pid=0,1=>r.current_fault_pid=2,
            2=>r.managed[0].exited=true,3=>r.managed[0].vm_group=0,_=>r.managed[0].pid=2 }
        assert!(!production::can_try_cold_write_view(&r,1));
        assert!(!production::can_try_cold_write_view(&r,0));
    }
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
        process.vm_writable_private_shared = false;
    }
    production::mark_shared(&mut runtime, 1);
    assert!(runtime.managed[..7].iter().all(|p| p.vm_private_shared));
    assert!(!runtime.managed[7].vm_private_shared);
    assert!(runtime.managed[..7].iter().all(|p| p.vm_writable_private_shared));
    assert!(!runtime.managed[7].vm_writable_private_shared);
}

#[test]
fn checked_writes_in_another_vm_preserve_both_immutable_origins_even_at_the_same_va() {
    for copied in [false,true] {
        let mut r = setup();
        for p in &mut r.managed { p.vm_writable_private_shared = false; }
        r.managed[7].vm_group = 8;
        r.managed[7].mappings.0 = 99;
        let constructor = if copied { state::ReadonlyMapping::copied } else { state::ReadonlyMapping::new };
        let origin = constructor(1, 1 << 32 | 7, 8192, 4096, 8192, 0).in_vm(1);
        r.readonly_mapping.set(origin);
        for i in 0..2048 {
            // Coincident VAs in different VMs do not identify the same backing.
            production::with_checked_private_write(&r,8,4096 + i % 8192,8,|| Ok(())).unwrap();
            assert!(r.readonly_mapping.get().covers(1,1 << 32 | 7,8192,4096,8192,0));
        }
        calls(&[]);
        // Privileged unchecked writes retain full conservative invalidation.
        production::with_private_write(&r,8,4096,8,|| Ok(())).unwrap();
        assert_eq!(r.readonly_mapping.get().generation,0);
    }
}

#[test]
fn foreign_vm_hint_never_authorizes_shared_vm_overlap_or_unknown_stale_and_failed_checks() {
    let origin = state::ReadonlyMapping::copied(1,1 << 32 | 7,8192,4096,8192,0).in_vm(1);
    for case in 0..7 {
        let mut r=setup();
        r.readonly_mapping.set(origin);
        let pid=if case==5 {99} else {8};
        match case {
            0 => {}, // CLONE_VM peer: same group and overlapping write
            1 => { r.managed[7].mappings.0=2; }, // stale same-VM generation
            2 => { r.managed[7].vm_group=0; },
            3 => { r.managed[7].vm_group=8;r.managed[7].mappings.0=0; },
            4 => { r.managed[7].vm_group=8;r.managed[7].exited=true; },
            5 => {},
            _ => { r.managed[7].vm_group=8; },
        }
        let (base,bytes)=if case==6 {(usize::MAX-1,4)} else {(4096,8)};
        production::invalidate_checked_write_origin(&r,pid,base,bytes);
        assert_eq!(r.readonly_mapping.get().generation,0,"case={case}");
    }
    let mut r=setup();r.readonly_mapping.set(origin);
    production::invalidate_checked_write_origin(&r,8,12288,16);
    assert!(r.readonly_mapping.get().covers(1,1 << 32 | 7,8192,4096,8192,0));
    r.managed[0].mappings.0=2;
    r.managed[7].vm_group=8;r.managed[7].mappings.0=99;
    production::invalidate_checked_write_origin(&r,8,4096,8);
    assert!(!r.readonly_mapping.get().covers(2,1 << 32 | 7,8192,4096,8192,0),"foreign write cannot revive stale owner generation");
}

#[test]
fn checked_disjoint_writes_keep_immutable_provenance_but_overlap_and_stale_vm_clear_it() {
    let mut r = setup();
    for p in &mut r.managed { p.vm_writable_private_shared = false; }
    let origin = state::ReadonlyMapping::new(1, 1 << 32 | 7, 8192, 4096, 8192, 0);
    r.readonly_mapping.set(origin);
    for i in 0..2048 {
        let address = if i % 2 == 0 { 4080 } else { 12288 };
        production::with_checked_private_write(&r, 1, address, 16, || {
            assert!(r.readonly_mapping.get().covers(1, 1 << 32 | 7, 8192, 4096, 8192, 0));
            Ok(())
        }).unwrap();
    }
    calls(&[]);
    for (pid, address, bytes) in [(1,4095,2),(1,4096,1),(1,12287,2),
        (1,usize::MAX-1,4),(99,12288,16),(2,12288,16)] {
        r.managed[1].mappings.0 = 2;
        r.readonly_mapping.set(origin);
        production::invalidate_checked_write_origin(&r, pid, address, bytes);
        assert_eq!(r.readonly_mapping.get().generation, 0);
    }
    r.readonly_mapping.set(origin);
    production::with_private_write(&r, 1, 12288, 16, || Ok(())).unwrap();
    assert_eq!(r.readonly_mapping.get().generation, 0);
}

#[test]
fn readonly_checked_copyout_skips_preparation_and_preserves_origin_invalidation() {
    for cached in [false, true] { for peers in [1, 2] {
        let mut r = setup(); r.managed.truncate(peers); r.cached_owner = cached;
        for p in &mut r.managed { p.vm_private_shared = false; p.vm_writable_private_shared = false; }
        production::mark_readonly_shared(&mut r, 1);
        assert!(r.managed.iter().all(|p| p.vm_private_shared && !p.vm_writable_private_shared));
        r.readonly_mapping.set(state::ReadonlyMapping::new(1, 1 << 32, 4096, 4096, 4096, 0));
        BACKEND.with(|b| b.borrow_mut().fail_detach = true);
        assert_eq!(production::with_checked_private_write(&r, 1, 4096, 16, || {
            assert_eq!(r.readonly_mapping.get().generation, 0); write()
        }), Ok(91));
        calls(&[Call::Write]);
    }}
}

#[test]
fn readonly_markers_keep_unchecked_writes_and_protect_on_the_full_cow_path() {
    for protect in [false, true] { for failed in [false, true] {
        let mut r = setup(); r.managed.truncate(2);
        for p in &mut r.managed { p.vm_private_shared = false; p.vm_writable_private_shared = false; }
        production::mark_readonly_shared(&mut r, 1);
        BACKEND.with(|b| b.borrow_mut().fail_detach = failed);
        let result = if protect { production::with_private_protect(&r, 1, 4096, 4096, write) }
            else { production::with_private_write(&r, 1, 4096, 16, write) };
        assert_eq!(result, if failed { Err(EIO) } else { Ok(91) });
        if failed { calls(&[Call::Query, Call::Suspend(102), Call::Detach, Call::Resume(102)]); }
        else { calls(&[Call::Query, Call::Suspend(102), Call::Detach, Call::Write, Call::Resume(102)]); }
    }}
}

#[test]
fn readonly_admission_never_downgrades_a_writable_fork_marker_and_hot_writes_skip_empty_ipc() {
    let mut r = setup(); r.managed.truncate(1);
    for p in &mut r.managed { p.vm_private_shared = false; p.vm_writable_private_shared = false; }
    production::mark_readonly_shared(&mut r, 1);
    BACKEND.with(|b| b.borrow_mut().shared = false);
    for _ in 0..2048 { production::with_private_write(&r, 1, 4096, 16, write).unwrap(); }
    let old = BACKEND.with(|b| b.borrow().calls.iter().filter(|c| matches!(c, Call::Query | Call::Detach)).count());
    BACKEND.with(|b| b.borrow_mut().calls.clear());
    for _ in 0..2048 { production::with_checked_private_write(&r, 1, 4096, 16, write).unwrap(); }
    let new = BACKEND.with(|b| b.borrow().calls.iter().filter(|c| matches!(c, Call::Query | Call::Detach)).count());
    assert_eq!((old, new), (2048, 0));
    std::println!("READONLY-COPYOUT writes=2048 old-prep-requests={old} new-prep-requests={new}");
    production::mark_shared(&mut r, 1);
    production::mark_readonly_shared(&mut r, 1);
    assert!(r.managed[0].vm_writable_private_shared);
    BACKEND.with(|b| { let mut b = b.borrow_mut(); b.shared = true; b.calls.clear(); });
    production::with_checked_private_write(&r, 1, 4096, 16, write).unwrap();
    calls(&[Call::Detach, Call::Write]);
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
fn large_mapping_mutations_stop_peers_before_the_rpc_and_resume_after_failure() {
    for fail in [false, true] {
        let mut r=setup(); r.managed.truncate(2);
        r.managed[0].mappings.1.push(Mapping {base:0x400000,size:1<<21,prot:3});
        let result=production::with_mapping_mutation(&r,1,0x401000,4096,false,|| {
            write()?; if fail {Err(EIO)} else {Ok(91)}
        });
        assert_eq!(result,if fail {Err(EIO)} else {Ok(91)});
        calls(&[Call::Suspend(102),Call::Write,Call::Resume(102)]);
    }
}

#[test]
fn large_constructors_stop_peers_before_vma_publication_but_small_fresh_chunks_do_not() {
    for (base,bytes,creating,barrier) in [
        (0x400000,1<<21,true,true),
        (0x400000,65536,true,false),
        (0x3ff000,4096,false,false),
        (0x600000,4096,false,false),
        (0x5ff000,4096,false,true),
    ] {
        let mut r=setup(); r.managed.truncate(2);
        r.managed[0].mappings.1.push(Mapping {base:0x400000,size:1<<21,prot:3});
        production::with_mapping_mutation(&r,1,base,bytes,creating,write).unwrap();
        if barrier { calls(&[Call::Suspend(102),Call::Write,Call::Resume(102)]); }
        else {calls(&[Call::Write]);}
    }
}

#[test]
fn huge_protect_combines_shape_and_cow_barriers_without_nested_resume() {
    for marker in [false,true] {
        let mut r=setup(); r.managed.truncate(2);
        r.managed[0].vm_private_shared=marker;
        r.managed[0].mappings.1.push(Mapping {base:0x400000,size:1<<21,prot:3});
        BACKEND.with_borrow_mut(|b|b.shared=false);
        production::with_private_protect(&r,1,0x401000,4096,write).unwrap();
        if marker {calls(&[Call::Query,Call::Suspend(102),Call::Write,Call::Resume(102)]);}
        else {calls(&[Call::Suspend(102),Call::Write,Call::Resume(102)]);}
    }
    let mut r=setup();r.managed.truncate(2);
    r.managed[0].vm_writable_private_shared=false;
    r.managed[0].mappings.1.push(Mapping {base:0x400000,size:1<<21,prot:3});
    production::with_checked_private_write(&r,1,0x401000,4,write).unwrap();
    calls(&[Call::Write]);
}

#[test]
fn huge_mutation_failed_stop_never_issues_rpc_and_resumes_only_acknowledged_stops() {
    let mut r=setup();r.managed.truncate(3);
    BACKEND.with_borrow_mut(|b|b.fail_stop=103);
    assert_eq!(production::with_mapping_mutation(&r,1,0x400000,1<<21,true,write),Err(EIO));
    calls(&[Call::Suspend(102),Call::Suspend(103),Call::Resume(102)]);
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

#[test]
fn cow_fault_rejects_non_write_faults_and_requires_logical_rights_and_alpha_sharing() {
    for code in [0, 1, 4, 5, 15, 23, 39, 71] {
        let r = setup();
        assert!(!production::materialize_private_write_fault(&r, 1, 0x12345, code));
        calls(&[]);
    }
    for rights in [None, Some(0), Some(1), Some(4), Some(5), Some(11)] {
        let mut r = setup(); r.fault_permissions = rights;
        assert!(!production::materialize_private_write_fault(&r, 1, 0x12345, 7));
        calls(&[]);
    }
    let mut r = setup(); r.fault_access = false;
    assert!(!production::materialize_private_write_fault(&r, 1, 0x12345, 7)); calls(&[]);
    let mut r = setup(); r.managed.truncate(1);
    BACKEND.with(|b| b.borrow_mut().shared = false);
    assert!(!production::materialize_private_write_fault(&r, 1, 0x12345, 7)); calls(&[Call::Detach]);
}

#[test]
fn cow_fault_detaches_and_restores_rights_inside_acknowledged_vm_barrier() {
    for failure in 0..3 {
        let mut r = setup();
        r.managed.truncate(3);
        BACKEND.with(|b| { let mut b = b.borrow_mut();
            b.fail_stop = if failure == 1 { 103 } else { 0 };
            b.fail_detach = failure == 2;
        });
        assert_eq!(production::materialize_private_write_fault(&r, 1, 0x12345, 7), failure == 0);
        let mut expected = vec![Call::Suspend(102), Call::Suspend(103)];
        if failure != 1 {
            expected.push(Call::Detach);
            expected.push(Call::Resume(103));
        }
        expected.push(Call::Resume(102));
        calls(&expected);
    }
}

#[test]
fn missing_or_present_write_faults_use_the_sealed_population_mode_under_the_same_barrier() {
    for code in [2,3,6,7] {for shared in [false,true] {
        let mut r=setup();r.managed.truncate(1);
        BACKEND.with_borrow_mut(|b|b.shared=shared);
        assert_eq!(production::materialize_private_write_fault(&r,1,0x12345,code),shared);
        calls(&[Call::Detach]);
        BACKEND.with_borrow(|b|assert_eq!(b.detach_modes,vec![OS_PRIVATE_WRITE_DETACH_POPULATE]));
    }}
}

#[test]
fn cow_fault_success_is_final_without_an_extra_protection_transport() {
    for rights in [3, 7] {
        let mut r = setup(); r.fault_permissions = Some(rights);
        r.managed.truncate(1);
        // The old redundant transport would fail after a successful repair.
        BACKEND.with(|b| b.borrow_mut().fail_protect = true);
        assert!(production::materialize_private_write_fault(&r, 1, 0x12345, 7));
        calls(&[Call::Detach]);
    }
}

#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/snapshot_fault.rs"]
mod snapshot_fault;
thread_local! {static POPULATE: std::cell::Cell<Result<bool,()>> = const {std::cell::Cell::new(Ok(true))};}
pub fn request_process_populate_snapshot(_: Word, page: Word, required: Word) -> Result<bool,()> {
    assert_eq!(page,0x401000);assert!(matches!(required,1|2|4));
    BACKEND.with_borrow_mut(|b| b.calls.push(Call::Write));POPULATE.with(|p|p.get())
}
#[test]
fn snapshot_fault_requires_missing_pte_current_vma_and_alpha_admission_without_stopping_peers() {
    for code in [0,4,2,6,16,20] {
        let mut r=setup();r.fault_permissions=Some(7);
        assert!(snapshot_fault::materialize_snapshot_fault(&r,1,0x401123,code));
        BACKEND.with_borrow(|b|assert_eq!(b.calls,vec![Call::Write]));
    }
    for code in [1,3,5,8,9,32,64,usize::MAX] {
        let r=setup();assert!(!snapshot_fault::materialize_snapshot_fault(&r,1,0x401000,code));
        BACKEND.with_borrow(|b|assert!(b.calls.is_empty()));
    }
    for rights in [None,Some(0),Some(2),Some(8)] {
        let mut r=setup();r.fault_permissions=rights;
        assert!(!snapshot_fault::materialize_snapshot_fault(&r,1,0x401000,4));
        BACKEND.with_borrow(|b|assert!(b.calls.is_empty()));
    }
    for result in [Ok(false),Err(())] {
        let r=setup();POPULATE.with(|p|p.set(result));
        assert!(!snapshot_fault::materialize_snapshot_fault(&r,1,0x401000,4));
    }
    POPULATE.with(|p|p.set(Ok(true)));
}

mod private_memory { pub(super) use super::production::with_mapping_mutation; }

#[test]
fn unmarked_readonly_and_none_backing_still_detach_before_privileged_write_or_upgrade() {
    for rights in [0, 1, 4, 5] {
        for protect in [false, true] {
            let mut r = setup(); r.managed.truncate(1);
            r.managed[0].vm_private_shared = false;
            r.managed[0].vm_writable_private_shared = false;
            r.managed[0].mappings.1.push(Mapping { base: 0x400000, size: 4096, prot: rights });
            let result = if protect {
                production::with_private_protect(&r, 1, 0x400000, 4096, || write())
            } else {
                production::with_private_write(&r, 1, 0x400000, 4096, || write())
            };
            assert_eq!(result, Ok(91));
            calls(&[Call::Detach, Call::Write]);
            BACKEND.with_borrow(|b| assert_eq!(b.detach_modes,
                vec![if protect { OS_PRIVATE_WRITE_DETACH_DEFERRED } else { OS_PRIVATE_WRITE_DETACH }]));
        }
    }
}

#[test]
fn unmarked_readonly_backing_retains_query_stop_ack_and_failed_stop_policy() {
    for fail in [false, true] {
        let mut r = setup();
        for p in &mut r.managed { p.vm_private_shared = false; p.vm_writable_private_shared = false; }
        r.managed[0].mappings.1.push(Mapping { base: 0x400000, size: 4096, prot: 1 });
        if fail { BACKEND.with_borrow_mut(|b| b.fail_stop = 104); }
        assert_eq!(production::with_private_protect(&r, 1, 0x400000, 4096, || write()),
            if fail { Err(EIO) } else { Ok(91) });
        BACKEND.with_borrow(|b| {
            assert_eq!(b.calls[0], Call::Query);
            let detach = b.calls.iter().position(|c| *c == Call::Detach);
            if fail { assert!(detach.is_none()); assert!(!b.calls.contains(&Call::Write)); }
            else {
                let detach = detach.unwrap();
                assert_eq!(&b.calls[1..detach], &(2..=8).map(|p| Call::Suspend(p + 100)).collect::<Vec<_>>());
                assert_eq!(b.calls[detach + 1], Call::Write);
            }
        });
    }
}
