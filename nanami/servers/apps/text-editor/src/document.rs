//! A UTF-8 gap buffer. Cursor offsets are bytes; screen columns expand tabs.
//! The line index and gap are updated locally, not rebuilt on each keystroke.
use alloc::{collections::VecDeque, string::String, vec, vec::Vec};

pub const MAX_BYTES: usize = 256 * 1024;
const HISTORY_BYTES: usize = 512 * 1024;
const HISTORY_STEPS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    cursor: usize,
    anchor: usize,
}

struct Edit {
    at: usize,
    removed: Vec<u8>,
    inserted: Vec<u8>,
    before: Position,
    after: Position,
    before_revision: u64,
    after_revision: u64,
}

impl Edit {
    fn cost(&self) -> usize {
        self.removed.len() + self.inserted.len()
    }
}

pub struct Document {
    bytes: Vec<u8>,
    gap_start: usize,
    gap_end: usize,
    lines: Vec<u32>,
    pub cursor: usize,
    pub anchor: usize,
    pub crlf: bool,
    preferred_column: Option<usize>,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    history_bytes: usize,
    revision: u64,
    saved_revision: u64,
    next_revision: u64,
}

impl Document {
    pub fn new(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_BYTES {
            return Err("File exceeds the 256 KiB editor limit");
        }
        if text.contains('\0') {
            return Err("Binary files cannot be edited");
        }
        let mut lines = vec![0];
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                lines.push((i + 1) as u32);
            }
        }
        let mut bytes = Vec::with_capacity(text.len() + 1024);
        bytes.extend_from_slice(text.as_bytes());
        bytes.resize(text.len() + 1024, 0);
        Ok(Self {
            gap_start: text.len(),
            gap_end: bytes.len(),
            bytes,
            lines,
            cursor: 0,
            anchor: 0,
            crlf: text.contains("\r\n"),
            preferred_column: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            history_bytes: 0,
            revision: 0,
            saved_revision: 0,
            next_revision: 1,
        })
    }

    pub fn len(&self) -> usize {
        self.bytes.len() - (self.gap_end - self.gap_start)
    }

    pub fn dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    pub fn byte(&self, index: usize) -> u8 {
        if index < self.gap_start {
            self.bytes[index]
        } else {
            self.bytes[index + self.gap_end - self.gap_start]
        }
    }

    pub fn text(&self) -> String {
        let mut text = Vec::with_capacity(self.len());
        text.extend_from_slice(&self.bytes[..self.gap_start]);
        text.extend_from_slice(&self.bytes[self.gap_end..]);
        // Only validated UTF-8 is inserted and every edit is on a character boundary.
        String::from_utf8(text).expect("UTF-8 document invariant")
    }

    pub fn character(&self, at: usize) -> char {
        let mut bytes = [0; 4];
        let count = self.next(at) - at;
        for (offset, byte) in bytes.iter_mut().enumerate().take(count.min(4)) {
            *byte = self.byte(at + offset);
        }
        core::str::from_utf8(&bytes[..count.min(4)])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('\u{fffd}')
    }

    pub fn next(&self, at: usize) -> usize {
        if at >= self.len() {
            return self.len();
        }
        if self.byte(at) == b'\r' && at + 1 < self.len() && self.byte(at + 1) == b'\n' {
            return at + 2;
        }
        let mut next = at + 1;
        while next < self.len() && self.byte(next) & 0xc0 == 0x80 {
            next += 1;
        }
        next
    }

    pub fn previous(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        if at >= 2 && self.byte(at - 1) == b'\n' && self.byte(at - 2) == b'\r' {
            return at - 2;
        }
        let mut previous = at - 1;
        while previous > 0 && self.byte(previous) & 0xc0 == 0x80 {
            previous -= 1;
        }
        previous
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_for(&self, at: usize) -> usize {
        self.lines
            .partition_point(|&start| start as usize <= at)
            .saturating_sub(1)
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.lines
            .get(line)
            .copied()
            .map(|n| n as usize)
            .unwrap_or(self.len())
    }

    pub fn line_end(&self, line: usize) -> usize {
        if line + 1 >= self.line_count() {
            return self.len();
        }
        let end = self.line_start(line + 1) - 1;
        if end > self.line_start(line) && self.byte(end - 1) == b'\r' {
            return end - 1;
        }
        end
    }

    pub fn column(&self, at: usize) -> usize {
        let mut position = self.line_start(self.line_for(at));
        let mut column = 0;
        while position < at {
            column = advance_column(column, self.character(position));
            position = self.next(position);
        }
        column
    }

    pub fn at_column(&self, line: usize, target: usize) -> usize {
        let mut position = self.line_start(line);
        let mut column = 0;
        while position < self.line_end(line) {
            let next_column = advance_column(column, self.character(position));
            if target < next_column {
                if target - column >= (next_column - column + 1) / 2 {
                    position = self.next(position);
                }
                break;
            }
            position = self.next(position);
            column = next_column;
        }
        position
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    fn boundary(&self, at: usize) -> bool {
        at <= self.len() && (at == self.len() || self.byte(at) & 0xc0 != 0x80)
    }

    pub fn select(&mut self, at: usize, extend: bool) {
        debug_assert!(self.boundary(at));
        self.cursor = at.min(self.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.preferred_column = None;
    }

    pub fn horizontal(&mut self, right: bool, extend: bool) {
        let (start, end) = self.selection();
        let at = match (extend, start != end, right) {
            (false, true, true) => end,
            (false, true, false) => start,
            (_, _, true) => self.next(self.cursor),
            (_, _, false) => self.previous(self.cursor),
        };
        self.select(at, extend);
    }

    pub fn vertical(&mut self, delta: isize, extend: bool) {
        let column = self
            .preferred_column
            .unwrap_or_else(|| self.column(self.cursor));
        let line = self
            .line_for(self.cursor)
            .saturating_add_signed(delta)
            .min(self.line_count() - 1);
        self.select(self.at_column(line, column), extend);
        self.preferred_column = Some(column);
    }

    pub fn selected_text(&self) -> String {
        let (start, end) = self.selection();
        let bytes: Vec<u8> = (start..end).map(|i| self.byte(i)).collect();
        String::from_utf8(bytes).expect("UTF-8 selection invariant")
    }

    pub fn insert(&mut self, text: &str) -> Result<(), &'static str> {
        if text.contains('\0') {
            return Err("NUL is not allowed in a text document");
        }
        let (start, end) = self.selection();
        if self.len() - (end - start) + text.len() > MAX_BYTES {
            return Err("Document exceeds the 256 KiB editor limit");
        }
        if start == end && text.is_empty() {
            return Ok(());
        }
        let before = Position {
            cursor: self.cursor,
            anchor: self.anchor,
        };
        let removed = (start..end).map(|i| self.byte(i)).collect();
        let after = Position {
            cursor: start + text.len(),
            anchor: start + text.len(),
        };
        let edit = Edit {
            at: start,
            removed,
            inserted: text.as_bytes().to_vec(),
            before,
            after,
            before_revision: self.revision,
            after_revision: self.next_revision,
        };
        for discarded in self.redo.drain(..) {
            self.history_bytes -= discarded.cost();
        }
        self.replace_raw(start, end, text.as_bytes());
        self.cursor = after.cursor;
        self.anchor = after.anchor;
        self.preferred_column = None;
        self.revision = self.next_revision;
        self.next_revision += 1;
        self.history_bytes += edit.cost();
        self.undo.push_back(edit);
        while self.history_bytes > HISTORY_BYTES || self.undo.len() > HISTORY_STEPS {
            if let Some(old) = self.undo.pop_front() {
                self.history_bytes -= old.cost();
            } else {
                break;
            }
        }
        Ok(())
    }

    pub fn delete(&mut self, forward: bool) -> Result<(), &'static str> {
        if self.cursor == self.anchor {
            self.anchor = if forward {
                self.next(self.cursor)
            } else {
                self.previous(self.cursor)
            };
        }
        self.insert("")
    }

    fn replace_raw(&mut self, start: usize, end: usize, inserted: &[u8]) {
        debug_assert!(self.boundary(start) && self.boundary(end));
        let first = self.line_for(start) + 1;
        let last = self.line_for(end) + 1;
        self.lines.drain(first..last);
        let delta = inserted.len() as isize - (end - start) as isize;
        for position in &mut self.lines[first..] {
            *position = (*position as isize + delta) as u32;
        }
        self.lines.splice(
            first..first,
            inserted.iter().enumerate().filter_map(|(i, &byte)| {
                if byte == b'\n' {
                    Some((start + i + 1) as u32)
                } else {
                    None
                }
            }),
        );

        if start < self.gap_start {
            let count = self.gap_start - start;
            self.bytes
                .copy_within(start..self.gap_start, self.gap_end - count);
            self.gap_start = start;
            self.gap_end -= count;
        } else if start > self.gap_start {
            let count = start - self.gap_start;
            self.bytes
                .copy_within(self.gap_end..self.gap_end + count, self.gap_start);
            self.gap_start += count;
            self.gap_end += count;
        }
        self.gap_end += end - start;
        if self.gap_end - self.gap_start < inserted.len() {
            let extra = inserted.len() - (self.gap_end - self.gap_start) + 1024;
            let old_len = self.bytes.len();
            self.bytes.resize(old_len + extra, 0);
            self.bytes
                .copy_within(self.gap_end..old_len, self.gap_end + extra);
            self.gap_end += extra;
        }
        self.bytes[self.gap_start..self.gap_start + inserted.len()].copy_from_slice(inserted);
        self.gap_start += inserted.len();
    }

    pub fn undo(&mut self) {
        if let Some(edit) = self.undo.pop_back() {
            self.replace_raw(edit.at, edit.at + edit.inserted.len(), &edit.removed);
            self.cursor = edit.before.cursor;
            self.anchor = edit.before.anchor;
            self.revision = edit.before_revision;
            self.preferred_column = None;
            self.redo.push(edit);
        }
    }

    pub fn redo(&mut self) {
        if let Some(edit) = self.redo.pop() {
            self.replace_raw(edit.at, edit.at + edit.removed.len(), &edit.inserted);
            self.cursor = edit.after.cursor;
            self.anchor = edit.after.anchor;
            self.revision = edit.after_revision;
            self.preferred_column = None;
            self.undo.push_back(edit);
        }
    }
}

pub fn advance_column(column: usize, character: char) -> usize {
    if character == '\t' {
        (column / 4 + 1) * 4
    } else {
        column + 1
    }
}
