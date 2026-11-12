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
        pub const READY_PIPE: usize = 8;
        pub const READY_PIPE_READ: usize = 2048;
        pub const READY_PIPE_WRITE: usize = 4096;
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
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/pipe.rs"]
mod pipe_state;
use pipe_state::LinuxPipe as Pipe;
struct Runtime {
    pipe: Option<Pipe>,
    pipe_storage: Option<Box<[u8; LINUX_PIPE_BYTES]>>,
    files: [LinuxFile; 4],
    scratch: Box<[u8; 8192]>,
    posix_shm: Word,
    readiness_changes: Word,
    fail_copy: bool,
    partial_copy: usize,
    releases: usize,
}
impl Runtime {
    fn pipe(&self, id: Word) -> Option<Pipe> { if id == 0 { self.pipe } else { None } }
    fn new() -> Self {
        let mut scratch = Box::new([0; 8192]);
        let posix_shm = scratch.as_mut_ptr() as Word;
        Self {
            pipe: None,
            pipe_storage: None,
            files: [LinuxFile::EMPTY; 4],
            scratch,
            posix_shm,
            readiness_changes: 0,
            fail_copy: false,
            partial_copy: 0,
            releases: 0,
        }
    }
    fn allocate_pipe(&mut self) -> Option<Word> {
        if self.pipe.is_some() {
            return None;
        }
        // Model reused dirty allocator storage: no read may depend on zeros.
        let mut data = Box::new([0xa5; LINUX_PIPE_BYTES]);
        self.pipe = Some(Pipe {
            active: true,
            buffer: data.as_mut_ptr() as Word,
            readers: 1,
            writers: 1,
            read: 0,
            write: 0,
            len: 0,
        });
        self.pipe_storage = Some(data);
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
        self.pipe_storage = None;
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
mod guest_memory {
    use super::*;
    pub(super) fn copyout_owned(runtime: &Runtime, _: Word, address: Word, source: &[u8]) -> Result<(), i32> {
        if runtime.fail_copy { return Err(EFAULT); }
        unsafe { core::ptr::copy_nonoverlapping(source.as_ptr(), address as *mut u8, source.len()); }
        Ok(())
    }
    pub(super) fn copyin_owned(runtime: &Runtime, _: Word, address: Word, output: &mut [core::mem::MaybeUninit<u8>]) -> Result<(), i32> {
        if runtime.fail_copy {
            let prefix = runtime.partial_copy.min(output.len());
            unsafe { core::ptr::copy_nonoverlapping(address as *const u8, output.as_mut_ptr().cast::<u8>(), prefix); }
            return Err(EFAULT);
        }
        unsafe { core::ptr::copy_nonoverlapping(address as *const u8, output.as_mut_ptr().cast::<u8>(), output.len()); }
        Ok(())
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/pipe.rs"]
mod pipe;

#[test]
fn contiguous_pipe_io_avoids_scratch_and_wrap_keeps_original_staging() {
    for start in [0, 31, LINUX_PIPE_BYTES - 32, LINUX_PIPE_BYTES - 7] {
        let mut r = Runtime::new(); r.allocate_pipe().unwrap(); r.scratch.fill(0xdd);
        let p = r.pipe.as_mut().unwrap(); p.read = start; p.write = start;
        let source = [0x37u8; 32]; let mut output = [0xa5; 32];
        assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, source.as_ptr() as Word, source.len()), Ok(32));
        assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, output.as_mut_ptr() as Word, output.len()), Ok(32));
        assert_eq!(output, source); assert_eq!(r.pipe.unwrap().len, 0);
        assert_eq!(r.pipe.unwrap().read, (start + 32) % LINUX_PIPE_BYTES);
        if start <= LINUX_PIPE_BYTES - 32 { assert!(r.scratch.iter().all(|b| *b == 0xdd)); }
        else { assert_eq!(&r.scratch[..32], &source); assert!(r.scratch[32..].iter().all(|b| *b == 0xdd)); }
    }
}

#[test]
fn failed_direct_copyin_never_publishes_free_prefix_or_changes_live_bytes() {
    let mut r = Runtime::new(); r.allocate_pipe().unwrap();
    let queued = [0x22u8; 4];
    assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, queued.as_ptr() as Word, 4), Ok(4));
    let before = r.pipe.unwrap(); r.readiness_changes = 0; r.fail_copy = true; r.partial_copy = 5;
    let rejected = [0x7au8; 32];
    assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, rejected.as_ptr() as Word, 32), Err(EFAULT));
    let after = r.pipe.unwrap(); assert_eq!((after.read, after.write, after.len), (before.read, before.write, before.len));
    assert_eq!(r.readiness_changes, 0); r.fail_copy = false;
    let mut actual = [0; 4];
    assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, actual.as_mut_ptr() as Word, 4), Ok(4)); assert_eq!(actual, queued);
    let replacement = [0x19u8; 8]; let mut output = [0; 8];
    assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, replacement.as_ptr() as Word, 8), Ok(8));
    assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, output.as_mut_ptr() as Word, 8), Ok(8)); assert_eq!(output, replacement);
}

