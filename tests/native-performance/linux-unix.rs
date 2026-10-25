//! Production AF_UNIX handlers, with native IPC and guest memory mocked.
#![allow(dead_code, private_interfaces)]
extern crate alloc;
extern crate self as nanami_runtime;
pub type Word = usize;
const LINUX_FD_MAX: usize = 64;
const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 4;
#[derive(Debug, Clone, Copy)]
enum RequestError {
    Status(Word),
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/arch/x86_64.rs"]
mod numbers;
use numbers::*;
#[derive(Clone, Copy)]
struct LinuxSyscallContext {
    number: Word,
    args: [Word; 6],
}
impl LinuxSyscallContext {
    const EMPTY: Self = Self {
        number: 0,
        args: [0; 6],
    };
}
#[derive(Debug, PartialEq)]
enum EmulationAction {
    Return(isize),
    Park,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LinuxFileKind {
    Empty,
    Posix,
    Terminal,
    PipeRead,
    PipeWrite,
    SocketUnix,
    SocketUdp,
    SocketTcp,
    SocketTcpListener,
    SocketIcmp,
    SocketNetlink,
    EventFd,
    Epoll,
    VirtualDirectory,
    VirtualFile,
    EvdevKeyboard,
    EvdevMouse,
    Framebuffer,
}
#[derive(Clone, Copy, Debug)]
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
    fn has_posix_backend(self) -> bool {
        self.kind == LinuxFileKind::Posix
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/poll_objects.rs"]
pub mod poll_objects;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/readiness.rs"]
pub mod readiness;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/unix_sockets.rs"]
pub mod unix_sockets;
mod state {
    pub const LINUX_FD_MAX: usize = crate::LINUX_FD_MAX;
    pub use crate::{poll_objects, readiness, unix_sockets};
}
use readiness::*;
struct Rootfs {
    leader: Word,
}
struct Process {
    pid: Word,
    files: [LinuxFile; LINUX_FD_MAX],
    rootfs: Rootfs,
    readiness_wait: Option<ReadinessWait>,
}
struct Runtime {
    unix_sockets: unix_sockets::Sockets,
    poll_objects: poll_objects::Objects,
    managed: Vec<Process>,
    scratch: Box<[u8]>,
    posix_shm: Word,
    posix_shm_size: Word,
    posix_port: Word,
    readiness_changes: Word,
    bad_read: Word,
    bad_write: Word,
    released: Vec<LinuxFile>,
    signals: Vec<Word>,
}
impl Runtime {
    fn new() -> Self {
        FS.with(|fs| fs.borrow_mut().clear());
        let mut scratch = vec![0; 65536].into_boxed_slice();
        let address = scratch.as_mut_ptr() as Word;
        Self {
            unix_sockets: unix_sockets::Sockets::new(),
            poll_objects: poll_objects::Objects::new(),
            managed: [10, 20]
                .into_iter()
                .map(|pid| Process {
                    pid,
                    files: [LinuxFile::EMPTY; LINUX_FD_MAX],
                    rootfs: Rootfs { leader: 0 },
                    readiness_wait: None,
                })
                .collect(),
            scratch,
            posix_shm: address,
            posix_shm_size: 65536,
            posix_port: address,
            readiness_changes: 0,
            bad_read: Word::MAX,
            bad_write: Word::MAX,
            released: vec![],
            signals: vec![],
        }
    }
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn managed_process_mut(&mut self, pid: Word) -> Option<&mut Process> {
        self.managed.iter_mut().find(|p| p.pid == pid)
    }
    fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        self.managed_process(pid)?
            .files
            .get(fd)
            .copied()
            .filter(|f| f.kind != LinuxFileKind::Empty)
    }
    fn allocate_linux_file(&mut self, pid: Word, file: LinuxFile, from: Word) -> Option<Word> {
        let process = self.managed_process_mut(pid)?;
        let fd = (from..LINUX_FD_MAX).find(|fd| process.files[*fd].kind == LinuxFileKind::Empty)?;
        process.files[fd] = file;
        Some(fd)
    }
    fn clear_linux_file(&mut self, pid: Word, fd: Word) -> Option<LinuxFile> {
        Some(core::mem::replace(
            self.managed_process_mut(pid)?.files.get_mut(fd)?,
            LinuxFile::EMPTY,
        ))
    }
}
fn read_target_memory(r: &Runtime, _: Word, pointer: Word, len: Word) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    if pointer == 0 || pointer == r.bad_read || len > r.posix_shm_size {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(pointer as *const u8, r.posix_shm as *mut u8, len);
    }
    Ok(())
}
fn write_target_memory(r: &Runtime, _: Word, pointer: Word, len: Word) -> Result<(), i32> {
    if len == 0 {
        return Ok(());
    }
    if pointer == 0 || pointer == r.bad_write || len > r.posix_shm_size {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(r.posix_shm as *const u8, pointer as *mut u8, len);
    }
    Ok(())
}
fn write_u32_to_target(r: &mut Runtime, pid: Word, pointer: Word, value: u32) -> Result<(), i32> {
    unsafe {
        write_u32(r.posix_shm, value);
    }
    write_target_memory(r, pid, pointer, 4)
}
unsafe fn write_u32(pointer: Word, value: u32) {
    (pointer as *mut u32).write_unaligned(value);
}
unsafe fn write_u64(pointer: Word, value: Word) {
    (pointer as *mut u64).write_unaligned(value as u64);
}
fn read_shm_u32(r: &Runtime, offset: usize) -> u32 {
    unsafe { ((r.posix_shm + offset) as *const u32).read_unaligned() }
}
fn read_shm_u64(r: &Runtime, offset: usize) -> Word {
    unsafe { ((r.posix_shm + offset) as *const u64).read_unaligned() as Word }
}
fn read_shm_u16(r: &Runtime, offset: usize) -> u16 {
    unsafe { ((r.posix_shm + offset) as *const u16).read_unaligned() }
}
fn read_linux_iovecs(
    r: &mut Runtime,
    pid: Word,
    pointer: Word,
    count: Word,
    bases: &mut [Word; LINUX_IOV_MAX as usize],
    lengths: &mut [Word; LINUX_IOV_MAX as usize],
) -> Result<(), i32> {
    if count > LINUX_IOV_MAX {
        return Err(EINVAL);
    }
    read_target_memory(r, pid, pointer, count * 16)?;
    for i in 0..count {
        bases[i] = read_shm_u64(r, i * 16);
        lengths[i] = read_shm_u64(r, i * 16 + 8);
    }
    Ok(())
}
fn resolve_current_shm_path(_: &mut Runtime, _: Word, len: Word) -> Result<Word, i32> {
    Ok(len)
}
fn translate_guest_path_for_vfs(_: &mut Runtime, _: Word, len: Word) -> Result<Word, i32> {
    Ok(len)
}
fn reject_virtual_fs_mutation(_: &Runtime, _: Word, _: Word, _: Word) -> Result<(), i32> {
    Ok(())
}
fn map_path_request_error(error: RequestError) -> i32 {
    match error {
        RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR) => ENOENT,
        _ => EIO,
    }
}
type Nodes = std::collections::BTreeMap<Vec<u8>, (Word, Word)>;
thread_local! { static FS: std::cell::RefCell<Nodes> = const { std::cell::RefCell::new(Nodes::new()) }; }
mod posix {
    use super::*;
    pub const POSIX_FILE_TYPE_SOCKET: Word = 5;
    pub fn posix_create_socket_node(
        shm: Word,
        _: Word,
        len: Word,
    ) -> Result<(Word, Word), RequestError> {
        let name = unsafe { core::slice::from_raw_parts(shm as *const u8, len) }.to_vec();
        FS.with(|fs| {
            let mut fs = fs.borrow_mut();
            if fs.contains_key(&name) {
                return Err(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT));
            }
            let inode = fs.len() + 1;
            fs.insert(name, (inode, POSIX_FILE_TYPE_SOCKET));
            Ok((inode, inode + 1000))
        })
    }
    pub fn posix_close(_: Word, _: Word) -> Result<(), RequestError> {
        Ok(())
    }
    pub fn posix_stat(
        shm: Word,
        _: Word,
        len: Word,
    ) -> Result<(Word, Word, Word, Word, Word), RequestError> {
        let name = unsafe { core::slice::from_raw_parts(shm as *const u8, len) };
        FS.with(|fs| {
            fs.borrow()
                .get(name)
                .map(|(inode, kind)| (*inode, 0, *kind, 0, 0))
                .ok_or(RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR))
        })
    }
}
fn duplicate_linux_file(_: &mut Runtime, file: LinuxFile) -> Result<LinuxFile, i32> {
    Ok(file)
}
fn close_linux_fd(r: &mut Runtime, pid: Word, fd: Word) -> Result<(), i32> {
    let file = r.clear_linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::SocketUnix {
        unix::collect(r);
    } else {
        descriptors::release_file_description(r, file);
    }
    Ok(())
}
mod descriptors {
    use super::*;
    pub(super) fn release_file_description(r: &mut Runtime, file: LinuxFile) {
        r.released.push(file);
        if file.kind == LinuxFileKind::EventFd {
            eventfd::release(r, file);
        }
    }
}
mod epoll {
    use super::*;
    pub(super) fn forget_closed(_: &mut Runtime, _: Word, _: LinuxFile) {}
}
mod common {
    pub mod signals {
        pub struct Info;
        impl Info {
            pub fn new(_: usize, _: i32, _: usize) -> Self {
                Self
            }
        }
    }
}
mod signals_runtime {
    use super::*;
    pub(super) fn queue(r: &mut Runtime, pid: Word, _: common::signals::Info) -> Result<(), i32> {
        r.signals.push(pid);
        Ok(())
    }
}
fn result_to_linux_return(result: Result<Word, i32>) -> isize {
    result.map_or_else(|e| -(e as isize), |v| v as isize)
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/eventfd.rs"]
mod eventfd;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/unix.rs"]
mod unix;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/unix_message.rs"]
mod unix_message;
use unix::*;
fn call(r: &mut Runtime, pid: Word, nr: Word, args: [Word; 6]) -> EmulationAction {
    unix::dispatch(r, pid, LinuxSyscallContext { number: nr, args }).unwrap()
}
fn value(action: EmulationAction) -> Word {
    match action {
        EmulationAction::Return(v) if v >= 0 => v as Word,
        other => panic!("{other:?}"),
    }
}
fn pair(r: &mut Runtime, kind: Word) -> [i32; 2] {
    let mut out = [-1; 2];
    assert_eq!(
        call(
            r,
            10,
            SYS_SOCKETPAIR,
            [1, kind, 0, out.as_mut_ptr() as Word, 0, 0]
        ),
        EmulationAction::Return(0)
    );
    out
}
fn send(r: &mut Runtime, fd: i32, bytes: &[u8]) -> EmulationAction {
    call(
        r,
        10,
        SYS_WRITE,
        [fd as Word, bytes.as_ptr() as Word, bytes.len(), 0, 0, 0],
    )
}
fn receive(r: &mut Runtime, fd: i32, bytes: &mut [u8]) -> EmulationAction {
    call(
        r,
        10,
        SYS_READ,
        [fd as Word, bytes.as_mut_ptr() as Word, bytes.len(), 0, 0, 0],
    )
}

#[test]
fn stream_pair_coalesces_writes_supports_short_reads_and_half_close() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1 | LINUX_SOCK_NONBLOCK);
    assert_eq!(
        receive(&mut r, b, &mut [0; 4]),
        EmulationAction::Return(-(EAGAIN as isize))
    );
    assert_eq!(send(&mut r, a, b"ab"), EmulationAction::Return(2));
    assert_eq!(send(&mut r, a, b"cde"), EmulationAction::Return(3));
    let mut out = [0; 3];
    assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(3));
    assert_eq!(&out, b"abc");
    assert_eq!(
        call(&mut r, 10, SYS_SHUTDOWN, [a as Word, 1, 0, 0, 0, 0]),
        EmulationAction::Return(0)
    );
    assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(2));
    assert_eq!(&out[..2], b"de");
    assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(0));
    assert_eq!(send(&mut r, b, b"x"), EmulationAction::Return(1));
    assert_eq!(
        send(&mut r, a, b"x"),
        EmulationAction::Return(-(EPIPE as isize))
    );
    assert_eq!(r.signals, [10]);
}

