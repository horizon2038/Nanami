use crate::{
    ddc,
    edid::Edid,
    gen9::{Mode, Registers},
    vbt::{self, Wiring},
};
use nanami_client::{gfx::display::*, pci};
use nanami_runtime::{RequestError, Word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub port: u8,
    pub kind: u8,
    pub pipe: u8,
}

/// Read active transcoder/DDI routing, including eDP's separate transcoder.
/// Do not infer connector type from the GPU model or a port's letter alone.
pub fn active_route(regs: &impl Registers, pipe: u8) -> Result<Route, RequestError> {
    if pipe >= 3 {
        return Err(RequestError::InvalidArgument);
    }
    let mut result = None;
    for (transcoder, offset) in [(0, 0x60400), (1, 0x61400), (2, 0x62400), (3, 0x6f400)] {
        let control = regs.read(offset);
        if control & (1 << 31) == 0 {
            continue;
        }
        let input = if transcoder == 3 {
            match (control >> 12) & 7 {
                0 | 4 => 0,
                5 => 1,
                6 => 2,
                _ => return Err(RequestError::Unsupported),
            }
        } else {
            transcoder
        };
        if input != pipe {
            continue;
        }
        let port = if transcoder == 3 {
            0
        } else {
            ((control >> 28) & 7) as u8
        };
        if port > 4 || regs.read(0x64000 + port as Word * 0x100) & (1 << 31) == 0 {
            return Err(RequestError::Unsupported);
        }
        let protocol = (control >> 24) & 7;
        let kind = match (transcoder == 3, protocol) {
            (true, 2) => CONNECTOR_EDP,
            (false, 0) => CONNECTOR_HDMI,
            (false, 1) => CONNECTOR_DVI,
            (false, 2) => CONNECTOR_DP,
            _ => return Err(RequestError::Unsupported), // MST/FDI need their own topology.
        };
        if result.is_some() {
            return Err(RequestError::Unsupported);
        } // Cloned routing is not one output.
        result = Some(Route { port, kind, pipe });
    }
    result.ok_or(RequestError::Unsupported)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pch {
    Sunrise,
    Cannon,
}
impl Pch {
    pub fn from_id(vendor: u16, device: u16) -> Option<Self> {
        if vendor != 0x8086 {
            return None;
        }
        match device & 0xff80 {
            0xa100 | 0x9d00 | 0xa280 | 0xa380 => Some(Self::Sunrise),
            0xa300 | 0x9d80 | 0x0280 | 0x0680 => Some(Self::Cannon),
            _ => None,
        }
    }
    pub fn pin(self, port: u8, alternate: u8) -> Result<u8, RequestError> {
        let pin = match self {
            Self::Sunrise => {
                if alternate != 0 {
                    alternate
                } else {
                    match port {
                        1 => 5,
                        2 => 4,
                        3 => 6,
                        _ => return Err(RequestError::Unsupported),
                    }
                }
            }
            Self::Cannon => {
                let vbt = if alternate == 0 {
                    match port {
                        1..=3 => port,
                        _ => return Err(RequestError::Unsupported),
                    }
                } else {
                    alternate
                };
                match vbt {
                    1 => 1,
                    2 => 2,
                    3 => 4,
                    4 => 3,
                    _ => return Err(RequestError::Unsupported),
                }
            }
        };
        let valid = match self {
            Self::Sunrise => matches!(pin, 4 | 5 | 6),
            Self::Cannon => (1..=4).contains(&pin),
        };
        if !valid {
            return Err(RequestError::Unsupported);
        }
        Ok(pin)
    }
}

pub struct Connector {
    pub route: Route,
    pub edid: Edid,
    pub width: u16,
    pub height: u16,
}
impl Connector {
    pub fn info(&self) -> ConnectorInfo {
        let mut flags = CONNECTOR_ACTIVE;
        if self.edid.len != 0 {
            flags |= CONNECTOR_EDID_VALID;
        }
        if self.edid.truncated {
            flags |= CONNECTOR_EDID_TRUNCATED;
        }
        ConnectorInfo {
            id: self.route.port + 1,
            kind: self.route.kind,
            pipe: self.route.pipe,
            flags,
            width: self.width,
            height: self.height,
            edid_bytes: self.edid.len as u16,
            mode_count: self.edid.timing_count as u16,
        }
    }
    pub fn read_edid(&self, offset: Word) -> Result<(Word, Word), RequestError> {
        if offset >= self.edid.len {
            return Err(RequestError::InvalidArgument);
        }
        let mut bytes = [0u8; 16];
        let count = (self.edid.len - offset).min(bytes.len());
        bytes[..count].copy_from_slice(&self.edid.bytes[offset..offset + count]);
        Ok((
            u64::from_le_bytes(bytes[..8].try_into().unwrap()) as Word,
            u64::from_le_bytes(bytes[8..].try_into().unwrap()) as Word,
        ))
    }
    pub fn monitor_mode(&self, ordinal: Word, part: Word) -> Result<(Word, Word), RequestError> {
        if part > 1 {
            return Err(RequestError::InvalidArgument);
        }
        if ordinal >= self.edid.timing_count {
            return Ok((MODE_END, 0));
        }
        let words = self.edid.timings[ordinal].words();
        if part == 0 {
            Ok((words[0], words[1]))
        } else {
            Ok((words[2], 0))
        }
    }
}

fn mapped_bytes(address: Word, len: Word) -> Result<&'static [u8], RequestError> {
    if len == 0 || len > 65536 || address.checked_add(len).is_none() {
        return Err(RequestError::Protocol);
    }
    let base = address & !4095;
    let prefix = address - base;
    let extent = prefix.checked_add(len).ok_or(RequestError::Protocol)?;
    let (_, mapped) = nanami_runtime::request_mmio(base, extent)?;
    Ok(unsafe { core::slice::from_raw_parts((mapped + prefix) as *const u8, len) })
}
fn wiring(port: Word, address: pci::Address, output: u8) -> Result<Option<Wiring>, RequestError> {
    let physical = pci::read(port, address, 0xfc, 4)? as Word;
    if physical == 0 {
        return Ok(None);
    }
    if physical & 4095 != 0 {
        return Err(RequestError::Protocol);
    }
    let region = mapped_bytes(physical, 8192)?;
    if let Ok(Some((address, bytes))) = vbt::extended_vbt(region, physical) {
        if let Ok(result) =
            mapped_bytes(address, bytes).and_then(|bytes| vbt::wiring(bytes, output))
        {
            return Ok(result);
        }
    }
    vbt::wiring(vbt::embedded_vbt(region)?, output)
}

