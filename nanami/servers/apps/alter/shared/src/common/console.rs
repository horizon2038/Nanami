//! Window-local Linux console state and PS/2 byte protocol, never host hardware.
#[derive(Clone, Copy)]
pub struct Console {
    pub keyboard_mode: u32,
    pub display_mode: u32,
    pub vt_mode: [u8; 8],
    pub termios: Option<[u8; 36]>,
    pub leds: u8,
    pub mouse: Ps2Mouse,
}
impl Console {
    pub const EMPTY: Self = Self {
        keyboard_mode: 1, // K_XLATE; the X server explicitly selects K_MEDIUMRAW.
        display_mode: 0,
        vt_mode: [0; 8],
        termios: None,
        leds: 0,
        mouse: Ps2Mouse::EMPTY,
    };
}

/// US console keymap in Linux keyboard.h's KT_* encoding.
pub fn keymap(table: u8, key: u8) -> u16 {
    let shifted = table & 1 != 0;
    let character = match key {
        1 => 27,
        2..=13 => {
            let row = if shifted {
                b"!@#$%^&*()_+"
            } else {
                b"1234567890-="
            };
            row[(key - 2) as usize]
        }
        14 => 127,
        15 => 9,
        16..=27 => {
            let row = if shifted {
                b"QWERTYUIOP{}"
            } else {
                b"qwertyuiop[]"
            };
            row[(key - 16) as usize]
        }
        30..=41 => {
            let row = if shifted {
                b"ASDFGHJKL:\"~"
            } else {
                b"asdfghjkl;'`"
            };
            row[(key - 30) as usize]
        }
        43 => {
            if shifted {
                b'|'
            } else {
                b'\\'
            }
        }
        44..=53 => {
            let row = if shifted {
                b"ZXCVBNM<>?"
            } else {
                b"zxcvbnm,./"
            };
            row[(key - 44) as usize]
        }
        57 => b' ',
        _ => 0,
    };
    if character != 0 {
        return character as u16;
    }
    match key {
        28 => 0x0201,      // K_ENTER
        29 | 97 => 0x0702, // K_CTRL
        42 | 54 => 0x0700, // K_SHIFT
        56 => 0x0703,      // K_ALT
        100 => 0x0701,     // K_ALTGR
        58 => 0x0207,      // K_CAPS
        59..=68 => 0x0100 + (key - 59) as u16,
        87 | 88 => 0x010a + (key - 87) as u16,
        96 => 0x030e,  // K_PENTER
        102 => 0x0114, // K_FIND
        103 => 0x0603,
        104 => 0x0118,
        105 => 0x0601,
        106 => 0x0602,
        107 => 0x0117,
        108 => 0x0600,
        109 => 0x0119,
        110 => 0x0115,
        111 => 0x0116,
        _ => 0x0200, // K_HOLE: an unmapped key, not another layout.
    }
}

pub fn mediumraw(key: u16, value: i32) -> Option<u8> {
    if key >= 128 || !(0..=2).contains(&value) {
        return None;
    }
    let mut byte = key as u8;
    if value == 0 {
        byte |= 0x80;
    }
    Some(byte)
}

