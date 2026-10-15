#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
    Find,
    FindNext,
    FindPrevious,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Close,
    DocumentHome,
    DocumentEnd,
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
    pub fn shifted(&self) -> bool {
        self.shift != 0
    }

    pub fn event(&mut self, code: usize, pressed: bool) -> Option<Key> {
        let extended = code & 0x100 != 0;
        let scan = code & 0xff;
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
        let shift = self.shifted();
        let control = self.control != 0;
        if control {
            return match scan {
                0x1e => Some(Key::SelectAll),
                0x2e => Some(Key::Copy),
                0x2d => Some(Key::Cut),
                0x2f => Some(Key::Paste),
                0x31 => Some(Key::New),
                0x18 => Some(Key::Open),
                0x1f if shift => Some(Key::SaveAs),
                0x1f => Some(Key::Save),
                0x2c if shift => Some(Key::Redo),
                0x2c => Some(Key::Undo),
                0x15 => Some(Key::Redo),
                0x21 => Some(Key::Find),
                0x11 | 0x10 => Some(Key::Close),
                0x47 => Some(Key::DocumentHome),
                0x4f => Some(Key::DocumentEnd),
                _ => None,
            };
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
            0x3d if shift => Some(Key::FindPrevious),
            0x3d => Some(Key::FindNext),
            _ if !extended => ascii(scan, shift, self.caps).map(Key::Character),
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
    if let Some(letter) = letter {
        if shift ^ caps {
            return Some(letter.to_ascii_uppercase() as char);
        }
        return Some(letter as char);
    }
    let (normal, shifted) = match scan {
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
    Some(if shift {
        shifted as char
    } else {
        normal as char
    })
}
