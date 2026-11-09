//! Production procfs lookup, enumeration and Linux text formats; only IPC is mocked.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
thread_local! {
    static PROCESS_QUERIES: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}
type Word = usize;
const ENOENT: i32 = 2;
const EBADF: i32 = 9;
const EIO: i32 = 5;
const EINVAL: i32 = 22;
const ESRCH: i32 = 3;
const ENOTDIR: i32 = 20;
const LINUX_PAGE_SIZE: usize = 4096;
const ALTER_LAUNCH_MAX_ARGS: usize = 64;
const LINUX_EXEC_STRING_MAX: usize = 4096;
const LINUX_SIGCHLD: usize = 17;
const LINUX_FD_MAX: usize = 64;
mod arch {
    pub const PROC_VERSION: &[u8] = b"test";
    pub const CPU_INFO: &[u8] = b"test";
}
mod posix {
    pub const POSIX_FILE_TYPE_DIRECTORY: usize = 2;
    pub const POSIX_FILE_TYPE_REGULAR: usize = 1;
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/virtual_fs.rs"]
mod virtual_fs;
use virtual_fs::VirtualNode;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/procfs.rs"]
mod procfs;
mod state {
    pub use super::Process as ManagedProcess;
    pub mod pty {
        pub const TERMINAL_TAG: usize = 1 << 63;
    }
}
#[derive(Default)]
struct Root {
    leader: usize,
}
#[derive(Default)]
struct Job {
    controlling_terminal: usize,
    group: usize,
    session: usize,
}
#[derive(Default)]
struct Mapping {
    base: usize,
    size: usize,
}
#[derive(Default)]
pub struct Process {
    pid: usize,
    tgid: usize,
    parent_pid: usize,
    rootfs: Root,
    exited: bool,
    stopped: bool,
    signal_waiting: bool,
    terminal_read_waiting: bool,
    device_read_waiting: bool,
    network_waiting: bool,
    sleep_waiting: bool,
    readiness_wait: Option<()>,
    image_name: [u8; 32],
    image_name_len: usize,
    comm: [u8; 16],
    mappings: Vec<Mapping>,
    job: Job,
    started_at: usize,
    exit_status: usize,
    argument_count: usize,
    argument_vector: usize,
}
impl Process {
    fn signal_stopped(&self) -> bool {
        self.stopped
    }
    fn new(pid: usize, group: usize, leader: usize) -> Self {
        let mut process = Self {
            pid,
            tgid: group,
            rootfs: Root { leader },
            ..Self::default()
        };
        process.image_name[..8].copy_from_slice(b"foo) bar");
        process.image_name_len = 8;
        process.comm[..8].copy_from_slice(b"foo) bar");
        process
    }
}
struct Task {
    wait: Option<()>,
}
struct Tasks;
impl Tasks {
    fn get(&self, _: usize) -> Option<&Task> {
        None
    }
}
struct Pty {
    number: u32,
}
struct Ptys;
impl Ptys {
    fn get(&self, _: usize) -> Result<&Pty, ()> {
        Err(())
    }
}
struct Terminal {
    id: usize,
    foreground: usize,
}
struct Runtime {
    managed: Vec<Process>,
    tasks: Tasks,
    ptys: Ptys,
    terminal_controls: Vec<Terminal>,
    monotonic_ticks: usize,
    monotonic_tick_hz: usize,
    posix_shm: usize,
    posix_shm_size: usize,
    _shm: Box<[u8; 4096]>,
}
impl Runtime {
    fn new() -> Self {
        let mut shm = Box::new([0u8; 4096]);
        Self {
            managed: vec![
                Process::new(9, 9, 9),
                Process::new(3, 3, 9),
                Process::new(8, 3, 9),
                Process::new(7, 7, 7),
            ],
            tasks: Tasks,
            ptys: Ptys,
            terminal_controls: vec![],
            monotonic_ticks: 1245,
            monotonic_tick_hz: 100,
            posix_shm: shm.as_mut_ptr() as usize,
            posix_shm_size: 4096,
            _shm: shm,
        }
    }
    fn managed_process(&self, pid: usize) -> Option<&Process> {
        PROCESS_QUERIES.with(|count| count.set(count.get() + 1));
        self.managed.iter().find(|process| process.pid == pid)
    }
    fn thread_group(&self, pid: usize) -> usize {
        self.managed_process(pid)
            .map_or(pid, |process| process.tgid)
    }
}
mod clocks {
    pub fn refresh_clock(_: &mut super::Runtime) -> Result<(), i32> {
        Ok(())
    }
}
thread_local! { static MEMORY: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) }; }
fn register(bytes: &[u8]) {
    MEMORY.with(|memory| {
        memory
            .borrow_mut()
            .push((bytes.as_ptr() as usize, bytes.len()))
    });
}
fn request_process_memory_read_checked(
    _: usize,
    address: usize,
    out: usize,
    bytes: usize,
) -> Result<(), ()> {
    let valid = MEMORY.with(|memory| {
        memory
            .borrow()
            .iter()
            .any(|&(base, len)| address >= base && address + bytes <= base + len)
    });
    if !valid {
        return Err(());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(address as *const u8, out as *mut u8, bytes);
    }
    Ok(())
}
fn entry(runtime: &Runtime, caller: usize, path: &[u8]) -> usize {
    procfs::lookup(runtime, caller, path).unwrap()
}
fn read(runtime: &mut Runtime, value: usize) -> String {
    let mut bytes = [0u8; 2048];
    let count = procfs::read(runtime, 9, value, 0, &mut bytes).unwrap();
    String::from_utf8(bytes[..count].to_vec()).unwrap()
}

