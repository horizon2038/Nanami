//! Production System V IPC namespace/lifetime decisions; VM IPC is mocked.
#![allow(dead_code)]
extern crate alloc;
type Word = usize;
const EINVAL: i32 = 22;
const EFAULT: i32 = 14;
const ESRCH: i32 = 3;
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
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn vm_group(&self, pid: Word) -> Word {
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
    pub fn realtime(_: &mut super::Runtime) -> Result<(usize, usize), i32> {
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
