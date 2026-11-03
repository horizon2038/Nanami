//! Production Linux directory packing and mount selection; only IPC is mocked.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
pub type Word = usize;
mod arch {
    pub const CPU_INFO: &[u8] = b"test CPU";
    pub const PROC_VERSION: &[u8] = b"test kernel";
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/directory.rs"]
mod directory;
#[path = "../../nanami/servers/apps/alter/shared/src/common/virtual_fs.rs"]
mod virtual_fs;
mod vfs {
    pub use nanami_uapi::vfs::*;
}
const ALTER_IO_OFFSET: usize = 512;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
const ENOTDIR: i32 = 20;
const LINUX_DIRENT64_NAME_OFFSET: usize = 19;
const LINUX_DT_UNKNOWN: usize = 0;
const LINUX_DT_DIR: usize = 4;
const LINUX_DT_REG: usize = 8;
const LINUX_DT_BLK: usize = 6;
const LINUX_DT_CHR: usize = 2;
const LINUX_DT_SOCK: usize = 12;

#[test]
fn legacy_virtual_devices_are_exposed_only_with_a_graphics_grant() {
    use virtual_fs::{lookup, VirtualNode};
    for name in [
        b"/dev/tty0".as_slice(),
        b"/dev/tty1",
        b"/dev/console",
        b"/dev/input/mice",
        b"/dev/psaux",
        b"/dev/mouse",
    ] {
        assert!(!matches!(
            lookup(name, false),
            Some(VirtualNode::DevConsole | VirtualNode::DevPs2Mouse)
        ));
        let node = lookup(name, true).unwrap();
        assert!(matches!(
            node,
            VirtualNode::DevConsole | VirtualNode::DevPs2Mouse
        ));
        assert!(VirtualNode::from_id(node.id()) == Some(node));
    }
    assert!(lookup(b"/dev/fb0", false).is_none());
    assert!(lookup(b"/dev/fb0", true) == Some(VirtualNode::DevFramebuffer));
    assert!(lookup(b"/dev/tty", false) == Some(VirtualNode::DevTty));
}

#[derive(Clone, Copy, PartialEq)]
enum LinuxFileKind {
    VirtualDirectory,
    Posix,
}
#[derive(Clone, Copy)]
struct LinuxFile {
    kind: LinuxFileKind,
    posix_fd: Word,
}
impl LinuxFile {
    fn is_root_directory(self) -> bool {
        self.kind == LinuxFileKind::VirtualDirectory
    }
}
struct Rootfs {
    root_len: usize,
}
struct Process {
    rootfs: Rootfs,
}
struct Runtime {
    posix_port: Word,
    posix_shm: Word,
    posix_shm_size: Word,
    process: Process,
    file: LinuxFile,
    copy_error: Option<i32>,
    _buffer: Box<[u8; 65536]>,
}
impl Runtime {
    fn linux_file(&self, _: Word, fd: Word) -> Option<LinuxFile> {
        (fd < 2).then_some(self.file)
    }
    fn managed_process(&self, _: Word) -> Option<&Process> {
        Some(&self.process)
    }
}
#[derive(Default)]
struct Backend {
    names: Vec<String>,
    cursor: usize,
    shm: usize,
    reads: usize,
}
thread_local! {
    static BACKEND: RefCell<Backend> = RefCell::new(Backend::default());
}
mod posix {
    use super::*;
    pub use nanami_uapi::posix::*;
    pub fn posix_seek(_: Word, _: Word, offset: Word, whence: Word) -> Result<Word, i32> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            match whence {
                POSIX_SEEK_SET => backend.cursor = offset,
                POSIX_SEEK_CUR => backend.cursor += offset,
                _ => return Err(EINVAL),
            }
            Ok(backend.cursor)
        })
    }
    pub fn posix_read_dir(
        _: Word,
        _: Word,
        count: Word,
        offset: Word,
    ) -> Result<(Word, Word), i32> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            backend.reads += 1;
            let end = (backend.cursor + count).min(backend.names.len());
            for (record, index) in (backend.cursor..end).enumerate() {
                let name = backend.names[index].as_bytes();
                let entry = backend.shm + offset + record * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
                unsafe {
                    (entry as *mut Word).write_unaligned(100 + index);
                    ((entry + vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET) as *mut Word)
                        .write_unaligned(POSIX_FILE_TYPE_DIRECTORY);
                    ((entry + vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET) as *mut Word)
                        .write_unaligned(name.len());
                    std::ptr::copy_nonoverlapping(
                        name.as_ptr(),
                        (entry + vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET) as *mut u8,
                        name.len(),
                    );
                }
            }
            let count = end - backend.cursor;
            backend.cursor = end;
            Ok((count, end))
        })
    }
}
fn align_up_word(value: Word, align: Word) -> Word {
    (value + align - 1) & !(align - 1)
}
fn map_request_error(error: i32) -> i32 {
    error
}
fn write_u16(address: Word, value: u16) {
    unsafe { (address as *mut u16).write_unaligned(value) }
}
fn write_u64(address: Word, value: Word) {
    unsafe { (address as *mut u64).write_unaligned(value as u64) }
}
fn write_target_memory(
    runtime: &Runtime,
    _: Word,
    destination: Word,
    count: Word,
) -> Result<(), i32> {
    if let Some(error) = runtime.copy_error {
        return Err(error);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            destination as *mut u8,
            count,
        );
    }
    Ok(())
}
fn sys_virtual_getdents(_: &mut Runtime, _: Word, _: Word, _: Word, _: Word) -> Result<Word, i32> {
    panic!("the root must use its shared POSIX directory cursor")
}
fn fixture(private_root: bool, names: Vec<&str>) -> Runtime {
    let mut buffer = Box::new([0; 65536]);
    let shm = buffer.as_mut_ptr() as usize;
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Backend {
            names: names.into_iter().map(str::to_owned).collect(),
            shm,
            cursor: 0,
            reads: 0,
        }
    });
    Runtime {
        posix_port: 1,
        posix_shm: shm,
        posix_shm_size: buffer.len(),
        process: Process {
            rootfs: Rootfs {
                root_len: usize::from(private_root),
            },
        },
        file: LinuxFile {
            kind: LinuxFileKind::VirtualDirectory,
            posix_fd: 4,
        },
        copy_error: None,
        _buffer: buffer,
    }
}
fn read(runtime: &mut Runtime, fd: Word, capacity: usize) -> Vec<(String, Word, Word)> {
    let mut output = vec![0u8; capacity];
    let count =
        directory::sys_getdents64(runtime, 1, fd, output.as_mut_ptr() as Word, capacity).unwrap();
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < count {
        let record = &output[offset..count];
        let bytes = u16::from_ne_bytes(record[16..18].try_into().unwrap()) as usize;
        assert!(bytes >= 24 && bytes % 8 == 0 && bytes <= record.len());
        let name = record[19..bytes].split(|byte| *byte == 0).next().unwrap();
        assert!(record[19 + name.len()..bytes].iter().all(|byte| *byte == 0));
        assert_eq!(record[18], LINUX_DT_DIR as u8);
        result.push((
            std::str::from_utf8(name).unwrap().to_owned(),
            u64::from_ne_bytes(record[..8].try_into().unwrap()) as Word,
            u64::from_ne_bytes(record[8..16].try_into().unwrap()) as Word,
        ));
        offset += bytes;
    }
    result
}
fn all(runtime: &mut Runtime) -> Vec<(String, Word, Word)> {
    let mut result = Vec::new();
    for _ in 0..100 {
        let batch = read(runtime, 0, 32);
        if batch.is_empty() {
            return result;
        }
        result.extend(batch);
    }
    panic!("directory did not reach EOF")
}
#[test]
fn private_root_lists_real_entries_without_a_phantom_temp_alias() {
    let mut runtime = fixture(
        true,
        vec![".", "..", "dev", "proc", "sys", "bin", "etc", "tmp"],
    );
    let entries = all(&mut runtime);
    let names: Vec<_> = entries.iter().map(|entry| entry.0.as_str()).collect();
    assert_eq!(
        names,
        ["dev", "proc", "sys", ".", "..", "bin", "etc", "tmp"]
    );
    assert!(read(&mut runtime, 0, 32).is_empty());
}
#[test]
fn private_temp_is_not_shadowed_but_the_legacy_alias_is_retained() {
    let mut private = fixture(true, vec!["dev", "proc", "sys", "temp", "file"]);
    let entries = all(&mut private);
    assert_eq!(
        entries.iter().find(|entry| entry.0 == "temp").unwrap().1,
        103
    );
    let mut legacy = fixture(false, vec!["dev", "proc", "sys", "temp", "file"]);
    let entries = all(&mut legacy);
    let temp: Vec<_> = entries.iter().filter(|entry| entry.0 == "temp").collect();
    assert_eq!(temp.len(), 1);
    assert_eq!(temp[0].1, virtual_fs::VirtualNode::Temp.id());
}
#[test]
fn root_cookie_rewind_and_dup_keep_the_same_stream_for_both_profiles() {
    for private in [false, true] {
        let mut runtime = fixture(private, vec![".", "..", "dev", "proc", "sys", "bin", "tmp"]);
        let first = read(&mut runtime, 0, 32);
        assert_eq!(first[0].0, "dev");
        assert_eq!(read(&mut runtime, 1, 32)[0].0, "proc");
        posix::posix_seek(1, 4, first[0].2, posix::POSIX_SEEK_SET).unwrap();
        assert_eq!(read(&mut runtime, 0, 32)[0].0, "proc");
        posix::posix_seek(1, 4, 0, posix::POSIX_SEEK_SET).unwrap();
        let expected = all(&mut runtime);
        for (index, entry) in expected.iter().enumerate() {
            posix::posix_seek(1, 4, entry.2, posix::POSIX_SEEK_SET).unwrap();
            assert_eq!(all(&mut runtime), expected[index + 1..]);
        }
    }
}

