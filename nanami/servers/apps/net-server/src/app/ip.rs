use super::arp::update_arp;
use super::dhcp::process_dhcp_payload;
use super::dns::process_dns_payload;
use super::icmp::process_icmp;
use super::tcp::process_tcp;
use super::udp::try_queue_udp;
use super::*;

pub(crate) fn process_ipv4(runtime: &mut NetRuntime, stats: &mut NetStats, frame: &[u8]) {
    if frame.len() < ETH_HDR_LEN + IPV4_HDR_LEN {
        return;
    }
    let src_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
    let ihl = (frame[ETH_HDR_LEN] & 0x0f) as usize * 4;
    if ihl < IPV4_HDR_LEN || frame.len() < ETH_HDR_LEN + ihl {
        return;
    }

    let proto = frame[ETH_HDR_LEN + 9];
    let total_len = read_u16_be(&frame[ETH_HDR_LEN + 2..ETH_HDR_LEN + 4]) as usize;
    if total_len < ihl {
        return;
    }
    let ip_end = ETH_HDR_LEN + total_len;
    if frame.len() < ip_end {
        return;
    }
    let src_ip = [
        frame[ETH_HDR_LEN + 12],
        frame[ETH_HDR_LEN + 13],
        frame[ETH_HDR_LEN + 14],
        frame[ETH_HDR_LEN + 15],
    ];
    let dst_ip = [
        frame[ETH_HDR_LEN + 16],
        frame[ETH_HDR_LEN + 17],
        frame[ETH_HDR_LEN + 18],
        frame[ETH_HDR_LEN + 19],
    ];

    update_arp(runtime, src_ip, src_mac);

    // During DHCP bootstrap, server may unicast DHCPACK to yiaddr before
    // runtime.ip is updated. Handle DHCP prior to generic dst-ip filtering.
    if runtime.dhcp_waiting && proto == 17 {
        let udp_base = ETH_HDR_LEN + ihl;
        if frame.len() >= udp_base + UDP_HDR_LEN {
            let src_port = read_u16_be(&frame[udp_base..udp_base + 2]);
            let dst_port = read_u16_be(&frame[udp_base + 2..udp_base + 4]);
            let udp_len = read_u16_be(&frame[udp_base + 4..udp_base + 6]) as usize;
            if udp_len >= UDP_HDR_LEN && udp_base + udp_len <= ip_end {
                let payload_len = min(
                    udp_len - UDP_HDR_LEN,
                    ip_end.saturating_sub(udp_base + UDP_HDR_LEN),
                );
                let payload = &frame[udp_base + UDP_HDR_LEN..udp_base + UDP_HDR_LEN + payload_len];
                if process_dhcp_payload(runtime, src_ip, src_port, dst_port, payload) {
                    return;
                }
            }
        }
    }

    let is_broadcast = dst_ip == [255, 255, 255, 255];
    if dst_ip != runtime.ip && !is_broadcast {
        return;
    }

    if proto == 1 {
        process_icmp(runtime, frame, ihl, src_ip, dst_ip, ip_end);
        return;
    }

    if proto == 6 {
        process_tcp(runtime, stats, frame, ihl, src_ip, ip_end);
        return;
    }
    if proto != 17 {
        return;
    }

    let udp_base = ETH_HDR_LEN + ihl;
    if frame.len() < udp_base + UDP_HDR_LEN {
        return;
    }

    let src_port = read_u16_be(&frame[udp_base..udp_base + 2]);
    let dst_port = read_u16_be(&frame[udp_base + 2..udp_base + 4]);
    let udp_len = read_u16_be(&frame[udp_base + 4..udp_base + 6]) as usize;
    if udp_len < UDP_HDR_LEN {
        return;
    }
    if udp_base + udp_len > ip_end {
        return;
    }
    let payload_len = min(
        udp_len - UDP_HDR_LEN,
        ip_end.saturating_sub(udp_base + UDP_HDR_LEN),
    );
    let payload = &frame[udp_base + UDP_HDR_LEN..udp_base + UDP_HDR_LEN + payload_len];

    if process_dhcp_payload(runtime, src_ip, src_port, dst_port, payload) {
        return;
    }
    if process_dns_payload(runtime, src_ip, src_port, dst_port, payload) {
        return;
    }

    try_queue_udp(runtime, src_ip, dst_ip, src_port, dst_port, payload);
}
