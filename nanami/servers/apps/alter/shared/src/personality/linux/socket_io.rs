use super::{
    build_netlink_dump, ensure_udp_bound, map_network_error, map_request_error, net, network_ipv4,
    process_netlink_request, read_linux_iovecs, read_linux_msghdr, read_network_u16_be,
    read_network_u32_be, read_sockaddr_in, read_target_memory_to_network, sys_netlink_send,
    write_linux_ipv4_header, write_sockaddr_nl_value, write_sockaddr_result, write_target_memory,
    write_u32_to_target, LinuxFileKind, RequestError, Runtime, Word, EAGAIN, EBADF, EDESTADDRREQ,
    EFAULT, EMSGSIZE, ENOTCONN, ENOTSOCK, EOPNOTSUPP, ICMP_SOCKET_PAYLOAD_MAX,
    LINUX_ICMP_HEADER_LEN, LINUX_IOV_MAX, LINUX_IPPROTO_ICMP, LINUX_IPV4_HEADER_LEN,
    LINUX_SOCKADDR_NL_LEN, NETWORK_PAYLOAD_OFFSET, NETWORK_SEND_RETRIES, TCP_FLAG_ACK,
    TCP_FLAG_PSH, TCP_SOCKET_PAYLOAD_MAX, UDP_SOCKET_PAYLOAD_MAX,
};

pub(super) fn sys_sendto(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    sys_socket_send(runtime, pid, fd, buffer, len, address, address_len)
}

pub(super) fn sys_socket_send(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    match file.kind {
        LinuxFileKind::SocketUdp => {
            if len > UDP_SOCKET_PAYLOAD_MAX {
                return Err(EMSGSIZE);
            }
            let (peer_ip, peer_port) = if address != 0 {
                read_sockaddr_in(runtime, pid, address, address_len)?
            } else if file.peer_ip != 0 && file.peer_port != 0 {
                (file.peer_ip, file.peer_port)
            } else {
                return Err(EDESTADDRREQ);
            };
            file = ensure_udp_bound(runtime, pid, fd, file)?;
            read_target_memory_to_network(runtime, pid, buffer, len)?;
            let mut attempts = 0usize;
            loop {
                match net::net_service_udp_send(
                    runtime.network_port,
                    0,
                    len,
                    file.local_port,
                    peer_port,
                    peer_ip,
                ) {
                    Ok(_) => return Ok(len),
                    Err(error) if attempts < NETWORK_SEND_RETRIES => {
                        attempts += 1;
                        let _ = net::net_service_control(
                            runtime.network_port,
                            net::NET_SERVICE_CONTROL_POLL,
                            0,
                            0,
                        );
                        if !matches!(
                            error,
                            RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)
                        ) {
                            return Err(map_network_error(error));
                        }
                    }
                    Err(error) => return Err(map_network_error(error)),
                }
            }
        }
        LinuxFileKind::SocketTcp => {
            if file.posix_fd == 0 {
                return Err(ENOTCONN);
            }
            let mut done = 0;
            while done < len {
                let chunk = ::core::cmp::min(
                    len - done,
                    runtime.network_shm_size.min(TCP_SOCKET_PAYLOAD_MAX),
                );
                read_target_memory_to_network(runtime, pid, buffer + done, chunk)?;
                let sent = net::net_service_tcp_send_on_connection(
                    runtime.network_port,
                    file.posix_fd,
                    0,
                    chunk,
                    TCP_FLAG_ACK | TCP_FLAG_PSH,
                )
                .map_err(map_network_error)?;
                done += sent;
                if sent < chunk {
                    break;
                }
            }
            Ok(done)
        }
        LinuxFileKind::SocketIcmp => {
            if len < LINUX_ICMP_HEADER_LEN || len > ICMP_SOCKET_PAYLOAD_MAX {
                return Err(EMSGSIZE);
            }
            let (peer_ip, _) = if address != 0 {
                read_sockaddr_in(runtime, pid, address, address_len)?
            } else if file.peer_ip != 0 {
                (file.peer_ip, 0)
            } else {
                return Err(EDESTADDRREQ);
            };
            read_target_memory_to_network(runtime, pid, buffer, len)?;
            let identifier = read_network_u16_be(runtime, 4);
            file.local_port = identifier;
            file.peer_ip = peer_ip;
            if !runtime.set_linux_file(pid, fd, file) {
                return Err(EBADF);
            }
            let mut attempts = 0usize;
            loop {
                match net::net_service_icmp_send(runtime.network_port, 0, len, identifier, peer_ip)
                {
                    Ok(_) => return Ok(len),
                    Err(error) if attempts < NETWORK_SEND_RETRIES => {
                        attempts += 1;
                        let _ = net::net_service_control(
                            runtime.network_port,
                            net::NET_SERVICE_CONTROL_POLL,
                            0,
                            0,
                        );
                        if !matches!(
                            error,
                            RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)
                        ) {
                            return Err(map_network_error(error));
                        }
                    }
                    Err(error) => return Err(map_network_error(error)),
                }
            }
        }
        LinuxFileKind::SocketNetlink => sys_netlink_send(runtime, pid, fd, buffer, len),
        LinuxFileKind::SocketTcpListener => Err(ENOTCONN),
        _ => Err(ENOTSOCK),
    }
}

pub(super) fn sys_recvfrom(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    sys_socket_recv(runtime, pid, fd, buffer, len, address, address_len)
}

