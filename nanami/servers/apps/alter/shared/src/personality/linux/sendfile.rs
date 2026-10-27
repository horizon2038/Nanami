//! Transfer through service shared memory, without a bounce through guest RAM.
use super::*;

pub(super) fn sys_sendfile(
    runtime: &mut Runtime,
    pid: Word,
    out_fd: Word,
    in_fd: Word,
    offset_ptr: Word,
    count: Word,
) -> Result<Word, i32> {
    let input = runtime.linux_file(pid, in_fd).ok_or(EBADF)?;
    let output = runtime.linux_file(pid, out_fd).ok_or(EBADF)?;
    if input.kind != LinuxFileKind::Posix {
        return Err(
            if offset_ptr != 0
                && matches!(
                    input.kind,
                    LinuxFileKind::PipeRead | LinuxFileKind::SocketTcp | LinuxFileKind::SocketUdp
                )
            {
                ESPIPE
            } else {
                EINVAL
            },
        );
    }
    if input.resource & LINUX_O_ACCMODE == LINUX_O_WRONLY {
        return Err(EBADF);
    }
    let (_, _, kind, _, _) =
        posix::posix_fstat(runtime.posix_port, input.posix_fd).map_err(map_request_error)?;
    if kind != posix::POSIX_FILE_TYPE_REGULAR {
        return Err(EINVAL);
    }
    let terminal_id;
    let capacity = match output.kind {
        LinuxFileKind::Posix => {
            if output.resource & LINUX_O_ACCMODE == LINUX_O_RDONLY {
                return Err(EBADF);
            }
            let flags = posix::posix_fcntl_status_flags(runtime.posix_port, output.posix_fd, None)
                .map_err(map_request_error)?;
            if flags & posix::POSIX_O_APPEND != 0 {
                return Err(EINVAL);
            }
            terminal_id = 0;
            runtime.posix_write_buffer().1
        }
        LinuxFileKind::Terminal => {
            terminal_id = terminal_id_for_pid(runtime, pid)?;
            terminal_bounded_len(runtime, Word::MAX)?
        }
        LinuxFileKind::SocketTcp => {
            if output.posix_fd == 0 {
                return Err(ENOTCONN);
            }
            terminal_id = 0;
            runtime.network_shm_size.min(TCP_SOCKET_PAYLOAD_MAX)
        }
        LinuxFileKind::PipeWrite => {
            terminal_id = 0;
            let pipe = runtime.pipe_mut(output.posix_fd).ok_or(EBADF)?;
            if pipe.readers == 0 {
                return Err(EPIPE);
            }
            if pipe.len == LINUX_PIPE_BYTES && count != 0 {
                return Err(EAGAIN);
            }
            LINUX_PIPE_BYTES - pipe.len
        }
        LinuxFileKind::VirtualFile
            if matches!(
                VirtualNode::from_id(output.resource),
                Some(VirtualNode::DevNull | VirtualNode::DevZero)
            ) =>
        {
            terminal_id = 0;
            runtime.posix_read_buffer_size()
        }
        LinuxFileKind::PipeRead => return Err(EBADF),
        _ => return Err(EINVAL),
    };
    let offset = if offset_ptr == 0 {
        posix::posix_seek(runtime.posix_port, input.posix_fd, 0, posix::POSIX_SEEK_CUR)
            .map_err(map_request_error)?
    } else {
        read_target_memory(runtime, pid, offset_ptr, 8).map_err(|_| EFAULT)?;
        unsafe { core::ptr::read_unaligned(runtime.posix_shm as *const u64) as Word }
    };
    if offset > isize::MAX as Word {
        return Err(EINVAL);
    }
    let count = count
        .min(0x7fff_f000)
        .min(capacity)
        .min(runtime.posix_read_buffer_size());
    if count == 0 {
        return Ok(0);
    }
    // A short sendfile is legal. One bounded batch also keeps a large transfer
    // from monopolizing Alter. pread leaves unsent bytes available on EAGAIN or
    // a short output write; advance the input by only the accepted byte count.
    let (bytes, source) = runtime
        .pread_posix(input.posix_fd, 0, count, offset)
        .map_err(map_request_error)?;
    if bytes == 0 {
        return Ok(0);
    }
    let written = match output.kind {
        LinuxFileKind::Posix => {
            copy_bytes(source, runtime.posix_write_buffer().0, bytes);
            runtime
                .write_posix(output.posix_fd, 0, bytes, None)
                .map_err(map_request_error)?
        }
        LinuxFileKind::Terminal => {
            copy_bytes(source, runtime.terminal_shm, bytes);
            nanami_client::terminal::terminal_write_output(
                runtime.terminal_port,
                terminal_id,
                0,
                bytes,
            )
            .map_err(map_request_error)?
        }
        LinuxFileKind::SocketTcp => {
            copy_bytes(source, runtime.network_shm, bytes);
            net::net_service_tcp_send_on_connection(
                runtime.network_port,
                output.posix_fd,
                0,
                bytes,
                TCP_FLAG_ACK | TCP_FLAG_PSH,
            )
            .map_err(map_network_error)?
        }
        LinuxFileKind::PipeWrite => {
            let pipe = runtime.pipe_mut(output.posix_fd).ok_or(EBADF)?;
            for index in 0..bytes {
                pipe.write_byte(pipe.write, unsafe { *((source + index) as *const u8) });
                pipe.write = (pipe.write + 1) % LINUX_PIPE_BYTES;
            }
            pipe.len += bytes;
            runtime.readiness_changes |= crate::state::readiness::READY_PIPE;
            bytes
        }
        _ => bytes, // /dev/null and /dev/zero
    };
    if written > bytes {
        return Err(EIO);
    }
    if offset_ptr == 0 {
        posix::posix_seek(
            runtime.posix_port,
            input.posix_fd,
            offset + written,
            posix::POSIX_SEEK_SET,
        )
        .map_err(map_request_error)?;
    } else {
        unsafe {
            write_u64(runtime.posix_shm, offset + written);
        }
        write_target_memory(runtime, pid, offset_ptr, 8).map_err(|_| EFAULT)?;
    }
    Ok(written)
}

fn copy_bytes(source: Word, destination: Word, bytes: Word) {
    if source != destination {
        unsafe {
            core::ptr::copy(source as *const u8, destination as *mut u8, bytes);
        }
    }
}
