use super::{
    map_network_bind_error, map_network_error, net, network_ipv4, read_network_u16_be,
    read_network_u32_be, read_shm_u16, read_shm_u32, read_shm_u8, read_sockaddr_nl,
    read_target_memory, write_sockaddr_nl, write_sockaddr_result, write_target_memory, write_u32,
    LinuxFile, LinuxFileKind, RequestError, Runtime, Word, ALTER_DEFAULT_SHM_BYTES, EADDRINUSE,
    EADDRNOTAVAIL, EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINPROGRESS, EINVAL, EMFILE, ENETDOWN,
    ENOTCONN, ENOTSOCK, EPROTONOSUPPORT, ESOCKTNOSUPPORT, LINUX_AF_INET, LINUX_AF_NETLINK,
    LINUX_FD_CLOEXEC, LINUX_IPPROTO_ICMP, LINUX_IPPROTO_TCP, LINUX_IPPROTO_UDP,
    LINUX_NETLINK_ROUTE, LINUX_SOCKADDR_IN_LEN, LINUX_SOCK_CLOEXEC, LINUX_SOCK_DGRAM,
    LINUX_SOCK_NONBLOCK, LINUX_SOCK_RAW, LINUX_SOCK_STREAM, LINUX_SOCK_TYPE_MASK, LINUX_SOL_SOCKET,
    LINUX_SO_ACCEPTCONN, LINUX_SO_TYPE, SLOT_NETWORK_SERVICE, TCP_FLAG_ACK, TCP_FLAG_FIN,
};

pub(super) fn ensure_network(runtime: &mut Runtime) -> Result<(), i32> {
    if runtime.network_port != 0 && runtime.network_shm != 0 {
        return Ok(());
    }
    nanami_services::registry::connect_network_service(SLOT_NETWORK_SERVICE)
        .map_err(|_| ENETDOWN)?;
    let port = libnanami::ipc::process_slot_descriptor(SLOT_NETWORK_SERVICE);
    let self_pid = libnanami::get_self_pid().map_err(|_| ENETDOWN)?;
    let (status, peer, size) = net::net_service_control_ex(
        port,
        net::NET_SERVICE_CONTROL_ATTACH_SHARED_MEMORY,
        self_pid,
        ALTER_DEFAULT_SHM_BYTES,
    )
    .map_err(map_network_error)?;
    if status != libnanami::OS_RESPONSE_OK || peer == 0 || size == 0 {
        return Err(ENETDOWN);
    }
    net::net_service_attach_rx_notification(port, libnanami::PROCESS_SLOT_NOTIFICATION)
        .map_err(map_network_error)?;
    runtime.network_port = port;
    runtime.network_shm = peer;
    runtime.network_shm_size = size;
    Ok(())
}

pub(super) fn sys_socket(
    runtime: &mut Runtime,
    pid: Word,
    domain: Word,
    socket_type: Word,
    protocol: Word,
) -> Result<Word, i32> {
    let base_type = socket_type & LINUX_SOCK_TYPE_MASK;
    let flags = (if (socket_type & LINUX_SOCK_CLOEXEC) != 0 {
        LINUX_FD_CLOEXEC
    } else {
        0
    }) | (socket_type & LINUX_SOCK_NONBLOCK);
    let file = match domain {
        LINUX_AF_INET => {
            ensure_network(runtime)?;
            match base_type {
                LINUX_SOCK_DGRAM if protocol == 0 || protocol == LINUX_IPPROTO_UDP => {
                    LinuxFile::socket_udp(flags)
                }
                LINUX_SOCK_STREAM if protocol == 0 || protocol == LINUX_IPPROTO_TCP => {
                    LinuxFile::socket_tcp(flags)
                }
                LINUX_SOCK_RAW if protocol == LINUX_IPPROTO_ICMP => LinuxFile::socket_icmp(flags),
                LINUX_SOCK_DGRAM | LINUX_SOCK_STREAM | LINUX_SOCK_RAW => {
                    return Err(EPROTONOSUPPORT);
                }
                _ => return Err(ESOCKTNOSUPPORT),
            }
        }
        LINUX_AF_NETLINK => {
            if base_type != LINUX_SOCK_RAW {
                return Err(ESOCKTNOSUPPORT);
            }
            if protocol != LINUX_NETLINK_ROUTE {
                return Err(EPROTONOSUPPORT);
            }
            ensure_network(runtime)?;
            LinuxFile::socket_netlink(flags)
        }
        _ => return Err(EAFNOSUPPORT),
    };
    runtime.allocate_linux_file(pid, file, 0).ok_or(EMFILE)
}