#[test]
fn records_preserve_boundaries_and_zero_length_datagrams() {
    for kind in [2, 5] {
        let mut r = Runtime::new();
        let [a, b] = pair(&mut r, kind | LINUX_SOCK_NONBLOCK);
        assert_eq!(send(&mut r, a, b"abcdef"), EmulationAction::Return(6));
        assert_eq!(send(&mut r, a, b"xy"), EmulationAction::Return(2));
        let mut out = [0; 3];
        assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(3));
        assert_eq!(&out, b"abc");
        assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(2));
        assert_eq!(&out[..2], b"xy");
        assert_eq!(send(&mut r, a, b""), EmulationAction::Return(0));
        assert_eq!(receive(&mut r, b, &mut out), EmulationAction::Return(0));
    }
}

#[repr(C)]
struct Iov {
    pointer: Word,
    len: Word,
}
#[repr(C)]
struct Header {
    name: Word,
    name_len: u32,
    pad: u32,
    iov: Word,
    count: Word,
    control: Word,
    control_len: Word,
    flags: u32,
    tail: u32,
}
fn header(iov: &mut [Iov], control: &mut [u8]) -> Header {
    Header {
        name: 0,
        name_len: 0,
        pad: 0,
        iov: iov.as_mut_ptr() as Word,
        count: iov.len(),
        control: control.as_mut_ptr() as Word,
        control_len: control.len(),
        flags: 0,
        tail: 0,
    }
}
fn send_rights(r: &mut Runtime, socket: i32, fds: &[i32], bytes: &[u8]) -> EmulationAction {
    let mut control = vec![0; (16 + fds.len() * 4 + 7) & !7];
    control[..8].copy_from_slice(&((16 + fds.len() * 4) as u64).to_ne_bytes());
    control[8..12].copy_from_slice(&1u32.to_ne_bytes());
    control[12..16].copy_from_slice(&1u32.to_ne_bytes());
    for (index, fd) in fds.iter().enumerate() {
        control[16 + index * 4..20 + index * 4].copy_from_slice(&fd.to_ne_bytes());
    }
    let mut iov = [Iov {
        pointer: bytes.as_ptr() as Word,
        len: bytes.len(),
    }];
    let mut msg = header(&mut iov, &mut control);
    call(
        r,
        10,
        SYS_SENDMSG,
        [socket as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0],
    )
}
fn addr(bytes: &[u8]) -> Vec<u8> {
    [vec![1, 0], bytes.to_vec()].concat()
}
fn create_socket(r: &mut Runtime, pid: Word, kind: Word) -> Word {
    value(call(r, pid, SYS_SOCKET, [1, kind, 0, 0, 0, 0]))
}
fn bind_socket(r: &mut Runtime, pid: Word, fd: Word, address: &[u8]) -> EmulationAction {
    call(
        r,
        pid,
        SYS_BIND,
        [fd, address.as_ptr() as Word, address.len(), 0, 0, 0],
    )
}
fn connect_socket(r: &mut Runtime, pid: Word, fd: Word, address: &[u8]) -> EmulationAction {
    call(
        r,
        pid,
        SYS_CONNECT,
        [fd, address.as_ptr() as Word, address.len(), 0, 0, 0],
    )
}

