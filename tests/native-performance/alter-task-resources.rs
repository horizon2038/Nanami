//! Real suspended installer, FileTable and resource sharing; other services modeled.
#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as nanami_runtime;
extern crate self as nanami_client;
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell};
type Word = usize;
const ALTER_IMAGE_NAME_MAX: usize = 24;
const LINUX_CWD_MAX: usize = 128;
const LINUX_TERMINAL_LINE_MAX: usize = 256;
const LINUX_FD_MAX: usize = 64;
mod abi { pub const ALTER_PATH_MAX: usize = 256; pub const SLOT_TIMER_SERVICE: usize = 31; }
#[path = "../../nanami/servers/apps/alter/shared/src/common/context.rs"]
mod context;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinuxFileKind { Empty, Terminal, Posix }
#[derive(Clone, Copy)]
pub struct LinuxFile {
    kind: LinuxFileKind, posix_fd: Word, flags: Word,
    local_port: u16, peer_port: u16, peer_ip: u32, offset: Word, resource: Word,
}
impl LinuxFile {
    const EMPTY: Self = Self { kind: LinuxFileKind::Empty, posix_fd: 0, flags: 0,
        local_port: 0, peer_port: 0, peer_ip: 0, offset: 0, resource: 0 };
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/file_table.rs"]
mod file_table;
use file_table::FileTable;
#[derive(Clone, Copy)]
struct MappingTable { address: usize, fail: bool }
impl MappingTable {
    const EMPTY: Self = Self { address: 0, fail: false };
    fn ensure(&mut self) -> bool { if self.fail { false } else { self.address = 1; true } }
    fn release(&mut self) { self.address = 0; }
}
#[derive(Clone, Copy)]
struct ProcessJobControl { session: Word, group: Word, controlling_terminal: Word, executed: bool }
impl ProcessJobControl {
    const EMPTY: Self = Self { session: 0, group: 0, controlling_terminal: 0, executed: false };
}
#[derive(Clone, Copy)]
struct FramebufferSize;
impl FramebufferSize { const DEFAULT: Self = Self; }
#[derive(Clone, Copy)]
struct LinuxSyscallContext;
impl LinuxSyscallContext { const EMPTY: Self = Self; }
#[derive(Clone, Copy)]
enum OsPersonality { Linux }
mod readiness { #[derive(Clone, Copy)] pub struct ReadinessWait; }
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize, usize, usize)> = const { Cell::new((0,0,0,0)) };
    static CLOCK_CALLS: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TRACK.with(|t| if t.get() { COUNTS.with(|c| {
            let (a,b,d,e) = c.get(); c.set((a+1,b+layout.size(),d,e));
        }); });
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        TRACK.with(|t| if t.get() { COUNTS.with(|c| {
            let (a,b,d,e) = c.get(); c.set((a,b,d+1,e+layout.size()));
        }); });
        System.dealloc(pointer, layout)
    }
}
#[global_allocator] static ALLOCATOR: Allocator = Allocator;
fn measured(f: impl FnOnce()) -> (usize,usize,usize,usize) {
    COUNTS.with(|c| c.set((0,0,0,0))); TRACK.with(|t| t.set(true));
    f(); TRACK.with(|t| t.set(false)); COUNTS.with(Cell::get)
}
pub mod registry { pub fn connect_timer_service(_: usize) -> Result<(), ()> { Ok(()) } }
pub mod ipc { pub fn process_slot_descriptor(slot: usize) -> usize { slot + 100 } }
pub mod timer {
    pub fn timer_service_monotonic_ticks(_: usize) -> Result<(usize,usize), ()> {
        super::CLOCK_CALLS.with(|c| c.set(c.get()+1)); Ok((1234,1000))
    }
}
#[derive(Clone, Copy)]
pub struct ManagedProcess {
    pub pid: Word,
    pub tgid: Word,
    pub vm_group: Word,
    pub vm_private_shared: bool,
    pub fs_group: Word,
    pub umask: Word,
    pub parent_pid: Word,
    // Guest argv table survives fork; exec replaces it. No copied command-line buffer.
    pub argument_vector: Word,
    pub argument_count: Word,
    // Linux USER_HZ (100), measured from the timer service's monotonic epoch.
    pub started_at: Word,
    pub job: ProcessJobControl,
    pub owner_pid: Word,
    pub rootfs: context::RootfsContext,
    pub pcb: Word,
    pub program_break: Word,
    pub mapped_break: Word,
    pub fs_base: Word,
    pub terminal_id: Word,
    pub trace_enabled: bool,
    pub diagnostics_enabled: bool,
    pub graphics_enabled: bool,
    pub framebuffer_size: FramebufferSize,
    pub graphics_session: Word,
    pub exited: bool,
    pub exit_status: Word,
    // Owned by Runtime. ManagedProcess copies only borrow this lazy signal state.
    pub signal_state: Word,
    pub signal_waiting: bool,
    pub signal_wait_target: Word,
    pub signal_context: LinuxSyscallContext,
    pub last_syscall: Word,
    pub last_syscall_return: isize,
    pub personality: OsPersonality,
    pub image_name: [u8; ALTER_IMAGE_NAME_MAX],
    pub image_name_len: usize,
    /// Linux task name, independent of the executable used by fork/exec.
    pub comm: [u8; 16],
    pub no_new_privs: bool,
    pub cwd: [u8; LINUX_CWD_MAX],
    pub cwd_len: usize,
    pub files: FileTable,
    pub terminal_line: [u8; LINUX_TERMINAL_LINE_MAX],
    pub terminal_line_read: usize,
    pub terminal_line_len: usize,
    pub terminal_line_ready: bool,
    pub terminal_canonical: bool,
    pub terminal_echo: bool,
    pub terminal_termios: Option<[u8; 36]>,
    pub terminal_read_waiting: bool,
    pub terminal_read_buffer: Word,
    pub terminal_read_len: Word,
    pub terminal_read_context: LinuxSyscallContext,
    pub network_waiting: bool,
    pub network_wait_context: LinuxSyscallContext,
    pub device_read_waiting: bool,
    pub device_read_fd: Word,
    pub device_read_buffer: Word,
    pub device_read_len: Word,
    pub device_read_context: LinuxSyscallContext,
    pub sleep_waiting: bool,
    pub sleep_deadline: Word,
    pub sleep_context: LinuxSyscallContext,
    pub readiness_wait: Option<readiness::ReadinessWait>,
    pub mappings: MappingTable,
}