#[test]
fn large_directory_reads_compact_forward_without_clobbering_unread_records() {
    let names: Vec<_> = (0..300).map(|index| format!("entry-{index:03}")).collect();
    let mut runtime = fixture(true, names.iter().map(String::as_str).collect());
    runtime.file.kind = LinuxFileKind::Posix;
    let first = read(&mut runtime, 0, 32768);
    // Reserve a worst-case packed record for each backend entry. A large
    // caller buffer must no longer be limited to the old 512-byte prefix.
    assert_eq!(first.len(), 32768 / 280);
    let mut entries = first;
    loop {
        let batch = read(&mut runtime, 1, 32768);
        if batch.is_empty() {
            break;
        }
        entries.extend(batch);
    }
    for (index, (name, inode, cookie)) in entries.iter().enumerate() {
        assert_eq!(name, &names[index]);
        assert_eq!(*inode, 100 + index);
        assert_eq!(*cookie, index + 1);
    }
    assert_eq!(entries.len(), names.len());
    BACKEND.with(|backend| assert_eq!(backend.borrow().reads, 4));
}

#[test]
fn maximum_length_directory_names_and_short_buffers_preserve_every_cookie() {
    let names: Vec<_> = (0..240)
        .map(|index| format!("{index:03}{}", "x".repeat(252)))
        .collect();
    let mut runtime = fixture(true, names.iter().map(String::as_str).collect());
    runtime.file.kind = LinuxFileKind::Posix;
    let first = read(&mut runtime, 0, 32768);
    assert_eq!(first.len(), 117);
    assert_eq!(first.last().unwrap().2, 117);
    // A buffer too short for the next record must not consume its cookie.
    let mut output = [0u8; 32];
    assert_eq!(
        directory::sys_getdents64(&mut runtime, 1, 1, output.as_mut_ptr() as Word, 32),
        Err(EINVAL)
    );
    BACKEND.with(|backend| assert_eq!(backend.borrow().cursor, 117));
    let next = read(&mut runtime, 1, 280);
    assert_eq!(next, vec![(names[117].clone(), 217, 118)]);
    posix::posix_seek(1, 4, first[10].2, posix::POSIX_SEEK_SET).unwrap();
    assert_eq!(read(&mut runtime, 0, 280)[0].0, names[11]);
}