#[derive(Clone, Copy)]
pub struct Ps2Mouse {
    queue: [u8; 32],
    head: usize,
    count: usize,
    response: [u8; 5],
    response_head: usize,
    response_count: usize,
    rates: [u8; 3],
    parameter: u8,
    wheel: bool,
    enabled: bool,
    buttons: u8,
    pending_buttons: bool,
    dx: i32,
    dy: i32,
    dz: i32,
    announced: bool,
}
impl Ps2Mouse {
    pub const EMPTY: Self = Self {
        queue: [0; 32],
        head: 0,
        count: 0,
        response: [0; 5],
        response_head: 0,
        response_count: 0,
        rates: [0; 3],
        parameter: 0,
        wheel: false,
        enabled: true,
        buttons: 0,
        pending_buttons: false,
        dx: 0,
        dy: 0,
        dz: 0,
        announced: false,
    };
    pub fn announce(&mut self) -> bool {
        if self.announced {
            return false;
        }
        self.announced = true;
        // Kdrive validates five PS/2 packets before accepting motion. Publish
        // the initial all-buttons-up state without moving the pointer, so its
        // protocol detection cannot discard the first absolute-to-relative
        // alignment or leave host and nested cursors permanently offset.
        for _ in 0..5 {
            self.event(0, 0, 0);
        }
        true
    }
    pub fn ready(&self) -> bool {
        self.response_count != 0 || self.motion_ready()
    }
    pub fn is_announced(&self) -> bool {
        self.announced
    }
    pub fn writable(&self) -> bool {
        true
    }
    pub fn pop(&mut self) -> Option<u8> {
        if self.response_count != 0 {
            let byte = self.response[self.response_head];
            self.response_head += 1;
            self.response_count -= 1;
            return Some(byte);
        }
        self.pop_motion()
    }
    fn motion_ready(&self) -> bool {
        self.count != 0
            || (self.enabled
                && (self.dx != 0 || self.dy != 0 || self.dz != 0 || self.pending_buttons))
    }
    fn pop_motion(&mut self) -> Option<u8> {
        if self.count == 0 && self.motion_ready() {
            self.event(0, 0, 0);
        }
        if self.count == 0 {
            return None;
        }
        let byte = self.queue[self.head];
        self.head = (self.head + 1) % self.queue.len();
        self.count -= 1;
        Some(byte)
    }
    fn respond(&mut self, bytes: &[u8]) {
        self.response[..bytes.len()].copy_from_slice(bytes);
        self.response_head = 0;
        self.response_count = bytes.len();
    }
    fn push(&mut self, byte: u8) {
        if self.count < self.queue.len() {
            self.queue[(self.head + self.count) % self.queue.len()] = byte;
            self.count += 1;
        }
    }
    pub fn command(&mut self, byte: u8) -> bool {
        // Linux mousedev keeps one command response, separate from motion.
        // A later write replaces an unread reply. Queuing hardware-style ACKs
        // here makes Xfbdev interpret leftover ACK bytes as pointer movement.
        self.respond(&[0xfa]);
        if self.parameter != 0 {
            if self.parameter == 0xf3 {
                self.rates.rotate_left(1);
                self.rates[2] = byte;
                if self.rates == [200, 100, 80] {
                    self.wheel = true;
                }
            }
            self.parameter = 0;
            return true;
        }
        match byte {
            0xf2 => {
                self.respond(&[0xfa, if self.wheel { 3 } else { 0 }]);
            }
            0xf3 | 0xe8 => {
                self.parameter = byte;
            }
            0xf4 => {
                self.enabled = true;
            }
            0xf5 => {
                self.enabled = false;
            }
            0xf6 | 0xff => {
                self.wheel = false;
                self.rates = [0; 3];
                self.enabled = true;
                if byte == 0xff {
                    self.respond(&[0xfa, 0xaa, 0]);
                }
            }
            0xe9 => self.respond(&[0xfa, 0x60, 3, 200]),
            0xeb => {
                let size = if self.wheel { 4 } else { 3 };
                if !self.motion_ready() {
                    self.event(0, 0, 0);
                }
                let mut packet = [0xfa; 5];
                for byte in &mut packet[1..=size] {
                    *byte = self.pop_motion().unwrap_or(0);
                }
                self.respond(&packet[..=size]);
            }
            _ => {}
        }
        true
    }
    /// Consume Linux EV_* records and emit complete packets at SYN_REPORT.
    pub fn event(&mut self, kind: u16, code: u16, value: i32) {
        match (kind, code) {
            (1, 0x110..=0x112) => {
                let previous = self.buttons;
                let bit = match code {
                    0x110 => 1,
                    0x111 => 2,
                    _ => 4,
                };
                if value == 0 {
                    self.buttons &= !bit;
                } else {
                    self.buttons |= bit;
                }
                self.pending_buttons |= previous != self.buttons;
            }
            (2, 0) => self.dx = self.dx.saturating_add(value),
            (2, 1) => self.dy = self.dy.saturating_sub(value),
            (2, 8) => self.dz = self.dz.saturating_sub(value),
            (0, 0) => {
                while self.enabled && self.count <= self.queue.len() - 4 {
                    let x = self.dx.clamp(-127, 127);
                    let y = self.dy.clamp(-127, 127);
                    let z = self.dz.clamp(-127, 127);
                    let mut first = 8 | self.buttons;
                    if x < 0 {
                        first |= 0x10;
                    }
                    if y < 0 {
                        first |= 0x20;
                    }
                    self.push(first);
                    self.push(x as u8);
                    self.push(y as u8);
                    if self.wheel {
                        self.push(z as u8);
                    }
                    self.dx -= x;
                    self.dy -= y;
                    self.dz -= z;
                    self.pending_buttons = false;
                    if self.dx == 0 && self.dy == 0 && self.dz == 0 {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
}
