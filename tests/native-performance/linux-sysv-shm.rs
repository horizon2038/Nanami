//! Production System V IPC namespace/lifetime decisions; VM IPC is mocked.
#![allow(dead_code)]
extern crate alloc;
thread_local! {
    static ATTACHMENT_ALLOCATIONS: core::cell::Cell<Option<usize>> = const { core::cell::Cell::new(None) };
}
struct CountingAllocator;
fn note_allocation() {
    let _ = ATTACHMENT_ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() { count.set(Some(value + 1)); }
    });
}
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        note_allocation();
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        note_allocation();
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
type Word = usize;
const EINVAL: i32 = 22;
const EFAULT: i32 = 14;
const ESRCH: i32 = 3;
const EIO: i32 = 5;
const ENOENT: i32 = 2;
const EEXIST: i32 = 17;
const EACCES: i32 = 13;
const EOPNOTSUPP: i32 = 95;
const LINUX_PAGE_SIZE: Word = 4096;
const LINUX_FD_MAX: usize = 64;
const LINUX_PROT_READ: Word = 1;
const LINUX_PROT_WRITE: Word = 2;
const LINUX_PROT_EXEC: Word = 4;
const LINUX_MAP_FIXED: Word = 16;
const LINUX_O_RDONLY: Word = 0;
const LINUX_O_RDWR: Word = 2;
const LINUX_VIRTUAL_RESERVATION_BASE: Word = 0x100000000;
const LINUX_VIRTUAL_RESERVATION_LIMIT: Word = 0x400000000000;
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinuxFileKind {
    Empty,
    MemoryFile,
    Terminal,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    resource: Word,
    flags: Word,
}
impl LinuxFile {
    const EMPTY: Self = Self {
        kind: LinuxFileKind::Empty,
        posix_fd: 0,
        resource: 0,
        flags: 0,
    };
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/poll_objects.rs"]
mod poll_objects;
mod state {
    pub(crate) use super::poll_objects;
}
use poll_objects::{MemoryMapping, Object};
struct Root {
    leader: Word,
}
struct Process {
    pid: Word,
    vm: Word,
    rootfs: Root,
    exited: bool,
}
struct Runtime {
    poll_objects: poll_objects::Objects,
    managed: Vec<Process>,
    posix_shm: Word,
    scratch: Box<[u8; 4096]>,
    next_address: Word,
    vm_queries: core::cell::Cell<usize>,
    unmaps: Vec<(Word, Word)>,
    fail_unmap: Option<usize>,
    clocks: usize,
    clock_error: bool,
}
impl Runtime {
    fn new() -> Self {
        let mut scratch = Box::new([0; 4096]);
        Self {
            poll_objects: poll_objects::Objects::new(),
            managed: vec![
                Process {
                    pid: 1,
                    vm: 1,
                    rootfs: Root { leader: 1 },
                    exited: false,
                },
                Process {
                    pid: 2,
                    vm: 2,
                    rootfs: Root { leader: 1 },
                    exited: false,
                },
                Process {
                    pid: 3,
                    vm: 3,
                    rootfs: Root { leader: 3 },
                    exited: false,
                },
            ],
            posix_shm: scratch.as_mut_ptr() as Word,
            scratch,
            next_address: LINUX_VIRTUAL_RESERVATION_BASE,
            vm_queries: core::cell::Cell::new(0),
            unmaps: Vec::new(),
            fail_unmap: None,
            clocks: 0,
            clock_error: false,
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn vm_group(&self, pid: Word) -> Word {
        self.vm_queries.set(self.vm_queries.get() + 1);
        self.managed_process(pid).map_or(pid, |p| p.vm)
    }
    fn next_mapping(&self, _: Word, _: Word, _: Word) -> Option<()> {
        None
    }
}
fn align_up_word(value: Word, alignment: Word) -> Word {
    (value + alignment - 1) & !(alignment - 1)
}
fn read_target_memory(runtime: &Runtime, _: Word, source: Word, bytes: Word) -> Result<(), i32> {
    if source < 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(source as *const u8, runtime.posix_shm as *mut u8, bytes);
    }
    Ok(())
}
fn copyout_from(
    _: &Runtime,
    _: Word,
    destination: Word,
    source: Word,
    bytes: Word,
) -> Result<(), i32> {
    if destination < 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes);
    }
    Ok(())
}
fn read_shm_u32(runtime: &Runtime, offset: usize) -> u32 {
    u32::from_le_bytes(runtime.scratch[offset..offset + 4].try_into().unwrap())
}
mod realtime {
    pub fn realtime(runtime: &mut super::Runtime) -> Result<(usize, usize), i32> {
        runtime.clocks += 1;
        if runtime.clock_error { return Err(super::EIO); }
        Ok((1234, 0))
    }
}
mod memfd {
    use super::*;
    pub fn collect(runtime: &mut Runtime, id: Word) {
        if matches!(runtime.poll_objects.get(id), Some(Object::Memory(memory)) if memory.maps.is_empty() && memory.sysv.as_ref().is_some_and(|s| s.removed))
        {
            runtime.poll_objects.remove(id);
        }
    }
    pub fn map(
        runtime: &mut Runtime,
        pid: Word,
        _: Word,
        bytes: Word,
        prot: Word,
        _: Word,
        file: LinuxFile,
        _: Word,
    ) -> Result<Word, i32> {
        let base = runtime.next_address;
        runtime.next_address += align_up_word(bytes, 4096);
        let Some(Object::Memory(memory)) = runtime.poll_objects.get_mut(file.resource) else {
            return Err(EINVAL);
        };
        memory.maps.push(MemoryMapping {
            pid,
            base,
            bytes: align_up_word(bytes, 4096),
            offset: 0,
            prot,
            shared: true,
            attachment: Some(base),
            readonly_attachment: file.flags == 0,
        });
        Ok(base)
    }
}
fn sys_munmap(runtime: &mut Runtime, pid: Word, base: Word, bytes: Word) -> Result<Word, i32> {
    runtime.unmaps.push((base, bytes));
    if runtime.fail_unmap == Some(runtime.unmaps.len()) { return Err(EIO); }
    let ids: Vec<_> = runtime.poll_objects.iter().map(|(id, _)| id).collect();
    for id in ids {
        if let Some(Object::Memory(memory)) = runtime.poll_objects.get_mut(id) {
            memory
                .maps
                .retain(|m| m.pid != pid || m.base < base || m.base >= base + bytes);
        }
        memfd::collect(runtime, id);
    }
    Ok(0)
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/sysv_shm.rs"]
mod sysv_shm;

fn detach_mapping(pid: Word, base: Word, attachment: Option<Word>) -> MemoryMapping {
    MemoryMapping { pid, base, bytes: 4096, offset: base % 8192, prot: 1,
        shared: true, attachment, readonly_attachment: false }
}

#[test]
fn attachment_metadata_updates_do_not_allocate_id_snapshots() {
    for leaving in [false, true] {
        let mut counts = Vec::new();
        for objects in [0, 1, 8, 128, 256] {
            let mut r = Runtime::new();
            let mut ids = Vec::new();
            for i in 0..objects {
                let id = sysv_shm::get(&mut r, 1, 0, 4096, 0o600).unwrap();
                r.poll_objects.memory_mut(id).unwrap().maps.push(detach_mapping(1, 0x10000 + i * 8192, Some(0x10000)));
                ids.push(id);
            }
            r.clocks = 0;
            ATTACHMENT_ALLOCATIONS.with(|count| count.set(Some(0)));
            if leaving { sysv_shm::leaving_vm(&mut r, 1); } else { sysv_shm::inherited(&mut r, 1); }
            counts.push(ATTACHMENT_ALLOCATIONS.with(|count| count.replace(None).unwrap()));
            assert_eq!(r.clocks, usize::from(objects != 0));
            for id in ids {
                let info = r.poll_objects.memory_mut(id).unwrap().sysv.as_ref().unwrap();
                assert_eq!((info.attached, info.detached, info.last_pid), if leaving { (0, 1234, 1) } else { (1234, 0, 1) });
            }
        }
        println!("SysV attachment metadata leaving={leaving} allocations0/1/8/128/256: {counts:?}");
        assert_eq!(counts, [0; 5]);
    }
}

#[test]
fn attachment_metadata_stream_matches_snapshot_selection_and_clock_rules() {
    for leaving in [false, true] {
        for shared in [false, true] {
            for clock_error in [false, true] {
                for pid in [1, 2, 99] {
                    let mut r = Runtime::new();
                    if shared { r.managed[1].vm = 1; }
                    let mut ids = Vec::new();
                    for i in 0..12 {
                        let id = sysv_shm::get(&mut r, 1, 0, 4096, 0o600).unwrap();
                        let memory = r.poll_objects.memory_mut(id).unwrap();
                        memory.maps.push(detach_mapping([1, 2, 99, 3][i % 4], 0x10000 + i * 8192, Some(0x10000)));
                        if i % 3 == 0 { memory.sysv = None; }
                        ids.push(id);
                    }
                    // Snapshot the historical target predicate before any update.
                    let expected: Vec<_> = r.poll_objects.iter().filter_map(|(id, object)| match object {
                        Object::Memory(m) if m.sysv.is_some() && m.maps.iter().any(|m| m.pid == pid) => Some(id), _ => None,
                    }).collect();
                    let has_peer = r.managed.iter().any(|p| p.pid != 0 && p.pid != pid && !p.exited
                        && r.vm_group(p.pid) == r.vm_group(pid));
                    r.clocks = 0; r.clock_error = clock_error;
                    if leaving { sysv_shm::leaving_vm(&mut r, pid); } else { sysv_shm::inherited(&mut r, pid); }
                    let changed = !leaving || !has_peer;
                    assert_eq!(r.clocks, usize::from(changed && !expected.is_empty()));
                    for id in ids {
                        let memory = r.poll_objects.memory_mut(id).unwrap();
                        let Some(info) = &memory.sysv else { continue; };
                        let updated = changed && expected.contains(&id);
                        let time = if clock_error { 0 } else { 1234 };
                        assert_eq!((info.attached, info.detached, info.last_pid), if updated {
                            if leaving { (0, time, pid) } else { (time, 0, pid) }
                        } else { (0, 0, 0) });
                        assert_eq!(memory.maps.len(), 1);
                    }
                }
            }
        }
    }
}

#[test]
fn detach_filters_attachment_before_vm_queries_and_shares_target_group() {
    let mut counts = Vec::new();
    for mode in 0..3 {
        let mut r = Runtime::new();
        r.managed[1].vm = 1;
        let id = sysv_shm::get(&mut r, 1, 0, 4096, 0o600).unwrap();
        let memory = r.poll_objects.memory_mut(id).unwrap();
        memory.maps = (0..1024).map(|i| detach_mapping(if mode == 2 { 2 } else { 1 },
            0x10000 + i * 8192, Some(if mode == 2 { 0x123000 } else { i * 4096 }))).collect();
        let address = if mode == 0 { Word::MAX } else if mode == 1 { 1023 * 4096 } else { 0x123000 };
        let expected = if mode == 2 { memory.maps.iter().map(|m| (m.base, m.bytes)).collect::<Vec<_>>() }
            else if mode == 1 { vec![(memory.maps[1023].base, 4096)] } else { vec![] };
        r.vm_queries.set(0);
        assert_eq!(sysv_shm::detach(&mut r, 1, address), if mode == 0 { Err(EINVAL) } else { Ok(0) });
        counts.push(r.vm_queries.get());
        assert_eq!(r.unmaps, expected);
    }
    println!("SysV detach VM group queries miss/late-own/all-shared1024: {counts:?}");
    assert_eq!(counts, [0, 0, 1026]);
}

#[test]
fn detach_matches_group_first_reference_and_preserves_unmap_error_order() {
    for shared in [false, true] {
        for owner in [1, 2, 3, 99] {
            for address in [0, 0x10000, 0x20000, Word::MAX] {
                for fail in [None, Some(1), Some(2)] {
                    let mut r = Runtime::new();
                    if shared { r.managed[1].vm = 1; }
                    let id = sysv_shm::get(&mut r, 1, 0, 4096, 0o600).unwrap();
                    let maps = [detach_mapping(owner, 0x30000, Some(0x10000)),
                        detach_mapping(1, 0x40000, Some(0x20000)),
                        detach_mapping(owner, 0x50000, Some(0x10000)),
                        detach_mapping(owner, 0x60000, None)];
                    r.poll_objects.memory_mut(id).unwrap().maps.extend(maps);
                    // Historical group-first predicate, evaluated before mutation.
                    let ranges: Vec<_> = maps.iter().filter(|m| r.vm_group(m.pid) == r.vm_group(1)
                        && m.attachment == Some(address)).map(|m| (m.base, m.bytes)).collect();
                    r.fail_unmap = fail;
                    let expected = if ranges.is_empty() { Err(EINVAL) }
                        else if fail.is_some_and(|n| n <= ranges.len()) { Err(EIO) } else { Ok(0) };
                    let clocks = r.clocks;
                    assert_eq!(sysv_shm::detach(&mut r, 1, address), expected);
                    assert_eq!(r.unmaps, ranges[..fail.unwrap_or(ranges.len()).min(ranges.len())]);
                    assert_eq!(r.clocks - clocks, usize::from(!ranges.is_empty()));
                    let info = r.poll_objects.memory_mut(id).unwrap().sysv.as_ref().unwrap();
                    assert_eq!((info.detached, info.last_pid), if ranges.is_empty() { (0, 0) } else { (1234, 1) });
                }
            }
        }
    }
}

#[test]
fn keys_and_ids_cannot_cross_execution_contexts() {
    let mut runtime = Runtime::new();
    let a = sysv_shm::get(&mut runtime, 1, 0x1234, 4096, 0o1600).unwrap();
    assert_eq!(sysv_shm::get(&mut runtime, 2, 0x1234, 0, 0), Ok(a));
    assert_eq!(sysv_shm::get(&mut runtime, 3, 0x1234, 0, 0), Err(ENOENT));
    assert_eq!(sysv_shm::attach(&mut runtime, 3, a, 0, 0), Err(EINVAL));
    assert_eq!(sysv_shm::control(&mut runtime, 3, a, 0, 0), Err(EINVAL));
    let b = sysv_shm::get(&mut runtime, 3, 0x1234, 4096, 0o1600).unwrap();
    assert_ne!(a, b);
}
#[test]
fn removed_key_can_be_recreated_while_old_aliases_stay_attached() {
    let mut runtime = Runtime::new();
    let old = sysv_shm::get(&mut runtime, 1, 0x1234, 4096, 0o1600).unwrap();
    let address = sysv_shm::attach(&mut runtime, 1, old, 0, 0).unwrap();
    assert_eq!(sysv_shm::control(&mut runtime, 1, old, 0, 0), Ok(0));
    assert!(runtime.poll_objects.get(old).is_some());
    let new = sysv_shm::get(&mut runtime, 1, 0x1234, 4096, 0o1600).unwrap();
    assert_ne!(old, new);
    assert_eq!(sysv_shm::detach(&mut runtime, 1, address), Ok(0));
    assert!(runtime.poll_objects.get(old).is_none());
    assert!(runtime.poll_objects.get(new).is_some());
}
#[test]
fn last_task_cleans_only_its_own_namespace() {
    let mut runtime = Runtime::new();
    let a = sysv_shm::get(&mut runtime, 1, 0, 4096, 0o600).unwrap();
    let b = sysv_shm::get(&mut runtime, 3, 0, 4096, 0o600).unwrap();
    sysv_shm::context_exit(&mut runtime, 1);
    assert!(runtime.poll_objects.get(a).is_some());
    runtime.managed[0].exited = true;
    sysv_shm::context_exit(&mut runtime, 2);
    assert!(runtime.poll_objects.get(a).is_none());
    assert!(runtime.poll_objects.get(b).is_some());
}
#[test]
fn segment_and_namespace_limits_count_removed_but_still_attached_pages() {
    let mut runtime = Runtime::new();
    let a = sysv_shm::get(&mut runtime, 1, 0, 32 << 20, 0o600).unwrap();
    sysv_shm::attach(&mut runtime, 1, a, 0, 0).unwrap();
    sysv_shm::control(&mut runtime, 1, a, 0, 0).unwrap();
    sysv_shm::get(&mut runtime, 1, 0, 32 << 20, 0o600).unwrap();
    assert_eq!(sysv_shm::get(&mut runtime, 1, 0, 1, 0o600), Err(28));
    assert_eq!(
        sysv_shm::get(&mut runtime, 1, 0, (32 << 20) + 1, 0o600),
        Err(EINVAL)
    );
    assert!(sysv_shm::get(&mut runtime, 3, 0, 4096, 0o600).is_ok());
}
#[test]
fn stat_reports_actual_size_attachments_and_removed_state_without_overwrite() {
    let mut runtime = Runtime::new();
    let id = sysv_shm::get(&mut runtime, 1, 1234, 5000, 0o1600).unwrap();
    sysv_shm::attach(&mut runtime, 1, id, 0, 0).unwrap();
    sysv_shm::attach(&mut runtime, 2, id, 0, 0o10000).unwrap();
    let mut out = [0u64; 15];
    out[14] = 12345;
    assert_eq!(
        sysv_shm::control(&mut runtime, 1, id, 2, out.as_mut_ptr() as usize),
        Ok(0)
    );
    assert_eq!(out[6], 5000);
    assert_eq!(out[11], 2);
    assert_eq!(out[14], 12345);
    sysv_shm::control(&mut runtime, 1, id, 0, 0).unwrap();
    sysv_shm::control(&mut runtime, 1, id, 2, out.as_mut_ptr() as usize).unwrap();
    assert_eq!(out[0] as u32, 0);
    assert_eq!((out[2] >> 32) as u32 & 0o1000, 0o1000);
}