#[test]
fn ancillary_fd_survives_sender_close_peek_and_receiver_copy_fault() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let event = eventfd::create(&mut r, 10, 7, 0).unwrap();
    let object = r.linux_file(10, event).unwrap().resource;
    assert_eq!(
        send_rights(&mut r, a, &[event as i32], b"xy"),
        EmulationAction::Return(2)
    );
    close_linux_fd(&mut r, 10, event).unwrap();
    assert!(r.poll_objects.get(object).is_some());
    let mut bytes = [0; 1];
    let mut control = [0; 24];
    let mut iov = [Iov {
        pointer: bytes.as_mut_ptr() as Word,
        len: bytes.len(),
    }];
    let mut msg = header(&mut iov, &mut control);
    let ptr = &mut msg as *mut _ as Word;
    r.bad_write = ptr + 48;
    assert_eq!(
        call(&mut r, 10, SYS_RECVMSG, [b as Word, ptr, 0, 0, 0, 0]),
        EmulationAction::Return(-(EFAULT as isize))
    );
    r.bad_write = Word::MAX;
    let count = r.managed[0]
        .files
        .iter()
        .filter(|f| f.kind == LinuxFileKind::EventFd)
        .count();
    assert_eq!(count, 0);
    msg.control_len = 24;
    let ptr = std::hint::black_box(&mut msg as *mut _ as Word);
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_RECVMSG,
            [b as Word, ptr, MSG_PEEK | MSG_CMSG_CLOEXEC, 0, 0, 0]
        ),
        EmulationAction::Return(1)
    );
    let peek_fd = i32::from_ne_bytes(control[16..20].try_into().unwrap()) as Word;
    assert_eq!(
        r.linux_file(10, peek_fd).unwrap().flags & LINUX_FD_CLOEXEC,
        LINUX_FD_CLOEXEC
    );
    close_linux_fd(&mut r, 10, peek_fd).unwrap();
    assert!(r.poll_objects.get(object).is_some());
    msg.control_len = 24;
    let ptr = std::hint::black_box(&mut msg as *mut _ as Word);
    assert_eq!(
        call(&mut r, 10, SYS_RECVMSG, [b as Word, ptr, 0, 0, 0, 0]),
        EmulationAction::Return(1)
    );
    let received = i32::from_ne_bytes(control[16..20].try_into().unwrap()) as Word;
    assert_eq!(
        r.linux_file(10, received).unwrap().flags & LINUX_FD_CLOEXEC,
        0
    );
    assert_eq!(bytes, *b"x");
    let mut counter = 0u64;
    let file = r.linux_file(10, received).unwrap();
    assert_eq!(
        eventfd::read(&mut r, 10, file, &mut counter as *mut _ as Word, 8),
        Ok(8)
    );
    assert_eq!(counter, 7);
    close_linux_fd(&mut r, 10, received).unwrap();
    assert!(r.poll_objects.get(object).is_none());
    assert_eq!(receive(&mut r, b, &mut bytes), EmulationAction::Return(1));
    assert_eq!(bytes, *b"y");
}

