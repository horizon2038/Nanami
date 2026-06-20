use super::*;

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami;

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;
const ENABLE_ICMP_RX_LOG: bool = false;
const ICMP_LOG_EVERY: usize = 16;
static ICMP_RX_LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn write_ipv4(ip: [u8; 4]) {
    libnanami::print!("{}", ip[0] as usize);
    libnanami::debug::print_char('.');
    libnanami::print!("{}", ip[1] as usize);
    libnanami::debug::print_char('.');
    libnanami::print!("{}", ip[2] as usize);
    libnanami::debug::print_char('.');
    libnanami::print!("{}", ip[3] as usize);
}

fn log_icmp_packet(src_ip: [u8; 4], dst_ip: [u8; 4], icmp: &[u8]) {
    let icmp_type = if !icmp.is_empty() { icmp[0] } else { 0 };
    let icmp_code = if icmp.len() >= 2 { icmp[1] } else { 0 };
    let identifier = if icmp.len() >= 6 {
        read_u16_be(&icmp[4..6])
    } else {
        0
    };
    let sequence = if icmp.len() >= 8 {
        read_u16_be(&icmp[6..8])
    } else {
        0
    };

    libnanami::print!("[net-server][icmp] rx src=");
    write_ipv4(src_ip);
    libnanami::print!(" dst=");
    write_ipv4(dst_ip);
    libnanami::print!(" type=");
    libnanami::print!("{}", icmp_type as usize);
    libnanami::print!(" code=");
    libnanami::print!("{}", icmp_code as usize);
    libnanami::print!(" id=");
    libnanami::print!("{}", identifier as usize);
    libnanami::print!(" seq=");
    libnanami::print!("{}", sequence as usize);
    libnanami::print!(" len=");
    libnanami::print!("{}", icmp.len());
    libnanami::print!("\n");
}

fn should_log_icmp() -> bool {
    let n = ICMP_RX_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
    (n % ICMP_LOG_EVERY) == 0
}

pub(crate) fn process_icmp(
    runtime: &mut NetRuntime,
    frame: &[u8],
    ip_header_len: usize,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    ip_end: usize,
) {
    let icmp_base = ETH_HDR_LEN + ip_header_len;
    if ip_end < icmp_base + 8 || frame.len() < ip_end {
        return;
    }

    let icmp = &frame[icmp_base..ip_end];
    if icmp[0] != ICMP_ECHO_REQUEST || icmp[1] != 0 {
        return;
    }
    if ENABLE_ICMP_RX_LOG && should_log_icmp() {
        log_icmp_packet(src_ip, dst_ip, icmp);
    }

    let src_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
    let payload_len = icmp.len();
    let frame_len = ETH_HDR_LEN + IPV4_HDR_LEN + payload_len;
    let tx_ptr = get_backend_shm_ptr(runtime, BACKEND_TX_OFFSET);

    unsafe {
        let out = core::slice::from_raw_parts_mut(tx_ptr, frame_len);
        out[0..6].copy_from_slice(&src_mac);
        out[6..12].copy_from_slice(&runtime.mac);
        out[12..14].copy_from_slice(&[0x08, 0x00]);

        let ip = &mut out[ETH_HDR_LEN..ETH_HDR_LEN + IPV4_HDR_LEN];
        ip[0] = 0x45;
        ip[1] = 0;
        write_u16_be(&mut ip[2..4], (IPV4_HDR_LEN + payload_len) as u16);
        write_u16_be(&mut ip[4..6], 0);
        write_u16_be(&mut ip[6..8], 0x4000);
        ip[8] = 64;
        ip[9] = 1;
        ip[10] = 0;
        ip[11] = 0;
        ip[12..16].copy_from_slice(&dst_ip);
        ip[16..20].copy_from_slice(&src_ip);
        let ip_sum = ipv4_checksum(ip);
        write_u16_be(&mut ip[10..12], ip_sum);

        let out_icmp = &mut out[ETH_HDR_LEN + IPV4_HDR_LEN..];
        out_icmp.copy_from_slice(icmp);
        out_icmp[0] = ICMP_ECHO_REPLY;
        out_icmp[2] = 0;
        out_icmp[3] = 0;
        let icmp_sum = checksum16(out_icmp, 0);
        write_u16_be(&mut out_icmp[2..4], icmp_sum);
    }

    let _ = emit_frame(runtime, frame_len);
}
