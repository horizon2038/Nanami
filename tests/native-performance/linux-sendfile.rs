//! Production sendfile; service I/O and guest memory are deterministic mocks.
#![allow(dead_code)]
extern crate self as nanami_client;
type Word = usize;
mod state {
    pub const LINUX_FD_MAX: usize = 64;
    pub mod readiness {
        pub const READY_PIPE: usize = 1;
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
}
struct Pipe {
    buffer: [u8; LINUX_PIPE_BYTES],
    write: usize,
    len: usize,
    readers: usize,
}
impl Pipe {
    fn write_byte(&mut self, index: usize, value: u8) {
        self.buffer[index] = value;
    }
}
struct Runtime {
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
    readiness_changes: usize,
}
#[derive(Default)]
struct Backend {
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
    fn pread_posix(&self, _: Word, _: Word, len: Word, offset: Word) -> Result<(Word, Word), i32> {
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
        Ok((1, 100, POSIX_FILE_TYPE_REGULAR, 0, 0))
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
    let runtime = Runtime {
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
        },
        output: LinuxFile {
            kind: LinuxFileKind::Posix,
            resource: 1,
            posix_fd: 11,
        },
        pipe: Pipe {
            buffer: [0; LINUX_PIPE_BYTES],
            write: 0,
            len: 0,
            readers: 1,
        },
        readiness_changes: 0,
    };
    (memory, runtime)
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
