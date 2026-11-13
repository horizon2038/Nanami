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
    // PHY auto-negotiation is NOT CTRL.ASDE. I211 3.5.3.1.2.2 requires
    // ASDE=FRCSPD=0 for the internal PHY's SPD_IND to configure the MAC.
    // Enabling ASDE can leave a 10 Mb/s MAC on a gigabit PHY: TX descriptors
    // and GPTC advance while the link partner cannot decode the frames.
    let control =
        regs.read(CTRL) & !(CTRL_PHY_RESET | CTRL_FORCE | CTRL_MASTER_DISABLE | CTRL_AUTO_SPEED);
    if family == Family::I210 {
        regs.update(CTRL_EXT, SPEED_BYPASS, 0);
    }
    regs.write(CTRL, control | CTRL_RESET);
    // Intel's PCH reset sequence explicitly forbids a posted-write flush
    // here: reading the MAC before its reset delay can hang the function.
    if family != Family::I219 {
        regs.flush();
    }
    regs.delay_ms(20)?;
    regs.wait(CTRL, CTRL_RESET, 0, 100)?;
    regs.write(IMC, u32::MAX);
    let _ = regs.read(ICR);
    regs.write(WUC, 0);
    regs.write(CTRL, control | CTRL_LINK_UP);
    regs.update(CTRL_EXT, 0, DRIVER_LOADED);
    regs.flush();
    regs.delay_ms(20)?;

    // A MAC-only reset does not initialize a powered-down/hung discrete PHY.
    // Use CTRL.PHY_RST (not the clause-22 soft-reset bit, which bypasses the
    // I210/I211 internal configuration sequence). Never do this to a PCH PHY.
    if family != Family::I219 && regs.read(STATUS) & LINK_UP == 0 {
        regs.write(CTRL, control | CTRL_LINK_UP | CTRL_RESET | CTRL_PHY_RESET);
        regs.flush();
        regs.delay_ms(20)?;
        regs.write(CTRL, control | CTRL_LINK_UP);
        regs.flush();
        regs.delay_ms(20)?;
        regs.wait(CTRL, CTRL_RESET | CTRL_PHY_RESET, 0, 100)?;
        regs.write(IMC, u32::MAX);
        let _ = regs.read(ICR);
        regs.write(WUC, 0);
        regs.update(CTRL_EXT, 0, DRIVER_LOADED);
    }
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
    let advanced = matches!(family, Family::Igb | Family::I210);
    if family == Family::I219 {
        // PCH required MAC initialization, independent of PHY/ME ownership.
        // Preserve these policy bits when replacing the threshold fields below.
        regs.update(CTRL_EXT, 0, (1 << 22) | (1 << 17)); // required; relaxed-order disable
        regs.update(TXDCTL, 0, 1 << 22);
        regs.update(TXDCTL + 0x100, 0, 1 << 22);
        regs.update(TARC0, 0, (1 << 23) | (1 << 24) | (1 << 26) | (1 << 27));
        // This driver programs TCTL.MULR=0 when activating a port. TARC1's
        // bit 28 must have the opposite value, including for an unused queue.
        regs.update(TARC1, 0, (1 << 24) | (1 << 26) | (1 << 28) | (1 << 30));
        regs.update(RFCTL, 0, (1 << 6) | (1 << 7)); // disable NFS parsing
    }
    for (base, address) in [(RX_BASE, rx), (TX_BASE, tx)] {
        if advanced {
            regs.update(base + DCTL, QUEUE_ENABLE, 0);
            regs.wait(base + DCTL, QUEUE_ENABLE, 0, 100)?;
        }
        regs.write(base + BASE_LOW, address as u32);
        regs.write(base + BASE_HIGH, (address >> 32) as u32);
        regs.write(base + LENGTH, bytes as u32);
        if !advanced {
            // Legacy heads are writable only with global RX/TX disabled.
            regs.write(base + HEAD, 0);
            regs.write(base + TAIL, 0);
        }
        // I211 RDH/TDH are RO. QUEUE_ENABLE initializes head/tail, and tail
        // writes before its acknowledgement are ignored (333017, 4.5.9/10).
    }
    regs.write(RXCSUM, 0); // checksums remain in net-server

    // Legacy descriptors; preserve MAC policy.
    regs.update(RFCTL, 1 << 15, 0);

    // PCH has 32 multicast words and no discrete-controller VLAN filter
    // array. Do not clear reserved/manageability registers as if it were igb.
    let multicast_words = if family == Family::I219 { 32 } else { 128 };
    for index in 0..multicast_words {
        regs.write(MTA + index * 4, 0);
    }
    if family != Family::I219 {
        for index in 0..128 {
            regs.write(VFTA + index * 4, 0);
        }
    }
    if advanced {
        regs.write(SRRCTL, 2 | (1 << 25)); // 2 KiB; advanced one-buffer, no header split
        regs.write(RLPML, 1522);
        // I211 8.9.9/8.11.15: five-bit descriptor thresholds; bit 24 is
        // RESERVED, not GRAN. No stale SWFLUSH, priority or head-writeback
        // policy from firmware. RS with WTHRESH=0 writes DD immediately.
        regs.write(RXDCTL, QUEUE_ENABLE | 8 | (1 << 8) | (1 << 16));
        regs.write(TXDCTL, QUEUE_ENABLE | 8 | (1 << 8));
        regs.wait(RXDCTL, QUEUE_ENABLE, QUEUE_ENABLE, 100)?;
        regs.wait(TXDCTL, QUEUE_ENABLE, QUEUE_ENABLE, 100)?;
    } else {
        // PCH/e1000e have six-bit thresholds and GRAN=1 means descriptors.
        // Bit 25 is NOT QUEUE_ENABLE: RX reserves it; TX uses LWTHRESH.
        // Keep Intel's full-descriptor writeback policy (GRAN=1,WTHRESH=1),
        // required TX bit 22, and identical TXDCTL for both legacy queues.
        // One descriptor suffices even with all interrupts masked.
        let thresholds = DESCRIPTOR_GRANULARITY | 8 | (1 << 8) | (1 << 16);
        regs.write(RXDCTL, thresholds);
        regs.write(TXDCTL, thresholds | (1 << 22));
        regs.write(TXDCTL + 0x100, thresholds | (1 << 22));
    }
    regs.write(TIPG, 10 | (8 << 10) | (6 << 20));
    regs.flush();
    Ok(())
}

