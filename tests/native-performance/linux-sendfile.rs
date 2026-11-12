//! Production sendfile; service I/O and guest memory are deterministic mocks.
#![allow(dead_code)]
extern crate self as nanami_client;
extern crate self as nanami_runtime;
pub use std::{print, println};
type RequestError = i32;
type Word = usize;
mod state {
    pub const LINUX_FD_MAX: usize = 64;
    pub mod readiness {
        pub const READY_PIPE: usize = 8;
        pub const READY_PIPE_READ: usize = 2048;
        pub const READY_PIPE_WRITE: usize = 4096;
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const LINUX_PIPE_BYTES: usize = 4096;
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    Posix,
    Terminal,
    SocketTcp,
    SocketUdp,
    PipeRead,
    PipeWrite,
    VirtualFile,
    Empty,
}
enum VirtualNode {
    DevNull,
    DevZero,
}
impl VirtualNode {
    fn from_id(id: Word) -> Option<Self> {
        match id {
            1 => Some(Self::DevNull),
            2 => Some(Self::DevZero),
            _ => None,
        }
    }
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    resource: Word,
    posix_fd: Word,
    grant: Word,
    cursor: bool,
}
impl LinuxFile {
    fn posix_read_grant(&self) -> Word { self.grant }
    fn posix_read_cursor(&self) -> bool { self.cursor && self.grant != 0 }
    fn with_posix_read_grant(mut self, grant: Word) -> Self { self.grant=grant; self.cursor=false; self }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/file_reader.rs"]
mod file_reader;
struct Channel;
impl Channel {
    fn port(&mut self, _: Word) -> Option<Word> { Some(22) }
}
mod reader_scope {
    use super::*;
    mod production {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../nanami/servers/apps/alter/shared/src/common/state/file_reader.rs"));
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/pipe.rs"]
mod pipe_state;
use pipe_state::LinuxPipe as Pipe;
struct Runtime {
    file_reader: file_reader::Reader,
    rooted_channel: Channel,
    posix_direct_shm: Word,
    posix_port: Word,
    posix_shm: Word,
    terminal_port: Word,
    terminal_shm: Word,
    network_port: Word,
    network_shm: Word,
    network_shm_size: Word,
    input: LinuxFile,
    output: LinuxFile,
    pipe: Pipe,
    pipe_storage: Box<[u8; LINUX_PIPE_BYTES]>,
    readiness_changes: usize,
}
#[derive(Default)]
struct Backend {
    granted_reads: Vec<(Word, Word, Word)>,
    granted_stats: usize,
    proxy_stats: usize,
    grant_error: Option<i32>,
    oversized_read: Option<Word>,
    position: usize,
    data: Vec<u8>,
    sent: Vec<u8>,
    source: Word,
    max_write: usize,
    error: Option<i32>,
    flags: usize,
}
thread_local! { static BACKEND: std::cell::RefCell<Backend> = Default::default(); }
fn send(bytes: usize) -> Result<Word, i32> {
    BACKEND.with(|backend| {
        let mut b = backend.borrow_mut();
        if let Some(error) = b.error {
            return Err(error);
        }
        let bytes = bytes.min(b.max_write);
        let data = unsafe { std::slice::from_raw_parts(b.source as *const u8, bytes) };
        b.sent.extend_from_slice(data);
        Ok(bytes)
    })
}
impl Runtime {
    fn seek_posix(&self, fd: Word, offset: Word, whence: Word) -> Result<Word, i32> {
        self.file_reader.seek(fd, offset, whence).unwrap_or_else(|| posix::posix_seek(self.posix_port, fd, offset, whence))
    }
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        match fd {
            3 => Some(self.input),
            4 => Some(self.output),
            _ => None,
        }
    }
    fn posix_write_buffer(&self) -> (Word, Word) {
        (self.posix_shm, 64)
    }
    fn posix_read_buffer_size(&self) -> Word {
        64
    }
    fn pread_posix(&self, fd: Word, output: Word, len: Word, offset: Word) -> Result<(Word, Word), i32> {
        if let Some(Ok(bytes)) = self.file_reader.pread(fd, output, len, offset) {
            return Ok((bytes, self.posix_shm));
        }
        BACKEND.with(|backend| {
            let b = backend.borrow();
            let len = len.min(b.data.len().saturating_sub(offset));
            if len != 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        b.data.as_ptr().add(offset),
                        self.posix_shm as *mut u8,
                        len,
                    );
                }
            }
            Ok((len, self.posix_shm))
        })
    }
    fn write_posix(&self, _: Word, _: Word, len: Word, _: Option<Word>) -> Result<Word, i32> {
        send(len)
    }
    fn pipe_mut(&mut self, _: Word) -> Option<&mut Pipe> {
        Some(&mut self.pipe)
    }
}
mod posix {
    use super::*;
    pub const POSIX_FILE_TYPE_REGULAR: Word = 1;
    pub const POSIX_O_APPEND: Word = 8;
    pub const POSIX_SEEK_SET: Word = 0;
    pub const POSIX_SEEK_CUR: Word = 1;
    pub fn posix_fstat(_: Word, _: Word) -> Result<(Word, Word, Word, Word, Word), i32> {
        BACKEND.with(|b| b.borrow_mut().proxy_stats += 1);
        Ok((1, 100, POSIX_FILE_TYPE_REGULAR, 0, 0))
    }
    pub fn posix_fstat_flags(port: Word, fd: Word, _: Word) -> Result<(Word, Word, Word, Word, Word), i32> {
        posix_fstat(port, fd)
    }
    pub fn posix_fcntl_status_flags(_: Word, _: Word, _: Option<Word>) -> Result<Word, i32> {
        BACKEND.with(|b| Ok(b.borrow().flags))
    }
    pub fn posix_seek(_: Word, _: Word, offset: Word, whence: Word) -> Result<Word, i32> {
        BACKEND.with(|b| {
            let mut b = b.borrow_mut();
            if whence == POSIX_SEEK_SET {
                b.position = offset;
            } else {
                b.position += offset;
            }
            Ok(b.position)
        })
    }
}
mod vfs {
    use super::*;
    pub const VFS_FILE_TYPE_REGULAR: Word = 1;
    pub fn vfs_read_cursor_granted(port: Word, grant: Word, bytes: Word, output: Word) -> Result<Word, i32> {
        let position=BACKEND.with(|b|b.borrow().position);
        let read=vfs_pread_granted(port,grant,position,bytes,output)?;
        BACKEND.with(|b|b.borrow_mut().position=position+read);Ok(read)
    }
    pub fn vfs_seek_cursor_granted(port: Word, _: Word, offset: Word, whence: Word) -> Result<Word, i32> {
        posix::posix_seek(port,0,offset,whence)
    }
    pub fn vfs_fstat_granted(_: Word, _: Word, _: Word) -> Result<(Word, Word, Word), i32> {
        BACKEND.with(|b| {
            let mut b = b.borrow_mut(); b.granted_stats += 1;
            if let Some(error) = b.grant_error { return Err(error); }
            Ok((1, b.data.len(), VFS_FILE_TYPE_REGULAR))
        })
    }
    pub fn vfs_pread_granted(_: Word, grant: Word, offset: Word, bytes: Word, output: Word) -> Result<Word, i32> {
        BACKEND.with(|b| {
            let mut b = b.borrow_mut(); b.granted_reads.push((grant, offset, bytes));
            if let Some(error) = b.grant_error { return Err(error); }
            if let Some(bytes) = b.oversized_read { return Ok(bytes); }
            let bytes = bytes.min(b.data.len().saturating_sub(offset));
            if bytes != 0 {
                unsafe { std::ptr::copy_nonoverlapping(b.data.as_ptr().add(offset), (b.source + output) as *mut u8, bytes); }
            }
            Ok(bytes)
        })
    }
    pub fn vfs_read_versioned_granted(_: Word, _: Word, _: Word, _: Word) -> Result<(Word, Word), i32> { unreachable!() }
}
mod net {
    use super::*;
    pub fn net_service_tcp_send_on_connection(
        _: Word,
        _: Word,
        _: Word,
        len: Word,
        _: Word,
    ) -> Result<Word, i32> {
        send(len)
    }
}
mod terminal {
    use super::*;
    pub fn terminal_write_output(_: Word, _: Word, _: Word, len: Word) -> Result<Word, i32> {
        send(len)
    }
}
fn ensure_standard_terminal_fd(_: &mut Runtime, _: Word, _: Word) {}
fn map_request_error(error: i32) -> i32 {
    error
}
fn map_network_error(error: i32) -> i32 {
    error
}
fn terminal_id_for_pid(_: &Runtime, _: Word) -> Result<Word, i32> {
    Ok(1)
}
fn terminal_bounded_len(_: &Runtime, _: Word) -> Result<Word, i32> {
    Ok(64)
}
fn read_target_memory(runtime: &Runtime, _: Word, address: Word, len: Word) -> Result<(), i32> {
    if address < 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(address as *const u8, runtime.posix_shm as *mut u8, len);
    }
    Ok(())
}
fn write_target_memory(runtime: &Runtime, _: Word, address: Word, len: Word) -> Result<(), i32> {
    if address < 4096 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, address as *mut u8, len);
    }
    Ok(())
}
unsafe fn write_u64(address: Word, value: Word) {
    std::ptr::write_unaligned(address as *mut u64, value as u64);
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/sendfile.rs"]
mod production;
fn setup() -> (Box<[u8; 128]>, Runtime) {
    let mut memory = Box::new([0; 128]);
    let address = memory.as_mut_ptr() as Word;
    BACKEND.with(|b| {
        *b.borrow_mut() = Backend {
            data: (0..100).collect(),
            source: address,
            max_write: usize::MAX,
            ..Backend::default()
        }
    });
    let mut pipe_storage = Box::new([0; LINUX_PIPE_BYTES]);
    let runtime = Runtime {
        file_reader: file_reader::Reader::EMPTY,
        rooted_channel: Channel,
        posix_direct_shm: address,
        posix_port: 1,
        posix_shm: address,
        terminal_port: 2,
        terminal_shm: address,
        network_port: 3,
        network_shm: address,
        network_shm_size: 64,
        input: LinuxFile {
            kind: LinuxFileKind::Posix,
            resource: 0,
            posix_fd: 10,
            grant: 0,
            cursor: false,
        },
        output: LinuxFile {
            kind: LinuxFileKind::Posix,
            resource: 1,
            posix_fd: 11,
            grant: 0,
            cursor: false,
        },
        pipe: Pipe {
            active: true,
            buffer: pipe_storage.as_mut_ptr() as Word,
            read: 0,
            write: 0,
            len: 0,
            readers: 1,
            writers: 1,
        },
        pipe_storage,
        readiness_changes: 0,
    };
    (memory, runtime)
}

#[test]
fn sendfile_scopes_fresh_stat_and_pread_without_changing_short_write_cursor_rules() {
    let (_memory, mut r) = setup();
    r.input.grant = 71;
    r.file_reader = file_reader::Reader::new(90, 99, 22);
    BACKEND.with(|b| { let mut b = b.borrow_mut(); b.position = 7; b.max_write = 3; });
    assert_eq!(production::sys_sendfile(&mut r, 1, 4, 3, 0, 19), Ok(3));
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!((b.granted_stats, b.proxy_stats), (1, 0));
        assert_eq!(b.granted_reads, [(71, 7, 19)]);
        assert_eq!(b.sent, [7, 8, 9]);
        assert_eq!(b.position, 10);
    });
    assert!(r.file_reader.pread(10, 0, 1, 0).is_none());
    assert_eq!(r.file_reader.pread(90, 0, 1, 0), Some(Ok(1)));
    BACKEND.with(|b| assert_eq!(b.borrow().granted_reads.last().unwrap().0, 99));
}

