//! Default resolver configuration follows the native DHCP configuration.
//! The guest libc still performs normal Linux UDP DNS queries itself.
use super::{
    append_bytes_to_array, append_decimal_to_array, ensure_network, map_network_error, net,
    Runtime, EIO,
};

pub(super) fn read_resolv_conf(runtime: &mut Runtime, out: &mut [u8]) -> Result<usize, i32> {
    ensure_network(runtime)?;
    let (_, _, dns) =
        net::net_service_ipv4_config(runtime.network_port).map_err(map_network_error)?;
    format_resolv_conf(dns, out)
}

pub(super) fn format_resolv_conf(dns: [u8; 4], out: &mut [u8]) -> Result<usize, i32> {
    if out.len() < b"nameserver 255.255.255.255\n".len() {
        return Err(EIO);
    }
    let mut offset = append_bytes_to_array(out, 0, b"nameserver ");
    for (index, octet) in dns.iter().enumerate() {
        if index != 0 {
            offset = append_bytes_to_array(out, offset, b".");
        }
        offset = append_decimal_to_array(out, offset, *octet as usize);
    }
    Ok(append_bytes_to_array(out, offset, b"\n"))
}