pub(super) fn sys_connect(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::SocketNetlink {
        read_sockaddr_nl(runtime, pid, address, address_len)?;
        file.local_port = 1;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(0);
    }
    if file.kind != LinuxFileKind::SocketUdp && file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    let (ip, port) = read_sockaddr_in(runtime, pid, address, address_len)?;
    file.peer_ip = ip;
    file.peer_port = port;
    if file.kind == LinuxFileKind::SocketTcp {
        if file.local_port == 0 {
            file.local_port = allocate_ephemeral_port(runtime)?;
        }
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        match net::net_service_tcp_connect(
            runtime.network_port,
            file.local_port,
            file.peer_ip,
            file.peer_port,
        ) {
            Ok(connection_id) if connection_id != 0 => {
                file.posix_fd = connection_id;
                if !runtime.set_linux_file(pid, fd, file) {
                    return Err(EBADF);
                }
                return Ok(0);
            }
            Ok(_) | Err(RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)) => {
                return Err(EINPROGRESS);
            }
            Err(error) => return Err(map_network_error(error)),
        }
    }
    file = ensure_udp_bound(runtime, pid, fd, file)?;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

pub(super) fn sys_bind(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind == LinuxFileKind::SocketNetlink {
        if file.local_port != 0 {
            return Err(EINVAL);
        }
        read_sockaddr_nl(runtime, pid, address, address_len)?;
        file.local_port = 1;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
        return Ok(0);
    }
    if file.kind != LinuxFileKind::SocketUdp && file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    if file.local_port != 0 {
        return Err(EINVAL);
    }
    let (ip, mut port) = read_sockaddr_in(runtime, pid, address, address_len)?;
    if ip != 0 && ip != network_ipv4(runtime)? {
        return Err(EADDRNOTAVAIL);
    }
    if port == 0 {
        port = allocate_ephemeral_port(runtime)?;
    } else if socket_port_in_use(runtime, file.kind, port) {
        return Err(EADDRINUSE);
    }
    if file.kind == LinuxFileKind::SocketUdp {
        bind_network_port(runtime, net::NET_SERVICE_CONTROL_UDP_BIND, port)?;
    }
    file.local_port = port;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

pub(super) fn sys_listen(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketTcp {
        return Err(ENOTSOCK);
    }
    if file.posix_fd != 0 {
        return Err(EINVAL);
    }
    if file.local_port == 0 {
        file.local_port = allocate_ephemeral_port(runtime)?;
    }
    bind_network_port(runtime, net::NET_SERVICE_CONTROL_TCP_BIND, file.local_port)?;
    file.kind = LinuxFileKind::SocketTcpListener;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(0)
}

pub(super) fn sys_accept(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
    flags: Word,
) -> Result<Word, i32> {
    let listener = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if listener.kind != LinuxFileKind::SocketTcpListener {
        return Err(ENOTSOCK);
    }
    if flags & !(LINUX_SOCK_NONBLOCK | LINUX_SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let connection_id = net::net_service_tcp_accept(runtime.network_port, listener.local_port, 0)
        .map_err(map_network_error)?;
    if connection_id == 0 {
        return Err(EAGAIN);
    }
    let peer_ip = read_network_u32_be(runtime, 0);
    let peer_port = read_network_u16_be(runtime, 4);
    let mut accepted = LinuxFile::socket_tcp(
        (flags & LINUX_SOCK_NONBLOCK)
            | if (flags & LINUX_SOCK_CLOEXEC) != 0 {
                LINUX_FD_CLOEXEC
            } else {
                0
            },
    );
    accepted.posix_fd = connection_id;
    accepted.local_port = listener.local_port;
    accepted.peer_ip = peer_ip;
    accepted.peer_port = peer_port;
    let new_fd = runtime
        .allocate_linux_file(pid, accepted, 0)
        .ok_or(EMFILE)?;
    if let Err(errno) =
        write_sockaddr_result(runtime, pid, address, address_len, peer_ip, peer_port)
    {
        let _ = runtime.clear_linux_file(pid, new_fd);
        return Err(errno);
    }
    Ok(new_fd)
}

pub(super) fn sys_shutdown(runtime: &mut Runtime, pid: Word, fd: Word) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketTcp || file.posix_fd == 0 {
        return Err(ENOTCONN);
    }
    net::net_service_tcp_send_on_connection(
        runtime.network_port,
        file.posix_fd,
        0,
        0,
        TCP_FLAG_FIN | TCP_FLAG_ACK,
    )
    .map_err(map_network_error)?;
    Ok(0)
}

pub(super) fn sys_getsockname(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    address: Word,
    address_len: Word,
    peer: bool,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if file.kind == LinuxFileKind::SocketNetlink {
        if peer {
            return Err(ENOTCONN);
        }
        return write_sockaddr_nl(runtime, pid, address, address_len);
    }
    let (ip, port) = if peer {
        if file.peer_ip == 0 || file.peer_port == 0 {
            return Err(ENOTCONN);
        }
        (file.peer_ip, file.peer_port)
    } else {
        (network_ipv4(runtime)?, file.local_port)
    };
    write_sockaddr_result(runtime, pid, address, address_len, ip, port)?;
    Ok(0)
}

pub(super) fn sys_setsockopt(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    _level: Word,
    _option: Word,
    value: Word,
    value_len: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if value == 0 && value_len != 0 {
        return Err(EFAULT);
    }
    Ok(0)
}

pub(super) fn sys_getsockopt(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    level: Word,
    option: Word,
    value: Word,
    value_len: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if !is_socket_kind(file.kind) {
        return Err(ENOTSOCK);
    }
    if value == 0 || value_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, value_len, 4)?;
    let requested = read_shm_u32(runtime, 0) as Word;
    let result = if level == LINUX_SOL_SOCKET && option == LINUX_SO_TYPE {
        match file.kind {
            LinuxFileKind::SocketUdp => LINUX_SOCK_DGRAM,
            LinuxFileKind::SocketIcmp | LinuxFileKind::SocketNetlink => LINUX_SOCK_RAW,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener => LINUX_SOCK_STREAM,
            _ => return Err(ENOTSOCK),
        }
    } else if level == LINUX_SOL_SOCKET && option == LINUX_SO_ACCEPTCONN {
        (file.kind == LinuxFileKind::SocketTcpListener) as Word
    } else {
        0
    };
    let copy_len = requested.min(4);
    unsafe {
        write_u32(runtime.posix_shm, result as u32);
    }
    write_target_memory(runtime, pid, value, copy_len)?;
    unsafe {
        write_u32(runtime.posix_shm, 4);
    }
    write_target_memory(runtime, pid, value_len, 4)?;
    Ok(0)
}

