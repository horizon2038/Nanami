//! AF_UNIX is local to the Alter instance, not an IPv4/net-server transport.
use super::*;
use crate::state::readiness::{PollFd, ReadinessWait, READY_UNIX};
use crate::state::unix_sockets::{Address, Credentials, Kind, Name, Socket};
use alloc::vec::Vec;

pub(super) const AF_UNIX: Word = 1;
pub(super) const SOCK_SEQPACKET: Word = 5;
pub(super) const MSG_PEEK: Word = 2;
pub(super) const MSG_CTRUNC: Word = 8;
pub(super) const MSG_TRUNC: Word = 0x20;
pub(super) const MSG_DONTWAIT: Word = 0x40;
pub(super) const MSG_NOSIGNAL: Word = 0x4000;
pub(super) const MSG_CMSG_CLOEXEC: Word = 0x4000_0000;
pub(super) const ECONNREFUSED: i32 = 111;
pub(super) const EISCONN: i32 = 106;
pub(super) const EPROTOTYPE: i32 = 91;
const ENOPROTOOPT: i32 = 92;
const SO_REUSEADDR: Word = 2;
const SO_TYPE: Word = 3;
const SO_ERROR: Word = 4;
const SO_SNDBUF: Word = 7;
const SO_RCVBUF: Word = 8;
const SO_PEERCRED: Word = 17;
const SO_ACCEPTCONN: Word = 30;
const SO_PROTOCOL: Word = 38;
const SO_DOMAIN: Word = 39;

pub(super) fn file(id: Word, flags: Word) -> LinuxFile {
    LinuxFile {
        kind: LinuxFileKind::SocketUnix,
        posix_fd: id,
        resource: id,
        flags: if flags & LINUX_SOCK_CLOEXEC != 0 {
            LINUX_FD_CLOEXEC
        } else {
            0
        },
        ..LinuxFile::EMPTY
    }
}

fn credentials(pid: Word) -> Credentials {
    // Alter's current credential backend exposes uid/gid zero.
    Credentials {
        pid: pid as u32,
        uid: 0,
        gid: 0,
    }
}

fn kind(value: Word) -> Result<Kind, i32> {
    match value & LINUX_SOCK_TYPE_MASK {
        LINUX_SOCK_STREAM => Ok(Kind::Stream),
        LINUX_SOCK_DGRAM => Ok(Kind::Datagram),
        SOCK_SEQPACKET => Ok(Kind::SeqPacket),
        _ => Err(ESOCKTNOSUPPORT),
    }
}