pub(super) fn sys_socket_recv(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if buffer == 0 && len != 0 {
        return Err(EFAULT);
    }
    if len == 0 {
        return Ok(0);
    }
    let mut file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    let (received, source_offset, peer_ip, peer_port) = match file.kind {
        LinuxFileKind::SocketUdp => {
            file = ensure_udp_bound(runtime, pid, fd, file)?;
            let max_len = len.min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let received = net::net_service_udp_recv_on_port(
                runtime.network_port,
                file.local_port,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                return Err(EAGAIN);
            }
            (
                received,
                NETWORK_PAYLOAD_OFFSET,
                read_network_u32_be(runtime, 0),
                read_network_u16_be(runtime, 4),
            )
        }
        LinuxFileKind::SocketTcp => {
            if file.posix_fd == 0 {
                return Err(ENOTCONN);
            }
            let max_len = len.min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let (received, event_connection_id) = net::net_service_tcp_recv_on_connection(
                runtime.network_port,
                file.posix_fd,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                if event_connection_id == file.posix_fd {
                    return Ok(0);
                }
                return Err(EAGAIN);
            }
            (
                received,
                NETWORK_PAYLOAD_OFFSET,
                file.peer_ip,
                file.peer_port,
            )
        }
        LinuxFileKind::SocketIcmp => {
            if len <= LINUX_IPV4_HEADER_LEN {
                return Err(EMSGSIZE);
            }
            let max_len = (len - LINUX_IPV4_HEADER_LEN).min(
                runtime
                    .network_shm_size
                    .saturating_sub(NETWORK_PAYLOAD_OFFSET),
            );
            let received = net::net_service_icmp_recv(
                runtime.network_port,
                0,
                NETWORK_PAYLOAD_OFFSET,
                max_len,
                file.local_port,
            )
            .map_err(map_network_error)?;
            if received == 0 {
                return Err(EAGAIN);
            }
            let peer_ip = read_network_u32_be(runtime, 0);
            let ttl = unsafe { ::core::ptr::read((runtime.network_shm + 4) as *const u8) };
            unsafe {
                ::core::ptr::copy(
                    (runtime.network_shm + NETWORK_PAYLOAD_OFFSET) as *const u8,
                    (runtime.network_shm + LINUX_IPV4_HEADER_LEN) as *mut u8,
                    received as usize,
                );
                write_linux_ipv4_header(
                    runtime.network_shm,
                    received + LINUX_IPV4_HEADER_LEN,
                    peer_ip,
                    network_ipv4(runtime)?,
                    ttl,
                    LINUX_IPPROTO_ICMP as u8,
                );
            }
            (received + LINUX_IPV4_HEADER_LEN, 0, peer_ip, 0)
        }
        LinuxFileKind::SocketNetlink => return Err(EOPNOTSUPP),
        LinuxFileKind::SocketTcpListener => return Err(ENOTCONN),
        _ => return Err(ENOTSOCK),
    };
    if received != 0 {
        libnanami::request_process_memory_write(
            pid,
            buffer,
            runtime.network_shm + source_offset,
            received,
        )
        .map_err(map_request_error)?;
    }
    write_sockaddr_result(runtime, pid, address, address_len, peer_ip, peer_port)?;
    Ok(received)
}

pub(super) fn sys_sendmsg(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    message: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(EOPNOTSUPP);
    }
    let (_, _, iov, iov_count) = read_linux_msghdr(runtime, pid, message)?;
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov, iov_count, &mut bases, &mut lens)?;

    let mut total: Word = 0;
    let mut index = 0usize;
    while index < iov_count as usize {
        let len = lens[index];
        if total
            .checked_add(len)
            .filter(|end| *end <= runtime.posix_shm_size)
            .is_none()
        {
            return Err(EMSGSIZE);
        }
        if len != 0 {
            libnanami::request_process_memory_read(
                pid,
                bases[index],
                runtime.posix_shm + total,
                len,
            )
            .map_err(map_request_error)?;
        }
        total += len;
        index += 1;
    }
    process_netlink_request(runtime, pid, fd, total)
}

pub(super) fn sys_recvmsg(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    message: Word,
    _flags: Word,
) -> Result<Word, i32> {
    let file = runtime.linux_file(pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(EOPNOTSUPP);
    }
    if file.peer_port == 0 {
        return Err(EAGAIN);
    }

    let (name, name_len, iov, iov_count) = read_linux_msghdr(runtime, pid, message)?;
    let mut bases = [0 as Word; LINUX_IOV_MAX as usize];
    let mut lens = [0 as Word; LINUX_IOV_MAX as usize];
    read_linux_iovecs(runtime, pid, iov, iov_count, &mut bases, &mut lens)?;
    let response_len = build_netlink_dump(runtime, pid, file.posix_fd as u16, file.peer_ip)?;

    let mut copied = 0;
    let mut index = 0usize;
    while index < iov_count as usize && copied < response_len {
        let chunk = lens[index].min(response_len - copied);
        if chunk != 0 {
            libnanami::request_process_memory_write(
                pid,
                bases[index],
                runtime.posix_shm + copied,
                chunk,
            )
            .map_err(map_request_error)?;
            copied += chunk;
        }
        index += 1;
    }
    if copied < response_len {
        return Err(EMSGSIZE);
    }

    if name != 0 && name_len >= LINUX_SOCKADDR_NL_LEN {
        write_sockaddr_nl_value(runtime.posix_shm, 0);
        write_target_memory(runtime, pid, name, LINUX_SOCKADDR_NL_LEN)?;
    }
    write_u32_to_target(runtime, pid, message + 8, LINUX_SOCKADDR_NL_LEN as u32)?;
    write_u32_to_target(runtime, pid, message + 48, 0)?;

    let mut updated = file;
    updated.peer_port = 0;
    if !runtime.set_linux_file(pid, fd, updated) {
        return Err(EBADF);
    }
    Ok(response_len)
}
