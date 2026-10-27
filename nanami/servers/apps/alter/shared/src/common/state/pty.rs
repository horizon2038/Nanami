//! Unix98 PTY storage and line discipline. No host terminal or kernel resources.
use alloc::{collections::VecDeque, vec::Vec};

pub const CAPACITY: usize = 32 * 1024;
const LINE_MAX: usize = 4096;
const PAIR_MAX: usize = 32;
// Alter-local terminal identity only; never an A9N notification badge/handle.
pub const TERMINAL_TAG: usize = 1 << (usize::BITS - 1);
pub const ICANON: u32 = 2;
pub const TOSTOP: u32 = 0x100;
pub const NOFLSH: u32 = 0x80;
const ECHO: u32 = 8;
const ECHOE: u32 = 0x10;
const ECHOK: u32 = 0x20;
const ECHONL: u32 = 0x40;
const ECHOCTL: u32 = 0x200;
const IEXTEN: u32 = 0x8000;
const ISIG: u32 = 1;
const ECHOKE: u32 = 0x800;
const ISTRIP: u32 = 0x20;
const INLCR: u32 = 0x40;
const IGNCR: u32 = 0x80;
const ICRNL: u32 = 0x100;
const IXON: u32 = 0x400;
const IXANY: u32 = 0x800;
const OPOST: u32 = 1;
const ONLCR: u32 = 4;
const OCRNL: u32 = 8;
const ONOCR: u32 = 0x10;
const ONLRET: u32 = 0x20;
const TABDLY: u32 = 0x1800;
const TAB3: u32 = 0x1800;
const VINTR: usize = 0;
const VQUIT: usize = 1;
const VERASE: usize = 2;
const VKILL: usize = 3;
const VEOF: usize = 4;
const VSTART: usize = 8;
const VSTOP: usize = 9;
const VSUSP: usize = 10;
const VEOL: usize = 11;
const VWERASE: usize = 14;
const VLNEXT: usize = 15;
const VEOL2: usize = 16;
const O_ACCMODE: usize = 3;
const O_NONBLOCK: usize = 0x800;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Again,
    Io,
    Full,
    Missing,
    Locked,
}

struct Description {
    id: usize,
    master: bool,
    flags: usize,
}

pub struct Pty {
    pub id: usize,
    pub namespace: usize,
    pub number: u32,
    pub locked: bool,
    pub termios: [u8; 36],
    pub winsize: [u16; 4],
    pub last_input: usize,
    pub packet: bool,
    pub packet_status: u8,
    descriptions: Vec<Description>,
    next_description: usize,
    slave_opened: bool,
    hung_up: bool,
    input: VecDeque<u8>,
    records: VecDeque<usize>,
    line: Vec<u8>,
    output: VecDeque<u8>,
    stopped: bool,
    literal: bool,
    column: usize,
}

pub struct Ptys {
    pairs: Vec<Pty>,
    next_id: usize,
}

impl Ptys {
    pub const fn new() -> Self {
        Self {
            pairs: Vec::new(),
            next_id: 1,
        }
    }

    pub fn get(&self, id: usize) -> Result<&Pty, Error> {
        self.pairs
            .iter()
            .find(|pair| pair.id == id)
            .ok_or(Error::Missing)
    }

    pub fn get_mut(&mut self, id: usize) -> Result<&mut Pty, Error> {
        self.pairs
            .iter_mut()
            .find(|pair| pair.id == id)
            .ok_or(Error::Missing)
    }