#[test]
fn expired_sendfile_grant_uses_fresh_proxy_stat_and_same_positioned_offset() {
    let (_memory, mut r) = setup();
    r.input.grant = 71;
    let mut offset = 17u64;
    BACKEND.with(|b| b.borrow_mut().grant_error = Some(EIO));
    assert_eq!(production::sys_sendfile(&mut r, 1, 4, 3, &mut offset as *mut _ as Word, 5), Ok(5));
    assert_eq!(offset, 22);
    BACKEND.with(|b| {
        let b = b.borrow();
        assert_eq!((b.granted_stats, b.proxy_stats, b.position), (1, 1, 0));
        assert_eq!(b.granted_reads, [(71, 17, 5)]);
        assert_eq!(b.sent, [17, 18, 19, 20, 21]);
    });
}

#[test]
fn oversized_sendfile_reply_cannot_publish_bytes_or_advance_any_cursor() {
    let (_memory, mut r) = setup();
    r.input.grant = 71;
    BACKEND.with(|b| b.borrow_mut().oversized_read = Some(65));
    let mut offset = 17u64;
    assert_eq!(production::sys_sendfile(&mut r, 1, 4, 3, &mut offset as *mut _ as Word, 64), Err(EIO));
    assert_eq!(offset, 17);
    BACKEND.with(|b| { let b = b.borrow(); assert!(b.sent.is_empty()); assert_eq!(b.position, 0); });
}

