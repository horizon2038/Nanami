use super::*;

pub(super) fn socket_readiness(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    mut file: LinuxFile,
) -> Result<i16, i32> {
    if file.kind == LinuxFileKind::SocketTcp && file.posix_fd == 0 {
        if file.peer_ip == 0 || file.peer_port == 0 {
            return Ok(LINUX_POLLHUP | LINUX_POLLOUT | LINUX_POLLWRNORM);
        }
        // A nonblocking connect may still be waiting for ARP/SYN-ACK.
        // Reuse the tuple-based connect operation; no guest sockaddr re-copy.
        let id = match net::net_service_tcp_connect(
            runtime.network_port,
            file.local_port,
            file.peer_ip,
            file.peer_port,
        ) {
            Ok(id) => id,
            Err(RequestError::Status(libnanami::OS_RESPONSE_ILLEGAL_OPERATION)) => 0,
            Err(error) => return Err(map_network_error(error)),
        };
        if id == 0 {
            return Ok(0);
        }
        file.posix_fd = id;
        if !runtime.set_linux_file(pid, fd, file) {
            return Err(EBADF);
        }
    }
    let (kind, id) = match file.kind {
        LinuxFileKind::SocketUdp => (net::NET_READINESS_UDP, file.local_port as Word),
        LinuxFileKind::SocketTcp => (net::NET_READINESS_TCP, file.posix_fd),
        LinuxFileKind::SocketTcpListener => (net::NET_READINESS_LISTENER, file.local_port as Word),
        LinuxFileKind::SocketIcmp => (net::NET_READINESS_ICMP, file.local_port as Word),
        _ => return Err(EBADF),
    };
    let ready =
        net::net_service_readiness(runtime.network_port, kind, id).map_err(map_network_error)?;
    Ok((if ready & net::NET_READY_READ != 0 {
        LINUX_POLLIN | LINUX_POLLRDNORM
    } else {
        0
    }) | (if ready & net::NET_READY_WRITE != 0 {
        LINUX_POLLOUT | LINUX_POLLWRNORM
    } else {
        0
    }) | (if ready & net::NET_READY_HANGUP != 0 {
        LINUX_POLLHUP
    } else {
        0
    }) | (if ready & net::NET_READY_ERROR != 0 {
        LINUX_POLLERR
    } else {
        0
    }) | (if ready & net::NET_READY_READ_CLOSED != 0 {
        LINUX_POLLRDHUP
    } else {
        0
    }))
}
