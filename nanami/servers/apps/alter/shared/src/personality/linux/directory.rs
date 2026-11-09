//! Linux dirent64 packing and the guest-root mount overlay.
use super::{
    align_up_word, map_request_error, posix, sys_virtual_getdents, vfs, virtual_fs,
    write_target_memory, write_u16, write_u64, LinuxFile, LinuxFileKind, Runtime, Word,
    ALTER_IO_OFFSET, EBADF, EFAULT, EINVAL, EIO, ENOTDIR, LINUX_DIRENT64_NAME_OFFSET, LINUX_DT_BLK,
    LINUX_DT_CHR, LINUX_DT_DIR, LINUX_DT_REG, LINUX_DT_SOCK, LINUX_DT_UNKNOWN,
};
use nanami_runtime::RequestError;

const MAX_RECORD_BYTES: Word =
    (LINUX_DIRENT64_NAME_OFFSET + vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES).next_multiple_of(8);
const SMALL_BUFFER_BATCH_ENTRIES: Word = 16;
// Forward compaction must never overwrite the current or unread VFS record.
const _: () = assert!(ALTER_IO_OFFSET >= MAX_RECORD_BYTES);
const _: () = assert!(MAX_RECORD_BYTES <= vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES);

pub(super) fn sys_getdents64(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    user_buffer: Word,
    count: Word,
) -> Result<Word, i32> {
    if user_buffer == 0 {
        return Err(EFAULT);
    }
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.is_root_directory() {
        // Keep the entire overlay cursor in the POSIX open-file description:
        // dup/fork must share it, and lseek(d_off) must resume the same stream.
        let position = seek(runtime, file, 0, posix::POSIX_SEEK_CUR)?;
        let private_root = runtime
            .managed_process(pid)
            .is_some_and(|process| process.rootfs.root_len != 0);
        let mounts = virtual_fs::root_mounts(private_root);
        if position < mounts.len() {
            let mut index = position;
            let mut out = 0;
            while let Some(entry) = mounts.get(index) {
                let len = record_len(entry.name.len());
                if out + len > count.min(ALTER_IO_OFFSET).min(runtime.posix_shm_size) {
                    break;
                }
                write_entry(
                    runtime.posix_shm + out,
                    entry.node.id(),
                    index + 1,
                    LINUX_DT_DIR,
                    entry.name,
                );
                out += len;
                index += 1;
            }
            if out == 0 {
                return Err(EINVAL);
            }
            write_target_memory(runtime, pid, user_buffer, out)?;
            seek(runtime, file, index, posix::POSIX_SEEK_SET)?;
            return Ok(out);
        }
        if option_env!("NANAMI_BIASED_READDIR") != Some("0") {
            if let Some(bytes) = read_backing_inner(
                runtime, pid, file, user_buffer, count, mounts, Some(mounts.len()),
            )? {
                #[cfg(target_os = "none")]
                if option_env!("NANAMI_BIASED_READDIR_DIAG") == Some("1") && bytes != 0 {
                    use core::sync::atomic::{AtomicBool, Ordering};
                    static FILES: AtomicBool = AtomicBool::new(false);
                    if runtime.managed_process(pid).is_some_and(|process| {
                        process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next()
                            == Some(&b"xfe"[..])
                    }) && !FILES.swap(true, Ordering::Relaxed) {
                        nanami_runtime::println!("[linux.dir] biased-root-files pid={}", pid);
                    }
                }
                return Ok(bytes);
            }
        }
        seek(
            runtime,
            file,
            position - mounts.len(),
            posix::POSIX_SEEK_SET,
        )?;
        let result = read_backing(runtime, pid, file, user_buffer, count, mounts);
        // Restore the overlay cookie even on errors, never leaving a raw VFS
        // cursor visible to lseek or another descriptor sharing this file.
        let restored = seek(runtime, file, mounts.len(), posix::POSIX_SEEK_CUR);
        return result.and_then(|bytes| restored.map(|_| bytes));
    }
    if file.kind == LinuxFileKind::VirtualDirectory {
        return sys_virtual_getdents(runtime, pid, fd, user_buffer, count);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(ENOTDIR);
    }
    read_backing(runtime, pid, file, user_buffer, count, &[])
}

fn seek(runtime: &Runtime, file: LinuxFile, offset: Word, whence: Word) -> Result<Word, i32> {
    posix::posix_seek(runtime.posix_port, file.posix_fd, offset, whence).map_err(map_request_error)
}

fn read_backing(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    user_buffer: Word,
    count: Word,
    mounts: &[virtual_fs::DirectoryEntry],
) -> Result<Word, i32> {
    read_backing_inner(runtime, pid, file, user_buffer, count, mounts, None)?.ok_or(EIO)
}

