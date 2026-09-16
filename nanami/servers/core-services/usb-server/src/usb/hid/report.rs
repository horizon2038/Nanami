//! Compiled mouse input layouts; decoding performs no allocations or descriptor parsing.
use alloc::vec::Vec;

#[derive(Clone, Copy)]
pub(super) struct Field {
    pub offset: usize,
    pub width: u32,
    pub minimum: i64,
    pub maximum: i64,
}

impl Field {
    fn read(self, data: &[u8]) -> Option<i16> {
        let first = self.offset / 8;
        let shift = self.offset % 8;
        let bytes = (shift + self.width as usize).div_ceil(8);
        let mut raw = 0u64;
        for (index, byte) in data.get(first..first + bytes)?.iter().enumerate() {
            raw |= (*byte as u64) << (index * 8);
        }
        let raw = (raw >> shift) & ((1u64 << self.width) - 1);
        let value = if self.minimum < 0 {
            (raw << (64 - self.width)) as i64 >> (64 - self.width)
        } else {
            raw as i64
        };
        if value < self.minimum || value > self.maximum {
            return None;
        }
        Some(value.clamp(i16::MIN as i64, i16::MAX as i64) as i16)
    }
}

pub(super) struct Report {
    pub id: u8,
    pub bits: usize,
    // Buttons 1..3, X, Y, vertical Wheel. Missing fields leave held buttons alone.
    pub fields: [Option<Field>; 6],
}

pub struct MouseReport {
    pub(super) numbered: bool,
    pub(super) reports: Vec<Report>,
}

impl MouseReport {
    pub fn parse(descriptor: &[u8], packet_size: usize) -> Result<Self, ()> {
        super::descriptor::parse(descriptor, packet_size)
    }

    pub fn has_wheel(&self) -> bool {
        self.reports.iter().any(|report| report.fields[5].is_some())
    }

    pub(super) fn decode(&self, data: &[u8]) -> Option<[Option<i16>; 6]> {
        let (id, data) = if self.numbered {
            let (id, data) = data.split_first()?;
            (*id, data)
        } else {
            (0, data)
        };
        let report = self.reports.iter().find(|report| report.id == id)?;
        // Reject short packets as a whole, before changing any held-button state.
        if data.len() < report.bits.div_ceil(8) {
            return None;
        }
        let mut values = [None; 6];
        for (value, field) in values.iter_mut().zip(&report.fields) {
            if let Some(field) = field {
                *value = Some(field.read(data)?);
            }
        }
        Some(values)
    }
}
