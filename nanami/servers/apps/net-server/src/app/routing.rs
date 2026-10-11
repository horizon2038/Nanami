use super::NetRuntime;

pub(crate) fn valid_subnet_mask(mask: [u8; 4]) -> bool {
    let host_bits = !u32::from_be_bytes(mask);
    // A contiguous mask has only trailing host bits; /0 and /32 are valid.
    host_bits & host_bits.wrapping_add(1) == 0
}

pub(crate) fn next_hop_ip(runtime: &NetRuntime, destination: [u8; 4]) -> [u8; 4] {
    let mask = u32::from_be_bytes(runtime.subnet_mask);
    if u32::from_be_bytes(destination) & mask == u32::from_be_bytes(runtime.ip) & mask {
        destination
    } else {
        runtime.gateway_ip
    }
}