pub fn inspect(
    regs: &mut impl Registers,
    mode: Mode,
    port: Word,
    device: pci::Device,
    timer: Word,
) -> Result<Connector, RequestError> {
    let route = active_route(regs, mode.pipe as u8)?;
    let mut result = Connector {
        route,
        edid: Edid::default(),
        width: mode.width as u16,
        height: mode.height as u16,
    };
    // Invalid wiring does not authorize guessing a different board's pinout.
    let wiring = match wiring(port, device.address, route.port) {
        Ok(wiring) => wiring.unwrap_or_default(),
        Err(error) => {
            nanami_runtime::println!("[intel-graphics] VBT unavailable: {}; skipping DDC", error);
            return Ok(result);
        }
    };
    let mut delay = ddc::TimerDelay {
        port: timer,
        remaining: 256,
    };
    let edid = match route.kind {
        CONNECTOR_DP | CONNECTOR_EDP => {
            ddc::aux(regs, &mut delay, wiring.aux.unwrap_or(route.port))
        }
        CONNECTOR_HDMI | CONNECTOR_DVI => {
            let mut index = 0;
            let mut pch = None;
            while let Some(device) = pci::enumerate(port, index)? {
                if device.class >> 8 == 0x0601 {
                    pch = Pch::from_id(device.vendor_id, device.device_id);
                    break;
                }
                index += 1;
            }
            pch.ok_or(RequestError::Unsupported)
                .and_then(|pch| pch.pin(route.port, wiring.ddc_pin))
                .and_then(|pin| ddc::gmbus(regs, &mut delay, pin))
        }
        _ => Err(RequestError::Unsupported),
    };
    match edid {
        Ok(edid) => {
            nanami_runtime::println!(
                "[intel-graphics] output={} kind={} EDID={} bytes detailed-modes={}",
                route.port + 1,
                route.kind,
                edid.len,
                edid.timing_count
            );
            result.edid = edid;
        }
        Err(error) => nanami_runtime::println!(
            "[intel-graphics] output={} DDC unavailable: {}",
            route.port + 1,
            error
        ),
    }
    Ok(result)
}