pub(super) fn ensure_udp_bound(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    mut file: LinuxFile,
) -> Result<LinuxFile, i32> {
    if file.local_port != 0 {
        return Ok(file);
    }
    file.local_port = allocate_ephemeral_port(runtime)?;
    bind_network_port(runtime, net::NET_SERVICE_CONTROL_UDP_BIND, file.local_port)?;
    if !runtime.set_linux_file(pid, fd, file) {
        return Err(EBADF);
    }
    Ok(file)
}

pub(super) fn bind_network_port(runtime: &Runtime, control: Word, port: u16) -> Result<(), i32> {
    net::net_service_control(runtime.network_port, control, port as Word, 0)
        .map_err(map_network_bind_error)
}

pub(super) fn allocate_ephemeral_port(runtime: &mut Runtime) -> Result<u16, i32> {
    let mut attempts = 0usize;
    while attempts < 16384 {
        let port = runtime.next_ephemeral_port.max(49152);
        runtime.next_ephemeral_port = if port == 65535 { 49152 } else { port + 1 };
        if !socket_any_port_in_use(runtime, port) {
            return Ok(port);
        }
        attempts += 1;
    }
    Err(EADDRINUSE)
}

pub(super) fn socket_port_in_use(runtime: &Runtime, kind: LinuxFileKind, port: u16) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|file| {
                file.is_open()
                    && file.local_port == port
                    && socket_kinds_share_port_space(file.kind, kind)
            })
    })
}

