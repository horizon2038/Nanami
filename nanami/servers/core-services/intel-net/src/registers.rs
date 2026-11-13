//! Queue-zero register definitions (82574, PCH I219, 82576 and I210/I211).
pub const CTRL: usize = 0x0000;
pub const STATUS: usize = 0x0008;
pub const EERD: usize = 0x0014;
pub const CTRL_EXT: usize = 0x0018;
pub const MDIC: usize = 0x0020;
pub const ICR: usize = 0x00c0;
pub const ITR: usize = 0x00c4;
pub const IMS: usize = 0x00d0;
pub const IMC: usize = 0x00d8;
pub const RCTL: usize = 0x0100;
pub const TCTL: usize = 0x0400;
pub const TIPG: usize = 0x0410;
pub const RX_BASE: usize = 0x2800;
pub const TX_BASE: usize = 0x3800;
pub const BASE_LOW: usize = 0;
pub const BASE_HIGH: usize = 4;
pub const LENGTH: usize = 8;
pub const HEAD: usize = 0x10;
pub const TAIL: usize = 0x18;
pub const DCTL: usize = 0x28;
pub const RXDCTL: usize = RX_BASE + DCTL;
pub const TXDCTL: usize = TX_BASE + DCTL;
pub const SRRCTL: usize = 0x280c;
pub const RAL: usize = 0x5400;
pub const RAH: usize = 0x5404;
pub const MTA: usize = 0x5200;
pub const VFTA: usize = 0x5600;
pub const RXCSUM: usize = 0x5000;
pub const RFCTL: usize = 0x5008;
pub const GCR: usize = 0x5b00;
pub const TARC0: usize = 0x3840;
pub const TARC1: usize = 0x3940;
pub const WUC: usize = 0x5800;
pub const FEXTNVM11: usize = 0x5bbc;
pub const RX_SIZE: usize = 0x2404;
pub const TX_SIZE: usize = 0x3404;
pub const GPIE: usize = 0x1514;
pub const EIMC: usize = 0x1528;
pub const RLPML: usize = 0x5004;
pub const CTRL_RESET: u32 = 1 << 26;
pub const CTRL_MASTER_DISABLE: u32 = 1 << 2;
pub const CTRL_PHY_RESET: u32 = 1 << 31;
pub const CTRL_LINK_UP: u32 = 1 << 6;
pub const CTRL_AUTO_SPEED: u32 = 1 << 5;
pub const SPEED_BYPASS: u32 = 1 << 15;
pub const CTRL_FORCE: u32 = (1 << 3) | (1 << 11) | (1 << 12);
pub const LINK_UP: u32 = 1 << 1;
pub const ENABLE: u32 = 1 << 1;
pub const QUEUE_ENABLE: u32 = 1 << 25;
pub const DESCRIPTOR_GRANULARITY: u32 = 1 << 24;
pub const PCIE_NO_SNOOP: u32 = 0x3f;
pub const DRIVER_LOADED: u32 = 1 << 28;
pub const RX_CONTROL: u32 = ENABLE | (1 << 15) | (1 << 26); // broadcast, strip CRC, 2 KiB
pub const TX_CONTROL: u32 = ENABLE | (1 << 3) | (15 << 4) | (64 << 12);
pub const RX_INTERRUPTS: u32 = (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7);
pub const DONE: u8 = 1;
pub const END_PACKET: u8 = 2;
pub const TX_COMMAND: u8 = 1 | 2 | 8; // EOP, IFCS, RS; legacy (DEXT=0)

pub fn link_mbps(status: u32) -> usize {
    if status & LINK_UP == 0 {
        return 0;
    }
    match (status >> 6) & 3 {
        0 => 10,
        1 => 100,
        _ => 1000,
    }
}

pub trait Registers {
    fn read(&self, offset: usize) -> u32;
    fn write(&mut self, offset: usize, value: u32);
    fn delay_ms(&mut self, ms: usize) -> Result<(), nanami_runtime::RequestError>;

    fn update(&mut self, offset: usize, clear: u32, set: u32) {
        let value = (self.read(offset) & !clear) | set;
        self.write(offset, value);
    }
    fn flush(&self) {
        let _ = self.read(STATUS);
    }
    fn wait(
        &mut self,
        offset: usize,
        mask: u32,
        expected: u32,
        ms: usize,
    ) -> Result<(), nanami_runtime::RequestError> {
        for _ in 0..ms {
            if self.read(offset) & mask == expected {
                return Ok(());
            }
            self.delay_ms(1)?;
        }
        Err(nanami_runtime::RequestError::Transport)
    }
}
