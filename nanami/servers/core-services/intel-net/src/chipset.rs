//! MAC reset and queue setup; never write NVM/flash or reset a PCH-owned PHY.
use crate::registers::*;
use nanami_client::uapi::intel_net::Family;
use nanami_runtime::RequestError;

pub fn reset(regs: &mut impl Registers, family: Family) -> Result<[u8; 6], RequestError> {
    // Save the firmware-provisioned address before reset (I211 uses iNVM;
    // PCH flash reads are not the discrete controller's EERD protocol).
    let saved = mac_address(regs);
    regs.write(IMC, u32::MAX);
    if matches!(family, Family::Igb | Family::I210) {
        regs.write(EIMC, u32::MAX);
        regs.write(GPIE, 0); // legacy INTx, not MSI-X auto-masking
    }
    regs.write(RCTL, 0);
    regs.write(TCTL, 0);
    regs.flush();
    regs.delay_ms(10)?;
    if family == Family::I219 {
        // PCH I219 reset-hang workaround. No PHY reset: ME/firmware owns its
        // OEM configuration and a combined PHY/MAC reset can hang this device.
        regs.update(FEXTNVM11, 0, 1 << 13);
    }
    if family == Family::I210 {
        // These packet-buffer registers survive a warm reset.
        regs.write(RX_SIZE, 0xa2);
        regs.write(TX_SIZE, 0x0400_0014);
    }
    let control = regs.read(CTRL) & !(CTRL_PHY_RESET | CTRL_FORCE);
    regs.write(CTRL, control | CTRL_RESET);
    regs.flush();
    regs.delay_ms(20)?;
    regs.wait(CTRL, CTRL_RESET, 0, 100)?;
    regs.write(IMC, u32::MAX);
    let _ = regs.read(ICR);
    regs.write(WUC, 0);
    let autoneg = if family == Family::I219 { 0 } else { 1 << 5 };
    regs.write(CTRL, control | CTRL_LINK_UP | autoneg);
    regs.update(CTRL_EXT, 0, DRIVER_LOADED);
    regs.flush();
    regs.delay_ms(20)?;

    // Discrete copper PHYs use clause-22 at address 1. Preserve advertisement;
    // only leave power-down/isolate and restart an enabled autonegotiation.
    if family != Family::I219 {
        let phy = mdio_read(regs, 0)?;
        let next = (phy & !((1 << 11) | (1 << 10))) | (1 << 12) | (1 << 9);
        mdio_write(regs, 0, next)?;
    }
    let mac = match mac_address(regs).or(saved) {
        Some(mac) => mac,
        None if family != Family::I219 => eeprom_mac(regs)?,
        None => return Err(RequestError::Unsupported),
    };
    regs.write(RAL, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    regs.write(RAH, u16::from_le_bytes([mac[4], mac[5]]) as u32 | (1 << 31));
    Ok(mac)
}

fn eeprom_mac(regs: &mut impl Registers) -> Result<[u8; 6], RequestError> {
    let mut bytes = [0; 6];
    for index in 0..3 {
        regs.write(EERD, 1 | ((index as u32) << 2));
        regs.wait(EERD, 2, 2, 100)?;
        bytes[index * 2..index * 2 + 2]
            .copy_from_slice(&((regs.read(EERD) >> 16) as u16).to_le_bytes());
    }
    if bytes[0] & 1 != 0 || bytes == [0; 6] {
        return Err(RequestError::Unsupported);
    }
    Ok(bytes)
}

fn mac_address(regs: &impl Registers) -> Option<[u8; 6]> {
    let low = regs.read(RAL).to_le_bytes();
    let high = regs.read(RAH).to_le_bytes();
    let mac = [low[0], low[1], low[2], low[3], high[0], high[1]];
    if mac[0] & 1 != 0 || mac == [0; 6] || mac == [0xff; 6] {
        return None;
    }
    Some(mac)
}

fn mdio_read(regs: &mut impl Registers, register: u32) -> Result<u16, RequestError> {
    regs.write(MDIC, (2 << 26) | (1 << 21) | (register << 16));
    regs.wait(MDIC, 1 << 28, 1 << 28, 20)?;
    let value = regs.read(MDIC);
    if value & (1 << 30) != 0 {
        return Err(RequestError::Transport);
    }
    Ok(value as u16)
}

fn mdio_write(regs: &mut impl Registers, register: u32, value: u16) -> Result<(), RequestError> {
    regs.write(
        MDIC,
        (1 << 26) | (1 << 21) | (register << 16) | value as u32,
    );
    regs.wait(MDIC, 1 << 28, 1 << 28, 20)?;
    if regs.read(MDIC) & (1 << 30) != 0 {
        return Err(RequestError::Transport);
    }
    Ok(())
}

pub fn configure_queues(
    regs: &mut impl Registers,
    family: Family,
    rx: u64,
    tx: u64,
    bytes: usize,
) -> Result<(), RequestError> {
    for (base, address) in [(RX_BASE, rx), (TX_BASE, tx)] {
        regs.write(base + DCTL, 0);
        if matches!(family, Family::Igb | Family::I210) {
            regs.wait(base + DCTL, QUEUE_ENABLE, 0, 100)?;
        }
        regs.write(base + BASE_LOW, address as u32);
        regs.write(base + BASE_HIGH, (address >> 32) as u32);
        regs.write(base + LENGTH, bytes as u32);
        regs.write(base + HEAD, 0);
        regs.write(base + TAIL, 0);
    }
    regs.write(RXCSUM, 0); // checksums remain in net-server
    regs.write(RFCTL, 0); // legacy descriptors
    for index in 0..128 {
        regs.write(MTA + index * 4, 0);
        regs.write(VFTA + index * 4, 0);
    }
    if matches!(family, Family::Igb | Family::I210) {
        regs.write(SRRCTL, 2 | (1 << 25)); // 2 KiB; advanced one-buffer, no header split
        regs.write(RLPML, 1522);
    }
    regs.write(RXDCTL, QUEUE_ENABLE | 8 | (8 << 8) | (4 << 16));
    regs.write(TXDCTL, QUEUE_ENABLE | 8 | (1 << 8) | (1 << 16));
    if matches!(family, Family::Igb | Family::I210) {
        regs.wait(RXDCTL, QUEUE_ENABLE, QUEUE_ENABLE, 100)?;
        regs.wait(TXDCTL, QUEUE_ENABLE, QUEUE_ENABLE, 100)?;
    }
    regs.write(TIPG, 10 | (8 << 10) | (6 << 20));
    regs.flush();
    Ok(())
}
