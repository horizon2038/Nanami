//! Deliberately bounded device table shared by discovery and the NIC backend.
//! The backend uses queue zero: legacy e1000e and advanced igb descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    E1000e,
    I219,
    Igb,
    I210,
}

impl Family {
    pub fn driver(self) -> &'static str {
        match self {
            Self::E1000e | Self::I219 => "e1000e",
            Self::Igb | Self::I210 => "igb",
        }
    }
}

pub fn family(vendor: u16, device: u16, class: u32) -> Option<Family> {
    if vendor != 0x8086 || class != 0x020000 {
        return None;
    }
    match device {
        0x10d3 | 0x150c => Some(Family::E1000e), // 82574 / 82583 copper
        0x156f | 0x1570 | 0x15b7 | 0x15b8 | 0x15b9 | 0x15bb | 0x15bc | 0x15bd | 0x15be | 0x15d6
        | 0x15d7 | 0x15d8 | 0x15df | 0x15e0 | 0x15e1 | 0x15e2 | 0x15e3 => Some(Family::I219),
        0x10c9 | 0x150e | 0x1521 => Some(Family::Igb), // 82576 / 82580 / I350 copper
        0x1533 | 0x1539 | 0x157b => Some(Family::I210), // I210 / I211 copper
        _ => None,
    }
}