#[test]
fn sendfile_pipe_burst_preserves_bytes_and_counts_readable_notifications() {
    let (_memory, mut r) = setup();
    r.output.kind = LinuxFileKind::PipeWrite;
    let mut notifications = 0;
    for _ in 0..20 {
        r.readiness_changes = 0x8000;
        assert_eq!(production::sys_sendfile(&mut r, 1, 4, 3, 0, 4), Ok(4));
        assert_ne!(r.readiness_changes & 0x8000, 0);
        notifications += usize::from(r.readiness_changes
            & (state::readiness::READY_PIPE | state::readiness::READY_PIPE_READ) != 0);
    }
    let mut output = [0u8; 80];
    r.pipe.copy_out(&mut output);
    assert_eq!(output, core::array::from_fn(|index| index as u8));
    BACKEND.with(|b| assert_eq!(b.borrow().position, 80));
    println!("sendfile-pipe-burst readable-notifications={notifications}");
}

#[test]
fn sendfile_pipe_wrap_copies_current_source_and_commits_only_accepted_bytes() {
    for start in [0, 1, LINUX_PIPE_BYTES - 1, LINUX_PIPE_BYTES - 17] {
        let (_memory, mut runtime) = setup();
        runtime.output.kind = LinuxFileKind::PipeWrite;
        runtime.pipe.read = start;
        runtime.pipe.write = start;
        assert_eq!(production::sys_sendfile(&mut runtime, 1, 4, 3, 0, 37), Ok(37));
        assert_eq!(runtime.pipe.len, 37);
        assert_eq!(runtime.pipe.write, (start + 37) % LINUX_PIPE_BYTES);
        let mut result = [0; 37];
        runtime.pipe.copy_out(&mut result);
        assert_eq!(result, core::array::from_fn(|i| i as u8));
        BACKEND.with(|b| assert_eq!(b.borrow().position, 37));
    }
}