impl ManagedProcess {
    pub const EMPTY: Self = Self {
        pid: 0,
        tgid: 0,
        vm_group: 0,
        vm_private_shared: false,
        fs_group: 0,
        umask: 0o022,
        parent_pid: 0,
        argument_vector: 0,
        argument_count: 0,
        started_at: 0,
        job: ProcessJobControl::EMPTY,
        owner_pid: 0,
        rootfs: context::RootfsContext::DEFAULT,
        pcb: 0,
        program_break: 0,
        mapped_break: 0,
        fs_base: 0,
        terminal_id: 0,
        trace_enabled: false,
        diagnostics_enabled: false,
        graphics_enabled: false,
        framebuffer_size: FramebufferSize::DEFAULT,
        graphics_session: 0,
        exited: false,
        exit_status: 0,
        signal_state: 0,
        signal_waiting: false,
        signal_wait_target: 0,
        signal_context: LinuxSyscallContext::EMPTY,
        last_syscall: 0,
        last_syscall_return: 0,
        personality: OsPersonality::Linux,
        image_name: [0; ALTER_IMAGE_NAME_MAX],
        image_name_len: 0,
        comm: [0; 16],
        no_new_privs: false,
        cwd: [0; LINUX_CWD_MAX],
        cwd_len: 0,
        files: FileTable::EMPTY,
        terminal_line: [0; LINUX_TERMINAL_LINE_MAX],
        terminal_line_read: 0,
        terminal_line_len: 0,
        terminal_line_ready: false,
        terminal_canonical: true,
        terminal_echo: true,
        terminal_termios: None,
        terminal_read_waiting: false,
        terminal_read_buffer: 0,
        terminal_read_len: 0,
        terminal_read_context: LinuxSyscallContext::EMPTY,
        network_waiting: false,
        network_wait_context: LinuxSyscallContext::EMPTY,
        device_read_waiting: false,
        device_read_fd: 0,
        device_read_buffer: 0,
        device_read_len: 0,
        device_read_context: LinuxSyscallContext::EMPTY,
        sleep_waiting: false,
        sleep_deadline: 0,
        sleep_context: LinuxSyscallContext::EMPTY,
        readiness_wait: None,
        mappings: MappingTable::EMPTY,
    };
}


