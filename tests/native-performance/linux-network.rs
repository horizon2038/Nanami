//! Production Linux network ABI handlers; guest memory and native IPC mocked.
#![allow(dead_code)]
extern crate self as nanami_runtime;
extern crate self as nanami_client;
type Word = usize;
mod vfs { pub use nanami_uapi::vfs::VFS_RESPONSE_MAPPING_BUSY; }
mod state {
    pub const LINUX_FD_MAX: usize = 64;
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/constants.rs"]
mod constants;
use constants::*;
const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
const OS_RESPONSE_PERMISSION_DENIED: Word = 2;
const OS_RESPONSE_ILLEGAL_OPERATION: Word = 3;
const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 4;
#[derive(Debug, Clone, Copy)]
enum RequestError {
    InvalidArgument,
    Unsupported,
    Status(Word),
    Transport,
    Protocol,
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/errors.rs"]
mod errors;
use errors::*;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/proc_net.rs"]
mod proc_net;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/socket_ioctl.rs"]
mod socket_ioctl;
struct Runtime {
    network_port: Word,
    network_shm: Word,
    network_shm_size: Word,
    posix_shm: Word,
}
fn ensure_network(_: &mut Runtime) -> Result<(), i32> {
    Ok(())
}
fn read_target_memory(runtime: &Runtime, _: Word, address: Word, len: Word) -> Result<(), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(address as *const u8, runtime.posix_shm as *mut u8, len);
    }
    Ok(())
}
fn write_target_memory(runtime: &Runtime, _: Word, address: Word, len: Word) -> Result<(), i32> {
    if address == 0 {
        return Err(EFAULT);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(runtime.posix_shm as *const u8, address as *mut u8, len);
    }
    Ok(())
}
mod net {
    use super::*;
    thread_local! { pub static MASK: std::cell::Cell<[u8; 4]> = const { std::cell::Cell::new([255, 255, 255, 0]) }; }
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../nanami/servers/uapi/src/net.rs"
    ));
    pub fn net_service_ipv4_config(_: Word) -> Result<([u8; 4], [u8; 4], [u8; 4]), RequestError> {
        Ok(([192, 168, 1, 26], [192, 168, 1, 1], [192, 168, 1, 1]))
    }
    pub fn net_service_mac_address(_: Word) -> Result<[u8; 6], RequestError> {
        Ok([2, 3, 4, 5, 6, 7])
    }
    pub fn net_service_ipv4_netmask(_: Word) -> Result<[u8; 4], RequestError> {
        Ok(MASK.get())
    }
    pub fn net_service_arp_snapshot(_: Word, _: Word, _: Word) -> Result<Word, RequestError> {
        Ok(0)
    }
}
fn setup() -> (Box<[u8; 4096]>, Runtime) {
    let mut memory = Box::new([0u8; 4096]);
    let runtime = Runtime {
        network_port: 1,
        network_shm: 0,
        network_shm_size: 0,
        posix_shm: memory.as_mut_ptr() as Word,
    };
    (memory, runtime)
}
#[test]
fn interface_queries_follow_linux_ifreq_layout() {
    let (_memory, mut runtime) = setup();
    for (request, expected) in [
        (0x8933, vec![2, 0, 0, 0]),
        (0x8913, vec![0x43, 0x10]),
        (0x8921, 1500u32.to_le_bytes().to_vec()),
        (0x8927, vec![1, 0, 2, 3, 4, 5, 6, 7]),
        (0x8915, vec![2, 0, 0, 0, 192, 168, 1, 26]),
        (0x8919, vec![2, 0, 0, 0, 192, 168, 1, 255]),
        (0x891b, vec![2, 0, 0, 0, 255, 255, 255, 0]),
    ] {
        let mut data = [0u8; 40];
        data[..4].copy_from_slice(b"eth0");
        assert_eq!(
            socket_ioctl::sys_socket_ioctl(&mut runtime, 1, request, data.as_mut_ptr() as Word),
            Ok(0)
        );
        assert_eq!(&data[16..16 + expected.len()], &expected);
        assert_eq!(&data[..4], b"eth0");
    }
}

#[test]
fn interface_mask_and_broadcast_follow_native_dhcp_configuration() {
    let (_memory, mut runtime) = setup();
    for (mask, broadcast) in [
        ([255, 255, 254, 0], [192, 168, 1, 255]),
        ([255, 255, 255, 128], [192, 168, 1, 127]),
        ([255, 255, 0, 0], [192, 168, 255, 255]),
    ] {
        net::MASK.set(mask);
        for (request, expected) in [(0x891b, mask), (0x8919, broadcast)] {
            let mut data = [0xa5u8; 40];
            data[..5].copy_from_slice(b"eth0\0");
            let before = data;
            assert_eq!(
                socket_ioctl::sys_socket_ioctl(&mut runtime, 1, request, data.as_mut_ptr() as Word),
                Ok(0)
            );
            assert_eq!(&data[20..24], &expected);
            assert_eq!(&data[..16], &before[..16]);
            assert_eq!(&data[32..], &before[32..]);
        }
    }
    net::MASK.set([255, 255, 255, 0]);
}

