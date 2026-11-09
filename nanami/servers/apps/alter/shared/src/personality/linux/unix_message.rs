//! LP64 msghdr/cmsghdr marshaling and transactional local-socket I/O.
use super::unix::*;
use super::*;
use crate::state::readiness::{READY_UNIX_READ, READY_UNIX_WRITE};
use crate::state::unix_sockets::{
    Address, Kind, Packet, Socket, QUEUE_BYTES, QUEUE_MESSAGES, RIGHTS_LIMIT,
};
use alloc::vec::Vec;

// Gather eligible multi-packet payloads once, keeping ordered checked-copy and
// queue commit semantics. =0 restores the original path for comparison.
const BATCH_COALESCED_RECVMSG: bool = match option_env!("NANAMI_UNIX_COALESCED_RECVMSG") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};

#[cfg(target_os = "none")]
fn diagnostic_files_receiver(runtime: &Runtime, pid: Word) -> bool {
    runtime.managed_process(pid).is_some_and(|process| {
        process.image_name.get(..process.image_name_len).is_some_and(|name| {
            name.rsplit(|byte| *byte == b'/').next() == Some(&b"xfe"[..])
        })
    })
}

enum Vectors<'a> {
    One { base: Word, length: Word },
    Many {
        bases: &'a [Word],
        lengths: &'a [Word],
    },
}
impl Vectors<'_> {
    fn parts(&self) -> (&[Word], &[Word]) {
        match self {
            Self::One { base, length } => (core::slice::from_ref(base), core::slice::from_ref(length)),
            Self::Many { bases, lengths } => (bases, lengths),
        }
    }
}

struct Message<'a> {
    vectors: Vectors<'a>,
    control: Word,
    control_len: Word,
    name: Word,
    name_len: Word,
    header: Option<Word>,
}

fn length(message: &Message) -> Result<usize, i32> {
    message.vectors.parts().1
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
    copy_in_append(runtime, pid, message, len, &mut data)?;
    Ok(data)
}

