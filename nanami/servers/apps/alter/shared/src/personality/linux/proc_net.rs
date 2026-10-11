use super::{ensure_network, map_network_error, net, Runtime, EIO};
use core::fmt::{self, Write};

// A bounded snapshot of the native neighbor cache, not an independent Linux cache.
pub(super) fn read_arp(runtime: &mut Runtime, text: &mut [u8]) -> Result<usize, i32> {
    ensure_network(runtime)?;
    let capacity = net::NET_ARP_MAX_ENTRIES * net::NET_ARP_ENTRY_BYTES;
    if capacity > runtime.network_shm_size {
        return Err(EIO);
    }
    let count = net::net_service_arp_snapshot(runtime.network_port, 0, capacity)
        .map_err(map_network_error)?;
    if count > net::NET_ARP_MAX_ENTRIES {
        return Err(EIO);
    }
    let records = unsafe {
        core::slice::from_raw_parts(
            runtime.network_shm as *const u8,
            count * net::NET_ARP_ENTRY_BYTES,
        )
    };
    format_arp(records, text).map_err(|_| EIO)
}

pub(super) fn format_arp(records: &[u8], text: &mut [u8]) -> Result<usize, fmt::Error> {
    struct Output<'a> {
        bytes: &'a mut [u8],
        len: usize,
    }
    impl Write for Output<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
            self.bytes
                .get_mut(self.len..end)
                .ok_or(fmt::Error)?
                .copy_from_slice(s.as_bytes());
            self.len = end;
            Ok(())
        }
    }
    let mut out = Output {
        bytes: text,
        len: 0,
    };
    out.write_str(
        "IP address       HW type     Flags       HW address            Mask     Device\n",
    )?;
    for entry in records.chunks_exact(net::NET_ARP_ENTRY_BYTES) {
        writeln!(
            out,
            "{}.{}.{}.{} 0x1 0x2 {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} * eth0",
            entry[0],
            entry[1],
            entry[2],
            entry[3],
            entry[4],
            entry[5],
            entry[6],
            entry[7],
            entry[8],
            entry[9]
        )?;
    }
    Ok(out.len)
}