#[test]
fn control_truncation_and_plain_read_close_undelivered_descriptors() {
    for space in [0, 16, 20] {
        let mut r = Runtime::new();
        let [a, b] = pair(&mut r, 1);
        let x = eventfd::create(&mut r, 10, 1, 0).unwrap();
        let y = eventfd::create(&mut r, 10, 1, 0).unwrap();
        let ids = [
            r.linux_file(10, x).unwrap().resource,
            r.linux_file(10, y).unwrap().resource,
        ];
        send_rights(&mut r, a, &[x as i32, y as i32], b"x");
        close_linux_fd(&mut r, 10, x).unwrap();
        close_linux_fd(&mut r, 10, y).unwrap();
        let mut data = [0];
        let mut control = vec![0; space];
        let mut iov = [Iov {
            pointer: data.as_mut_ptr() as Word,
            len: 1,
        }];
        let mut msg = header(&mut iov, &mut control);
        assert_eq!(
            call(
                &mut r,
                10,
                SYS_RECVMSG,
                [b as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
            ),
            EmulationAction::Return(1)
        );
        assert_eq!(msg.flags & MSG_CTRUNC as u32, MSG_CTRUNC as u32);
        assert!(r.poll_objects.get(ids[1]).is_none());
        if space == 20 {
            assert!(r.poll_objects.get(ids[0]).is_some());
            let fd = i32::from_ne_bytes(control[16..20].try_into().unwrap()) as Word;
            close_linux_fd(&mut r, 10, fd).unwrap();
        }
        assert!(r.poll_objects.get(ids[0]).is_none());
    }
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let fd = eventfd::create(&mut r, 10, 1, 0).unwrap();
    let id = r.linux_file(10, fd).unwrap().resource;
    send_rights(&mut r, a, &[fd as i32], b"x");
    close_linux_fd(&mut r, 10, fd).unwrap();
    assert_eq!(receive(&mut r, b, &mut [0]), EmulationAction::Return(1));
    assert!(r.poll_objects.get(id).is_none());
}

#[test]
fn cyclic_unix_rights_and_external_resources_are_collected_without_recursion() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let [c, d] = pair(&mut r, 1);
    let counter = eventfd::create(&mut r, 10, 1, 0).unwrap();
    let id = r.linux_file(10, counter).unwrap().resource;
    send_rights(&mut r, a, &[d, counter as i32], b"x");
    send_rights(&mut r, c, &[b], b"y");
    for fd in [a, b, c, d, counter as i32] {
        close_linux_fd(&mut r, 10, fd as Word).unwrap();
    }
    assert!(r.unix_sockets.is_empty());
    assert!(r.poll_objects.get(id).is_none());
}