fn copy_in_append(
    runtime: &mut Runtime,
    pid: Word,
    message: &Message,
    len: usize,
    data: &mut Vec<u8>,
) -> Result<(), i32> {
    let original = data.len();
    let (bases, lengths) = message.vectors.parts();
    data.try_reserve_exact(len).map_err(|_| ENOMEM)?;
    let result = (|| {
        let mut copied = 0;
        for index in 0..bases.len() {
            let count = lengths[index].min(len - copied);
            let mut done = 0;
            while done < count {
                let chunk = (count - done).min(runtime.posix_shm_size);
                let initialized = data.len();
                guest_memory::copyin_owned(
                    runtime,
                    pid,
                    bases[index].checked_add(done).ok_or(EFAULT)?,
                    &mut data.spare_capacity_mut()[..chunk],
                )
                .map_err(|_| EFAULT)?;
                // Expose bytes only after copyin succeeded; failures restore
                // the old length before queued storage becomes visible again.
                unsafe {
                    data.set_len(initialized + chunk);
                }
                done += chunk;
                copied += chunk;
            }
            if copied == len {
                break;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        data.truncate(original);
    }
    result
}

// A stream has no record boundaries without ancillary data. Reuse the last
// packet's owned storage, restoring it on every failure before returning.
fn append_stream(
    runtime: &mut Runtime, pid: Word, destination: Word, source: Address,
    message: &Message, count: usize,
) -> Option<Result<Word, i32>> {
    let (mut data, offset) = {
        let target = runtime.unix_sockets.get_mut(destination)?;
        let tail = target.packets.back_mut()?;
        if !tail.rights.is_empty() || tail.source != source {
            return None;
        }
        (core::mem::take(&mut tail.data), tail.offset)
    };
    let mut next_offset = offset;
    let result = (|| {
        let required = data.len().checked_add(count).ok_or(ENOMEM)?;
        if required > 2 * QUEUE_BYTES {
            // A large read may have left a long consumed prefix. Prepare new
            // storage transactionally rather than changing old bytes on fault.
            let mut compact = Vec::new();
            compact.try_reserve_exact(data.len() - offset + count).map_err(|_| ENOMEM)?;
            compact.extend_from_slice(&data[offset..]);
            copy_in_append(runtime, pid, message, count, &mut compact)?;
            data = compact;
            next_offset = 0;
            return Ok(());
        }
        if required > data.capacity() {
            let capacity = data.capacity().saturating_mul(2).max(required).min(2 * QUEUE_BYTES);
            data.try_reserve_exact(capacity - data.len()).map_err(|_| ENOMEM)?;
        }
        // No reentrant service dispatch occurs while the owned tail is taken.
        copy_in_append(runtime, pid, message, count, &mut data)?;
        // Amortize compaction: tiny interleaved reads/writes must not move the
        // entire live queue on each send. Never compact before a fallible copy.
        if offset != 0 && offset >= data.len().div_ceil(2) {
            data.copy_within(offset.., 0);
            data.truncate(data.len() - offset);
            next_offset = 0;
        }
        Ok(())
    })();
    let target = runtime.unix_sockets.get_mut(destination).unwrap();
    let tail = target.packets.back_mut().unwrap();
    tail.data = data;
    if result.is_ok() {
        tail.offset = next_offset;
        target.queued_bytes += count;
    }
    Some(result.map(|()| count))
}

fn copy_out(runtime: &Runtime, pid: Word, message: &Message, data: &[u8]) -> Result<(), i32> {
    let mut copied = 0;
    let (bases, lengths) = message.vectors.parts();
    for index in 0..bases.len() {
        let count = lengths[index].min(data.len() - copied);
        let mut done = 0;
        while done < count {
            let chunk = (count - done).min(runtime.posix_shm_size);
            guest_memory::copyout_owned(
                runtime,
                pid,
                bases[index].checked_add(done).ok_or(EFAULT)?,
                &data[copied..copied + chunk],
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

fn copy_queued_payload(
    runtime: &Runtime,
    pid: Word,
    socket_id: Word,
    message: &Message,
    bytes: usize,
) -> Result<(), i32> {
    let this = socket(runtime, socket_id)?;
    let first = this.packets.front().ok_or(EAGAIN)?;
    if bytes <= first.data.len() - first.offset {
        // The queue owns these bytes throughout checked copyout. Alter does
        // not dispatch another request while blocked in the memory service.
        return copy_out(
            runtime,
            pid,
            message,
            &first.data[first.offset..first.offset + bytes],
        );
    }
    if let Some(data) = coalesced_scratch(runtime, this, bytes)? {
        return copy_out(runtime, pid, message, data);
    }
    let data = coalesced_payload(this, bytes)?;
    copy_out(runtime, pid, message, &data)
}

fn coalesced_scratch<'a>(runtime: &'a Runtime, this: &Socket, bytes: usize) -> Result<Option<&'a [u8]>, i32> {
    if runtime.posix_shm == 0 || bytes > runtime.posix_shm_size || bytes > isize::MAX as usize {
        return Ok(None);
    }
    // Message/iovec/control planning has finished. This is owned native scratch,
    // not a guest view. Payload copyout and write_receive_result use independent
    // stack descriptors and Alpha transfers; neither overwrites this source.
    // No service dispatch occurs until these bytes have been consumed.
    let output = unsafe { core::slice::from_raw_parts_mut(runtime.posix_shm as *mut u8, bytes) };
    gather_payload(this, output)?;
    Ok(Some(output))
}

fn coalesced_payload(this: &Socket, bytes: usize) -> Result<Vec<u8>, i32> {
    // Coalesce plain stream writes as before: borrowing each packet separately
    // would add copyout requests when one guest iovec spans several packets.
    let mut data = Vec::new();
    data.try_reserve_exact(bytes).map_err(|_| ENOMEM)?;
    for packet in &this.packets {
        let count = (packet.data.len() - packet.offset).min(bytes - data.len());
        data.extend_from_slice(&packet.data[packet.offset..packet.offset + count]);
        if data.len() == bytes {
            break;
        }
    }
    Ok(data)
}

fn gather_payload(this: &Socket, output: &mut [u8]) -> Result<(), i32> {
    let mut copied = 0;
    for packet in &this.packets {
        let count = (packet.data.len() - packet.offset).min(output.len() - copied);
        output[copied..copied + count]
            .copy_from_slice(&packet.data[packet.offset..packet.offset + count]);
        copied += count;
        if copied == output.len() {
            return Ok(());
        }
    }
    // A receive plan cannot outlive or mutate its queue. If that invariant is
    // ever broken, fail before publishing a short/zero-filled payload.
    Err(EAGAIN)
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
    if option_env!("NANAMI_UNIX_STREAM_APPEND") != Some("0")
        && kind == Kind::Stream && files.is_empty()
    {
        if let Some(result) = append_stream(runtime, pid, destination, source, message, count) {
            #[cfg(target_os = "none")]
            if result.is_ok() && option_env!("NANAMI_UNIX_STREAM_APPEND_DIAG") == Some("1")
                && diagnostic_files_receiver(runtime, pid)
            {
                use core::sync::atomic::{AtomicBool, Ordering};
                static FILES: AtomicBool = AtomicBool::new(false);
                if !FILES.swap(true, Ordering::Relaxed) {
                    nanami_runtime::println!("[linux.unix] stream-append-files pid={} bytes={}", pid, count);
                }
            }
            return result;
        }
    }
    let data = if kind == Kind::Stream && files.is_empty() {
        let mut data = runtime.unix_sockets.get_mut(destination).unwrap().take_stream_buffer();
        let result = (|| {
            // A drained spare is empty. Amortize increasing small roundtrips
            // without growing beyond the endpoint's existing retention bound.
            // Fresh and large messages keep exact-sized allocation.
            if count > data.capacity() && data.capacity() != 0
                && count <= crate::state::unix_sockets::STREAM_SPARE_BYTES
            {
                let capacity = data.capacity().saturating_mul(2).max(count)
                    .min(crate::state::unix_sockets::STREAM_SPARE_BYTES);
                data.try_reserve_exact(capacity).map_err(|_| ENOMEM)?;
            }
            copy_in_append(runtime, pid, message, count, &mut data)
        })();
        if let Err(error) = result {
            // A late iovec fault exposes no packet and restores ownership of
            // the empty spare. No dispatch can change this destination here.
            runtime.unix_sockets.get_mut(destination).unwrap().recycle_stream_buffer(data);
            return Err(error);
        }
        data
    } else {
        copy_in(runtime, pid, message, count)?
    };
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
    let became_readable = target.packets.is_empty();
    target.queued_bytes += count;
    target.queued_rights += held.len();
    target.packets.push_back(Packet {
        data,
        offset: 0,
        rights: held,
        source,
    });
    // More data in an already-readable queue cannot wake another reader.
    // Receive still notifies writers when constrained capacity frees.
    if became_readable {
        runtime.readiness_changes |= READY_UNIX_READ;
    }
    Ok(count)
}

fn writer_notification_needed(runtime: &Runtime, id: Word) -> bool {
    let Some(this) = runtime.unix_sockets.get(id) else { return true; };
    // Record sends may need more space even while POLLOUT is set. Ancillary
    // sends may need any number of rights slots. Keep those wakeups unchanged.
    if this.kind != Kind::Stream || this.queued_rights != 0
        || this.packets.len() >= QUEUE_MESSAGES
    {
        return true;
    }
    let Some(peer) = this.peer.and_then(|peer| runtime.unix_sockets.get(peer)) else { return true; };
    if peer.kind != Kind::Stream || peer.peer != Some(id) { return true; }
    // A plain stream send accepts a partial payload whenever any byte fits.
    // With packet/rights room too, it could not have parked for this queue.
    this.queued_bytes >= this.receive_limit.min(peer.send_limit)
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
    // Plan the receive without copying payloads. Keep ancillary stream barriers
    // and leave every packet intact until payload and metadata copyout succeed.
    let mut queued = 0;
    let mut files = Vec::new();
    let mut source = Address::UNNAMED;
    let mut packet_length = 0;
    for packet in &this.packets {
        let count = (packet.data.len() - packet.offset).min(capacity - queued);
        if queued == 0 {
            source = packet.source;
            packet_length = packet.data.len() - packet.offset;
        }
        queued += count;
        if count != 0 || datagram {
            files.try_reserve(packet.rights.len()).map_err(|_| ENOMEM)?;
            files.extend_from_slice(&packet.rights);
        }
        // Ancillary data is a stream barrier, not a message framing protocol.
        if datagram || queued == capacity || !packet.rights.is_empty() {
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
        #[cfg(target_os = "none")]
        if option_env!("NANAMI_UNIX_RECVMSG_DIAG") == Some("1")
            && diagnostic_files_receiver(runtime, pid)
        {
            use core::sync::atomic::{AtomicUsize, Ordering};
            // At most four lines, keyed by receive operation rather than PID
            // modulus. This diagnoses shape, never frequency or CPU cost.
            static REPORTED: AtomicUsize = AtomicUsize::new(0);
            let bit = match context.number {
                SYS_READ => 1,
                SYS_READV => 2,
                SYS_RECVFROM => 4,
                SYS_RECVMSG => 8,
                _ => 0,
            };
            if bit != 0 && REPORTED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                let this = socket(runtime, file.resource)?;
                let first = this.packets.front().ok_or(EAGAIN)?;
                nanami_runtime::println!(
                    "[unix.files] recv-shape pid={} call={} iov={} queued={} first={} rights={} name={} header={}",
                    pid, context.number, message.vectors.parts().0.len(), queued,
                    first.data.len() - first.offset, files.len(), message.name != 0,
                    message.header.is_some(),
                );
            }
        }
        // The usual X11 receive has one bounded iovec, one queued packet and
        // no address/rights output. Keep borrowing that payload, and append
        // the result fields to the same ordered checked-copy request. Bound
        // it to one original chunk so a later EFAULT keeps scalar prefix
        // behavior; larger/scattered/ancillary messages retain the old path.
        if let Some(header) = message.header {
            if message.name == 0
                && files.is_empty()
                && message.vectors.parts().0.len() == 1
                && queued != 0
                && queued <= runtime.posix_shm_size
            {
                let this = socket(runtime, file.resource)?;
                let packet = this.packets
                    .front()
                    .ok_or(EAGAIN)?;
                if queued <= packet.data.len() - packet.offset {
                    return write_receive_result(
                        runtime,
                        pid,
                        header,
                        0,
                        0,
                        output_flags as u32,
                        Some((
                            message.vectors.parts().0[0],
                            &packet.data[packet.offset..packet.offset + queued],
                        )),
                    );
                }
                if BATCH_COALESCED_RECVMSG {
                    // The legacy payload copy already gathers these bytes into
                    // one owned buffer and preflights one bounded chunk. Keep
                    // that exact chunk, then append metadata in the same order.
                    // Actual Files traffic includes64-byte coalesced receives.
                    // Keep small gathered bytes in bounded owned stack storage;
                    // larger chunks reuse owned transfer scratch, with the
                    // original fallible allocation if scratch is unavailable.
                    let mut small = [0u8; 256];
                    let large;
                    let data: &[u8] = if queued <= small.len() {
                        gather_payload(this, &mut small[..queued])?;
                        &small[..queued]
                    } else {
                        if let Some(scratch) = coalesced_scratch(runtime, this, queued)? {
                            scratch
                        } else {
                            large = coalesced_payload(this, queued)?;
                            &large
                        }
                    };
                    #[cfg(target_os = "none")]
                    if option_env!("NANAMI_UNIX_RECVMSG_DIAG") == Some("1") {
                        use core::sync::atomic::{AtomicBool, Ordering};
                        static REPORTED: AtomicBool = AtomicBool::new(false);
                        if !REPORTED.swap(true, Ordering::Relaxed) {
                            nanami_runtime::println!("[unix.recvmsg] coalesced-result-batch");
                        }
                        static FILES_REPORTED: AtomicBool = AtomicBool::new(false);
                        if diagnostic_files_receiver(runtime, pid)
                            && !FILES_REPORTED.swap(true, Ordering::Relaxed)
                        {
                            nanami_runtime::println!(
                                "[unix.files] coalesced-result-batch pid={} bytes={}", pid, queued,
                            );
                        }
                    }
                    return write_receive_result(
                        runtime, pid, header, 0, 0, output_flags as u32,
                        Some((message.vectors.parts().0[0], data)),
                    );
                }
            }
        }
        copy_queued_payload(runtime, pid, file.resource, message, queued)?;
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
            write_receive_result(
                runtime,
                pid,
                header,
                address_len as u32,
                control_len,
                output_flags as u32,
                None,
            )?;
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
        let notify_writers = writer_notification_needed(runtime, file.resource);
        let mut remaining = queued;
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
                let packet = this.packets.pop_front().unwrap();
                if !datagram && rights.is_empty() {
                    this.recycle_stream_buffer(packet.data);
                }
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
        if notify_writers {
            runtime.readiness_changes |= READY_UNIX_WRITE;
        }
    }
    if flags & MSG_TRUNC != 0 && datagram {
        Ok(packet_length)
    } else {
        Ok(queued)
    }
}

fn write_receive_result(
    runtime: &Runtime,
    pid: Word,
    header: Word,
    address_len: u32,
    control_len: Word,
    flags: u32,
    payload: Option<(Word, &[u8])>,
) -> Result<(), i32> {
    let address_len = address_len.to_ne_bytes();
    let control_len = (control_len as u64).to_ne_bytes();
    let flags = flags.to_ne_bytes();
    let fields = [
        (header + 8, &address_len[..]),
        (header + 40, &control_len[..]),
        (header + 48, &flags[..]),
    ];
    if let Some(payload) = payload {
        super::guest_memory::copyout_owned_many(
            runtime,
            pid,
            &[payload, fields[0], fields[1], fields[2]],
        )
    } else {
        super::guest_memory::copyout_owned_many(runtime, pid, &fields)
    }
}

fn empty_receive(runtime: &mut Runtime, pid: Word, message: &Message) -> Result<Word, i32> {
    if let Some(header) = message.header {
        write_receive_result(runtime, pid, header, 0, 0, 0, None)?;
    }
    Ok(0)
}

pub(super) fn io(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    context: LinuxSyscallContext,
) -> Result<Word, i32> {
    let mut message = Message {
        vectors: Vectors::One { base: 0, length: 0 },
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
            message.name = read_shm_u64(runtime, 0);
            message.name_len = read_shm_u32(runtime, 8) as Word;
            message.control = read_shm_u64(runtime, 32);
            message.control_len = read_shm_u64(runtime, 40);
            message.header = Some(a[1]);
            Some((read_shm_u64(runtime, 16), read_shm_u64(runtime, 24)))
        }
        SYS_READV | SYS_WRITEV => Some((a[1], a[2])),
        _ => None,
    };
    if let Some((iov, count)) = vector {
        if count == 1 {
            let mut bases = [0; 1];
            let mut lengths = [0; 1];
            read_linux_iovecs(runtime, pid, iov, count, &mut bases, &mut lengths)?;
            message.vectors = Vectors::One { base: bases[0], length: lengths[0] };
        } else if count == 0 {
            let mut bases = [];
            let mut lengths = [];
            read_linux_iovecs(runtime, pid, iov, count, &mut bases, &mut lengths)?;
            message.vectors = Vectors::Many { bases: &[], lengths: &[] };
        } else {
            return io_many(runtime, pid, file, context, message, iov, count);
        }
    } else {
        message.vectors = Vectors::One { base: a[1], length: a[2] };
        if a[1] == 0 && a[2] != 0 {
            return Err(EFAULT);
        }
        if context.number == SYS_SENDTO || context.number == SYS_RECVFROM {
            message.name = a[4];
            message.name_len = a[5];
        }
    }
    transfer(runtime, pid, file, context, &message)
}

// Keep the maximum-sized arrays out of scalar and single-iovec stack frames.
#[inline(never)]
fn io_many(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    context: LinuxSyscallContext,
    metadata: Message<'_>,
    iov: Word,
    count: Word,
) -> Result<Word, i32> {
    let mut bases = [0; LINUX_IOV_MAX as usize];
    let mut lengths = [0; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov, count, &mut bases, &mut lengths)?;
    let message = Message {
        vectors: Vectors::Many { bases: &bases[..count], lengths: &lengths[..count] },
        control: metadata.control,
        control_len: metadata.control_len,
        name: metadata.name,
        name_len: metadata.name_len,
        header: metadata.header,
    };
    transfer(runtime, pid, file, context, &message)
}

fn transfer(
    runtime: &mut Runtime,
    pid: Word,
    file: LinuxFile,
    context: LinuxSyscallContext,
    message: &Message<'_>,
) -> Result<Word, i32> {
    let flags = match context.number {
        SYS_SENDMSG | SYS_RECVMSG => context.args[2],
        SYS_SENDTO | SYS_RECVFROM => context.args[3],
        _ => 0,
    };
    match context.number {
        SYS_WRITE | SYS_WRITEV | SYS_SENDTO | SYS_SENDMSG => {
            send(runtime, pid, file, message, flags)
        }
        _ => receive(runtime, pid, file, message, flags, context),
    }
}

pub(super) fn discard(runtime: &mut Runtime, id: Word) {
    if let Some(socket) = runtime.unix_sockets.get_mut(id) {
        drop(socket.take_stream_buffer());
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
