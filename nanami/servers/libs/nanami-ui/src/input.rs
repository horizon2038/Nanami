use alloc::string::{String, ToString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Control(char),
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F5,
}
#[derive(Default)]
pub struct Keyboard {
    shift: u8,
    control: u8,
    alt: u8,
    caps: bool,
    caps_down: bool,
}
impl Keyboard {
    pub fn shift(&self) -> bool {
        self.shift != 0
    }
    pub fn control(&self) -> bool {
        self.control != 0
    }
    pub fn event(&mut self, code: usize, pressed: bool) -> Option<Key> {
        let scan = code & 255;
        let extended = code & 256 != 0;
        let bit = if extended { 2 } else { 1 };
        let modifier = match scan {
            0x2a => Some((&mut self.shift, 1)),
            0x36 => Some((&mut self.shift, 2)),
            0x1d => Some((&mut self.control, bit)),
            0x38 => Some((&mut self.alt, bit)),
            _ => None,
        };
        if let Some((state, bit)) = modifier {
            if pressed {
                *state |= bit;
            } else {
                *state &= !bit;
            }
            return None;
        }
        if scan == 0x3a {
            if pressed && !self.caps_down {
                self.caps = !self.caps;
            }
            self.caps_down = pressed;
            return None;
        }
        if !pressed || self.alt != 0 {
            return None;
        }
        if self.control() {
            return ascii(scan, false, false).map(Key::Control);
        }
        match scan {
            0x01 => Some(Key::Escape),
            0x0e => Some(Key::Backspace),
            0x53 => Some(Key::Delete),
            0x1c => Some(Key::Enter),
            0x0f => Some(Key::Tab),
            0x4b => Some(Key::Left),
            0x4d => Some(Key::Right),
            0x48 => Some(Key::Up),
            0x50 => Some(Key::Down),
            0x47 => Some(Key::Home),
            0x4f => Some(Key::End),
            0x49 => Some(Key::PageUp),
            0x51 => Some(Key::PageDown),
            0x3f => Some(Key::F5),
            _ if !extended => ascii(scan, self.shift(), self.caps).map(Key::Character),
            _ => None,
        }
    }
}
fn ascii(scan: usize, shift: bool, caps: bool) -> Option<char> {
    let letter = match scan {
        0x10..=0x19 => Some(b"qwertyuiop"[scan - 0x10]),
        0x1e..=0x26 => Some(b"asdfghjkl"[scan - 0x1e]),
        0x2c..=0x32 => Some(b"zxcvbnm"[scan - 0x2c]),
        _ => None,
    };
    if let Some(c) = letter {
        return Some(if shift ^ caps {
            c.to_ascii_uppercase() as char
        } else {
            c as char
        });
    }
    let (a, b) = match scan {
        0x02..=0x0b => (b"1234567890"[scan - 2], b"!@#$%^&*()"[scan - 2]),
        0x0c => (b'-', b'_'),
        0x0d => (b'=', b'+'),
        0x1a => (b'[', b'{'),
        0x1b => (b']', b'}'),
        0x27 => (b';', b':'),
        0x28 => (b'\'', b'"'),
        0x29 => (b'`', b'~'),
        0x2b => (b'\\', b'|'),
        0x33 => (b',', b'<'),
        0x34 => (b'.', b'>'),
        0x35 => (b'/', b'?'),
        0x39 => (b' ', b' '),
        _ => return None,
    };
    Some(if shift { b as char } else { a as char })
}

pub struct Field {
    pub text: String,
    pub cursor: usize,
    pub selected: bool,
}
impl Field {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: text.len(),
            selected: true,
        }
    }
    pub fn key(&mut self, key: Key) {
        match key {
            Key::Control('a') => {
                self.selected = true;
            }
            Key::Character(c) if self.selected || self.text.len() + c.len_utf8() <= 255 => {
                if self.selected {
                    self.text.clear();
                    self.cursor = 0;
                }
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                self.selected = false;
            }
            Key::Backspace | Key::Delete => {
                if self.selected {
                    self.text.clear();
                    self.cursor = 0;
                } else if key == Key::Backspace && self.cursor > 0 {
                    let previous = self.text[..self.cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.text.drain(previous..self.cursor);
                    self.cursor = previous;
                } else if key == Key::Delete && self.cursor < self.text.len() {
                    let end =
                        self.cursor + self.text[self.cursor..].chars().next().unwrap().len_utf8();
                    self.text.drain(self.cursor..end);
                }
                self.selected = false;
            }
            Key::Left => {
                self.cursor = self.text[..self.cursor]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.selected = false;
            }
            Key::Right => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
                self.selected = false;
            }
            Key::Home => {
                self.cursor = 0;
                self.selected = false;
            }
            Key::End => {
                self.cursor = self.text.len();
                self.selected = false;
            }
            _ => {}
        }
    }
}