#[test]
fn abstract_addresses_are_length_sensitive_and_scope_execution_contexts() {
    let mut r = Runtime::new();
    let a = create_socket(&mut r, 10, 1);
    let b = create_socket(&mut r, 20, 1 | LINUX_SOCK_NONBLOCK);
    let name = addr(b"\0wayland\0test");
    assert_eq!(
        bind_socket(&mut r, 10, a, &name),
        EmulationAction::Return(0)
    );
    assert_eq!(
        call(&mut r, 10, SYS_LISTEN, [a, 1, 0, 0, 0, 0]),
        EmulationAction::Return(0)
    );
    assert_eq!(
        connect_socket(&mut r, 20, b, &addr(b"\0wayland")),
        EmulationAction::Return(-(ECONNREFUSED as isize))
    );
    r.managed[1].rootfs.leader = 20;
    assert_eq!(
        connect_socket(&mut r, 20, b, &name),
        EmulationAction::Return(-(ECONNREFUSED as isize))
    );
    r.managed[1].rootfs.leader = 0;
    assert_eq!(
        connect_socket(&mut r, 20, b, &name),
        EmulationAction::Return(0)
    );
    let accepted = value(call(
        &mut r,
        10,
        SYS_ACCEPT4,
        [a, 0, 0, LINUX_SOCK_CLOEXEC | LINUX_SOCK_NONBLOCK, 0, 0],
    ));
    let accepted_file = r.linux_file(10, accepted).unwrap();
    assert!(unix::nonblocking(&r, accepted_file));
    assert_eq!(accepted_file.flags, LINUX_FD_CLOEXEC);
    let mut peer = [0u32; 3];
    let mut len = 12u32;
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_GETSOCKOPT,
            [
                accepted,
                1,
                17,
                peer.as_mut_ptr() as Word,
                &mut len as *mut _ as Word,
                0
            ]
        ),
        EmulationAction::Return(0)
    );
    assert_eq!(peer, [20, 0, 0]);
}

