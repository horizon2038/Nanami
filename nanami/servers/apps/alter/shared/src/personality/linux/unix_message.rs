//! LP64 msghdr/cmsghdr marshaling and transactional local-socket I/O.
use super::unix::*;
use super::*;
use crate::state::readiness::READY_UNIX;
use crate::state::unix_sockets::{
    Address, Kind, Packet, QUEUE_BYTES, QUEUE_MESSAGES, RIGHTS_LIMIT,
};
use alloc::vec::Vec;

struct Message {
    bases: [Word; LINUX_IOV_MAX as usize],
    lengths: [Word; LINUX_IOV_MAX as usize],
    count: usize,
    control: Word,
    control_len: Word,
    name: Word,
    name_len: Word,
    header: Option<Word>,
}

fn message(runtime: &mut Runtime, pid: Word, context: LinuxSyscallContext) -> Result<Message, i32> {
    let mut result = Message {
        bases: [0; LINUX_IOV_MAX as usize],
        lengths: [0; LINUX_IOV_MAX as usize],
        count: 1,
        control: 0,
        control_len: 0,
        name: 0,
        name_len: 0,
        header: None,
    };
    let a = context.args;
    let vector = match context.number {
        SYS_SENDMSG | SYS_RECVMSG => {
            read_target_memory(runtime, pid, a[1], 56).map_err(|_| EFAULT)?;
            result.name = read_shm_u64(runtime, 0);
            result.name_len = read_shm_u32(runtime, 8) as Word;
            result.control = read_shm_u64(runtime, 32);
            result.control_len = read_shm_u64(runtime, 40);
            result.header = Some(a[1]);
            Some((read_shm_u64(runtime, 16), read_shm_u64(runtime, 24)))
        }
        SYS_READV | SYS_WRITEV => Some((a[1], a[2])),
        _ => None,
    };
    if let Some((iov, count)) = vector {
        read_linux_iovecs(
            runtime,
            pid,
            iov,
            count,
            &mut result.bases,
            &mut result.lengths,
        )?;
        result.count = count;
    } else {
        result.bases[0] = a[1];
        result.lengths[0] = a[2];
        if a[1] == 0 && a[2] != 0 {
            return Err(EFAULT);
        }
        if context.number == SYS_SENDTO || context.number == SYS_RECVFROM {
            result.name = a[4];
            result.name_len = a[5];
        }
    }
    Ok(result)
}

fn length(message: &Message) -> Result<usize, i32> {
    message.lengths[..message.count]
        .iter()
        .try_fold(0usize, |sum, length| {
            sum.checked_add(*length)
                .filter(|length| *length <= isize::MAX as usize)
                .ok_or(EINVAL)
        })
}

fn copy_in(
    runtime: &mut Runtime,
    pid: Word,
    message: &Message,
    len: usize,
) -> Result<Vec<u8>, i32> {
    let mut data: Vec<u8> = Vec::new();
    data.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    for index in 0..message.count {
        let count = message.lengths[index].min(len - data.len());
        let mut done = 0;
        while done < count {
            let chunk = (count - done).min(runtime.posix_shm_size);
            let initialized = data.len();
            copyin_into(
                runtime,
                pid,
                message.bases[index].checked_add(done).ok_or(EFAULT)?,
                unsafe { data.as_mut_ptr().add(initialized) } as Word,
                chunk,
            )
            .map_err(|_| EFAULT)?;
            // Expose bytes only after copyin succeeded; failed transfers never
            // commit a packet or reveal uninitialized allocation contents.
            unsafe {
                data.set_len(initialized + chunk);
            }
            done += chunk;
        }
        if data.len() == len {
            break;
        }
    }
    Ok(data)
}

fn copy_out(runtime: &mut Runtime, pid: Word, message: &Message, data: &[u8]) -> Result<(), i32> {
    let mut copied = 0;
    for index in 0..message.count {
        let count = message.lengths[index].min(data.len() - copied);
        let mut done = 0;
        while done < count {
            let chunk = (count - done).min(runtime.posix_shm_size);
            copyout_from(
                runtime,
                pid,
                message.bases[index].checked_add(done).ok_or(EFAULT)?,
                data[copied..].as_ptr() as Word,
                chunk,
            )
            .map_err(|_| EFAULT)?;
            copied += chunk;
            done += chunk;
        }
        if copied == data.len() {
            break;
        }
    }
    Ok(())
}