    pub fn create(&mut self, namespace: usize, flags: usize) -> Result<(usize, usize), Error> {
        if self.pairs.len() >= PAIR_MAX || self.next_id >= TERMINAL_TAG {
            return Err(Error::Full);
        }
        let mut number = 0;
        while self
            .pairs
            .iter()
            .any(|p| p.namespace == namespace && p.number == number)
        {
            number += 1;
        }
        let mut pair = Pty {
            id: self.next_id,
            namespace,
            number,
            locked: true,
            termios: default_termios(),
            winsize: [24, 80, 0, 0],
            last_input: 0,
            packet: false,
            packet_status: 0,
            descriptions: Vec::new(),
            next_description: 2,
            slave_opened: false,
            hung_up: false,
            input: VecDeque::new(),
            records: VecDeque::new(),
            line: Vec::new(),
            output: VecDeque::new(),
            stopped: false,
            literal: false,
            column: 0,
        };
        pair.input
            .try_reserve_exact(CAPACITY)
            .map_err(|_| Error::Full)?;
        pair.output
            .try_reserve_exact(CAPACITY)
            .map_err(|_| Error::Full)?;
        pair.records
            .try_reserve_exact(LINE_MAX)
            .map_err(|_| Error::Full)?;
        pair.line
            .try_reserve_exact(LINE_MAX)
            .map_err(|_| Error::Full)?;
        pair.descriptions.try_reserve(1).map_err(|_| Error::Full)?;
        pair.descriptions.push(Description {
            id: 1,
            master: true,
            flags,
        });
        self.pairs.try_reserve(1).map_err(|_| Error::Full)?;
        let id = pair.id;
        self.pairs.push(pair);
        self.next_id += 1;
        Ok((id, 1))
    }

    pub fn find(&self, namespace: usize, number: u32) -> Option<&Pty> {
        self.pairs
            .iter()
            .find(|p| p.namespace == namespace && p.number == number && p.master_open())
    }

    pub fn entries(&self, namespace: usize) -> impl Iterator<Item = &Pty> {
        self.pairs
            .iter()
            .filter(move |p| p.namespace == namespace && p.master_open())
    }

    /// Remove an open description only after the FD/SCM_RIGHTS ownership scan.
    /// Return true exactly once when the final master reference disappears.
    pub fn close(&mut self, id: usize, description: usize) -> bool {
        let Ok(pair) = self.get_mut(id) else {
            return false;
        };
        let was_master = pair.master_open();
        pair.descriptions.retain(|d| d.id != description);
        let hangup = was_master && !pair.master_open();
        if hangup {
            pair.hangup();
        } else if pair.slave_opened && !pair.slave_open() {
            pair.stopped = false;
        }
        self.pairs.retain(|p| !p.descriptions.is_empty());
        hangup
    }
}

impl Pty {
    pub fn terminal_id(&self) -> usize {
        TERMINAL_TAG | self.id
    }
    pub fn master_open(&self) -> bool {
        self.descriptions.iter().any(|d| d.master)
    }
    pub fn slave_open(&self) -> bool {
        self.descriptions.iter().any(|d| !d.master)
    }
    pub fn canonical(&self) -> bool {
        self.lflag() & ICANON != 0
    }
    pub fn lflag(&self) -> u32 {
        u32::from_le_bytes(self.termios[12..16].try_into().unwrap())
    }
    pub fn cc(&self, index: usize) -> u8 {
        self.termios[17 + index]
    }
    fn iflag(&self) -> u32 {
        u32::from_le_bytes(self.termios[..4].try_into().unwrap())
    }
    fn oflag(&self) -> u32 {
        u32::from_le_bytes(self.termios[4..8].try_into().unwrap())
    }
    fn control(&self, byte: u8, index: usize) -> bool {
        byte != 0 && byte == self.cc(index)
    }

    pub fn open_slave(&mut self, flags: usize) -> Result<usize, Error> {
        if !self.master_open() || self.hung_up {
            return Err(Error::Io);
        }
        if self.locked {
            return Err(Error::Locked);
        }
        self.descriptions.try_reserve(1).map_err(|_| Error::Full)?;
        let id = self.next_description;
        self.next_description = id.checked_add(1).ok_or(Error::Full)?;
        self.descriptions.push(Description {
            id,
            master: false,
            flags,
        });
        self.slave_opened = true;
        Ok(id)
    }

    pub fn flags(&mut self, description: usize, update: Option<usize>) -> Result<usize, Error> {
        let entry = self
            .descriptions
            .iter_mut()
            .find(|d| d.id == description)
            .ok_or(Error::Missing)?;
        if let Some(flags) = update {
            entry.flags = (entry.flags & O_ACCMODE) | (flags & O_NONBLOCK);
        }
        Ok(entry.flags)
    }

