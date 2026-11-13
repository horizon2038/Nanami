//! Select among physical ports only during DHCP acquisition, before sessions exist.
use nanami_client::net::*;
use nanami_runtime::Word;

pub fn select_linked(
    mut control: impl FnMut(Word, Word) -> Option<(Word, Word)>,
    rotate: bool,
) -> Option<(Word, Word)> {
    let (features, _) = control(NET_DEVICE_CONTROL_GET_FEATURES, 0)?;
    if features & NET_DEVICE_FEATURE_PORTS == 0 {
        return None; // Backward-compatible with virtio and other single-port drivers.
    }
    let (count, active) = control(NET_DEVICE_CONTROL_PORT_COUNT, 0)?;
    if count == 0 || count > 8 || active >= count {
        return None;
    }
    // Keep a working link unless DHCP timed out. At timeout, give another
    // connected LAN a chance instead of being pinned forever to PCI order.
    let start = if rotate { (active + 1) % count } else { active };
    for step in 0..count {
        let index = (start + step) % count;
        let (_, flags) = control(NET_DEVICE_CONTROL_PORT_INFO, index)?;
        if flags & NET_DEVICE_PORT_CARRIER == 0 {
            continue;
        }
        if index == active {
            return None;
        }
        let (mac, _) = control(NET_DEVICE_CONTROL_SELECT_PORT, index)?;
        return Some((index, mac));
    }
    None
}