fn create_id(
    runtime: &mut Runtime,
    pid: Word,
    socket_type: Word,
    protocol: Word,
) -> Result<Word, i32> {
    if socket_type & !(LINUX_SOCK_TYPE_MASK | LINUX_SOCK_NONBLOCK | LINUX_SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    if protocol != 0 {
        return Err(EPROTONOSUPPORT);
    }
    let owner = credentials(runtime.thread_group(pid));
    runtime
        .unix_sockets
        .create(
            kind(socket_type)?,
            owner,
            socket_type & LINUX_SOCK_NONBLOCK != 0,
        )
        .ok_or(ENOMEM)
}

pub(super) fn create(
    runtime: &mut Runtime,
    pid: Word,
    socket_type: Word,
    protocol: Word,
) -> Result<Word, i32> {
    let id = create_id(runtime, pid, socket_type, protocol)?;
    match runtime.allocate_linux_file(pid, file(id, socket_type), 0) {
        Some(fd) => Ok(fd),
        None => {
            runtime.unix_sockets.remove(id);
            Err(EMFILE)
        }
    }
}

fn pair(runtime: &mut Runtime, pid: Word, args: [Word; 6]) -> Result<Word, i32> {
    if args[0] != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let left = create_id(runtime, pid, args[1], args[2])?;
    let right = match create_id(runtime, pid, args[1], args[2]) {
        Ok(id) => id,
        Err(error) => {
            runtime.unix_sockets.remove(left);
            return Err(error);
        }
    };
    let owner = credentials(runtime.thread_group(pid));
    join(runtime, left, right, owner, owner);
    let a = runtime.allocate_linux_file(pid, file(left, args[1]), 0);
    let b = runtime.allocate_linux_file(pid, file(right, args[1]), 0);
    let result = match (a, b) {
        (Some(a), Some(b)) => {
            unsafe {
                write_u32(runtime.posix_shm, a as u32);
                write_u32(runtime.posix_shm + 4, b as u32);
            }
            write_target_memory(runtime, pid, args[3], 8)
                .map(|_| 0)
                .map_err(|_| EFAULT)
        }
        _ => Err(EMFILE),
    };
    if result.is_err() {
        if let Some(fd) = a {
            runtime.clear_linux_file(pid, fd);
        }
        if let Some(fd) = b {
            runtime.clear_linux_file(pid, fd);
        }
        collect(runtime);
    }
    result
}

fn join(runtime: &mut Runtime, left: Word, right: Word, a: Credentials, b: Credentials) {
    let left_address = runtime.unix_sockets.get(left).unwrap().address;
    let right_address = runtime.unix_sockets.get(right).unwrap().address;
    let socket = runtime.unix_sockets.get_mut(left).unwrap();
    socket.peer = Some(right);
    socket.peer_address = right_address;
    socket.peer_credentials = Some(b);
    let socket = runtime.unix_sockets.get_mut(right).unwrap();
    socket.peer = Some(left);
    socket.peer_address = left_address;
    socket.peer_credentials = Some(a);
}

pub(super) fn socket(runtime: &Runtime, id: Word) -> Result<&Socket, i32> {
    runtime.unix_sockets.get(id).ok_or(EBADF)
}

fn read_address(
    runtime: &mut Runtime,
    pid: Word,
    pointer: Word,
    len: Word,
) -> Result<Address, i32> {
    if !(3..=110).contains(&len) {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, pointer, len).map_err(|_| EFAULT)?;
    let bytes = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, len) };
    if u16::from_ne_bytes(bytes[..2].try_into().unwrap()) != AF_UNIX as u16 {
        return Err(EAFNOSUPPORT);
    }
    let mut address = Address::UNNAMED;
    address.bytes[..len].copy_from_slice(bytes);
    address.len = len;
    if bytes[2] != 0 {
        let used = bytes[2..]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(len - 2);
        if used == 0 {
            return Err(EINVAL);
        }
        address.len = used + 3;
    }
    Ok(address)
}

fn address_name(
    runtime: &mut Runtime,
    pid: Word,
    address: Address,
    create: bool,
) -> Result<(Name, Option<Word>), i32> {
    if address.bytes[2] == 0 {
        let namespace = runtime.managed_process(pid).ok_or(ESRCH)?.rootfs.leader;
        let mut bytes = Vec::new();
        bytes.try_reserve(address.len - 2).map_err(|_| ENOMEM)?;
        bytes.extend_from_slice(&address.bytes[2..address.len]);
        return Ok((Name::Abstract { namespace, bytes }, None));
    }
    let len = address.len - 3;
    unsafe {
        core::ptr::copy_nonoverlapping(
            address.bytes[2..].as_ptr(),
            runtime.posix_shm as *mut u8,
            len,
        );
    }
    let len = resolve_current_shm_path(runtime, pid, len)?;
    if create {
        reject_virtual_fs_mutation(runtime, pid, runtime.posix_shm, len)?;
    }
    let len = translate_guest_path_for_vfs(runtime, pid, len)?;
    if create {
        let mode = super::filesystem::creation_mode(runtime, pid, 0o777)?;
        let (inode, pin) = posix::posix_create_socket_node_mode(runtime.posix_port, 0, len, mode)
            .map_err(|error| match error {
            RequestError::Status(nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT) => EADDRINUSE,
            error => map_path_request_error(error),
        })?;
        return Ok((Name::Inode(inode), Some(pin)));
    }
    let stat = posix::posix_stat(runtime.posix_port, 0, len).map_err(map_path_request_error)?;
    if stat.2 != posix::POSIX_FILE_TYPE_SOCKET {
        return Err(ECONNREFUSED);
    }
    Ok((Name::Inode(stat.0), None))
}

