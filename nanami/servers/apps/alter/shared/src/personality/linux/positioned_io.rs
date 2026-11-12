//! Positioned I/O keeps descriptor cursors at POSIX and reads granted files directly.
use super::*;

pub(super) fn sys_positioned_io(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    offset: Word,
    write: bool,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::VirtualDirectory {
        return Err(EISDIR);
    }
    if file.kind == LinuxFileKind::MemoryFile {
        return super::memfd::io(runtime, pid, file, buffer, len, Some(offset), write);
    }
    if file.kind != LinuxFileKind::Posix {
        return Err(ESPIPE);
    }
    let access = file.resource & LINUX_O_ACCMODE;
    if (write && access == LINUX_O_RDONLY) || (!write && access == LINUX_O_WRONLY) {
        return Err(EBADF);
    }
    if offset > isize::MAX as Word {
        return Err(EINVAL);
    }
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let len = len.min(isize::MAX as Word);
    buffer.checked_add(len).ok_or(EFAULT)?;
    offset
        .checked_add(len)
        .filter(|end| *end <= isize::MAX as Word)
        .ok_or(EINVAL)?;
    if len == 0 {
        return Ok(0);
    }
    if write {
        transfer(runtime, pid, file, buffer, len, offset, true)
    } else {
        runtime.with_file_reader_operation(file, pid, "pread", |runtime| {
            transfer(runtime, pid, file, buffer, len, offset, false)
        })
    }
}

fn transfer(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    buffer: Word,
    len: Word,
    offset: Word,
    write: bool,
) -> Result<Word, i32> {
    let mut done = 0;
    while done < len {
        let chunk = (len - done).min(if write {
            runtime.posix_write_buffer().1
        } else {
            runtime.posix_read_buffer_size()
        });
        if chunk == 0 {
            return if done != 0 { Ok(done) } else { Err(EIO) };
        }
        let result = if write {
            super::guest_memory::copyin_posix_write(
                runtime,
                pid,
                buffer + done,
                runtime.posix_write_buffer().0,
                chunk,
            )
            .and_then(|_| {
                runtime
                    .write_posix(file.posix_fd, 0, chunk, Some(offset + done))
                    .map_err(map_request_error)
            })
        } else {
            runtime
                .pread_posix(file.posix_fd, 0, chunk, offset + done)
                .map_err(map_request_error)
                .and_then(|(bytes, source)| {
                    if bytes > chunk {
                        return Err(EIO);
                    }
                    if bytes != 0 {
                        super::copyout_from(runtime, pid, buffer + done, source, bytes)?;
                    }
                    Ok(bytes)
                })
        };
        match result {
            Ok(bytes) if bytes <= chunk => {
                done += bytes;
                if bytes < chunk {
                    break;
                }
            }
            Ok(_) => return if done != 0 { Ok(done) } else { Err(EIO) },
            Err(_) if done != 0 => break,
            Err(errno) => return Err(errno),
        }
    }
    Ok(done)
}
