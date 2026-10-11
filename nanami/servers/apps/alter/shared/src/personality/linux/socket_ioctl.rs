//! Linux LP64 ifreq/ifconf ABI (x86-64 and AArch64).
use super::{
    map_network_error, net, read_target_memory, write_target_memory, Runtime, Word, EFAULT, EINVAL,
    ENODEV, ENOTTY, LINUX_AF_INET, LINUX_ARPHRD_ETHER, LINUX_ARPHRD_LOOPBACK, LINUX_IFF_BROADCAST,
    LINUX_IFF_LOOPBACK, LINUX_IFF_MULTICAST, LINUX_IFF_RUNNING, LINUX_IFF_UP,
};

const IFREQ_BYTES: usize = 40;
const IFCONF_BYTES: usize = 16;
const SIOCGIFNAME: Word = 0x8910;
const SIOCGIFCONF: Word = 0x8912;
const SIOCGIFFLAGS: Word = 0x8913;
const SIOCGIFADDR: Word = 0x8915;
const SIOCGIFBRDADDR: Word = 0x8919;
const SIOCGIFNETMASK: Word = 0x891b;
const SIOCGIFMTU: Word = 0x8921;
const SIOCGIFHWADDR: Word = 0x8927;
const SIOCGIFINDEX: Word = 0x8933;
const SIOCGIFTXQLEN: Word = 0x8942;

pub(super) fn sys_socket_ioctl(
    runtime: &mut Runtime,
    pid: Word,
    request: Word,
    argument: Word,
) -> Result<Word, i32> {
    if !matches!(
        request,
        SIOCGIFNAME
            | SIOCGIFCONF
            | SIOCGIFFLAGS
            | SIOCGIFADDR
            | SIOCGIFBRDADDR
            | SIOCGIFNETMASK
            | SIOCGIFMTU
            | SIOCGIFHWADDR
            | SIOCGIFINDEX
            | SIOCGIFTXQLEN
    ) {
        return Err(ENOTTY);
    }
    if argument == 0 {
        return Err(EFAULT);
    }
    if request == SIOCGIFCONF {
        return interface_list(runtime, pid, argument);
    }
    read_target_memory(runtime, pid, argument, IFREQ_BYTES)?;
    let mut data = [0u8; IFREQ_BYTES];
    unsafe {
        core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            data.as_mut_ptr(),
            data.len(),
        );
    }
    let index = if request == SIOCGIFNAME {
        u32::from_le_bytes(data[16..20].try_into().unwrap())
    } else {
        let len = data[..16].iter().position(|c| *c == 0).unwrap_or(16);
        match &data[..len] {
            b"lo" => 1,
            b"eth0" => 2,
            _ => return Err(ENODEV),
        }
    };
    if !matches!(index, 1 | 2) {
        return Err(ENODEV);
    }
    match request {
        SIOCGIFNAME => {
            data[..16].fill(0);
            let name: &[u8] = if index == 1 { b"lo" } else { b"eth0" };
            data[..name.len()].copy_from_slice(name);
        }
        SIOCGIFINDEX => data[16..20].copy_from_slice(&index.to_le_bytes()),
        SIOCGIFFLAGS => {
            let flags = LINUX_IFF_UP
                | LINUX_IFF_RUNNING
                | if index == 1 {
                    LINUX_IFF_LOOPBACK
                } else {
                    LINUX_IFF_BROADCAST | LINUX_IFF_MULTICAST
                };
            data[16..18].copy_from_slice(&(flags as u16).to_le_bytes());
        }
        SIOCGIFMTU => {
            let mtu: u32 = if index == 1 { 65536 } else { 1500 };
            data[16..20].copy_from_slice(&mtu.to_le_bytes());
        }
        SIOCGIFTXQLEN => {
            // No software egress queue: the native stack submits directly to
            // the driver. This is not the hardware TX descriptor-ring size.
            data[16..20].copy_from_slice(&0i32.to_le_bytes());
        }
        SIOCGIFHWADDR => {
            let (kind, mac) = if index == 1 {
                (LINUX_ARPHRD_LOOPBACK, [0; 6])
            } else {
                (
                    LINUX_ARPHRD_ETHER,
                    net::net_service_mac_address(runtime.network_port)
                        .map_err(map_network_error)?,
                )
            };
            data[16..32].fill(0);
            data[16..18].copy_from_slice(&kind.to_le_bytes());
            data[18..24].copy_from_slice(&mac);
        }
        _ => {
            let address = match request {
                SIOCGIFNETMASK => interface_netmask(runtime, index)?,
                SIOCGIFBRDADDR => {
                    if index == 1 {
                        [0; 4]
                    } else {
                        let address = u32::from_be_bytes(interface_address(runtime, index)?);
                        let mask = u32::from_be_bytes(interface_netmask(runtime, index)?);
                        (address | !mask).to_be_bytes()
                    }
                }
                _ => interface_address(runtime, index)?,
            };
            write_address(&mut data, address);
        }
    }
    copy_result(runtime, pid, argument, &data)?;
    Ok(0)
}

fn interface_address(runtime: &Runtime, index: u32) -> Result<[u8; 4], i32> {
    if index == 1 {
        return Ok([127, 0, 0, 1]);
    }
    net::net_service_ipv4_config(runtime.network_port)
        .map(|(ip, _, _)| ip)
        .map_err(map_network_error)
}

fn interface_netmask(runtime: &Runtime, index: u32) -> Result<[u8; 4], i32> {
    if index == 1 {
        return Ok([255, 0, 0, 0]);
    }
    net::net_service_ipv4_netmask(runtime.network_port).map_err(map_network_error)
}

fn write_address(data: &mut [u8; IFREQ_BYTES], address: [u8; 4]) {
    data[16..32].fill(0);
    data[16..18].copy_from_slice(&(LINUX_AF_INET as u16).to_le_bytes());
    data[20..24].copy_from_slice(&address);
}

fn copy_result(runtime: &Runtime, pid: Word, address: Word, data: &[u8]) -> Result<(), i32> {
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), runtime.posix_shm as *mut u8, data.len());
    }
    write_target_memory(runtime, pid, address, data.len())
}

fn interface_list(runtime: &mut Runtime, pid: Word, argument: Word) -> Result<Word, i32> {
    read_target_memory(runtime, pid, argument, IFCONF_BYTES)?;
    let mut conf = [0u8; IFCONF_BYTES];
    unsafe {
        core::ptr::copy_nonoverlapping(
            runtime.posix_shm as *const u8,
            conf.as_mut_ptr(),
            conf.len(),
        );
    }
    let capacity = i32::from_le_bytes(conf[..4].try_into().unwrap());
    let address = u64::from_le_bytes(conf[8..].try_into().unwrap()) as Word;
    let count = if address == 0 {
        2
    } else {
        if capacity < 0 {
            return Err(EINVAL);
        }
        (capacity as usize / IFREQ_BYTES).min(2)
    };
    if address != 0 {
        for index in 0..count {
            let mut entry = [0u8; IFREQ_BYTES];
            let name: &[u8] = if index == 0 { b"lo" } else { b"eth0" };
            entry[..name.len()].copy_from_slice(name);
            write_address(&mut entry, interface_address(runtime, index as u32 + 1)?);
            let destination = address.checked_add(index * IFREQ_BYTES).ok_or(EFAULT)?;
            copy_result(runtime, pid, destination, &entry)?;
        }
    }
    conf[..4].copy_from_slice(&((count * IFREQ_BYTES) as i32).to_le_bytes());
    copy_result(runtime, pid, argument, &conf)?;
    Ok(0)
}