#[test]
fn pipe_write_burst_preserves_bytes_and_counts_readable_notifications() {
    let mut r = Runtime::new();
    r.allocate_pipe().unwrap();
    let mut notifications = 0;
    for sequence in 0..128u64 {
        r.readiness_changes = 0x8000;
        let bytes = sequence.to_ne_bytes();
        assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, bytes.as_ptr() as Word, bytes.len()), Ok(8));
        assert_ne!(r.readiness_changes & 0x8000, 0);
        notifications += usize::from(r.readiness_changes
            & (state::readiness::READY_PIPE | state::readiness::READY_PIPE_READ) != 0);
    }
    let mut output = [0u8; 128 * 8];
    r.readiness_changes = 0;
    assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, output.as_mut_ptr() as Word, output.len()), Ok(output.len()));
    assert_ne!(r.readiness_changes & (state::readiness::READY_PIPE | state::readiness::READY_PIPE_WRITE), 0);
    for (sequence, bytes) in output.chunks_exact(8).enumerate() {
        assert_eq!(bytes, (sequence as u64).to_ne_bytes());
    }
    println!("pipe-burst readable-notifications={notifications}");
}

#[test]
fn fresh_dirty_pipe_exposes_only_successfully_written_bytes() {
    let mut r = Runtime::new();
    for _ in 0..2 {
        r.allocate_pipe().unwrap();
        let mut target = [0x7bu8; 32];
        assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, target.as_mut_ptr() as Word, target.len()), Err(EAGAIN));
        assert_eq!(target, [0x7b; 32]);
        let input = [0u8, 1, 2, 0xfe, 0xff];
        r.fail_copy = true;
        assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, input.as_ptr() as Word, input.len()), Err(EFAULT));
        r.fail_copy = false;
        assert_eq!(r.pipe.unwrap().len, 0);
        assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, target.as_mut_ptr() as Word, target.len()), Err(EAGAIN));
        assert_eq!(pipe::sys_pipe_write(&mut r, 1, 0, input.as_ptr() as Word, input.len()), Ok(input.len()));
        assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, target.as_mut_ptr() as Word, target.len()), Ok(input.len()));
        assert_eq!(&target[..input.len()], &input);
        assert_eq!(&target[input.len()..], &[0x7b; 27]);
        r.pipe.as_mut().unwrap().writers = 0;
        assert_eq!(pipe::sys_pipe_read(&mut r, 1, 0, target.as_mut_ptr() as Word, target.len()), Ok(0));
        assert_eq!(&target[..input.len()], &input);
        assert_eq!(&target[input.len()..], &[0x7b; 27]);
        r.release_pipe(0);
    }
}

#[test]
fn bulk_ring_matches_byte_queue_through_wrap_full_capacity_and_copy_faults() {
    use std::collections::VecDeque;
    let mut runtime = Runtime::new();
    runtime.allocate_pipe().unwrap();
    runtime.pipe.as_mut().unwrap().read = LINUX_PIPE_BYTES - 1;
    runtime.pipe.as_mut().unwrap().write = LINUX_PIPE_BYTES - 1;
    let mut queue = VecDeque::new();
    let mut seed = 17u64;
    for step in 0..5000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let count = [0, 1, 2, 7, 63, 1023, 4095, 4096, 4097][(seed >> 32) as usize % 9];
        let before = runtime.pipe.unwrap();
        let old_bytes = **runtime.pipe_storage.as_ref().unwrap();
        runtime.readiness_changes = 0;
        runtime.fail_copy = step % 13 == 0;
        if seed & 1 == 0 {
            let source: Vec<_> = (0..count).map(|i| (i + step) as u8).collect();
            let available = LINUX_PIPE_BYTES - queue.len();
            let expected = if count == 0 { Ok(0) }
                else if available == 0 || (count <= LINUX_PIPE_BYTES && available < count) { Err(EAGAIN) }
                else if runtime.fail_copy { Err(EFAULT) }
                else { Ok(count.min(available)) };
            let result = pipe::sys_pipe_write(&mut runtime, 1, 0, source.as_ptr() as Word, count);
            assert_eq!(result, expected);
            if let Ok(n) = result { queue.extend(&source[..n]); }
        } else {
            let mut target = vec![0xa5; count + 2];
            let expected = if count == 0 { Ok(0) } else if queue.is_empty() { Err(EAGAIN) }
                else if runtime.fail_copy { Err(EFAULT) } else { Ok(count.min(queue.len())) };
            let result = pipe::sys_pipe_read(&mut runtime, 1, 0, target[1..].as_mut_ptr() as Word, count);
            assert_eq!(result, expected);
            if let Ok(n) = result {
                for byte in &target[1..1 + n] { assert_eq!(Some(*byte), queue.pop_front()); }
                assert!(target[1 + n..].iter().all(|b| *b == 0xa5));
            } else { assert!(target.iter().all(|b| *b == 0xa5)); }
            assert_eq!(target[0], 0xa5);
        }
        let after = runtime.pipe.unwrap();
        assert_eq!(after.len, queue.len());
        assert_eq!(after.write, (after.read + after.len) % LINUX_PIPE_BYTES);
        if runtime.fail_copy {
            assert_eq!((after.read, after.write, after.len), (before.read, before.write, before.len));
            assert_eq!(**runtime.pipe_storage.as_ref().unwrap(), old_bytes);
            assert_eq!(runtime.readiness_changes, 0);
        }
        let mut staged = vec![0; queue.len()];
        after.copy_out(&mut staged);
        assert!(staged.iter().copied().eq(queue.iter().copied()));
    }
}

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