#[test]
fn failed_large_directory_copyout_restores_the_shared_stream() {
    for root in [false, true] {
        let names: Vec<_> = (0..150).map(|index| format!("file-{index}")).collect();
        let mut runtime = fixture(true, names.iter().map(String::as_str).collect());
        if !root {
            runtime.file.kind = LinuxFileKind::Posix;
        }
        let prefix = if root {
            let entries = read(&mut runtime, 0, 512);
            assert_eq!(entries.len(), 3);
            3
        } else {
            0
        };
        runtime.copy_error = Some(EFAULT);
        let mut output = vec![0u8; 32768];
        assert_eq!(
            directory::sys_getdents64(
                &mut runtime,
                1,
                0,
                output.as_mut_ptr() as Word,
                output.len()
            ),
            Err(EFAULT)
        );
        BACKEND.with(|backend| assert_eq!(backend.borrow().cursor, prefix));
        runtime.copy_error = None;
        let retry = read(&mut runtime, 1, 32768);
        assert_eq!(retry.len(), 117);
        assert_eq!(retry[0], (names[0].clone(), 100, prefix + 1));
        assert_eq!(retry[116].0, names[116]);
    }
}

#[test]
fn large_root_batches_keep_overlay_shadowing_and_seek_cookies() {
    let mut names: Vec<_> = (0..130).map(|index| format!("dir-{index:03}")).collect();
    names[0] = "dev".into();
    names[30] = "proc".into();
    names[116] = "sys".into();
    let mut runtime = fixture(true, names.iter().map(String::as_str).collect());
    let prefix = read(&mut runtime, 0, 32768);
    assert_eq!(
        prefix.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
        ["dev", "proc", "sys"]
    );
    let first = read(&mut runtime, 1, 32768);
    assert_eq!(first.len(), 114);
    assert_eq!(first.last().unwrap().2, 119);
    posix::posix_seek(1, 4, first[50].2, posix::POSIX_SEEK_SET).unwrap();
    assert_eq!(read(&mut runtime, 0, 32)[0], first[51]);
    // Return to the batch boundary whose last backing record was shadowed.
    posix::posix_seek(1, 4, 120, posix::POSIX_SEEK_SET).unwrap();
    assert_eq!(read(&mut runtime, 1, 32768)[0].0, names[117]);
}

#[test]
fn oversized_guest_count_still_obeys_the_actual_shared_memory_extent() {
    let names = ["a".repeat(255), "b".repeat(255), "c".repeat(255)];
    let mut runtime = fixture(true, names.iter().map(String::as_str).collect());
    runtime.file.kind = LinuxFileKind::Posix;
    runtime.posix_shm_size = ALTER_IO_OFFSET + 2 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let end = runtime.posix_shm_size;
    runtime._buffer[end..].fill(0xa5);
    let mut output = [0xa5u8; 4096];
    let copied =
        directory::sys_getdents64(&mut runtime, 1, 0, output.as_mut_ptr() as Word, Word::MAX)
            .unwrap();
    assert_eq!(copied, 560);
    assert_eq!(&output[19..274], names[0].as_bytes());
    assert_eq!(&output[299..554], names[1].as_bytes());
    assert!(output[copied..].iter().all(|byte| *byte == 0xa5));
    assert!(runtime._buffer[end..].iter().all(|byte| *byte == 0xa5));
    BACKEND.with(|backend| assert_eq!(backend.borrow().cursor, 2));
}