pub(super) fn destination(
    runtime: &mut Runtime,
    pid: Word,
    pointer: Word,
    len: Word,
) -> Result<Word, i32> {
    let address = read_address(runtime, pid, pointer, len)?;
    let (name, _) = address_name(runtime, pid, address, false)?;
    runtime.unix_sockets.find_name(&name).ok_or(ECONNREFUSED)
}

fn bind(runtime: &mut Runtime, pid: Word, id: Word, pointer: Word, len: Word) -> Result<Word, i32> {
    if socket(runtime, id)?.address.len != 2 {
        return Err(EINVAL);
    }
    let address = read_address(runtime, pid, pointer, len)?;
    let (name, pin) = address_name(runtime, pid, address, true)?;
    if runtime.unix_sockets.find_name(&name).is_some() {
        if let Some(pin) = pin {
            let _ = posix::posix_close(runtime.posix_port, pin);
        }
        return Err(EADDRINUSE);
    }
    let socket = runtime.unix_sockets.get_mut(id).ok_or(EBADF)?;
    socket.name = Some(name);
    socket.inode_pin = pin;
    socket.address = address;
    Ok(0)
}

fn listen(runtime: &mut Runtime, pid: Word, id: Word, backlog: Word) -> Result<Word, i32> {
    let owner = credentials(runtime.thread_group(pid));
    let socket = runtime.unix_sockets.get_mut(id).ok_or(EBADF)?;
    if socket.kind == Kind::Datagram {
        return Err(EOPNOTSUPP);
    }
    if socket.peer.is_some() || socket.address.len == 2 {
        return Err(EINVAL);
    }
    socket.listening = true;
    socket.credentials = owner;
    socket.backlog = (backlog as i32).clamp(0, 127) as usize + 1;
    Ok(0)
}

fn connect(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    pointer: Word,
    len: Word,
) -> Result<Word, i32> {
    let this = socket(runtime, id)?;
    let kind = this.kind;
    if kind == Kind::Datagram && len == 2 {
        read_target_memory(runtime, pid, pointer, 2).map_err(|_| EFAULT)?;
        if read_shm_u16(runtime, 0) == 0 {
            runtime.unix_sockets.get_mut(id).unwrap().peer = None;
            return Ok(0);
        }
    }
    if this.listening {
        return Err(EINVAL);
    }
    if kind != Kind::Datagram && this.peer.is_some() {
        return Err(EISCONN);
    }
    let destination = destination(runtime, pid, pointer, len)?;
    let target = socket(runtime, destination)?;
    if target.kind != kind {
        return Err(EPROTOTYPE);
    }
    if kind == Kind::Datagram {
        let address = target.address;
        let socket = runtime.unix_sockets.get_mut(id).unwrap();
        socket.peer = Some(destination);
        socket.peer_address = address;
        return Ok(0);
    }
    if !target.listening {
        return Err(ECONNREFUSED);
    }
    if target.pending.len() >= target.backlog {
        return Err(EAGAIN);
    }
    let peer_cred = target.credentials;
    let local_address = target.address;
    runtime
        .unix_sockets
        .get_mut(destination)
        .unwrap()
        .pending
        .try_reserve(1)
        .map_err(|_| ENOMEM)?;
    let child = runtime
        .unix_sockets
        .create(kind, peer_cred, false)
        .ok_or(ENOMEM)?;
    runtime.unix_sockets.get_mut(child).unwrap().address = local_address;
    let owner = credentials(runtime.thread_group(pid));
    join(runtime, id, child, owner, peer_cred);
    runtime
        .unix_sockets
        .get_mut(destination)
        .unwrap()
        .pending
        .push_back(child);
    runtime.readiness_changes |= READY_UNIX;
    Ok(0)
}

pub(super) fn write_address(
    runtime: &mut Runtime,
    pid: Word,
    address: Address,
    pointer: Word,
    length: Word,
) -> Result<(), i32> {
    if pointer == 0 {
        return Ok(());
    }
    read_target_memory(runtime, pid, length, 4).map_err(|_| EFAULT)?;
    let capacity = read_shm_u32(runtime, 0) as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(
            address.bytes.as_ptr(),
            runtime.posix_shm as *mut u8,
            address.len,
        );
    }
    write_target_memory(runtime, pid, pointer, capacity.min(address.len)).map_err(|_| EFAULT)?;
    write_u32_to_target(runtime, pid, length, address.len as u32).map(|_| ())
}