fn rights(runtime: &mut Runtime, pid: Word, message: &Message) -> Result<Vec<LinuxFile>, i32> {
    let mut files = Vec::new();
    if message.control_len == 0 {
        return Ok(files);
    }
    if message.control == 0 {
        return Err(EFAULT);
    }
    if message.control_len > 4096 {
        return Err(EINVAL);
    }
    // Snapshot ancillary bytes with one memory-copy RPC, not one RPC per FD.
    read_target_memory(runtime, pid, message.control, message.control_len).map_err(|_| EFAULT)?;
    let mut offset = 0;
    while message.control_len - offset >= 16 {
        let len = read_shm_u64(runtime, offset);
        let level = read_shm_u32(runtime, offset + 8);
        let kind = read_shm_u32(runtime, offset + 12);
        if len < 16 || len > message.control_len - offset {
            return Err(EINVAL);
        }
        if level != LINUX_SOL_SOCKET as u32 || kind != 1 {
            return Err(EOPNOTSUPP);
        }
        if (len - 16) % 4 != 0 || (len - 16) / 4 + files.len() > RIGHTS_LIMIT {
            return Err(EINVAL);
        }
        files.try_reserve((len - 16) / 4).map_err(|_| ENOMEM)?;
        for index in 0..(len - 16) / 4 {
            let fd = read_shm_u32(runtime, offset + 16 + index * 4) as Word;
            let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
            // Transfer only stable open descriptions. Other devices derive
            // identity from the calling process; legacy Internet sockets keep
            // mutable bind/status metadata in the descriptor itself.
            if !file.has_posix_backend()
                && !matches!(
                    file.kind,
                    LinuxFileKind::PipeRead
                        | LinuxFileKind::PtyMaster
                        | LinuxFileKind::PtySlave
                        | LinuxFileKind::PipeWrite
                        | LinuxFileKind::SocketUnix
                        | LinuxFileKind::EventFd
                        | LinuxFileKind::TimerFd
                        | LinuxFileKind::SignalFd
                        | LinuxFileKind::MemoryFile
                        | LinuxFileKind::Epoll
                        | LinuxFileKind::IoUring
                )
            {
                return Err(EOPNOTSUPP);
            }
            files.push(file);
        }
        let aligned = len.checked_add(7).ok_or(EINVAL)? & !7;
        if aligned > message.control_len - offset {
            break;
        }
        offset += aligned;
    }
    Ok(files)
}

fn send(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    message: &Message,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(MSG_DONTWAIT | MSG_NOSIGNAL) != 0 {
        return Err(EOPNOTSUPP);
    }
    let this = socket(runtime, file.resource)?;
    let kind = this.kind;
    let source = this.address;
    if this.listening {
        return Err(ENOTCONN);
    }
    if this.write_closed {
        return Err(EPIPE);
    }
    let peer = this.peer;
    let send_limit = this.send_limit;
    let destination = if message.name != 0 && kind == Kind::Datagram {
        destination(runtime, pid, message.name, message.name_len)?
    } else {
        peer.ok_or(if kind == Kind::Datagram {
            EDESTADDRREQ
        } else {
            ENOTCONN
        })?
    };
    let target = runtime
        .unix_sockets
        .get(destination)
        .ok_or(if kind == Kind::Datagram {
            ECONNREFUSED
        } else {
            EPIPE
        })?;
    if target.kind != kind {
        return Err(EPROTOTYPE);
    }
    if target.read_closed {
        return Err(EPIPE);
    }
    if kind == Kind::Datagram && target.peer.is_some_and(|id| id != file.resource) {
        return Err(EPERM);
    }
    let total = length(message)?;
    let limit = target.receive_limit.min(send_limit);
    if kind != Kind::Stream && total > limit {
        return Err(EMSGSIZE);
    }
    if kind == Kind::Stream && total == 0 {
        return Ok(0);
    }
    let files = rights(runtime, pid, message)?;
    let target = socket(runtime, destination)?;
    let available = limit.saturating_sub(target.queued_bytes);
    if (kind == Kind::Stream && available == 0)
        || (kind != Kind::Stream && total > available)
        || target.packets.len() >= QUEUE_MESSAGES
        || target.queued_rights + files.len() > RIGHTS_LIMIT
    {
        return Err(EAGAIN);
    }
    let count = total.min(available);
    let data = copy_in(runtime, pid, message, count)?;
    runtime
        .unix_sockets
        .get_mut(destination)
        .unwrap()
        .packets
        .try_reserve(1)
        .map_err(|_| ENOMEM)?;
    let mut held = Vec::new();
    held.try_reserve_exact(files.len()).map_err(|_| ENOMEM)?;
    for file in files {
        match duplicate_linux_file(runtime, file) {
            Ok(file) => held.push(file),
            Err(error) => {
                for file in held {
                    release_held(runtime, file);
                }
                return Err(error);
            }
        }
    }
    let target = runtime.unix_sockets.get_mut(destination).unwrap();
    target.queued_bytes += count;
    target.queued_rights += held.len();
    target.packets.push_back(Packet {
        data,
        offset: 0,
        rights: held,
        source,
    });
    runtime.readiness_changes |= READY_UNIX;
    Ok(count)
}