#[test]
fn tx_queue_length_is_a_linux_int_and_preserves_the_rest_of_ifreq() {
    let (_memory, mut runtime) = setup();
    for name in [b"lo\0".as_slice(), b"eth0\0".as_slice()] {
        let mut data = [0xa5u8; 40];
        data[..name.len()].copy_from_slice(name);
        let before = data;
        assert_eq!(
            socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8942, data.as_mut_ptr() as Word),
            Ok(0)
        );
        assert_eq!(i32::from_le_bytes(data[16..20].try_into().unwrap()), 0);
        assert_eq!(&data[..16], &before[..16]);
        assert_eq!(&data[20..], &before[20..]);
    }
    let mut data = [0u8; 40];
    data[..4].copy_from_slice(b"eth9");
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8942, data.as_mut_ptr() as Word),
        Err(ENODEV)
    );
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8942, 0),
        Err(EFAULT)
    );
    // The getter must not silently enable an unimplemented setter.
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8943, data.as_mut_ptr() as Word),
        Err(ENOTTY)
    );
}
#[test]
fn interface_list_null_query_and_short_buffers() {
    let (_memory, mut runtime) = setup();
    let mut conf = [0u8; 16];
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8912, conf.as_mut_ptr() as Word),
        Ok(0)
    );
    assert_eq!(conf[0], 80);
    for capacity in [0, 39, 40, 79, 80, 100] {
        let mut entries = [0xa5u8; 100];
        conf[..4].copy_from_slice(&(capacity as i32).to_le_bytes());
        conf[8..].copy_from_slice(&(entries.as_mut_ptr() as u64).to_le_bytes());
        assert_eq!(
            socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8912, conf.as_mut_ptr() as Word),
            Ok(0)
        );
        let copied = (capacity / 40).min(2) * 40;
        assert_eq!(conf[0] as usize, copied);
        assert!(entries[copied..].iter().all(|v| *v == 0xa5));
        if copied >= 40 {
            assert_eq!(&entries[..3], b"lo\0");
        }
        if copied == 80 {
            assert_eq!(&entries[40..45], b"eth0\0");
        }
    }
}
#[test]
fn interface_name_errors_and_unsupported_mutations() {
    let (_memory, mut runtime) = setup();
    let mut data = [0u8; 40];
    data[16] = 1;
    let address = data.as_mut_ptr() as Word;
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8910, address),
        Ok(0)
    );
    assert_eq!(&data[..3], b"lo\0");
    data[..5].copy_from_slice(b"eth9\0");
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8933, address),
        Err(ENODEV)
    );
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8914, address),
        Err(ENOTTY)
    );
    assert_eq!(
        socket_ioctl::sys_socket_ioctl(&mut runtime, 1, 0x8915, 0),
        Err(EFAULT)
    );
}
#[test]
fn proc_arp_formats_linux_columns_without_fabricating_neighbors() {
    let mut text = [0u8; 4096];
    let length = proc_net::format_arp(&[], &mut text).unwrap();
    assert_eq!(
        std::str::from_utf8(&text[..length])
            .unwrap()
            .lines()
            .count(),
        1
    );
    let record = [192, 168, 1, 1, 2, 3, 4, 5, 6, 7, 0, 0];
    let length = proc_net::format_arp(&record, &mut text).unwrap();
    assert_eq!(
        std::str::from_utf8(&text[..length])
            .unwrap()
            .lines()
            .nth(1)
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>(),
        vec![
            "192.168.1.1",
            "0x1",
            "0x2",
            "02:03:04:05:06:07",
            "*",
            "eth0"
        ]
    );
    assert!(proc_net::format_arp(&record, &mut [0; 1]).is_err());
}
#[test]
fn link_errors_do_not_hide_missing_parents_or_real_io_failures() {
    assert_eq!(
        map_link_request_error(RequestError::Status(OS_RESPONSE_INVALID_DESCRIPTOR)),
        ENOENT
    );
    assert_eq!(
        map_link_request_error(RequestError::Status(OS_RESPONSE_INVALID_ARGUMENT)),
        EEXIST
    );
    assert_eq!(
        map_link_request_error(RequestError::Status(OS_RESPONSE_ILLEGAL_OPERATION)),
        EIO
    );
    assert_eq!(map_link_request_error(RequestError::Transport), EIO);
}