fn read_backing_inner(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    user_buffer: Word,
    count: Word,
    mounts: &[virtual_fs::DirectoryEntry],
    bias: Option<Word>,
) -> Result<Option<Word>, i32> {
    let source_offset = ALTER_IO_OFFSET as Word;
    let capacity = count.min(runtime.posix_shm_size);
    // Retain the small-buffer batch, but let large requests carry more entries.
    // Every worst-case record in the enlarged batch fits the output buffer.
    let max_records = (runtime.posix_shm_size.saturating_sub(source_offset)
        / vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES)
        .min((capacity / MAX_RECORD_BYTES).max(SMALL_BUFFER_BATCH_ENTRIES));
    if max_records == 0 {
        return Err(EIO);
    }
    // Packed Linux records are smaller than their VFS input records. Together
    // with the initial source gap, that permits output beyond the old prefix
    // while keeping each current name and all unread records untouched.
    let mut started = false;
    loop {
        let reply = if let Some(bias) = bias {
            posix::posix_read_dir_biased(
                runtime.posix_port, file.posix_fd, max_records, source_offset, bias,
            )
        } else {
            posix::posix_read_dir(
                runtime.posix_port, file.posix_fd, max_records, source_offset,
            )
        };
        let (entries, next_index) = match reply {
            Err(RequestError::Unsupported) if bias.is_some() && !started => return Ok(None),
            other => other.map_err(map_request_error)?,
        };
        started = true;
        if entries == 0 {
            return Ok(Some(0));
        }
        if entries > max_records || next_index < entries
            || next_index.checked_add(bias.unwrap_or(0)).is_none()
        {
            return Err(EIO);
        }
        let start = next_index - entries;
        let mut consumed = 0;
        let mut out = 0;
        let packed = (|| {
            while consumed < entries {
                let entry = runtime.posix_shm
                    + source_offset
                    + consumed * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
                let inode = unsafe { core::ptr::read_unaligned(entry as *const Word) };
                let kind = unsafe {
                    core::ptr::read_unaligned(
                        (entry + vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET) as *const Word,
                    )
                };
                let name_len = unsafe {
                    core::ptr::read_unaligned(
                        (entry + vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET) as *const Word,
                    )
                };
                if name_len == 0 || name_len >= vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES {
                    return Err(EIO);
                }
                let name = unsafe {
                    core::slice::from_raw_parts(
                        (entry + vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET) as *const u8,
                        name_len,
                    )
                };
                if mounts.iter().any(|mount| mount.name == name) {
                    consumed += 1;
                    continue;
                }
                let len = record_len(name_len);
                if out + len > capacity {
                    break;
                }
                let dtype = match kind {
                    posix::POSIX_FILE_TYPE_DIRECTORY => LINUX_DT_DIR,
                    posix::POSIX_FILE_TYPE_CHAR_DEVICE => LINUX_DT_CHR,
                    posix::POSIX_FILE_TYPE_BLOCK_DEVICE => LINUX_DT_BLK,
                    posix::POSIX_FILE_TYPE_REGULAR => LINUX_DT_REG,
                    posix::POSIX_FILE_TYPE_SOCKET => LINUX_DT_SOCK,
                    posix::POSIX_FILE_TYPE_SYMLINK => 10, // DT_LNK
                    _ => LINUX_DT_UNKNOWN,
                };
                write_entry(
                    runtime.posix_shm + out,
                    inode,
                    mounts.len() + start + consumed + 1,
                    dtype,
                    name,
                );
                out += len;
                consumed += 1;
            }
            if out != 0 {
                write_target_memory(runtime, pid, user_buffer, out)?;
            }
            Ok(())
        })();
        if packed.is_err() {
            seek(runtime, file, start + bias.unwrap_or(0), posix::POSIX_SEEK_SET)?;
            return packed.map(|_| Some(0));
        }
        // The backend advances by a batch; only commit entries actually copied
        // (or intentionally shadowed), not entries that did not fit the buffer.
        if consumed < entries {
            seek(runtime, file, start + consumed + bias.unwrap_or(0), posix::POSIX_SEEK_SET)?;
        }
        if out != 0 {
            return Ok(Some(out));
        }
        if consumed < entries {
            return Err(EINVAL);
        }
        // A batch containing only shadowed mountpoints is not EOF.
    }
}

fn record_len(name_len: usize) -> Word {
    align_up_word((LINUX_DIRENT64_NAME_OFFSET + name_len + 1) as Word, 8)
}

pub(super) fn write_entry(base: Word, inode: Word, next: Word, dtype: Word, name: &[u8]) {
    unsafe {
        // Header and name bytes are all overwritten below. Initialize only
        // their NUL/padding suffix, preserving the complete dirent64 output.
        let suffix = LINUX_DIRENT64_NAME_OFFSET + name.len();
        core::ptr::write_bytes((base + suffix) as *mut u8, 0, record_len(name.len()) - suffix);
        write_u64(base, inode);
        write_u64(base + 8, next);
        write_u16(base + 16, record_len(name.len()) as u16);
        core::ptr::write((base + 18) as *mut u8, dtype as u8);
        core::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (base + LINUX_DIRENT64_NAME_OFFSET) as *mut u8,
            name.len(),
        );
    }
}

#[cfg(test)]
pub(super) fn write_entry_for_test(base: Word, inode: Word, next: Word, dtype: Word, name: &[u8]) {
    write_entry(base, inode, next, dtype, name);
}