#[test]
fn pid_lookup_is_context_local_and_task_membership_is_checked() {
    let runtime = Runtime::new();
    assert_eq!(
        entry(&runtime, 8, b"/proc/self/stat"),
        entry(&runtime, 9, b"/proc/3/stat")
    );
    assert!(procfs::lookup(&runtime, 9, b"/proc/7/stat").is_none());
    assert!(procfs::lookup(&runtime, 9, b"/proc/3/task/8/status").is_some());
    assert!(procfs::lookup(&runtime, 9, b"/proc/3/task/9/status").is_none());
    for path in [
        b"/proc/03/stat".as_slice(),
        b"/proc/0/stat",
        b"/proc/+3/stat",
        b"/proc/4294967296/stat",
        b"/proc/9999999999999999999999",
        b"/proc/3/stat/extra",
    ] {
        assert!(procfs::lookup(&runtime, 9, path).is_none());
    }
    assert_eq!(
        procfs::stat(&runtime, 7, entry(&runtime, 9, b"/proc/3/stat")),
        Err(ENOENT)
    );
}

#[test]
fn self_visibility_avoids_reloading_the_same_pid_but_checks_unknown_identity() {
    let r = Runtime::new();
    let mut counts = Vec::new();
    for (caller, target, visible) in [(9, 9, true), (9, 3, true), (9, 7, false), (99, 99, false), (9, 0, true)] {
        PROCESS_QUERIES.with(|count| count.set(0));
        let result = procfs::validate(&r, caller, (target << 32) | VirtualNode::ProcStat.id());
        counts.push(PROCESS_QUERIES.with(|count| count.get()));
        assert_eq!(result, if visible { Ok(()) } else { Err(ENOENT) });
    }
    println!("proc visibility process queries self/same-scope/other-scope/unknown-self/global: {counts:?}");
    assert_eq!(counts, [1, 2, 2, 1, 0]);
}

