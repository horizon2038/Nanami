use super::{State, Terminal};

impl Terminal {
    pub fn process_byte(&mut self, byte: u8) {
        self.view_offset = 0;
        // CAN/SUB cancel even incomplete escape sequences.
        if matches!(byte, 0x18 | 0x1a) {
            self.state = State::Ground;
            return;
        }
        if byte == 0x1b && !matches!(self.state, State::String | State::StringEscape) {
            self.state = State::Escape;
            return;
        }
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::Charset => self.state = State::Ground,
            State::String => {
                if byte == 7 {
                    self.state = State::Ground;
                } else if byte == 0x1b {
                    self.state = State::StringEscape;
                }
            }
            State::StringEscape => {
                self.state = if byte == b'\\' {
                    State::Ground
                } else {
                    State::String
                }
            }
            State::Csi => self.csi_byte(byte),
        }
    }

    fn ground(&mut self, byte: u8) {
        let blank = self.style.cell(b' ');
        match byte {
            b'\r' => {
                self.screen.x = 0;
                self.screen.wrap_pending = false;
            }
            b'\n' | 0x0b | 0x0c => self.screen.index(blank),
            8 => {
                self.screen.x = self.screen.x.saturating_sub(1);
                self.screen.wrap_pending = false;
            }
            b'\t' => {
                self.screen.x = ((self.screen.x / 8 + 1) * 8).min(self.cols() - 1);
                self.screen.wrap_pending = false;
            }
            0x20..=0x7e => self
                .screen
                .put(self.style.cell(byte), blank, self.wrap, self.insert),
            _ => {}
        }
    }

    fn escape(&mut self, byte: u8) {
        self.state = State::Ground;
        match byte {
            b'[' => {
                self.params.fill(0);
                self.count = 1;
                self.private = false;
                self.ignored = false;
                self.state = State::Csi;
            }
            b']' | b'P' | b'_' | b'^' => self.state = State::String,
            b'(' | b')' | b'*' | b'+' | b'%' => self.state = State::Charset,
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.screen.index(self.style.cell(b' ')),
            b'E' => {
                self.screen.x = 0;
                self.screen.index(self.style.cell(b' '));
            }
            b'M' => self.screen.reverse_index(self.style.cell(b' ')),
            b'c' => self.reset(),
            _ => {}
        }
    }

    fn csi_byte(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                self.params[self.count - 1] = self.params[self.count - 1]
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as usize)
            }
            b';' => {
                if self.count < self.params.len() {
                    self.count += 1;
                } else {
                    self.ignored = true;
                }
            }
            b'?' => self.private = true,
            0x20..=0x3f => self.ignored = true,
            0x40..=0x7e => {
                self.state = State::Ground;
                if !self.ignored {
                    self.csi(byte);
                }
            }
            0..=0x1f => self.ground(byte),
            _ => {
                self.state = State::Ground;
            }
        }
    }

    fn csi(&mut self, byte: u8) {
        let n = self.param(0, 1);
        let blank = self.style.cell(b' ');
        if self.private {
            if matches!(byte, b'h' | b'l') {
                let enabled = byte == b'h';
                for i in 0..self.count {
                    match self.params[i] {
                        1 => self.application_cursor = enabled,
                        6 => {
                            self.origin = enabled;
                            self.screen.x = 0;
                            self.screen.y = if enabled { self.screen.top } else { 0 };
                        }
                        7 => {
                            self.wrap = enabled;
                            self.screen.wrap_pending = false;
                        }
                        25 => self.cursor_visible = enabled,
                        47 | 1047 => self.alternate(enabled, false),
                        1048 => {
                            if enabled {
                                self.save_cursor();
                            } else {
                                self.restore_cursor();
                            }
                        }
                        1049 => self.alternate(enabled, true),
                        _ => {}
                    }
                }
            }
            return;
        }
        let (top, bottom) = if self.origin {
            (self.screen.top, self.screen.bottom)
        } else {
            (0, self.rows() - 1)
        };
        // SGR and device queries do not cancel deferred autowrap.
        if !matches!(byte, b'm' | b'n' | b'c') {
            self.screen.wrap_pending = false;
        }
        match byte {
            b'A' => self.screen.y = self.screen.y.saturating_sub(n).max(top),
            b'B' | b'e' => self.screen.y = self.screen.y.saturating_add(n).min(bottom),
            b'C' | b'a' => self.screen.x = self.screen.x.saturating_add(n).min(self.cols() - 1),
            b'D' => self.screen.x = self.screen.x.saturating_sub(n),
            b'E' => {
                self.screen.x = 0;
                self.screen.y = self.screen.y.saturating_add(n).min(bottom);
            }
            b'F' => {
                self.screen.x = 0;
                self.screen.y = self.screen.y.saturating_sub(n).max(top);
            }
            b'G' | b'`' => self.screen.x = n.saturating_sub(1).min(self.cols() - 1),
            b'd' => self.screen.y = n.saturating_sub(1).saturating_add(top).min(bottom),
            b'H' | b'f' => {
                self.screen.y = n.saturating_sub(1).saturating_add(top).min(bottom);
                self.screen.x = self.param(1, 1).saturating_sub(1).min(self.cols() - 1);
            }
            b'J' => {
                let p = self.screen.y * self.cols() + self.screen.x;
                match self.params[0] {
                    0 => self.screen.cells[p..].fill(blank),
                    1 => self.screen.cells[..=p].fill(blank),
                    2 => self.screen.cells.fill(blank),
                    3 => self.screen.history.clear(),
                    _ => {}
                }
            }
            b'K' => {
                let start = self.screen.y * self.cols();
                let end = start + self.cols();
                let p = start + self.screen.x;
                match self.params[0] {
                    0 => self.screen.cells[p..end].fill(blank),
                    1 => self.screen.cells[start..=p].fill(blank),
                    2 => self.screen.cells[start..end].fill(blank),
                    _ => {}
                }
            }
            b'@' => self.screen.insert_cells(n, blank),
            b'P' => self.screen.delete_cells(n, blank),
            b'X' => {
                let p = self.screen.y * self.cols() + self.screen.x;
                let end = p + n.min(self.cols() - self.screen.x);
                self.screen.cells[p..end].fill(blank);
            }
            b'L' if (self.screen.top..=self.screen.bottom).contains(&self.screen.y) => self
                .screen
                .scroll_down(self.screen.y, self.screen.bottom, n, blank),
            b'M' if (self.screen.top..=self.screen.bottom).contains(&self.screen.y) => self
                .screen
                .scroll_up(self.screen.y, self.screen.bottom, n, blank),
            b'S' => self
                .screen
                .scroll_up(self.screen.top, self.screen.bottom, n, blank),
            b'T' => self
                .screen
                .scroll_down(self.screen.top, self.screen.bottom, n, blank),
            b'r' => {
                let first = n.saturating_sub(1);
                let last = self.param(1, self.rows()).saturating_sub(1);
                if first < last && last < self.rows() {
                    self.screen.top = first;
                    self.screen.bottom = last;
                    self.screen.x = 0;
                    self.screen.y = if self.origin { first } else { 0 };
                }
            }
            b'm' => self.style.apply(&self.params[..self.count]),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'h' | b'l' => {
                for i in 0..self.count {
                    if self.params[i] == 4 {
                        self.insert = byte == b'h';
                    }
                }
            }
            b'Z' => {
                self.screen.x = ((self.screen.x.saturating_sub(1) / 8) * 8)
                    .saturating_sub(n.saturating_sub(1).saturating_mul(8));
            }
            b'n' if self.params[0] == 5 => self.respond(b"\x1b[0n"),
            b'n' if self.params[0] == 6 => {
                self.respond(b"\x1b[");
                self.respond_number(self.screen.y.saturating_sub(top) + 1);
                self.respond(b";");
                self.respond_number(self.screen.x + 1);
                self.respond(b"R");
            }
            b'c' if self.params[0] == 0 => self.respond(b"\x1b[?1;2c"),
            _ => {}
        }
    }
}
