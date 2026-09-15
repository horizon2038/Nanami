//! USB 2 link-speed classification, separate from the controller's Speed ID.
//! Each two-bit field describes one PSIV: 1 = FS, 2 = LS, 3 = HS, 0 = unsupported.
pub const DEFAULT_SPEEDS: u32 = (1 << 2) | (2 << 4) | (3 << 6);

pub fn classify(speeds: u32, id: u8) -> u8 {
    ((speeds >> (id * 2)) & 3) as u8
}

pub fn add_psi(speeds: &mut u32, psi: u32) -> Result<(), ()> {
    let id = (psi & 15) as u8;
    if id == 0 || classify(*speeds, id) != 0 {
        return Err(());
    }
    // USB 2 is symmetric and half-duplex. Do not interpret a USB 3 PSI as USB 2.
    if psi & ((7 << 6) | (3 << 14)) != 0 {
        return Err(());
    }
    let rate = (psi >> 16) as u64 * [1, 1000, 1_000_000, 1_000_000_000][((psi >> 4) & 3) as usize];
    let class = match rate {
        12_000_000 => 1,
        1_500_000 => 2,
        480_000_000 => 3,
        _ => 0,
    };
    *speeds |= class << (id * 2);
    Ok(())
}
