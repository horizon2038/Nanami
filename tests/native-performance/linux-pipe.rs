//! Production pipe I/O with deterministic guest-memory and descriptor mocks.
#![allow(dead_code)]
type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const LINUX_PIPE_BYTES: usize = 4096;
mod state {
    pub const LINUX_FD_MAX: usize = 64;
    pub mod readiness {
        pub const READY_PIPE: usize = 1;
    }
}
#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    Empty,
    PipeRead,
    PipeWrite,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
    flags: Word,
}
impl LinuxFile {
    const EMPTY: Self = Self {
        kind: LinuxFileKind::Empty,
        posix_fd: 0,
        flags: 0,
    };
    fn pipe_read(id: Word, flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::PipeRead,
            posix_fd: id,
            flags,
        }
    }
    fn pipe_write(id: Word, flags: Word) -> Self {
        Self {
            kind: LinuxFileKind::PipeWrite,
            posix_fd: id,
            flags,
        }
    }
}
struct Pipe {
    data: Box<[u8; LINUX_PIPE_BYTES]>,
    readers: Word,
    writers: Word,
    read: usize,
    write: usize,
    len: usize,
}
impl Pipe {
    fn read_byte(&self, index: usize) -> u8 {
        self.data[index]
    }
    fn write_byte(&mut self, index: usize, byte: u8) {
        self.data[index] = byte;
    }
}
struct Runtime {
    pipe: Option<Pipe>,
    files: [LinuxFile; 4],
    scratch: Box<[u8; 8192]>,
    posix_shm: Word,
    readiness_changes: Word,
    fail_copy: bool,
    releases: usize,
}
impl Runtime {
    fn new() -> Self {
        let mut scratch = Box::new([0; 8192]);
        let posix_shm = scratch.as_mut_ptr() as Word;
        Self {
            pipe: None,
            files: [LinuxFile::EMPTY; 4],
            scratch,
            posix_shm,
            readiness_changes: 0,
            fail_copy: false,
            releases: 0,
        }
    }
    fn allocate_pipe(&mut self) -> Option<Word> {
        if self.pipe.is_some() {
            return None;
        }
        self.pipe = Some(Pipe {
            data: Box::new([0; LINUX_PIPE_BYTES]),
            readers: 1,
            writers: 1,
            read: 0,
            write: 0,
            len: 0,
        });
        Some(0)
    }
    fn pipe_mut(&mut self, id: Word) -> Option<&mut Pipe> {
        if id != 0 {
            return None;
        }
        self.pipe.as_mut()
    }
    fn release_pipe(&mut self, _: Word) {
        assert!(self.pipe.take().is_some());
        self.releases += 1;
    }
    fn allocate_linux_file(&mut self, _: Word, file: LinuxFile, _: Word) -> Option<Word> {
        let index = self
            .files
            .iter()
            .position(|entry| entry.kind == LinuxFileKind::Empty)?;
        self.files[index] = file;
        Some(index)
    }
    fn clear_linux_file(&mut self, _: Word, fd: Word) -> Option<LinuxFile> {
        Some(core::mem::replace(
            self.files.get_mut(fd)?,
            LinuxFile::EMPTY,
        ))
    }
}
fn bounded_len(runtime: &Runtime, bytes: Word) -> Result<Word, i32> {
    if bytes > runtime.scratch.len() {
        return Err(EINVAL);
    }
    Ok(bytes)
}
fn read_target_memory(
    runtime: &mut Runtime,
    _: Word,
    address: Word,
    bytes: Word,
) -> Result<(), i32> {
    if runtime.fail_copy {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(address as *const u8, runtime.posix_shm as *mut u8, bytes);
    }
    Ok(())
}
fn write_target_memory(
    runtime: &mut Runtime,
    _: Word,
    address: Word,
    bytes: Word,
) -> Result<(), i32> {
    if runtime.fail_copy {
        return Err(EFAULT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, address as *mut u8, bytes);
    }
    Ok(())
}
unsafe fn write_u32(address: Word, value: u32) {
    (address as *mut u32).write_unaligned(value);
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/pipe.rs"]
mod pipe;

#[test]
fn live_writer_is_not_eof() {
    let mut runtime = Runtime::new();
    runtime.allocate_pipe().unwrap();
    let mut bytes = [0; 8];
    assert_eq!(
        pipe::sys_pipe_read(&mut runtime, 1, 0, bytes.as_mut_ptr() as Word, 8),
        Err(EAGAIN)
    );
    runtime.pipe.as_mut().unwrap().writers = 0;
    assert_eq!(
        pipe::sys_pipe_read(&mut runtime, 1, 0, bytes.as_mut_ptr() as Word, 8),
        Ok(0)
    );
}

#[test]
fn circular_buffer_preserves_bytes_and_failed_reads_do_not_consume_them() {
    let mut runtime = Runtime::new();
    runtime.allocate_pipe().unwrap();
    let metadata = runtime.pipe.as_mut().unwrap();
    metadata.read = LINUX_PIPE_BYTES - 2;
    metadata.write = metadata.read;
    let source = b"pipe bytes";
    assert_eq!(
        pipe::sys_pipe_write(&mut runtime, 1, 0, source.as_ptr() as Word, source.len()),
        Ok(source.len())
    );
    let mut output = [0; 10];
    runtime.fail_copy = true;
    assert_eq!(
        pipe::sys_pipe_read(
            &mut runtime,
            1,
            0,
            output.as_mut_ptr() as Word,
            output.len()
        ),
        Err(EFAULT)
    );
    assert_eq!(runtime.pipe.as_ref().unwrap().len, source.len());
    runtime.fail_copy = false;
    assert_eq!(
        pipe::sys_pipe_read(
            &mut runtime,
            1,
            0,
            output.as_mut_ptr() as Word,
            output.len()
        ),
        Ok(source.len())
    );
    assert_eq!(&output, source);
    assert_ne!(runtime.readiness_changes, 0);
}

#[test]
fn atomic_writes_wait_but_large_writes_can_be_short() {
    let mut runtime = Runtime::new();
    runtime.allocate_pipe().unwrap();
    runtime.pipe.as_mut().unwrap().len = LINUX_PIPE_BYTES - 4;
    let source = [1; LINUX_PIPE_BYTES + 1];
    assert_eq!(
        pipe::sys_pipe_write(&mut runtime, 1, 0, source.as_ptr() as Word, 8),
        Err(EAGAIN)
    );
    assert_eq!(
        pipe::sys_pipe_write(&mut runtime, 1, 0, source.as_ptr() as Word, source.len()),
        Ok(4)
    );
    runtime.pipe.as_mut().unwrap().readers = 0;
    assert_eq!(
        pipe::sys_pipe_write(&mut runtime, 1, 0, source.as_ptr() as Word, 1),
        Err(EPIPE)
    );
}

#[test]
fn failed_pipe_copyout_rolls_back_both_endpoints_and_the_buffer() {
    let mut runtime = Runtime::new();
    runtime.fail_copy = true;
    let mut fds = [0u32; 2];
    assert_eq!(
        pipe::sys_pipe(&mut runtime, 1, fds.as_mut_ptr() as Word, 0),
        Err(EFAULT)
    );
    assert!(runtime
        .files
        .iter()
        .all(|file| file.kind == LinuxFileKind::Empty));
    assert!(runtime.pipe.is_none());
    assert_eq!(runtime.releases, 1);
}

#[test]
fn last_endpoint_releases_the_buffer_once() {
    let mut runtime = Runtime::new();
    runtime.allocate_pipe().unwrap();
    pipe::release_pipe_file(&mut runtime, LinuxFile::pipe_read(0, 0));
    assert!(runtime.pipe.is_some());
    pipe::release_pipe_file(&mut runtime, LinuxFile::pipe_write(0, 0));
    assert!(runtime.pipe.is_none());
    assert_eq!(runtime.releases, 1);
}
