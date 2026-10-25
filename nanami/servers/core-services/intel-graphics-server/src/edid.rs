//! Bounded EDID 1.x/CTA detailed-timing parsing. No hardware mode is changed.
use nanami_client::gfx::display::*;
use nanami_runtime::RequestError;
pub const BLOCK_BYTES: usize = 128;
const MAX_TIMINGS: usize = 16;

pub struct Edid {
    pub bytes: [u8; EDID_MAX_BYTES],
    pub len: usize,
    pub timings: [Timing; MAX_TIMINGS],
    pub timing_count: usize,
    pub truncated: bool,
}
impl Default for Edid {
    fn default() -> Self {
        Self {
            bytes: [0; EDID_MAX_BYTES],
            len: 0,
            timings: [Timing::default(); MAX_TIMINGS],
            timing_count: 0,
            truncated: false,
        }
    }
}
pub fn valid_block(block: &[u8]) -> bool {
    block.len() == BLOCK_BYTES && block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}
pub fn base_extensions(block: &[u8]) -> Result<u8, RequestError> {
    if !valid_block(block) || block[..8] != [0, 255, 255, 255, 255, 255, 255, 0] || block[18] != 1 {
        return Err(RequestError::Protocol);
    }
    Ok(block[126])
}
impl Edid {
    pub fn parse(bytes: &[u8]) -> Result<Self, RequestError> {
        if !matches!(bytes.len(), 128 | 256) {
            return Err(RequestError::Protocol);
        }
        let extensions = base_extensions(&bytes[..128])?;
        if bytes.len() == 256 && (extensions == 0 || !valid_block(&bytes[128..])) {
            return Err(RequestError::Protocol);
        }
        let mut result = Self::default();
        result.len = bytes.len();
        result.truncated = extensions as usize > bytes.len() / 128 - 1;
        result.bytes[..bytes.len()].copy_from_slice(bytes);
        for (index, descriptor) in bytes[54..126].chunks_exact(18).enumerate() {
            let preferred = index == 0 && bytes[24] & 2 != 0;
            result.add_timing(descriptor, preferred);
        }
        if bytes.len() == 256 && bytes[128] == 2 {
            let extension = &bytes[128..];
            let start = extension[2] as usize;
            if start != 0 {
                if !(4..=127).contains(&start) {
                    return Err(RequestError::Protocol);
                }
                for descriptor in extension[start..127].chunks_exact(18) {
                    result.add_timing(descriptor, false);
                }
            }
        }
        Ok(result)
    }
    fn add_timing(&mut self, bytes: &[u8], preferred: bool) {
        let Some(mut timing) = detailed_timing(bytes) else {
            return;
        };
        if preferred {
            timing.flags |= MODE_PREFERRED;
        }
        for existing in &mut self.timings[..self.timing_count] {
            let mut comparable = *existing;
            comparable.flags &= !MODE_PREFERRED;
            let mut candidate = timing;
            candidate.flags &= !MODE_PREFERRED;
            if comparable == candidate {
                existing.flags |= timing.flags & MODE_PREFERRED;
                return;
            }
        }
        if self.timing_count < self.timings.len() {
            self.timings[self.timing_count] = timing;
            self.timing_count += 1;
        }
    }
}
fn detailed_timing(bytes: &[u8]) -> Option<Timing> {
    let clock_khz = u16::from_le_bytes([bytes[0], bytes[1]]) as u32 * 10;
    // Interlaced and composite/analog sync require different programming paths.
    if clock_khz == 0 || bytes[17] & 0x80 != 0 || bytes[17] & 0x18 != 0x18 {
        return None;
    }
    let ha = bytes[2] as u16 | (((bytes[4] >> 4) as u16) << 8);
    let hb = bytes[3] as u16 | (((bytes[4] & 15) as u16) << 8);
    let va = bytes[5] as u16 | (((bytes[7] >> 4) as u16) << 8);
    let vb = bytes[6] as u16 | (((bytes[7] & 15) as u16) << 8);
    let hp = bytes[8] as u16 | (((bytes[11] >> 6) as u16) << 8);
    let hs = bytes[9] as u16 | ((((bytes[11] >> 4) & 3) as u16) << 8);
    let vp = (bytes[10] >> 4) as u16 | ((((bytes[11] >> 2) & 3) as u16) << 4);
    let vs = (bytes[10] & 15) as u16 | (((bytes[11] & 3) as u16) << 4);
    let flags = ((bytes[17] & 2 != 0) as u32) * MODE_HSYNC_POSITIVE
        | ((bytes[17] & 4 != 0) as u32) * MODE_VSYNC_POSITIVE;
    let timing = Timing {
        clock_khz,
        flags,
        hactive: ha,
        hsync_start: ha + hp,
        hsync_end: ha + hp + hs,
        htotal: ha + hb,
        vactive: va,
        vsync_start: va + vp,
        vsync_end: va + vp + vs,
        vtotal: va + vb,
    };
    timing.valid().then_some(timing)
}
