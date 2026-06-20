use super::arp::process_arp;
use super::ip::process_ipv4;
use super::*;

pub(crate) fn process_ethernet_frame(runtime: &mut NetRuntime, stats: &mut NetStats, frame: &[u8]) {
    if frame.len() < ETH_HDR_LEN {
        return;
    }
    let ethertype = read_u16_be(&frame[12..14]);
    if ethertype == 0x0806 {
        process_arp(runtime, frame);
    } else if ethertype == 0x0800 {
        process_ipv4(runtime, stats, frame);
    }
}
