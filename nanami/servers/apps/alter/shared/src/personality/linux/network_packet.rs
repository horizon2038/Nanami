use super::{
    map_network_error, map_request_error, net, read_shm_u32, read_target_memory,
    write_target_memory, write_u16, write_u32, write_u8, Runtime, Word, EFAULT, EMSGSIZE,
    LINUX_AF_INET, LINUX_IPV4_HEADER_LEN, LINUX_SOCKADDR_IN_LEN,
};

pub(super) fn align_up_usize(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

pub(super) fn write_sockaddr_result(
    runtime: &mut Runtime,
    pid: Word,
    address: Word,
    address_len: Word,
    ip: u32,
    port: u16,
) -> Result<(), i32> {
    if address == 0 {
        return Ok(());
    }
    if address_len == 0 {
        return Err(EFAULT);
    }
    read_target_memory(runtime, pid, address_len, 4)?;
    let available = read_shm_u32(runtime, 0) as Word;
    unsafe {
        ::core::ptr::write_bytes(
            runtime.posix_shm as *mut u8,
            0,
            LINUX_SOCKADDR_IN_LEN as usize,
        );
        write_u16(runtime.posix_shm, LINUX_AF_INET as u16);
        write_u8(runtime.posix_shm + 2, (port >> 8) as u8);
        write_u8(runtime.posix_shm + 3, port as u8);
        write_u8(runtime.posix_shm + 4, (ip >> 24) as u8);
        write_u8(runtime.posix_shm + 5, (ip >> 16) as u8);
        write_u8(runtime.posix_shm + 6, (ip >> 8) as u8);
        write_u8(runtime.posix_shm + 7, ip as u8);
    }
    write_target_memory(runtime, pid, address, available.min(LINUX_SOCKADDR_IN_LEN))?;
    unsafe {
        write_u32(runtime.posix_shm, LINUX_SOCKADDR_IN_LEN as u32);
    }
    write_target_memory(runtime, pid, address_len, 4)
}

pub(super) fn read_target_memory_to_network(
    runtime: &Runtime,
    pid: Word,
    user_ptr: Word,
    len: Word,
) -> Result<(), i32> {
    if len > runtime.network_shm_size {
        return Err(EMSGSIZE);
    }
    libnanami::request_process_memory_read(pid, user_ptr, runtime.network_shm, len)
        .map_err(map_request_error)
}

pub(super) fn network_ipv4(runtime: &Runtime) -> Result<u32, i32> {
    let (ip, _, _) =
        net::net_service_ipv4_config(runtime.network_port).map_err(map_network_error)?;
    Ok(((ip[0] as u32) << 24) | ((ip[1] as u32) << 16) | ((ip[2] as u32) << 8) | ip[3] as u32)
}

pub(super) fn read_network_u16_be(runtime: &Runtime, offset: Word) -> u16 {
    unsafe {
        let base = (runtime.network_shm + offset) as *const u8;
        ((*base as u16) << 8) | *base.add(1) as u16
    }
}

pub(super) fn read_network_u32_be(runtime: &Runtime, offset: Word) -> u32 {
    unsafe {
        let base = (runtime.network_shm + offset) as *const u8;
        ((*base as u32) << 24)
            | ((*base.add(1) as u32) << 16)
            | ((*base.add(2) as u32) << 8)
            | *base.add(3) as u32
    }
}

pub(super) unsafe fn write_linux_ipv4_header(
    base: Word,
    total_len: Word,
    src_ip: u32,
    dst_ip: u32,
    ttl: u8,
    protocol: u8,
) {
    ::core::ptr::write_bytes(base as *mut u8, 0, LINUX_IPV4_HEADER_LEN as usize);
    write_u8(base, 0x45);
    write_u8(base + 2, (total_len >> 8) as u8);
    write_u8(base + 3, total_len as u8);
    write_u8(base + 6, 0x40);
    write_u8(base + 8, ttl);
    write_u8(base + 9, protocol);
    write_u8(base + 12, (src_ip >> 24) as u8);
    write_u8(base + 13, (src_ip >> 16) as u8);
    write_u8(base + 14, (src_ip >> 8) as u8);
    write_u8(base + 15, src_ip as u8);
    write_u8(base + 16, (dst_ip >> 24) as u8);
    write_u8(base + 17, (dst_ip >> 16) as u8);
    write_u8(base + 18, (dst_ip >> 8) as u8);
    write_u8(base + 19, dst_ip as u8);
    let checksum = checksum_bytes(base, LINUX_IPV4_HEADER_LEN as usize);
    write_u8(base + 10, (checksum >> 8) as u8);
    write_u8(base + 11, checksum as u8);
}

pub(super) unsafe fn checksum_bytes(base: Word, len: usize) -> u16 {
    let bytes = ::core::slice::from_raw_parts(base as *const u8, len);
    let mut sum = 0u32;
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        sum = sum.wrapping_add(((bytes[index] as u32) << 8) | bytes[index + 1] as u32);
        index += 2;
    }
    if index < bytes.len() {
        sum = sum.wrapping_add((bytes[index] as u32) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