fn receive(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    message: &Message,
    flags: Word,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    if flags & !(MSG_DONTWAIT | MSG_PEEK | MSG_TRUNC | MSG_CMSG_CLOEXEC) != 0 {
        return Err(EOPNOTSUPP);
    }
    let capacity = length(message)?.min(QUEUE_BYTES);
    let this = socket(runtime, file.resource)?;
    if this.listening {
        return Err(EINVAL);
    }
    let datagram = this.kind != Kind::Stream;
    if capacity == 0 && !datagram {
        return empty_receive(runtime, pid, message);
    }
    if this.read_closed {
        return empty_receive(runtime, pid, message);
    }
    if this.packets.is_empty() {
        if this.kind != Kind::Datagram {
            let peer = this.peer.and_then(|id| runtime.unix_sockets.get(id));
            if this.peer.is_none() {
                return Err(ENOTCONN);
            }
            if peer.is_none_or(|peer| peer.write_closed) {
                return empty_receive(runtime, pid, message);
            }
        }
        return Err(EAGAIN);
    }
    // A large caller buffer need not mean a large allocation: X11 frequently
    // asks for 64 KiB while only a few bytes are queued. Retain the same stream
    // barrier/packet rules and allocate only the bytes this receive can return.
    let mut queued = 0;
    for packet in &this.packets {
        queued += (packet.data.len() - packet.offset).min(capacity - queued);
        if datagram || queued == capacity || !packet.rights.is_empty() {
            break;
        }
    }
    let mut data = Vec::new();
    data.try_reserve_exact(queued).map_err(|_| ENOMEM)?;
    let mut files = Vec::new();
    let mut source = Address::UNNAMED;
    let mut packet_length = 0;
    for packet in &this.packets {
        let count = (packet.data.len() - packet.offset).min(capacity - data.len());
        if data.is_empty() {
            source = packet.source;
            packet_length = packet.data.len() - packet.offset;
        }
        data.extend_from_slice(&packet.data[packet.offset..packet.offset + count]);
        if count != 0 || datagram {
            files.try_reserve(packet.rights.len()).map_err(|_| ENOMEM)?;
            files.extend_from_slice(&packet.rights);
        }
        // Ancillary data is a stream barrier, not a message framing protocol.
        if datagram || data.len() == capacity || !packet.rights.is_empty() {
            break;
        }
    }
    let mut output_flags = if datagram && packet_length > capacity {
        MSG_TRUNC
    } else {
        0
    };
    let available = if message.header.is_some() && message.control != 0 && message.control_len >= 20
    {
        ((message.control_len - 16) / 4).min(RIGHTS_LIMIT)
    } else {
        0
    };
    let mut descriptors = Vec::new();
    descriptors
        .try_reserve(files.len().min(available))
        .map_err(|_| ENOMEM)?;
    for file in files.iter().take(available) {
        let mut copy = match duplicate_linux_file(runtime, *file) {
            Ok(file) => file,
            Err(error) => {
                for fd in descriptors {
                    let _ = close_linux_fd(runtime, pid, fd);
                }
                return Err(error);
            }
        };
        copy.flags &= !LINUX_FD_CLOEXEC;
        if flags & MSG_CMSG_CLOEXEC != 0 {
            copy.flags |= LINUX_FD_CLOEXEC;
        }
        match runtime.allocate_linux_file(pid, copy, 0) {
            Some(fd) => descriptors.push(fd),
            None => {
                release_held(runtime, copy);
                break;
            }
        }
    }
    if descriptors.len() < files.len() {
        output_flags |= MSG_CTRUNC;
    }
    let output = (|| {
        copy_out(runtime, pid, message, &data)?;
        if context.number == SYS_RECVFROM {
            write_address(runtime, pid, source, message.name, message.name_len)?;
        }
        if let Some(header) = message.header {
            let address_len = if message.name == 0 { 0 } else { source.len };
            if address_len != 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        source.bytes.as_ptr(),
                        runtime.posix_shm as *mut u8,
                        source.len,
                    );
                }
                write_target_memory(runtime, pid, message.name, source.len.min(message.name_len))
                    .map_err(|_| EFAULT)?;
            }
            let mut control_len = 0;
            if !descriptors.is_empty() {
                let len = 16 + descriptors.len() * 4;
                control_len = ((len + 7) & !7).min(message.control_len);
                let mut control = [0u8; 16 + RIGHTS_LIMIT * 4];
                control[..8].copy_from_slice(&(len as u64).to_ne_bytes());
                control[8..12].copy_from_slice(&(LINUX_SOL_SOCKET as u32).to_ne_bytes());
                control[12..16].copy_from_slice(&1u32.to_ne_bytes());
                for (index, fd) in descriptors.iter().enumerate() {
                    control[16 + index * 4..20 + index * 4]
                        .copy_from_slice(&(*fd as u32).to_ne_bytes());
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        control.as_ptr(),
                        runtime.posix_shm as *mut u8,
                        control_len,
                    );
                }
                write_target_memory(runtime, pid, message.control, control_len)
                    .map_err(|_| EFAULT)?;
            }
            write_u32_to_target(runtime, pid, header + 8, address_len as u32)?;
            unsafe {
                write_u64(runtime.posix_shm, control_len);
            }
            write_target_memory(runtime, pid, header + 40, 8).map_err(|_| EFAULT)?;
            write_u32_to_target(runtime, pid, header + 48, output_flags as u32)?;
        }
        Ok(())
    })();
    if let Err(error) = output {
        for fd in descriptors {
            let _ = close_linux_fd(runtime, pid, fd);
        }
        return Err(error);
    }
    if flags & MSG_PEEK == 0 {
        let mut remaining = data.len();
        let mut collect_sockets = false;
        loop {
            let this = runtime.unix_sockets.get_mut(file.resource).unwrap();
            let packet = this.packets.front_mut().unwrap();
            let consumed = if datagram {
                packet.data.len() - packet.offset
            } else {
                remaining.min(packet.data.len() - packet.offset)
            };
            let rights = core::mem::take(&mut packet.rights);
            packet.offset += consumed;
            this.queued_bytes -= consumed;
            this.queued_rights -= rights.len();
            if packet.offset == packet.data.len() {
                this.packets.pop_front();
            }
            for file in rights {
                collect_sockets |= file.kind == LinuxFileKind::SocketUnix;
                release_held(runtime, file);
            }
            if datagram || consumed == remaining {
                break;
            }
            remaining -= consumed;
        }
        if collect_sockets {
            super::unix::collect(runtime);
        }
        runtime.readiness_changes |= READY_UNIX;
    }
    if flags & MSG_TRUNC != 0 && datagram {
        Ok(packet_length)
    } else {
        Ok(data.len())
    }
}