#[test]
fn pathname_outlives_close_but_rename_and_unlink_follow_inode_identity() {
    let mut r = Runtime::new();
    let server = create_socket(&mut r, 10, 1);
    let name = addr(b"/tmp/wayland-0\0");
    assert_eq!(
        bind_socket(&mut r, 10, server, &name),
        EmulationAction::Return(0)
    );
    call(&mut r, 10, SYS_LISTEN, [server, 1, 0, 0, 0, 0]);
    let renamed = addr(b"/tmp/wayland-new\0");
    FS.with(|fs| {
        let mut fs = fs.borrow_mut();
        let entry = fs.remove(b"/tmp/wayland-0".as_slice()).unwrap();
        fs.insert(b"/tmp/wayland-new".to_vec(), entry);
    });
    let client = create_socket(&mut r, 20, 1);
    assert_eq!(
        connect_socket(&mut r, 20, client, &renamed),
        EmulationAction::Return(0)
    );
    close_linux_fd(&mut r, 10, server).unwrap();
    let replacement = create_socket(&mut r, 10, 1);
    assert_eq!(
        bind_socket(&mut r, 10, replacement, &renamed),
        EmulationAction::Return(-(EADDRINUSE as isize))
    );
    let another_client = create_socket(&mut r, 20, 1);
    assert_eq!(
        connect_socket(&mut r, 20, another_client, &renamed),
        EmulationAction::Return(-(ECONNREFUSED as isize))
    );
}

#[test]
fn backpressure_parks_without_polling_and_nosignal_suppresses_sigpipe() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let data = vec![7; unix_sockets::QUEUE_BYTES + 1];
    assert_eq!(
        send(&mut r, a, &data),
        EmulationAction::Return(unix_sockets::QUEUE_BYTES as isize)
    );
    assert_eq!(send(&mut r, a, b"x"), EmulationAction::Park);
    assert_eq!(r.managed[0].readiness_wait.unwrap().sources, READY_UNIX);
    let mut out = [0];
    receive(&mut r, b, &mut out);
    let wait = r.managed[0].readiness_wait.unwrap();
    assert_eq!(unix::retry(&mut r, 10, wait.context), Some(Ok(1)));
    close_linux_fd(&mut r, 10, b as Word).unwrap();
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_SENDTO,
            [a as Word, data.as_ptr() as Word, 1, MSG_NOSIGNAL, 0, 0]
        ),
        EmulationAction::Return(-(EPIPE as isize))
    );
    assert!(r.signals.is_empty());
}

#[test]
fn failed_socketpair_copyout_does_not_leak_endpoints_or_descriptors() {
    let mut r = Runtime::new();
    assert_eq!(
        call(&mut r, 10, SYS_SOCKETPAIR, [1, 1, 0, 0, 0, 0]),
        EmulationAction::Return(-(EFAULT as isize))
    );
    assert!(r.unix_sockets.is_empty());
    assert!(r.managed[0]
        .files
        .iter()
        .all(|file| file.kind == LinuxFileKind::Empty));
    assert_eq!(
        call(&mut r, 10, SYS_SOCKET, [1, 1, 17, 0, 0, 0]),
        EmulationAction::Return(-(EPROTONOSUPPORT as isize))
    );
}

#[test]
fn accept_copy_fault_retains_pending_connection_even_after_client_closes() {
    let mut r = Runtime::new();
    let server = create_socket(&mut r, 10, 1 | LINUX_SOCK_NONBLOCK);
    let client = create_socket(&mut r, 20, 1);
    let name = addr(b"\0accept-rollback");
    assert_eq!(
        bind_socket(&mut r, 10, server, &name),
        EmulationAction::Return(0)
    );
    call(&mut r, 10, SYS_LISTEN, [server, 0, 0, 0, 0, 0]);
    assert_eq!(
        connect_socket(&mut r, 20, client, &name),
        EmulationAction::Return(0)
    );
    close_linux_fd(&mut r, 20, client).unwrap();
    let mut output = [0u8; 16];
    let mut length = output.len() as u32;
    r.bad_write = output.as_mut_ptr() as Word;
    let args = [
        server,
        output.as_mut_ptr() as Word,
        &mut length as *mut _ as Word,
        LINUX_SOCK_NONBLOCK,
        0,
        0,
    ];
    assert_eq!(
        call(&mut r, 10, SYS_ACCEPT4, args),
        EmulationAction::Return(-(EFAULT as isize))
    );
    r.bad_write = Word::MAX;
    let fd = value(call(&mut r, 10, SYS_ACCEPT4, args));
    assert_eq!(length, 2);
    assert_eq!(
        receive(&mut r, fd as i32, &mut [0]),
        EmulationAction::Return(0)
    );
    assert_eq!(
        call(&mut r, 10, SYS_ACCEPT, [server, 0, 0, 0, 0, 0]),
        EmulationAction::Return(-(EAGAIN as isize))
    );
}