fn accept(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    address: Word,
    len: Word,
    flags: Word,
) -> Result<Word, i32> {
    if flags & !(LINUX_SOCK_NONBLOCK | LINUX_SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let listener = socket(runtime, id)?;
    if !listener.listening {
        return Err(EINVAL);
    }
    let child = *listener.pending.front().ok_or(EAGAIN)?;
    let peer_address = socket(runtime, child)?.peer_address;
    let fd = runtime
        .allocate_linux_file(pid, file(child, flags), 0)
        .ok_or(EMFILE)?;
    if let Err(error) = write_address(runtime, pid, peer_address, address, len) {
        runtime.clear_linux_file(pid, fd);
        return Err(error);
    }
    runtime
        .unix_sockets
        .get_mut(id)
        .unwrap()
        .pending
        .pop_front();
    runtime.unix_sockets.get_mut(child).unwrap().nonblocking = flags & LINUX_SOCK_NONBLOCK != 0;
    runtime.readiness_changes |= READY_UNIX;
    Ok(fd)
}

fn shutdown(runtime: &mut Runtime, id: Word, how: Word) -> Result<Word, i32> {
    if how > 2 {
        return Err(EINVAL);
    }
    if socket(runtime, id)?.peer.is_none() {
        return Err(ENOTCONN);
    }
    let socket = runtime.unix_sockets.get_mut(id).unwrap();
    if how == 0 || how == 2 {
        socket.read_closed = true;
    }
    if how == 1 || how == 2 {
        socket.write_closed = true;
    }
    if how == 0 || how == 2 {
        super::unix_message::discard(runtime, id);
    }
    runtime.readiness_changes |= READY_UNIX;
    Ok(0)
}

pub(super) fn readiness(runtime: &Runtime, id: Word) -> Result<i16, i32> {
    let this = socket(runtime, id)?;
    if this.listening {
        return Ok(if this.pending.is_empty() {
            0
        } else {
            LINUX_POLLIN | LINUX_POLLRDNORM
        });
    }
    let peer = this.peer.and_then(|id| runtime.unix_sockets.get(id));
    let eof = this.read_closed
        || (this.kind != Kind::Datagram
            && (peer.is_none() || peer.is_some_and(|p| p.write_closed)));
    let mut ready = 0;
    if !this.packets.is_empty() || eof {
        ready |= LINUX_POLLIN | LINUX_POLLRDNORM;
    }
    if eof {
        ready |= LINUX_POLLRDHUP;
    }
    if this.write_closed || peer.is_none_or(|p| p.read_closed) {
        if this.peer.is_some() {
            ready |= LINUX_POLLOUT | LINUX_POLLERR;
        }
    } else if let Some(peer) = peer {
        if peer.queued_bytes < peer.receive_limit.min(this.send_limit)
            && peer.queued_rights < crate::state::unix_sockets::RIGHTS_LIMIT
            && peer.packets.len() < crate::state::unix_sockets::QUEUE_MESSAGES
        {
            ready |= LINUX_POLLOUT | LINUX_POLLWRNORM;
        }
    }
    if this.kind == Kind::Datagram && this.peer.is_none() {
        ready |= LINUX_POLLOUT | LINUX_POLLWRNORM;
    }
    if this.kind != Kind::Datagram
        && (peer.is_none()
            || (eof && this.write_closed)
            || peer.is_some_and(|peer| peer.read_closed && peer.write_closed))
    {
        ready |= LINUX_POLLHUP;
    }
    if this.read_closed && this.write_closed {
        ready |= LINUX_POLLHUP;
    }
    Ok(ready)
}

fn option(
    runtime: &mut Runtime,
    pid: Word,
    id: Word,
    args: [Word; 6],
    set: bool,
) -> Result<Word, i32> {
    if args[1] != LINUX_SOL_SOCKET {
        return Err(ENOPROTOOPT);
    }
    if set {
        if args[4] < 4 {
            return Err(EINVAL);
        }
        read_target_memory(runtime, pid, args[3], 4).map_err(|_| EFAULT)?;
        let value = read_shm_u32(runtime, 0) as usize;
        let socket = runtime.unix_sockets.get_mut(id).ok_or(EBADF)?;
        let size = value
            .saturating_mul(2)
            .clamp(256, crate::state::unix_sockets::QUEUE_BYTES);
        match args[2] {
            SO_SNDBUF => socket.send_limit = size,
            SO_RCVBUF => socket.receive_limit = size,
            SO_REUSEADDR => {} // This does not change UNIX name uniqueness.
            _ => return Err(ENOPROTOOPT),
        }
        runtime.readiness_changes |= READY_UNIX;
        return Ok(0);
    }
    read_target_memory(runtime, pid, args[4], 4).map_err(|_| EFAULT)?;
    let capacity = read_shm_u32(runtime, 0) as usize;
    let socket = socket(runtime, id)?;
    let mut output = [0u8; 12];
    let size = if args[2] == SO_PEERCRED {
        let cred = socket.peer_credentials.ok_or(ENOTCONN)?;
        output[..4].copy_from_slice(&cred.pid.to_ne_bytes());
        output[4..8].copy_from_slice(&cred.uid.to_ne_bytes());
        output[8..].copy_from_slice(&cred.gid.to_ne_bytes());
        12
    } else {
        let value: u32 = match args[2] {
            SO_TYPE => match socket.kind {
                Kind::Stream => 1,
                Kind::Datagram => 2,
                Kind::SeqPacket => 5,
            },
            SO_ERROR => 0, // Connect completion is synchronous locally.
            SO_SNDBUF => socket.send_limit as u32,
            SO_RCVBUF => socket.receive_limit as u32,
            SO_ACCEPTCONN => socket.listening as u32,
            SO_PROTOCOL => 0,
            SO_DOMAIN => AF_UNIX as u32,
            _ => return Err(ENOPROTOOPT),
        };
        output[..4].copy_from_slice(&value.to_ne_bytes());
        4
    };
    unsafe {
        core::ptr::copy_nonoverlapping(output.as_ptr(), runtime.posix_shm as *mut u8, size);
    }
    write_target_memory(runtime, pid, args[3], size.min(capacity)).map_err(|_| EFAULT)?;
    write_u32_to_target(runtime, pid, args[4], size as u32).map(|_| 0)
}

pub(super) fn nonblocking(runtime: &Runtime, file: LinuxFile) -> bool {
    runtime
        .unix_sockets
        .get(file.resource)
        .is_none_or(|socket| socket.nonblocking)
}

fn attempt(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Option<Result<Word, i32>> {
    let file = runtime.linux_file(pid, context.args[0])?;
    if file.kind != LinuxFileKind::SocketUnix {
        return None;
    }
    let id = file.resource;
    let a = context.args;
    Some(match context.number {
        SYS_BIND => bind(runtime, pid, id, a[1], a[2]),
        SYS_LISTEN => listen(runtime, pid, id, a[1]),
        SYS_CONNECT => connect(runtime, pid, id, a[1], a[2]),
        SYS_ACCEPT | SYS_ACCEPT4 => accept(
            runtime,
            pid,
            id,
            a[1],
            a[2],
            if context.number == SYS_ACCEPT4 {
                a[3]
            } else {
                0
            },
        ),
        SYS_SHUTDOWN => shutdown(runtime, id, a[1]),
        SYS_GETSOCKNAME | SYS_GETPEERNAME => {
            let address = socket(runtime, id).and_then(|socket| {
                let address = if context.number == SYS_GETPEERNAME {
                    socket
                        .peer
                        .map(|id| {
                            runtime
                                .unix_sockets
                                .get(id)
                                .map_or(socket.peer_address, |peer| peer.address)
                        })
                        .ok_or(ENOTCONN)
                } else {
                    Ok(socket.address)
                }?;
                Ok(address)
            });
            address.and_then(|address| write_address(runtime, pid, address, a[1], a[2]).map(|_| 0))
        }
        SYS_SETSOCKOPT | SYS_GETSOCKOPT => {
            option(runtime, pid, id, a, context.number == SYS_SETSOCKOPT)
        }
        SYS_READ | SYS_WRITE | SYS_READV | SYS_WRITEV | SYS_SENDTO | SYS_RECVFROM | SYS_SENDMSG
        | SYS_RECVMSG => super::unix_message::io(runtime, pid, file, context),
        _ => return None,
    })
}

fn message_flags(context: LinuxSyscallContext) -> Word {
    match context.number {
        SYS_SENDTO | SYS_RECVFROM => context.args[3],
        SYS_SENDMSG | SYS_RECVMSG => context.args[2],
        _ => 0,
    }
}

pub(super) fn retry(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Option<Result<Word, i32>> {
    let result = attempt(runtime, pid, context)?;
    if result == Err(EPIPE) && message_flags(context) & MSG_NOSIGNAL == 0 {
        let _ = signals_runtime::queue(runtime, pid, crate::common::signals::Info::new(13, 0, pid));
    }
    Some(result)
}

pub(super) fn dispatch(
    runtime: &mut Runtime,
    pid: Word,
    context: LinuxSyscallContext,
) -> Option<EmulationAction> {
    let result = if context.number == SYS_SOCKETPAIR {
        pair(runtime, pid, context.args)
    } else if context.number == SYS_SOCKET && context.args[0] == AF_UNIX {
        create(runtime, pid, context.args[1], context.args[2])
    } else {
        retry(runtime, pid, context)?
    };
    let flags = message_flags(context);
    if result == Err(EAGAIN) {
        let file = runtime.linux_file(pid, context.args[0])?;
        if !nonblocking(runtime, file) && flags & MSG_DONTWAIT == 0 {
            let events = match context.number {
                SYS_READ | SYS_READV | SYS_RECVFROM | SYS_RECVMSG | SYS_ACCEPT | SYS_ACCEPT4 => {
                    LINUX_POLLIN
                }
                SYS_WRITE | SYS_WRITEV | SYS_SENDTO | SYS_SENDMSG | SYS_CONNECT => LINUX_POLLOUT,
                _ => return Some(EmulationAction::Return(-(EAGAIN as isize))),
            };
            let mut entries = [PollFd::EMPTY; LINUX_FD_MAX];
            entries[0] = PollFd {
                fd: context.args[0] as i32,
                events,
                revents: 0,
            };
            runtime.managed_process_mut(pid)?.readiness_wait = Some(ReadinessWait {
                context,
                entries,
                count: 1,
                sources: READY_UNIX,
                deadline: None,
                select: false,
            });
            return Some(EmulationAction::Park);
        }
    }
    Some(EmulationAction::Return(result_to_linux_return(result)))
}

/// Trace descriptor/accept ownership so SCM_RIGHTS cycles cannot leak sockets.
pub(super) fn collect(runtime: &mut Runtime) {
    if runtime.unix_sockets.is_empty() {
        return;
    }
    runtime.unix_sockets.reset_marks();
    for process in &runtime.managed {
        if process.pid == 0 {
            continue;
        }
        for file in process.files.iter() {
            if file.kind == LinuxFileKind::SocketUnix {
                runtime.unix_sockets.mark(file.resource);
            }
        }
    }
    runtime.unix_sockets.trace_marks();
    // Queued descriptions in unreachable endpoints are not GC roots. Exclude
    // them during the sweep so cleanup needs no allocation or recursion.
    runtime.unix_sockets.set_sweeping(true);
    while let Some(socket) = runtime.unix_sockets.take_unmarked() {
        super::epoll::forget_closed(runtime, 0, file(socket.id, 0));
        if let Some(pin) = socket.inode_pin {
            let _ = posix::posix_close(runtime.posix_port, pin);
        }
        for packet in socket.packets {
            for file in packet.rights {
                release_held(runtime, file);
            }
        }
    }
    runtime.unix_sockets.set_sweeping(false);
    runtime.readiness_changes |= READY_UNIX;
}

pub(super) fn release_held(runtime: &mut Runtime, file: LinuxFile) {
    if file.kind == LinuxFileKind::SocketUnix {
        return;
    }
    super::epoll::forget_closed(runtime, 0, file);
    super::descriptors::release_file_description(runtime, file);
}