/// PCI BME must be enabled before queue initialization: master-disable holds
/// I210/I211 queues in reset. Descriptors must already refer to pinned memory.
pub fn start_queues(
    regs: &mut impl Registers,
    family: Family,
    rx: u64,
    tx: u64,
    bytes: usize,
    receive_tail: u32,
    enable_bus_master: impl FnOnce() -> Result<(), RequestError>,
) -> Result<(), RequestError> {
    if matches!(family, Family::I219 | Family::E1000e) {
        // Rings and packet buffers are write-back/coherent memory. Neither
        // compiler fences nor volatile accesses make no-snoop DMA coherent.
        // Disable descriptor AND payload no-snoop before enabling mastering.
        // ICH8 has inverted bits and is deliberately not a supported family.
        regs.update(GCR, PCIE_NO_SNOOP, 0);
        regs.flush();
    }
    if family == Family::I210 {
        // I211 8.2.3: GCR low bits are not its per-queue no-snoop controls.
        // NS_DIS overrides RXCTRL/TXCTRL and PCIe policy for all DMA. This
        // driver uses ordinary coherent WB buffers, never no-snoop buffers.
        regs.update(CTRL_EXT, 0, (1 << 16) | (1 << 17));
        regs.flush();
    }
    enable_bus_master()?;
    configure_queues(regs, family, rx, tx, bytes)?;
    regs.write(RX_BASE + TAIL, receive_tail);
    regs.flush();
    Ok(())
}