fn empty_receive(runtime: &mut Runtime, pid: Word, message: &Message) -> Result<Word, i32> {
    if let Some(header) = message.header {
        write_u32_to_target(runtime, pid, header + 8, 0)?;
        unsafe {
            write_u64(runtime.posix_shm, 0);
        }
        write_target_memory(runtime, pid, header + 40, 8).map_err(|_| EFAULT)?;
        write_u32_to_target(runtime, pid, header + 48, 0)?;
    }
    Ok(0)
}

pub(super) fn io(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let message = message(runtime, pid, context)?;
    let flags = match context.number {
        SYS_SENDMSG | SYS_RECVMSG => context.args[2],
        SYS_SENDTO | SYS_RECVFROM => context.args[3],
        _ => 0,
    };
    match context.number {
        SYS_WRITE | SYS_WRITEV | SYS_SENDTO | SYS_SENDMSG => {
            send(runtime, pid, file, &message, flags)
        }
        _ => receive(runtime, pid, file, &message, flags, context),
    }
}

pub(super) fn discard(runtime: &mut Runtime, id: Word) {
    if let Some(socket) = runtime.unix_sockets.get_mut(id) {
        let packets = core::mem::take(&mut socket.packets);
        socket.queued_bytes = 0;
        socket.queued_rights = 0;
        for packet in packets {
            for file in packet.rights {
                release_held(runtime, file);
            }
        }
    }
    super::unix::collect(runtime);
}