struct Runtime { managed: [ManagedProcess;16], timer_port: Word }
impl Runtime {
    fn new() -> Self { Self { managed: [ManagedProcess::EMPTY;16], timer_port: 0 } }
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        self.managed.iter().find(|p| pid != 0 && p.pid == pid)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut ManagedProcess> {
        self.managed.iter_mut().find(|p| pid != 0 && p.pid == pid)
    }
    // Job-control dependency is modeled; its production implementation does
    // not inspect or mutate descriptors during installation.
    fn initialize_job_control(&mut self, pid: Word, parent: Word) {
        let job = self.managed_process(parent).map_or(ProcessJobControl::EMPTY, |p| p.job);
        self.managed_process_mut(pid).unwrap().job = job;
    }
    // Model only the unchanged last-record release rule from remove_process.
    fn remove(&mut self, pid: Word) {
        let index = self.managed.iter().position(|p| p.pid == pid).unwrap();
        let files = self.managed[index].files;
        if !self.managed.iter().any(|p| p.pid != pid && p.files.same_storage(files)) {
            self.managed[index].files.release();
        }
        self.managed[index] = ManagedProcess::EMPTY;
    }
}
fn root_cwd() -> [u8;LINUX_CWD_MAX] { let mut a=[0;LINUX_CWD_MAX]; a[0]=b'/'; a }
fn default_files(terminal: Word) -> [LinuxFile;LINUX_FD_MAX] {
    let mut files = [LinuxFile::EMPTY;LINUX_FD_MAX];
    if terminal != 0 { for f in &mut files[..3] { f.kind = LinuxFileKind::Terminal; } }
    files
}
mod bridge {
    use super::*;
    mod install { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../nanami/servers/apps/alter/shared/src/common/state/process_install.rs")); }
    mod groups { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../nanami/servers/apps/alter/shared/src/common/state/task_groups.rs")); }
}
fn parent(r: &mut Runtime) {
    assert!(r.install_managed_process(1,9,11,7,b"parent"));
    let p = r.managed_process_mut(1).unwrap();
    p.files[9] = LinuxFile { kind:LinuxFileKind::Posix,posix_fd:123,..LinuxFile::EMPTY };
    p.umask=0o077; p.comm=*b"parent-name\0\0\0\0\0"; p.no_new_privs=true;
    p.argument_vector=0x7710; p.argument_count=3;
    p.terminal_canonical=false; p.terminal_echo=false; p.terminal_termios=Some([0x72;36]);
    p.program_break=0x9000; p.mapped_break=0xa000; p.vm_private_shared=true;
}
#[test]
fn clone_files_installer_avoids_temporary_allocation_and_keeps_parent_table_alive() {
    let mut r=Runtime::new(); parent(&mut r);
    let size=std::mem::size_of::<[LinuxFile;LINUX_FD_MAX]>();
    let control=measured(|| {
        assert!(r.install_managed_child_process(2,1,9,12,7,b"child"));
        assert!(r.share_task_resources(1,2,true,true,true));
    });
    assert_eq!(control,(1,size,1,size)); r.remove(2);
    let shared=measured(|| {
        for _ in 0..128 {
            assert!(r.install_managed_child_task(2,1,9,12,7,b"child",true));
            assert!(r.files_shared_with_live_task(2));
            assert!(r.share_task_resources(1,2,true,true,true));
            let p=r.managed_process(2).unwrap();
            assert_eq!(p.files[9].posix_fd,123); assert_eq!(p.tgid,1);
            assert_eq!((p.program_break,p.mapped_break),(0x9000,0xa000));
            assert!(p.vm_private_shared);
            r.remove(2);
            assert_eq!(r.managed_process(1).unwrap().files[9].posix_fd,123);
        }
    });
    assert_eq!(shared,(0,0,0,0));
    println!("CLONE-FILES-TABLE control={control:?} shared128={shared:?}");
    assert_eq!(measured(||r.remove(1)),(0,0,1,size));
}
#[test]
fn rollback_before_and_after_resource_sharing_preserves_parent_and_last_owner_release() {
    let mut r=Runtime::new(); parent(&mut r);
    assert!(r.install_managed_child_task(2,1,9,12,7,b"child",true));
    r.managed_process_mut(1).unwrap().mappings.fail=true;
    assert!(!r.share_task_resources(1,2,true,false,false));
    assert!(r.files_shared_with_live_task(2));
    assert_eq!(measured(||r.remove(2)),(0,0,0,0));
    r.managed_process_mut(1).unwrap().mappings.fail=false;
    assert!(r.install_managed_child_task(2,1,9,12,7,b"child",true));
    assert!(r.share_task_resources(1,2,true,false,false));
    r.managed_process_mut(2).unwrap().files[9].flags=0x81;
    assert_eq!(r.managed_process(1).unwrap().files[9].flags,0x81);
    assert_eq!(measured(||r.remove(1)),(0,0,0,0));
    assert!(!r.files_shared_with_live_task(2));
    assert_eq!(r.managed_process(2).unwrap().files[9].posix_fd,123);
    let size=std::mem::size_of::<[LinuxFile;LINUX_FD_MAX]>();
    assert_eq!(measured(||r.remove(2)),(0,0,1,size));
}
#[test]
fn private_child_retains_default_stdio_and_installer_metadata() {
    let mut r=Runtime::new(); parent(&mut r);
    assert!(r.install_managed_child_task(2,1,9,12,7,b"child",false));
    let p=r.managed_process(2).unwrap();
    assert!(!p.files.same_storage(r.managed_process(1).unwrap().files));
    assert!(p.files[..3].iter().all(|f|f.kind==LinuxFileKind::Terminal));
    assert!(p.files[3..].iter().all(|f|f.kind==LinuxFileKind::Empty));
    assert_eq!(p.umask,0o077); assert_eq!(p.comm,*b"parent-name\0\0\0\0\0"); assert!(p.no_new_privs);
    assert_eq!((p.argument_vector,p.argument_count,p.started_at),(0x7710,3,123));
    assert!(!p.terminal_canonical && !p.terminal_echo); assert_eq!(p.terminal_termios,Some([0x72;36]));
    assert!(r.share_task_resources(1,2,false,false,false));
    r.managed_process_mut(2).unwrap().files[9].posix_fd=456;
    assert_eq!(r.managed_process(1).unwrap().files[9].posix_fd,123);
    r.remove(2);r.remove(1);
}
#[test]
fn invalid_name_missing_shared_parent_and_full_table_do_not_allocate() {
    let mut r=Runtime::new(); parent(&mut r);
    CLOCK_CALLS.with(|c|c.set(0));
    assert_eq!(measured(|| {
        assert!(!r.install_managed_child_task(2,99,9,12,7,b"child",true));
        assert!(!r.install_managed_child_task(2,1,9,12,7,b"",true));
    }),(0,0,0,0));
    CLOCK_CALLS.with(|c|assert_eq!(c.get(),0));
    for i in 1..16 {r.managed[i].pid=i+1;}
    assert_eq!(measured(||assert!(!r.install_managed_child_task(99,1,9,12,7,b"child",true))),(0,0,0,0));
    assert_eq!(r.managed_process(1).unwrap().files[9].posix_fd,123);
    r.remove(1);
}
