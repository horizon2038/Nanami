//! Boot-protocol reports translated to the existing input-service key codes.
//! State is per interface; detach synthesizes releases instead of leaving keys
//! or buttons stuck. Report-mode/NKRO-only devices are deliberately not guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: usize,
    pub code: usize,
    pub x: i16,
    pub y: i16,
}

pub struct Keyboard {
    modifiers: u8,
    keys: [u8; 6],
}
impl Keyboard {
    pub const fn new() -> Self {
        Self {
            modifiers: 0,
            keys: [0; 6],
        }
    }
    pub fn report(&mut self, data: &[u8], mut emit: impl FnMut(Event)) {
        if data.len() < 8 {
            return;
        }
        // ErrorRollOver/POSTFail/ErrorUndefined are not releases of held keys.
        if data[2..8].iter().any(|key| (1..=3).contains(key)) {
            return;
        }
        const MODIFIERS: [usize; 8] = [0x1d, 0x2a, 0x38, 0x15b, 0x11d, 0x36, 0x138, 0x15c];
        for (bit, code) in MODIFIERS.iter().enumerate() {
            if (self.modifiers ^ data[0]) & (1 << bit) != 0 {
                emit(Event {
                    kind: 1,
                    code: *code,
                    x: ((data[0] >> bit) & 1) as i16,
                    y: 0,
                });
            }
        }
        for (index, key) in self.keys.iter().copied().enumerate() {
            if key != 0 && !data[2..8].contains(&key) && !self.keys[..index].contains(&key) {
                if let Some(code) = keycode(key) {
                    emit(Event {
                        kind: 1,
                        code,
                        x: 0,
                        y: 0,
                    });
                }
            }
        }
        for (index, key) in data[2..8].iter().copied().enumerate() {
            if key != 0 && !self.keys.contains(&key) && !data[2..2 + index].contains(&key) {
                if let Some(code) = keycode(key) {
                    emit(Event {
                        kind: 1,
                        code,
                        x: 1,
                        y: 0,
                    });
                }
            }
        }
        self.modifiers = data[0];
        self.keys.copy_from_slice(&data[2..8]);
    }
    pub fn release(&mut self, emit: impl FnMut(Event)) {
        self.report(&[0; 8], emit);
    }
}

pub struct Mouse {
    buttons: u8,
}
impl Mouse {
    pub const fn new() -> Self {
        Self { buttons: 0 }
    }
    pub fn report(&mut self, data: &[u8], mut emit: impl FnMut(Event)) {
        if data.len() < 3 {
            return;
        }
        let buttons = data[0] & 7;
        for bit in 0..3 {
            if (buttons ^ self.buttons) & (1 << bit) != 0 {
                emit(Event {
                    kind: 2,
                    code: bit + 1,
                    x: ((buttons >> bit) & 1) as i16,
                    y: 0,
                });
            }
        }
        let x = data[1] as i8 as i16;
        let y = data[2] as i8 as i16;
        if x != 0 || y != 0 {
            emit(Event {
                kind: 3,
                code: 0,
                x,
                y,
            });
        }
        self.buttons = buttons;
    }
    pub fn release(&mut self, emit: impl FnMut(Event)) {
        self.report(&[0; 3], emit);
    }
}

fn keycode(usage: u8) -> Option<usize> {
    const LETTERS: [u8; 26] = [
        0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
    ];
    Some(match usage {
        4..=29 => LETTERS[(usage - 4) as usize] as usize,
        30..=38 => (usage - 30 + 2) as usize,
        39 => 0x0b,
        40 => 0x1c,
        41 => 0x01,
        42 => 0x0e,
        43 => 0x0f,
        44 => 0x39,
        45 => 0x0c,
        46 => 0x0d,
        47 => 0x1a,
        48 => 0x1b,
        49 | 50 => 0x2b,
        51 => 0x27,
        52 => 0x28,
        53 => 0x29,
        54 => 0x33,
        55 => 0x34,
        56 => 0x35,
        57 => 0x3a,
        58..=67 => (usage - 58 + 0x3b) as usize,
        68 => 0x57,
        69 => 0x58,
        70 => 0x137,
        71 => 0x46,
        73 => 0x152,
        74 => 0x147,
        75 => 0x149,
        76 => 0x153,
        77 => 0x14f,
        78 => 0x151,
        79 => 0x14d,
        80 => 0x14b,
        81 => 0x150,
        82 => 0x148,
        83 => 0x45,
        84 => 0x135,
        85 => 0x37,
        86 => 0x4a,
        87 => 0x4e,
        88 => 0x11c,
        89 => 0x4f,
        90 => 0x50,
        91 => 0x51,
        92 => 0x4b,
        93 => 0x4c,
        94 => 0x4d,
        95 => 0x47,
        96 => 0x48,
        97 => 0x49,
        98 => 0x52,
        99 => 0x53,
        100 => 0x56,
        101 => 0x15d,
        _ => return None,
    })
}