#[test]
fn pure_proc_shm_output_matches_staged_read_for_limits_offsets_and_scope() {
    let mut cases = 0;
    for node in [VirtualNode::ProcStat, VirtualNode::ProcStatus, VirtualNode::ProcComm, VirtualNode::ProcStatm] {
        for caller in [9, 3, 7, 99] {
            for subject in [9, 3, 7, 99] {
                for offset in [0, 1, 8, 128, 2048, usize::MAX] {
                    for len in [0, 1, 16, 127, 4096, 4097, usize::MAX] {
                        let mut r = Runtime::new();
                        r.posix_shm_size = if subject == 3 { 128 } else { 4096 };
                        // External SHM ownership stays outside Runtime's borrowed fields.
                        let mut shm = [0xa5u8; 4096];
                        r.posix_shm = shm.as_mut_ptr() as Word;
                        let value = (subject << 32) | node.id();
                        let limit = len.min(4096).min(r.posix_shm_size);
                        let mut staged = [0u8; 4096];
                        let expected = procfs::read(&mut r, caller, value, offset, &mut staged[..limit]);
                        let mut expected_shm = shm;
                        if let Ok(amount) = expected { expected_shm[..amount].copy_from_slice(&staged[..amount]); }
                        assert_eq!(procfs::try_read_current_shm(&mut r, caller, value, offset, len), Some(expected));
                        assert_eq!(shm, expected_shm);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("pure proc direct SHM staged-reference cases: {cases}");
}

#[test]
fn proc_shm_route_rejects_scratch_using_or_other_nodes_without_side_effects() {
    for node in [VirtualNode::ProcCmdline, VirtualNode::ProcUptime, VirtualNode::ProcSelfExe,
        VirtualNode::ProcMemInfo, VirtualNode::ProcNetArp, VirtualNode::DevZero] {
        let mut r = Runtime::new();
        let mut shm = [0xa5u8; 4096]; r.posix_shm = shm.as_mut_ptr() as Word;
        PROCESS_QUERIES.with(|count| count.set(0));
        assert_eq!(procfs::try_read_current_shm(&mut r, 9, (9 << 32) | node.id(), 0, 4096), None);
        assert_eq!(PROCESS_QUERIES.with(|count| count.get()), 0);
        assert_eq!(shm, [0xa5; 4096]);
    }
}

#[test]
fn self_visibility_matches_two_lookup_reference_after_exit_scope_and_pid_reuse() {
    let mut cases = 0;
    for scenario in 0..4 {
        let mut r = Runtime::new();
        match scenario {
            1 => r.managed[0].exited = true,
            2 => r.managed[0].rootfs.leader = 7,
            3 => {
                r.managed[0].pid = 0;
                r.managed.push(Process::new(9, 9, 7));
                r.managed.reverse();
            }
            _ => {},
        }
        for caller in [0, 3, 7, 8, 9, 99] {
            for target in [0, 3, 7, 8, 9, 99] {
                let old = target == 0 || r.managed_process(caller).zip(r.managed_process(target))
                    .is_some_and(|(a,b)| a.rootfs.leader == b.rootfs.leader);
                assert_eq!(procfs::validate(&r, caller, (target << 32) | VirtualNode::ProcStat.id()),
                    if old { Ok(()) } else { Err(ENOENT) });
                cases += 1;
            }
        }
    }
    println!("proc self visibility old-reference cases: {cases}");
}

#[test]
fn enumeration_uses_stable_pid_cookies_not_reusable_process_table_indices() {
    let mut runtime = Runtime::new();
    let mut name = [0; 20];
    let root = VirtualNode::Proc.id();
    let first = procfs::directory_entry(&runtime, 9, root, 6, &mut name)
        .unwrap()
        .unwrap();
    assert_eq!(first.name, b"3");
    let cookie = first.next;
    runtime.managed.retain(|process| process.pid != 3);
    runtime.managed.push(Process::new(2, 2, 9));
    let next = procfs::directory_entry(&runtime, 9, root, cookie, &mut name)
        .unwrap()
        .unwrap();
    assert_eq!(next.name, b"9"); // Never repeat a PID or expose thread 8 / context 7.
    let tasks = entry(&runtime, 9, b"/proc/9/task");
    let task = procfs::directory_entry(&runtime, 9, tasks, 0, &mut name)
        .unwrap()
        .unwrap();
    assert_eq!(task.name, b"9");
}

#[test]
fn stat_has_52_linux_fields_correct_units_and_signed_foreground() {
    let mut runtime = Runtime::new();
    runtime.managed[0].parent_pid = 7; // Parent outside the execution context is hidden.
    runtime.managed[0].job.group = 9;
    runtime.managed[0].job.session = 9;
    runtime.managed[0].started_at = 123;
    runtime.managed[0].mappings.push(Mapping {
        base: 4096,
        size: 8192,
    });
    let value = entry(&runtime, 9, b"/proc/9/stat");
    let text = read(&mut runtime, value);
    assert!(text.starts_with("9 (foo) bar) R 0 9 9 0 -1 "));
    let fields: Vec<_> = text
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    assert_eq!(fields.len(), 50);
    assert_eq!(fields[17], "1"); // Linux field 20: threads.
    assert_eq!(fields[19], "123"); // field 22, USER_HZ.
    assert_eq!(fields[20], "8192"); // field 23, bytes (not pages).
    assert_eq!(fields[21], "0"); // No invented RSS or CPU accounting.
    assert_eq!(fields[35], "17"); // field 38, exit_signal.
    let stat = procfs::stat(&runtime, 9, value).unwrap();
    assert_eq!(stat.0 >> 32, 0); // Inode must not be misinterpreted as a mode.
    assert_eq!(stat.1, 0);
    let status = entry(&runtime, 9, b"/proc/9/status");
    assert!(read(&mut runtime, status).contains("VmSize:\t8 kB\n"));
    for (exited, stopped, waiting, expected) in [
        (false, false, true, "S"),
        (false, true, true, "T"),
        (true, true, true, "Z"),
    ] {
        runtime.managed[0].exited = exited;
        runtime.managed[0].stopped = stopped;
        runtime.managed[0].sleep_waiting = waiting;
        assert!(read(&mut runtime, value).contains(&format!(") {expected} ")));
    }
}

#[test]
fn stat_zero_fields_skip_generic_numeric_formatting() {
    let mut r = Runtime::new();
    let value = entry(&r, 9, b"/proc/9/stat");
    procfs::take_stat_numeric_formats();
    let text = read(&mut r, value);
    let formats = procfs::take_stat_numeric_formats();
    let fields: Vec<_> = text[text.rfind(')').unwrap()+2..].split_whitespace().collect();
    assert_eq!(fields.len(), 50); // state plus numeric fields4..52
    let numeric: Vec<isize> = fields[1..].iter().map(|v| v.parse().unwrap()).collect();
    assert_eq!(numeric.iter().filter(|v| **v != 0).count(), 4);
    println!("proc stat generic numeric format calls for49 fields: {formats}");
    assert_eq!(formats, 4);
}

#[test]
fn stat_numeric_field_bytes_match_decimal_reference_and_keep_guards() {
    let mut cases = 0;
    for value in (-1024isize..=1024).chain([isize::MIN, isize::MAX, -1000000000, 1000000000]) {
        let expected = format!(" {value}");
        for alignment in 0..8 {
            let mut out = [0xa5u8; 64];
            let count = procfs::stat_field_for_test(value, &mut out[alignment..48]).unwrap();
            assert_eq!(count, expected.len());
            assert_eq!(&out[alignment..alignment+count], expected.as_bytes());
            assert!(out[..alignment].iter().chain(out[alignment+count..].iter()).all(|b| *b == 0xa5));
            cases += 1;
        }
    }
    println!("proc stat numeric field decimal reference cases: {cases}");
}

#[test]
fn task_directories_list_all_and_only_the_groups_threads() {
    let mut runtime = Runtime::new();
    let tasks = entry(&runtime, 9, b"/proc/3/task");
    let mut name = [0; 20];
    let first = procfs::directory_entry(&runtime, 9, tasks, 0, &mut name)
        .unwrap()
        .unwrap();
    assert_eq!(first.name, b"3");
    let next = first.next;
    let second = procfs::directory_entry(&runtime, 9, tasks, next, &mut name)
        .unwrap()
        .unwrap();
    assert_eq!(second.name, b"8");
    let next = second.next;
    assert!(procfs::directory_entry(&runtime, 9, tasks, next, &mut name)
        .unwrap()
        .is_none());
    let status = entry(&runtime, 9, b"/proc/3/task/8/status");
    let text = read(&mut runtime, status);
    assert!(text.contains("Tgid:\t3\nPid:\t8\n"));
    assert!(text.contains("Threads:\t2\n"));
}

#[test]
fn files_support_short_reads_eof_and_removed_subjects() {
    let mut runtime = Runtime::new();
    let value = entry(&runtime, 9, b"/proc/9/comm");
    let mut byte = [0];
    let mut text = vec![];
    for offset in 0..20 {
        let count = procfs::read(&mut runtime, 9, value, offset, &mut byte).unwrap();
        if count == 0 {
            break;
        }
        text.push(byte[0]);
    }
    assert_eq!(text, b"foo) bar\n");
    assert_eq!(
        procfs::read(&mut runtime, 9, value, usize::MAX, &mut byte),
        Ok(0)
    );
    runtime.managed.retain(|process| process.pid != 9);
    assert_eq!(
        procfs::read(&mut runtime, 3, value, 0, &mut byte),
        Err(ENOENT)
    );
}

#[test]
fn comm_reads_do_not_query_parent_or_compute_other_proc_fields() {
    let mut r = Runtime::new();
    let value = entry(&r, 9, b"/proc/9/comm");
    r.managed[0].parent_pid = 3;
    r.managed[0].mappings = (0..1024).map(|i| Mapping { base: 0x10000 + i * 4096, size: 4096 }).collect();
    PROCESS_QUERIES.with(|count| count.set(0));
    let mut out = [0u8; 32];
    assert_eq!(procfs::read(&mut r, 9, value, 0, &mut out), Ok(9));
    let count = PROCESS_QUERIES.with(|count| count.get());
    assert_eq!(&out[..9], b"foo) bar\n");
    println!("proc comm managed-process queries: {count}");
    assert_eq!(count, 2); // One identity lookup for each self visibility check.
}

#[test]
fn task_directory_reuses_one_group_query_per_cookie() {
    let mut r = Runtime::new();
    r.managed = (9..265).rev().map(|pid| Process::new(pid, 9, 9)).collect();
    let value = entry(&r, 9, b"/proc/9/task");
    PROCESS_QUERIES.with(|count| count.set(0));
    let mut cookie = 0;
    let mut pid = 9;
    loop {
        let mut name = [0u8; 20];
        let Some(found) = procfs::directory_entry(&r, 9, value, cookie, &mut name).unwrap() else { break; };
        assert_eq!(found.name, pid.to_string().as_bytes());
        assert_eq!(found.resource >> 32, pid);
        assert_eq!(found.next, pid + 1);
        cookie = found.next; pid += 1;
    }
    assert_eq!(pid, 265);
    let count = PROCESS_QUERIES.with(|count| count.get());
    println!("proc task256 full enumeration managed-process queries: {count}");
    assert_eq!(count, 770);
}

#[test]
fn task_directory_matches_old_group_predicate_across_scope_cookie_and_buffer_cases() {
    let mut r = Runtime::new();
    for pid in 20..32 {
        let mut p = Process::new(pid, if pid % 3 == 0 { 9 } else { 3 }, if pid % 4 == 0 { 7 } else { 9 });
        p.exited = pid % 2 == 0;
        r.managed.push(p);
    }
    r.managed.reverse();
    let mut cases = 0;
    for caller in [9, 3, 7, 99] {
        for subject in [9, 3, 7, 99, 0] {
            for cookie in [0, 9, 10, 23, usize::MAX] {
                for capacity in [0, 1, 20] {
                    let current = r.managed_process(caller);
                    let target = r.managed_process(subject);
                    let expected = if subject != 0 && current.zip(target).is_none_or(|(a,b)| a.rootfs.leader != b.rootfs.leader) {
                        Err(ENOENT)
                    } else if let Some(current) = current {
                        let found = r.managed.iter().filter(|p| p.pid != 0 && p.pid >= cookie
                            && p.rootfs.leader == current.rootfs.leader && p.tgid == r.thread_group(subject)).min_by_key(|p| p.pid);
                        if let Some(p) = found {
                            let name = p.pid.to_string().into_bytes();
                            if name.len() > capacity { Err(EIO) }
                            else { Ok(Some((name, (p.pid << 32) | VirtualNode::ProcProcess.id(), p.pid + 1))) }
                        } else { Ok(None) }
                    } else { Err(ESRCH) };
                    let mut name = [0xa5; 20];
                    let actual = procfs::directory_entry(&r, caller, (subject << 32) | VirtualNode::ProcTask.id(), cookie, &mut name[..capacity])
                        .map(|entry| entry.map(|e| (e.name.to_vec(), e.resource, e.next)));
                    assert_eq!(actual, expected, "caller={caller} subject={subject} cookie={cookie} capacity={capacity}");
                    cases += 1;
                }
            }
        }
    }
    println!("proc task old-predicate reference cases: {cases}");
}

#[test]
fn comm_direct_bytes_match_format_reference_for_all_small_windows() {
    let names: [[u8; 16]; 6] = [
        [0; 16], *b"123456789abcdefg", *b"abc\0xxxxxxxxxxxx",
        *b"bad\xffxxxxxxxxxxxx", *b"a\n()b\0xxxxxxxxxx", *b"\xc3\xa9\0xxxxxxxxxxxxx",
    ];
    let mut cases = 0;
    for comm in names {
        for exited in [false, true] {
            let mut r = Runtime::new();
            let value = entry(&r, 9, b"/proc/9/comm");
            r.managed[0].comm = comm; r.managed[0].exited = exited;
            let name = &comm[..comm.iter().position(|b| *b == 0).unwrap_or(15)];
            let expected = format!("{}\n", core::str::from_utf8(name).unwrap_or("?"));
            for offset in (0..=18).chain([usize::MAX]) {
                for length in 0..=18 {
                    let mut out = [0xa5; 24];
                    let bytes = &expected.as_bytes()[offset.min(expected.len())..];
                    let count = length.min(bytes.len());
                    assert_eq!(procfs::read(&mut r, 9, value, offset, &mut out[..length]), Ok(count));
                    assert_eq!(&out[..count], &bytes[..count]);
                    assert!(out[count..].iter().all(|b| *b == 0xa5));
                    cases += 1;
                }
            }
        }
    }
    println!("proc comm reference windows: {cases}");
}

#[test]
fn cmdline_preserves_nul_separators_live_arguments_and_offsets() {
    let mut runtime = Runtime::new();
    // Permit the same page-bounded copyin used by production.
    let mut a = [0u8; 4096];
    let mut b = [0u8; 4096];
    a[..3].copy_from_slice(b"sh\0");
    b[..3].copy_from_slice(b"-i\0");
    let pointers = [a.as_ptr() as usize, b.as_ptr() as usize];
    register(&a);
    register(&b);
    register(unsafe { std::slice::from_raw_parts(pointers.as_ptr() as *const u8, 16) });
    runtime.managed[0].argument_vector = pointers.as_ptr() as usize;
    runtime.managed[0].argument_count = 2;
    let value = entry(&runtime, 9, b"/proc/self/cmdline");
    let mut text = [0; 6];
    assert_eq!(procfs::read(&mut runtime, 9, value, 0, &mut text), Ok(6));
    assert_eq!(&text, b"sh\0-i\0");
    b[1] = b'c';
    assert_eq!(b[1], b'c');
    assert_eq!(procfs::read(&mut runtime, 9, value, 4, &mut text), Ok(2));
    assert_eq!(&text[..2], b"c\0");
    runtime.managed[0].exited = true;
    assert_eq!(procfs::read(&mut runtime, 9, value, 0, &mut text), Ok(0));
}

#[test]
fn proc_dirfd_paths_and_uptime_have_linux_formats() {
    let mut runtime = Runtime::new();
    for (path, expected) in [
        (b"/proc/self".as_slice(), b"/proc/9".as_slice()),
        (b"/proc/3/task", b"/proc/3/task"),
    ] {
        let value = entry(&runtime, 9, path);
        let mut out = [0; 64];
        let count = procfs::directory_path(value, &mut out).unwrap();
        assert_eq!(&out[..count], expected);
    }
    assert_eq!(
        read(&mut runtime, VirtualNode::ProcUptime.id()),
        "12.45 0.00\n"
    );
}