pub(super) fn socket_kinds_share_port_space(left: LinuxFileKind, right: LinuxFileKind) -> bool {
    matches!(left, LinuxFileKind::SocketUdp) && matches!(right, LinuxFileKind::SocketUdp)
        || matches!(
            left,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener
        ) && matches!(
            right,
            LinuxFileKind::SocketTcp | LinuxFileKind::SocketTcpListener
        )
}

pub(super) fn socket_any_port_in_use(runtime: &Runtime, port: u16) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|file| {
                file.is_open()
                    && file.local_port == port
                    && matches!(
                        file.kind,
                        LinuxFileKind::SocketUdp
                            | LinuxFileKind::SocketTcp
                            | LinuxFileKind::SocketTcpListener
                    )
            })
    })
}

pub(super) fn is_socket_kind(kind: LinuxFileKind) -> bool {
    matches!(
        kind,
        LinuxFileKind::SocketUdp
            | LinuxFileKind::SocketTcp
            | LinuxFileKind::SocketTcpListener
            | LinuxFileKind::SocketIcmp
            | LinuxFileKind::SocketNetlink
    )
}

pub(super) fn read_sockaddr_in(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<(u32, u16), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    if address_len < LINUX_SOCKADDR_IN_LEN {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, address, LINUX_SOCKADDR_IN_LEN)?;
    if read_shm_u16(runtime, 0) as Word != LINUX_AF_INET {
        return Err(EAFNOSUPPORT);
    }
    let port = ((read_shm_u8(runtime, 2) as u16) << 8) | read_shm_u8(runtime, 3) as u16;
    let ip = ((read_shm_u8(runtime, 4) as u32) << 24)
        | ((read_shm_u8(runtime, 5) as u32) << 16)
        | ((read_shm_u8(runtime, 6) as u32) << 8)
        | read_shm_u8(runtime, 7) as u32;
    Ok((ip, port))
}

pub(super) fn close_socket_file(runtime: &mut Runtime, file: LinuxFile) {
    if socket_file_still_open(runtime, file) || runtime.network_port == 0 {
        return;
    }
    match file.kind {
        LinuxFileKind::SocketUdp if file.local_port != 0 => {
            let _ = net::net_service_control(
                runtime.network_port,
                net::NET_SERVICE_CONTROL_UDP_UNBIND,
                file.local_port as Word,
                0,
            );
        }
        LinuxFileKind::SocketTcpListener if file.local_port != 0 => {
            let _ = net::net_service_control(
                runtime.network_port,
                net::NET_SERVICE_CONTROL_TCP_UNBIND,
                file.local_port as Word,
                0,
            );
        }
        LinuxFileKind::SocketTcp if file.posix_fd != 0 => {
            let _ = net::net_service_tcp_send_on_connection(
                runtime.network_port,
                file.posix_fd,
                0,
                0,
                TCP_FLAG_FIN | TCP_FLAG_ACK,
            );
        }
        _ => {}
    }
}

pub(super) fn socket_file_still_open(runtime: &Runtime, file: LinuxFile) -> bool {
    runtime.managed.iter().any(|process| {
        process.pid != 0
            && process.files.iter().any(|candidate| {
                candidate.is_open()
                    && candidate.kind == file.kind
                    && candidate.posix_fd == file.posix_fd
                    && candidate.local_port == file.local_port
            })
    })
}
