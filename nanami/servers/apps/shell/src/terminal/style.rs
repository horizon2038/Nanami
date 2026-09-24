use super::screen::{Cell, DEFAULT_BG, DEFAULT_FG};

#[derive(Clone, Copy)]
pub struct Style {
    fg: u32,
    bg: u32,
    bold: bool,
    inverse: bool,
    underline: bool,
}

impl Style {
    pub const DEFAULT: Self = Self {
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
        bold: false,
        inverse: false,
        underline: false,
    };

    pub fn cell(self, byte: u8) -> Cell {
        let fg = if self.bold {
            self.fg | 0x0040_4040
        } else {
            self.fg
        };
        let (fg, bg) = if self.inverse {
            (self.bg, fg)
        } else {
            (fg, self.bg)
        };
        Cell {
            byte,
            fg,
            bg,
            underline: self.underline,
        }
    }

    pub fn apply(&mut self, params: &[usize]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => *self = Self::DEFAULT,
                1 => self.bold = true,
                22 => self.bold = false,
                4 => self.underline = true,
                24 => self.underline = false,
                7 => self.inverse = true,
                27 => self.inverse = false,
                30..=37 => self.fg = palette(params[i] - 30),
                40..=47 => self.bg = palette(params[i] - 40),
                90..=97 => self.fg = palette(params[i] - 90 + 8),
                100..=107 => self.bg = palette(params[i] - 100 + 8),
                39 => self.fg = DEFAULT_FG,
                49 => self.bg = DEFAULT_BG,
                38 | 48 => {
                    let foreground = params[i] == 38;
                    let color = match params.get(i + 1) {
                        Some(5) if i + 2 < params.len() => {
                            i += 2;
                            Some(palette(params[i].min(255)))
                        }
                        Some(2) if i + 4 < params.len() => {
                            let color = (params[i + 2].min(255) << 16)
                                | (params[i + 3].min(255) << 8)
                                | params[i + 4].min(255);
                            i += 4;
                            Some(color as u32)
                        }
                        _ => None,
                    };
                    if let Some(color) = color {
                        if foreground {
                            self.fg = color;
                        } else {
                            self.bg = color;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn palette(index: usize) -> u32 {
    const BASIC: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    if index < 16 {
        return BASIC[index];
    }
    if index >= 232 {
        return ((8 + (index - 232) * 10) * 0x010101) as u32;
    }
    let n = index - 16;
    let level = |v| if v == 0 { 0 } else { 55 + v * 40 };
    ((level(n / 36) << 16) | (level(n / 6 % 6) << 8) | level(n % 6)) as u32
}