    pub fn set_termios(&mut self, value: [u8; 36], flush: bool) {
        let canonical = self.canonical();
        let old_ixon = self.iflag() & IXON != 0;
        self.termios = value;
        if canonical != self.canonical() {
            self.records.clear();
            self.input.extend(self.line.drain(..));
            if self.canonical() && !self.input.is_empty() {
                self.records.push_back(self.input.len());
            }
        }
        if old_ixon != (self.iflag() & IXON != 0) {
            self.packet_status |= if old_ixon { 16 } else { 32 };
        }
        if self.iflag() & IXON == 0 {
            self.stopped = false;
        }
        if flush {
            self.flush(0);
        }
    }

    pub fn flush(&mut self, direction: usize) {
        if direction == 0 || direction == 2 {
            self.input.clear();
            self.line.clear();
            self.records.clear();
            self.literal = false;
            self.packet_status |= 1;
        }
        if direction == 1 || direction == 2 {
            self.output.clear();
            self.packet_status |= 2;
        }
    }

    pub fn flow(&mut self, stop: bool) {
        if self.stopped != stop {
            self.stopped = stop;
            self.packet_status |= if stop { 4 } else { 8 };
        }
    }

    pub fn hangup(&mut self) {
        self.hung_up = true;
        // Canonical input not yet terminated becomes readable on hangup.
        if !self.line.is_empty() {
            let len = self.line.len();
            self.input.extend(self.line.drain(..));
            self.records.push_back(len);
        }
        self.stopped = false;
    }

    pub fn input_len(&self) -> usize {
        self.input.len()
    }
    pub fn output_len(&self) -> usize {
        self.output.len()
    }
    pub fn eof(&self) -> bool {
        self.hung_up || !self.master_open()
    }
    pub fn master_hup(&self) -> bool {
        self.hung_up || (self.slave_opened && !self.slave_open())
    }

    pub fn readable(&self, master: bool) -> bool {
        if master {
            return (self.packet && self.packet_status != 0)
                || (!self.stopped && !self.output.is_empty())
                || self.master_hup();
        }
        if self.eof() {
            return true;
        }
        if self.canonical() {
            return !self.records.is_empty();
        }
        let minimum = if self.cc(VTIME) != 0 {
            1
        } else {
            usize::from(self.cc(VMIN)).max(1)
        };
        self.input.len() >= minimum
    }

    pub fn writable(&self, master: bool) -> bool {
        if self.eof() {
            return false;
        }
        if master {
            if self.slave_opened && !self.slave_open() {
                return false;
            }
            return self.input.len() + self.line.len() < CAPACITY - 1
                && self.records.len() < LINE_MAX - 1
                && (self.lflag() & (ECHO | ECHONL) == 0 || self.output.len() <= CAPACITY - 8);
        }
        self.output.len() <= CAPACITY - 8
    }

    /// Peek/copyout/consume ensures a bad guest address never loses input.
    pub fn peek(&self, master: bool, output: &mut [u8]) -> Result<usize, Error> {
        if output.is_empty() {
            return Ok(0);
        }
        if master {
            if self.packet && self.packet_status != 0 {
                output[0] = self.packet_status;
                return Ok(1);
            }
            if self.output.is_empty() || self.stopped {
                return if self.master_hup() {
                    Err(Error::Io)
                } else {
                    Err(Error::Again)
                };
            }
            let prefix = usize::from(self.packet);
            if self.packet {
                output[0] = 0;
            }
            let count = (output.len() - prefix).min(self.output.len());
            for (to, from) in output[prefix..][..count].iter_mut().zip(&self.output) {
                *to = *from;
            }
            return Ok(prefix + count);
        }
        let count = if self.canonical() {
            match self.records.front() {
                Some(len) => *len,
                None if self.eof() => return Ok(0),
                None => return Err(Error::Again),
            }
        } else {
            self.input.len()
        };
        if count == 0 && !self.canonical() && !self.eof() {
            return Err(Error::Again);
        }
        let count = count.min(output.len());
        for (to, from) in output[..count].iter_mut().zip(&self.input) {
            *to = *from;
        }
        Ok(count)
    }

