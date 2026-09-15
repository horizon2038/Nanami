use super::{
    align_up_usize, map_network_error, net, read_shm_u16, read_shm_u32, read_target_memory,
    write_target_memory, write_u16, write_u32, write_u32_to_target, write_u8, LinuxFileKind,
    Runtime, Word, EAFNOSUPPORT, EBADF, EFAULT, EINVAL, EMSGSIZE, ENOTSOCK, EOPNOTSUPP,
    LINUX_AF_INET, LINUX_AF_NETLINK, LINUX_ARPHRD_ETHER, LINUX_ARPHRD_LOOPBACK,
    LINUX_IFADDRMSG_LEN, LINUX_IFA_ADDRESS, LINUX_IFA_BROADCAST, LINUX_IFA_LABEL, LINUX_IFA_LOCAL,
    LINUX_IFF_BROADCAST, LINUX_IFF_LOOPBACK, LINUX_IFF_MULTICAST, LINUX_IFF_RUNNING, LINUX_IFF_UP,
    LINUX_IFINFOMSG_LEN, LINUX_IFLA_ADDRESS, LINUX_IFLA_BROADCAST, LINUX_IFLA_IFNAME,
    LINUX_IFLA_MTU, LINUX_NLMSG_DONE, LINUX_NLMSG_HEADER_LEN, LINUX_NLM_F_MULTI, LINUX_RTM_GETADDR,
    LINUX_RTM_GETLINK, LINUX_RTM_NEWADDR, LINUX_RTM_NEWLINK, LINUX_RT_SCOPE_HOST,
    LINUX_RT_SCOPE_UNIVERSE, LINUX_SOCKADDR_NL_LEN,
};

pub(super) fn sys_netlink_send(
    runtime: &mut Runtime,
    pid: Word,
    fd: Word,
    buffer: Word,
    len: Word,
) -> Result<Word, i32> {
    if len > runtime.posix_shm_size {
        return Err(EMSGSIZE);
    }
    read_target_memory(runtime, pid, buffer, len)?;
    process_netlink_request(runtime, pid, fd, len)
}

pub(super) fn process_netlink_request(
    runtime: &mut Runtime,
    _pid: Word,
    fd: Word,
    len: Word,
) -> Result<Word, i32> {
    if len < LINUX_NLMSG_HEADER_LEN {
        return Err(EINVAL);
    }
    let declared_len = read_shm_u32(runtime, 0) as Word;
    if declared_len < LINUX_NLMSG_HEADER_LEN || declared_len > len {
        return Err(EINVAL);
    }
    let message_type = read_shm_u16(runtime, 4);
    if message_type != LINUX_RTM_GETLINK && message_type != LINUX_RTM_GETADDR {
        return Err(EOPNOTSUPP);
    }
    let mut file = runtime.linux_file(_pid, fd).ok_or(EBADF)?;
    if file.kind != LinuxFileKind::SocketNetlink {
        return Err(ENOTSOCK);
    }
    file.posix_fd = message_type as Word;
    file.peer_ip = read_shm_u32(runtime, 8);
    file.peer_port = 1;
    if !runtime.set_linux_file(_pid, fd, file) {
        return Err(EBADF);
    }
    Ok(len)
}

pub(super) fn read_sockaddr_nl(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<(), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    if address_len < LINUX_SOCKADDR_NL_LEN {
        return Err(EINVAL);
    }
    read_target_memory(runtime, pid, address, LINUX_SOCKADDR_NL_LEN)?;
    if read_shm_u16(runtime, 0) as Word != LINUX_AF_NETLINK {
        return Err(EAFNOSUPPORT);
    }
    Ok(())
}

pub(super) fn write_sockaddr_nl(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
) -> Result<Word, i32> {
    if address == 0 || address_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, address_len, 4)?;
    let available = read_shm_u32(runtime, 0) as Word;
    write_sockaddr_nl_value(runtime.posix_shm, pid as u32);
    write_target_memory(runtime, pid, address, available.min(LINUX_SOCKADDR_NL_LEN))?;
    write_u32_to_target(runtime, pid, address_len, LINUX_SOCKADDR_NL_LEN as u32)?;
    Ok(0)
}

pub(super) fn write_sockaddr_nl_value(base: Word, nl_pid: u32) {
    unsafe {
        ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_SOCKADDR_NL_LEN as usize);
        write_u16(base, LINUX_AF_NETLINK as u16);
        write_u32(base + 4, nl_pid);
    }
}