#[test]
fn short_output_advances_input_only_by_bytes_sent() {
    let (_memory, mut runtime) = setup();
    BACKEND.with(|b| {
        b.borrow_mut().position = 7;
        b.borrow_mut().max_write = 3;
    });
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 0, 20),
        Ok(3)
    );
    BACKEND.with(|b| {
        assert_eq!(b.borrow().position, 10);
        assert_eq!(b.borrow().sent, [7, 8, 9]);
    });
}
#[test]
fn explicit_offset_is_updated_but_shared_file_position_is_not() {
    let (_memory, mut runtime) = setup();
    let mut offset: u64 = 9;
    BACKEND.with(|b| b.borrow_mut().max_write = 2);
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, &mut offset as *mut _ as Word, 20),
        Ok(2)
    );
    assert_eq!(offset, 11);
    BACKEND.with(|b| {
        assert_eq!(b.borrow().position, 0);
        assert_eq!(b.borrow().sent, [9, 10]);
    });
}
#[test]
fn output_error_does_not_consume_input_or_change_explicit_offset() {
    for kind in [
        LinuxFileKind::Posix,
        LinuxFileKind::Terminal,
        LinuxFileKind::SocketTcp,
    ] {
        let (_memory, mut runtime) = setup();
        runtime.output.kind = kind;
        BACKEND.with(|b| b.borrow_mut().error = Some(EAGAIN));
        let mut offset: u64 = 4;
        assert_eq!(
            production::sys_sendfile(&mut runtime, 1, 4, 3, &mut offset as *mut _ as Word, 20),
            Err(EAGAIN)
        );
        assert_eq!(offset, 4);
        assert_eq!(
            production::sys_sendfile(&mut runtime, 1, 4, 3, 0, 20),
            Err(EAGAIN)
        );
        BACKEND.with(|b| {
            assert_eq!(b.borrow().position, 0);
            assert!(b.borrow().sent.is_empty());
        });
    }
}
#[test]
fn bounded_calls_continue_until_eof_without_repeating_bytes() {
    let (_memory, mut runtime) = setup();
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 0, Word::MAX),
        Ok(64)
    );
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 0, Word::MAX),
        Ok(36)
    );
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 0, Word::MAX),
        Ok(0)
    );
    BACKEND.with(|b| assert_eq!(b.borrow().sent, b.borrow().data));
}
#[test]
fn append_and_bad_offset_fail_before_any_output() {
    let (_memory, mut runtime) = setup();
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 1, 20),
        Err(EFAULT)
    );
    let mut offset: u64 = u64::MAX;
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, &mut offset as *mut _ as Word, 20),
        Err(EINVAL)
    );
    BACKEND.with(|b| b.borrow_mut().flags = posix::POSIX_O_APPEND);
    assert_eq!(
        production::sys_sendfile(&mut runtime, 1, 4, 3, 0, 20),
        Err(EINVAL)
    );
    BACKEND.with(|b| assert!(b.borrow().sent.is_empty()));
}