    pub fn consume(&mut self, master: bool, count: usize) {
        if master {
            if self.packet && self.packet_status != 0 {
                self.packet_status = 0;
                return;
            }
            let payload = count.saturating_sub(usize::from(self.packet));
            self.output.drain(..payload.min(self.output.len()));
        } else {
            self.input.drain(..count.min(self.input.len()));
            if self.canonical() {
                if let Some(record) = self.records.front_mut() {
                    *record = record.saturating_sub(count);
                    if *record == 0 {
                        self.records.pop_front();
                    }
                }
            }
        }
    }

    fn output_byte(&mut self, byte: u8) {
        if self.oflag() & OPOST != 0 {
            if byte == b'\r' && self.oflag() & OCRNL != 0 {
                self.output.push_back(b'\n');
                self.column = 0;
                return;
            }
            if byte == b'\r' && self.oflag() & ONOCR != 0 && self.column == 0 {
                return;
            }
            if byte == b'\n' && self.oflag() & ONLCR != 0 {
                self.output.push_back(b'\r');
                self.column = 0;
            }
            if byte == b'\t' && self.oflag() & TABDLY == TAB3 {
                let spaces = 8 - self.column % 8;
                for _ in 0..spaces {
                    self.output.push_back(b' ');
                }
                self.column += spaces;
                return;
            }
        }
        self.output.push_back(byte);
        match byte {
            b'\r' => self.column = 0,
            b'\n' if self.oflag() & ONLRET != 0 => self.column = 0,
            8 => self.column = self.column.saturating_sub(1),
            b'\t' => self.column += 8 - self.column % 8,
            32..=255 => self.column = self.column.saturating_add(1),
            _ => {}
        }
    }

    fn echo(&mut self, byte: u8) {
        if self.lflag() & ECHO == 0 {
            if byte == b'\n' && self.lflag() & ECHONL != 0 {
                self.output_byte(byte);
            }
            return;
        }
        if self.lflag() & ECHOCTL != 0
            && (byte < 32 || byte == 127)
            && byte != b'\n'
            && byte != b'\t'
        {
            self.output_byte(b'^');
            self.output_byte(byte ^ 0x40);
        } else {
            self.output_byte(byte);
        }
    }

    fn erase(&mut self) {
        let Some(byte) = self.line.pop() else {
            return;
        };
        if self.lflag() & (ECHO | ECHOE) == ECHO | ECHOE {
            let width = if self.lflag() & ECHOCTL != 0 && (byte < 32 || byte == 127) {
                2
            } else {
                1
            };
            for _ in 0..width {
                self.output.extend([8, b' ', 8]);
                self.column = self.column.saturating_sub(1);
            }
        } else {
            self.echo(self.cc(VERASE));
        }
    }