pub(super) fn build_netlink_dump(
    runtime: &mut Runtime,
    pid: Word,
    request_type: u16,
    sequence: u32,
) -> Result<Word, i32> {
    let (ip, _, _) =
        net::net_service_ipv4_config(runtime.network_port).map_err(map_network_error)?;
    let mac = net::net_service_mac_address(runtime.network_port).map_err(map_network_error)?;
    let mut offset = 0usize;
    match request_type {
        LINUX_RTM_GETLINK => {
            offset = append_netlink_link(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                1,
                LINUX_ARPHRD_LOOPBACK,
                LINUX_IFF_UP | LINUX_IFF_LOOPBACK | LINUX_IFF_RUNNING,
                b"lo\0",
                &[0; 6],
            )?;
            offset = append_netlink_link(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                2,
                LINUX_ARPHRD_ETHER,
                LINUX_IFF_UP | LINUX_IFF_BROADCAST | LINUX_IFF_RUNNING | LINUX_IFF_MULTICAST,
                b"eth0\0",
                &mac,
            )?;
        }
        LINUX_RTM_GETADDR => {
            offset = append_netlink_address(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                1,
                8,
                LINUX_RT_SCOPE_HOST,
                [127, 0, 0, 1],
                [127, 255, 255, 255],
                b"lo\0",
            )?;
            offset = append_netlink_address(
                runtime.posix_shm,
                runtime.posix_shm_size as usize,
                offset,
                sequence,
                pid as u32,
                2,
                24,
                LINUX_RT_SCOPE_UNIVERSE,
                ip,
                [ip[0], ip[1], ip[2], 255],
                b"eth0\0",
            )?;
        }
        _ => return Err(EOPNOTSUPP),
    }
    offset = append_netlink_done(
        runtime.posix_shm,
        runtime.posix_shm_size as usize,
        offset,
        sequence,
        pid as u32,
    )?;
    Ok(offset as Word)
}

pub(super) fn append_netlink_link(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
    index: u32,
    hardware_type: u16,
    flags: u32,
    name: &[u8],
    address: &[u8; 6],
) -> Result<usize, i32> {
    let start = offset;
    let mut cursor = offset + LINUX_NLMSG_HEADER_LEN as usize + LINUX_IFINFOMSG_LEN;
    if cursor > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + start as Word) as *mut u8, 0, cursor - start);
        write_u16(base + start as Word + 4, LINUX_RTM_NEWLINK);
        write_u16(base + start as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + start as Word + 8, sequence);
        write_u32(base + start as Word + 12, pid);
        write_u16(
            base + start as Word + LINUX_NLMSG_HEADER_LEN + 2,
            hardware_type,
        );
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 4, index);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 8, flags);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 12, u32::MAX);
    }
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_IFNAME, name)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_ADDRESS, address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFLA_BROADCAST, &[0xff; 6])?;
    cursor = append_netlink_attr(
        base,
        capacity,
        cursor,
        LINUX_IFLA_MTU,
        &1500u32.to_ne_bytes(),
    )?;
    unsafe { write_u32(base + start as Word, (cursor - start) as u32) };
    Ok(align_up_usize(cursor, 4))
}

pub(super) fn append_netlink_address(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
    index: u32,
    prefix_len: u8,
    scope: u8,
    address: [u8; 4],
    broadcast: [u8; 4],
    label: &[u8],
) -> Result<usize, i32> {
    let start = offset;
    let mut cursor = offset + LINUX_NLMSG_HEADER_LEN as usize + LINUX_IFADDRMSG_LEN;
    if cursor > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + start as Word) as *mut u8, 0, cursor - start);
        write_u16(base + start as Word + 4, LINUX_RTM_NEWADDR);
        write_u16(base + start as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + start as Word + 8, sequence);
        write_u32(base + start as Word + 12, pid);
        write_u8(
            base + start as Word + LINUX_NLMSG_HEADER_LEN,
            LINUX_AF_INET as u8,
        );
        write_u8(
            base + start as Word + LINUX_NLMSG_HEADER_LEN + 1,
            prefix_len,
        );
        write_u8(base + start as Word + LINUX_NLMSG_HEADER_LEN + 3, scope);
        write_u32(base + start as Word + LINUX_NLMSG_HEADER_LEN + 4, index);
    }
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_ADDRESS, &address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_LOCAL, &address)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_BROADCAST, &broadcast)?;
    cursor = append_netlink_attr(base, capacity, cursor, LINUX_IFA_LABEL, label)?;
    unsafe { write_u32(base + start as Word, (cursor - start) as u32) };
    Ok(align_up_usize(cursor, 4))
}

pub(super) fn append_netlink_attr(
    base: Word,
    capacity: usize,
    offset: usize,
    attr_type: u16,
    value: &[u8],
) -> Result<usize, i32> {
    let len = 4usize.checked_add(value.len()).ok_or(EMSGSIZE)?;
    let end = offset.checked_add(align_up_usize(len, 4)).ok_or(EMSGSIZE)?;
    if end > capacity || len > u16::MAX as usize {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes((base + offset as Word) as *mut u8, 0, end - offset);
        write_u16(base + offset as Word, len as u16);
        write_u16(base + offset as Word + 2, attr_type);
        ::core::ptr::copy_nonoverlapping(
            value.as_ptr(),
            (base + offset as Word + 4) as *mut u8,
            value.len(),
        );
    }
    Ok(end)
}

pub(super) fn append_netlink_done(
    base: Word,
    capacity: usize,
    offset: usize,
    sequence: u32,
    pid: u32,
) -> Result<usize, i32> {
    let end = offset + LINUX_NLMSG_HEADER_LEN as usize;
    if end > capacity {
        return Err(EMSGSIZE);
    }
    unsafe {
        ::core::ptr::write_bytes(
            (base + offset as Word) as *mut u8,
            0,
            LINUX_NLMSG_HEADER_LEN as usize,
        );
        write_u32(base + offset as Word, LINUX_NLMSG_HEADER_LEN as u32);
        write_u16(base + offset as Word + 4, LINUX_NLMSG_DONE);
        write_u16(base + offset as Word + 6, LINUX_NLM_F_MULTI);
        write_u32(base + offset as Word + 8, sequence);
        write_u32(base + offset as Word + 12, pid);
    }
    Ok(end)
}