#[test]
fn full_listener_backlog_parks_connect_and_retries_after_accept() {
    let mut r = Runtime::new();
    let server = create_socket(&mut r, 10, 1);
    let name = addr(b"\0backlog");
    bind_socket(&mut r, 10, server, &name);
    call(&mut r, 10, SYS_LISTEN, [server, 0, 0, 0, 0, 0]);
    let first = create_socket(&mut r, 20, 1);
    let second = create_socket(&mut r, 20, 1);
    assert_eq!(
        connect_socket(&mut r, 20, first, &name),
        EmulationAction::Return(0)
    );
    assert_eq!(
        connect_socket(&mut r, 20, second, &name),
        EmulationAction::Park
    );
    let wait = r.managed[1].readiness_wait.unwrap();
    value(call(&mut r, 10, SYS_ACCEPT, [server, 0, 0, 0, 0, 0]));
    assert_eq!(unix::retry(&mut r, 20, wait.context), Some(Ok(0)));
}

#[test]
fn descriptor_limit_truncates_rights_and_releases_the_queued_description() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let event = eventfd::create(&mut r, 10, 1, 0).unwrap();
    let id = r.linux_file(10, event).unwrap().resource;
    assert_eq!(
        send_rights(&mut r, a, &[event as i32], b"x"),
        EmulationAction::Return(1)
    );
    close_linux_fd(&mut r, 10, event).unwrap();
    for slot in &mut r.managed[0].files {
        if slot.kind == LinuxFileKind::Empty {
            *slot = LinuxFile {
                kind: LinuxFileKind::Posix,
                ..LinuxFile::EMPTY
            };
        }
    }
    let mut bytes = [0];
    let mut control = [0; 24];
    let mut iov = [Iov {
        pointer: bytes.as_mut_ptr() as Word,
        len: 1,
    }];
    let mut msg = header(&mut iov, &mut control);
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_RECVMSG,
            [b as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
        ),
        EmulationAction::Return(1)
    );
    assert_eq!(msg.flags, MSG_CTRUNC as u32);
    assert_eq!(msg.control_len, 0);
    assert!(r.poll_objects.get(id).is_none());
}

#[test]
fn invalid_rights_do_not_enqueue_payload_or_duplicate_valid_descriptors() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1 | LINUX_SOCK_NONBLOCK);
    let event = eventfd::create(&mut r, 10, 1, 0).unwrap();
    assert_eq!(
        send_rights(&mut r, a, &[event as i32, 63], b"x"),
        EmulationAction::Return(-(EBADF as isize))
    );
    assert_eq!(
        receive(&mut r, b, &mut [0]),
        EmulationAction::Return(-(EAGAIN as isize))
    );
    assert!(r.released.is_empty());
    let process_relative = r
        .allocate_linux_file(
            10,
            LinuxFile {
                kind: LinuxFileKind::Framebuffer,
                ..LinuxFile::EMPTY
            },
            0,
        )
        .unwrap();
    assert_eq!(
        send_rights(&mut r, a, &[process_relative as i32], b"x"),
        EmulationAction::Return(-(EOPNOTSUPP as isize))
    );
    let network = r
        .allocate_linux_file(
            10,
            LinuxFile {
                kind: LinuxFileKind::SocketTcp,
                ..LinuxFile::EMPTY
            },
            0,
        )
        .unwrap();
    assert_eq!(
        send_rights(&mut r, a, &[network as i32], b"x"),
        EmulationAction::Return(-(EOPNOTSUPP as isize))
    );
}

#[test]
fn eof_clears_recvmsg_output_metadata_without_installing_rights() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    close_linux_fd(&mut r, 10, a as Word).unwrap();
    let mut bytes = [0];
    let mut control = [0; 24];
    let mut iov = [Iov {
        pointer: bytes.as_mut_ptr() as Word,
        len: 1,
    }];
    let mut msg = header(&mut iov, &mut control);
    msg.name_len = 100;
    msg.flags = u32::MAX;
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_RECVMSG,
            [b as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
        ),
        EmulationAction::Return(0)
    );
    assert_eq!((msg.name_len, msg.control_len, msg.flags), (0, 0, 0));
}