    /// Returns the accepted byte count and a bitmask of generated signals.
    /// Signal characters are consumed here, never delivered as ordinary input.
    pub fn write(&mut self, master: bool, bytes: &[u8], now: usize) -> Result<(usize, u32), Error> {
        if self.eof() || (master && self.slave_opened && !self.slave_open()) {
            return Err(Error::Io);
        }
        let mut done = 0;
        let mut signals = 0;
        for &original in bytes {
            if !master {
                if self.output.len() > CAPACITY - 8 {
                    break;
                }
                self.output_byte(original);
                done += 1;
                continue;
            }
            let mut byte = original;
            if self.iflag() & ISTRIP != 0 {
                byte &= 0x7f;
            }
            if byte == b'\r' {
                if self.iflag() & IGNCR != 0 {
                    done += 1;
                    continue;
                }
                if self.iflag() & ICRNL != 0 {
                    byte = b'\n';
                }
            } else if byte == b'\n' && self.iflag() & INLCR != 0 {
                byte = b'\r';
            }
            if !self.literal {
                // Flow and signal bytes must still work with full queues.
                if self.iflag() & IXON != 0 {
                    if self.control(byte, VSTOP) {
                        self.flow(true);
                        done += 1;
                        continue;
                    }
                    if self.control(byte, VSTART) {
                        self.flow(false);
                        done += 1;
                        continue;
                    }
                    if self.stopped && self.iflag() & IXANY != 0 {
                        self.flow(false);
                    }
                }
                if self.lflag() & ISIG != 0 {
                    let signal = if self.control(byte, VINTR) {
                        2
                    } else if self.control(byte, VQUIT) {
                        3
                    } else if self.control(byte, VSUSP) {
                        20
                    } else {
                        0
                    };
                    if signal != 0 {
                        if self.lflag() & NOFLSH == 0 {
                            self.flush(2);
                        }
                        if self.output.len() <= CAPACITY - 8 {
                            self.echo(byte);
                        }
                        done += 1;
                        signals |= 1 << signal;
                        continue;
                    }
                }
            }
            let editing = !self.literal
                && self.canonical()
                && (self.control(byte, VERASE)
                    || self.control(byte, VKILL)
                    || self.control(byte, VEOF)
                    || (self.lflag() & IEXTEN != 0
                        && (self.control(byte, VWERASE) || self.control(byte, VLNEXT))));
            if (!editing && self.input.len() + self.line.len() >= CAPACITY - 1)
                || self.records.len() >= LINE_MAX - 1
            {
                break;
            }
            let editing_line =
                editing && (self.control(byte, VKILL) || self.control(byte, VWERASE));
            let echo_space = if editing_line {
                self.line.len() * 6 + 8
            } else {
                8
            };
            if self.lflag() & (ECHO | ECHONL) != 0 && CAPACITY - self.output.len() < echo_space {
                break;
            }
            done += 1;
            if !self.literal && self.canonical() {
                if self.control(byte, VERASE) {
                    self.erase();
                    continue;
                }
                if self.control(byte, VKILL) {
                    if self.lflag() & ECHOKE != 0 {
                        while !self.line.is_empty() {
                            self.erase();
                        }
                    } else {
                        self.line.clear();
                        self.echo(byte);
                        if self.lflag() & ECHOK != 0 {
                            self.output_byte(b'\n');
                        }
                    }
                    continue;
                }
                if self.lflag() & IEXTEN != 0 {
                    if self.control(byte, VLNEXT) {
                        self.literal = true;
                        continue;
                    }
                    if self.control(byte, VWERASE) {
                        while self.line.last() == Some(&b' ') {
                            self.erase();
                        }
                        while self.line.last().is_some_and(|b| *b != b' ') {
                            self.erase();
                        }
                        continue;
                    }
                }
                if self.control(byte, VEOF) {
                    self.records.push_back(self.line.len());
                    self.input.extend(self.line.drain(..));
                    continue;
                }
            }
            let quoted = core::mem::take(&mut self.literal);
            self.echo(byte);
            if self.canonical() {
                let delimiter = !quoted
                    && (byte == b'\n'
                        || self.control(byte, VEOL)
                        || (self.lflag() & IEXTEN != 0 && self.control(byte, VEOL2)));
                if self.line.len() < LINE_MAX - 1 || delimiter {
                    self.line.push(byte);
                }
                if delimiter {
                    self.records.push_back(self.line.len());
                    self.input.extend(self.line.drain(..));
                }
            } else {
                self.input.push_back(byte);
            }
            self.last_input = now;
        }
        if done == 0 && !bytes.is_empty() {
            return Err(Error::Again);
        }
        Ok((done, signals))
    }
}

pub fn default_termios() -> [u8; 36] {
    let mut result = [0; 36];
    result[..4].copy_from_slice(&0x500u32.to_le_bytes());
    result[4..8].copy_from_slice(&5u32.to_le_bytes());
    result[8..12].copy_from_slice(&0xbfu32.to_le_bytes());
    result[12..16].copy_from_slice(&0x8a3bu32.to_le_bytes());
    result[17..34].copy_from_slice(&[3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 0, 18, 15, 23, 22, 0]);
    result
}
