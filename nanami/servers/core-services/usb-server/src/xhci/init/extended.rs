//! Initialization-only capability handling and BIOS/OS ownership handoff.
use super::super::{capabilities, protocol, read, write};
use libnanami::{RequestError, Word};

pub(super) struct Ports {
    pub slot_type: [u8; 256],
    pub port_major: [u8; 256],
    pub port_speeds: [u32; 256],
    pub superspeeds: [u16; 256],
}

fn malformed(error: capabilities::Error) -> RequestError {
    match error.value {
        Some(value) => libnanami::println!(
            "[usb-server] xHCI extcap error offset={:#x} value={:#010x}: {}",
            error.offset,
            value,
            error.reason
        ),
        None => libnanami::println!(
            "[usb-server] xHCI extcap error offset={:#x} (not read): {}",
            error.offset,
            error.reason
        ),
    }
    RequestError::Protocol
}

pub(super) fn configure(
    base: usize,
    bytes: usize,
    hcc: u32,
    port_count: usize,
    timer: Word,
) -> Result<Ports, RequestError> {
    let mut ports = Ports {
        slot_type: [0; 256],
        port_major: [0; 256],
        port_speeds: [0; 256],
        superspeeds: [0; 256],
    };
    let mut claimed = [false; 256];
    let mut read_cap = |offset| unsafe { read(base, offset) };
    let mut caps = capabilities::Capabilities::new(hcc, bytes);
    while let Some(cap) = caps.next(&mut read_cap).map_err(malformed)? {
        libnanami::println!(
            "[usb-server] xHCI extcap offset={:#x} header={:#010x}",
            cap.offset,
            cap.header
        );
        match cap.header as u8 {
            1 => {
                cap.require(8).map_err(malformed)?;
                unsafe { write(base, cap.offset, cap.header | (1 << 24)) };
                let mut last = read_cap(cap.offset);
                for _ in 0..1000 {
                    if last & (1 << 16) == 0 {
                        break;
                    }
                    crate::delay(timer, 1)?;
                    last = read_cap(cap.offset);
                }
                if last & (1 << 16) != 0 {
                    libnanami::println!(
                        "[usb-server] xHCI BIOS handoff timeout offset={:#x} USBLEGSUP={:#010x}",
                        cap.offset,
                        last
                    );
                    return Err(RequestError::Transport);
                }
                // Disable legacy SMIs without acknowledging arbitrary RW1C bits.
                let legacy = read_cap(cap.offset + 4);
                unsafe { write(base, cap.offset + 4, legacy & 0x000e_1fee) };
            }
            2 => {
                let protocol = cap
                    .protocol(&mut read_cap, &mut claimed[..=port_count])
                    .map_err(malformed)?;
                libnanami::println!(
                    "[usb-server] xHCI protocol offset={:#x} name={:#010x} revision={:02x}.{:02x} ports={}..{} slot-type={} PSIC={}",
                    cap.offset, protocol.name, protocol.major, protocol.minor,
                    protocol.ports.start, protocol.ports.end, protocol.slot_type, protocol.psi_count
                );
                let entries = &protocol.psi[..protocol.psi_count];
                for (index, value) in entries.iter().enumerate() {
                    libnanami::println!(
                        "[usb-server] xHCI PSI offset={:#x} value={:#010x}",
                        cap.offset + 16 + index * 4,
                        value
                    );
                }
                if protocol.ports.is_empty() {
                    continue;
                }
                if protocol.name != 0x2042_5355 || !matches!(protocol.major, 2 | 3) {
                    libnanami::println!("[usb-server] unsupported xHCI protocol; ports skipped");
                    continue;
                }
                let speeds = protocol::Speeds::parse(
                    protocol.major,
                    protocol.minor,
                    entries,
                    |index, reason| {
                        libnanami::println!(
                            "[usb-server] xHCI PSI skipped offset={:#x} value={:#010x}: {}",
                            cap.offset + 16 + index * 4,
                            entries[index],
                            reason
                        )
                    },
                )
                .map_err(|error| {
                    malformed(capabilities::Error {
                        offset: cap.offset + 16 + error.index * 4,
                        value: Some(entries[error.index]),
                        reason: error.reason,
                    })
                })?;
                if speeds.usb2 == 0 && speeds.usb3 == 0 {
                    libnanami::println!(
                        "[usb-server] no supported speed definitions; ports skipped"
                    );
                    continue;
                }
                for port in protocol.ports {
                    ports.slot_type[port] = protocol.slot_type;
                    ports.port_major[port] = protocol.major;
                    ports.port_speeds[port] = speeds.usb2;
                    ports.superspeeds[port] = speeds.usb3;
                }
            }
            _ => {} // Unknown capabilities need only a header to follow Next.
        }
    }
    Ok(ports)
}