#[test]
fn full_length_pathname_reports_appended_nul_and_truncates_sockaddr_output() {
    let mut r = Runtime::new();
    let fd = create_socket(&mut r, 10, 1);
    let name = addr(&[b'x'; 108]);
    assert_eq!(
        bind_socket(&mut r, 10, fd, &name),
        EmulationAction::Return(0)
    );
    let mut output = [0; 4];
    let mut length = output.len() as u32;
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_GETSOCKNAME,
            [
                fd,
                output.as_mut_ptr() as Word,
                &mut length as *mut _ as Word,
                0,
                0,
                0
            ]
        ),
        EmulationAction::Return(0)
    );
    assert_eq!(length, 111);
    assert_eq!(output, [1, 0, b'x', b'x']);
}

#[test]
fn pipe_status_updates_include_inflight_aliases_without_copying_cloexec() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1);
    let pipe = LinuxFile {
        kind: LinuxFileKind::PipeRead,
        posix_fd: 77,
        resource: 0,
        flags: LINUX_FD_CLOEXEC,
    };
    let fd = r.allocate_linux_file(10, pipe, 0).unwrap();
    assert_eq!(
        send_rights(&mut r, a, &[fd as i32], b"p"),
        EmulationAction::Return(1)
    );
    r.unix_sockets
        .update_flags(pipe, LINUX_O_NONBLOCK, LINUX_O_NONBLOCK);
    let mut data = [0];
    let mut control = [0; 24];
    let mut iov = [Iov {
        pointer: data.as_mut_ptr() as Word,
        len: 1,
    }];
    let mut msg = header(&mut iov, &mut control);
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_RECVMSG,
            [b as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
        ),
        EmulationAction::Return(1)
    );
    let received = i32::from_ne_bytes(control[16..20].try_into().unwrap()) as Word;
    assert_eq!(r.linux_file(10, received).unwrap().flags, LINUX_O_NONBLOCK);
}

#[test]
fn vectored_stream_receive_stops_at_rights_barrier_without_framing_plain_writes() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1 | LINUX_SOCK_NONBLOCK);
    let event = eventfd::create(&mut r, 10, 1, 0).unwrap();
    let first = b"ab";
    let second = b"c";
    let iov = [
        Iov {
            pointer: first.as_ptr() as Word,
            len: first.len(),
        },
        Iov {
            pointer: second.as_ptr() as Word,
            len: second.len(),
        },
    ];
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_WRITEV,
            [a as Word, iov.as_ptr() as Word, 2, 0, 0, 0]
        ),
        EmulationAction::Return(3)
    );
    assert_eq!(
        send_rights(&mut r, a, &[event as i32], b"de"),
        EmulationAction::Return(2)
    );
    assert_eq!(send(&mut r, a, b"fghi"), EmulationAction::Return(4));
    let mut head = [0];
    let mut tail = [0; 16];
    let mut iov = [
        Iov {
            pointer: head.as_mut_ptr() as Word,
            len: 1,
        },
        Iov {
            pointer: tail.as_mut_ptr() as Word,
            len: tail.len(),
        },
    ];
    let mut control = [0; 24];
    let mut msg = header(&mut iov, &mut control);
    assert_eq!(
        call(
            &mut r,
            10,
            SYS_RECVMSG,
            [b as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
        ),
        EmulationAction::Return(5)
    );
    assert_eq!(&head, b"a");
    assert_eq!(&tail[..4], b"bcde");
    assert_eq!(msg.control_len, 24);
    assert_eq!(receive(&mut r, b, &mut tail), EmulationAction::Return(4));
    assert_eq!(&tail[..4], b"fghi");
}

#[test]
fn malformed_or_unsupported_control_headers_never_enqueue_data() {
    let mut r = Runtime::new();
    let [a, b] = pair(&mut r, 1 | LINUX_SOCK_NONBLOCK);
    for (length, level, kind, error) in [
        (8u64, 1u32, 1u32, EINVAL),
        (25, 1, 1, EINVAL),
        (17, 1, 1, EINVAL),
        (16, 2, 1, EOPNOTSUPP),
        (16, 1, 2, EOPNOTSUPP),
    ] {
        let mut control = [0; 24];
        control[..8].copy_from_slice(&length.to_ne_bytes());
        control[8..12].copy_from_slice(&level.to_ne_bytes());
        control[12..16].copy_from_slice(&kind.to_ne_bytes());
        let bytes = b"x";
        let mut iov = [Iov {
            pointer: bytes.as_ptr() as Word,
            len: 1,
        }];
        let mut msg = header(&mut iov, &mut control);
        assert_eq!(
            call(
                &mut r,
                10,
                SYS_SENDMSG,
                [a as Word, &mut msg as *mut _ as Word, 0, 0, 0, 0]
            ),
            EmulationAction::Return(-(error as isize))
        );
        assert_eq!(
            receive(&mut r, b, &mut [0]),
            EmulationAction::Return(-(EAGAIN as isize))
        );
    }
}
