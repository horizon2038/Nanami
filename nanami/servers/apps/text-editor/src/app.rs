use crate::{
    document::Document,
    input::Key,
    layout::{Layout, CELL, EDIT_TOP, LINE, TOOLBAR},
};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

pub trait Files {
    fn read(&mut self, path: &str) -> Result<String, String>;
    fn exists(&mut self, path: &str) -> Result<bool, String>;
    fn write(&mut self, path: &str, text: &str) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    New,
    Open,
    Close,
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

    fn key(&mut self, key: Key) {
        match key {
            Key::SelectAll => {
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

pub enum Mode {
    Edit,
    Path { save: bool, field: Field },
    Find(Field),
    Confirm(Pending),
    Overwrite(String),
}

pub struct Editor {
    pub document: Document,
    pub path: Option<String>,
    pub mode: Mode,
    pub top: usize,
    pub left: usize,
    pub status: String,
    pub error: bool,
    pub closed: bool,
    pub layout: Layout,
    clipboard: String,
    search: String,
    pending: Option<Pending>,
    dragging: bool,
}

impl Editor {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            document: Document::new("").unwrap(),
            path: None,
            mode: Mode::Edit,
            top: 0,
            left: 0,
            status: "Ctrl+O Open   Ctrl+S Save   Ctrl+F Find".to_string(),
            error: false,
            closed: false,
            layout: Layout::new(width, height, 1),
            clipboard: String::new(),
            search: String::new(),
            pending: None,
            dragging: false,
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.layout = Layout::new(width, height, self.document.line_count());
        self.reveal();
    }

    pub fn message(&mut self, text: String, error: bool) {
        self.status = text;
        self.error = error;
    }

    pub fn open(&mut self, path: &str, files: &mut impl Files) -> bool {
        let loaded = normalize_path(path).and_then(|path| {
            let text = files.read(&path)?;
            let document = Document::new(&text).map_err(ToString::to_string)?;
            Ok((path, document))
        });
        match loaded {
            Ok((path, document)) => {
                self.document = document;
                self.path = Some(path);
                self.top = 0;
                self.left = 0;
                self.message("Opened".to_string(), false);
                true
            }
            Err(error) => {
                self.message(error, true);
                false
            }
        }
    }

    pub fn request(&mut self, pending: Pending) {
        self.dragging = false;
        if self.document.dirty() {
            self.mode = Mode::Confirm(pending);
        } else {
            self.proceed(pending);
        }
    }

    fn proceed(&mut self, pending: Pending) {
        self.pending = None;
        match pending {
            Pending::Close => {
                self.closed = true;
            }
            Pending::New => {
                self.document = Document::new("").unwrap();
                self.path = None;
                self.top = 0;
                self.left = 0;
                self.mode = Mode::Edit;
                self.message("New document".to_string(), false);
            }
            Pending::Open => {
                self.mode = Mode::Path {
                    save: false,
                    field: Field::new(self.path.as_deref().unwrap_or("/")),
                };
            }
        }
    }

    fn save_as(&mut self) {
        self.mode = Mode::Path {
            save: true,
            field: Field::new(self.path.as_deref().unwrap_or("/")),
        };
    }

    fn save(&mut self, files: &mut impl Files) {
        if let Some(path) = self.path.clone() {
            self.write(&path, files);
        } else {
            self.save_as();
        }
    }

    fn write(&mut self, path: &str, files: &mut impl Files) {
        match files.write(path, &self.document.text()) {
            Ok(()) => {
                self.path = Some(path.to_string());
                self.document.mark_saved();
                self.mode = Mode::Edit;
                self.message("Saved".to_string(), false);
                if let Some(pending) = self.pending.take() {
                    self.proceed(pending);
                }
            }
            Err(error) => {
                // A failed Save-and-close must not make a later ordinary Save
                // unexpectedly close the window (or replace the document).
                self.pending = None;
                self.message(error, true);
            }
        }
    }

    pub fn confirm(&mut self, choice: usize, files: &mut impl Files) {
        let mode = core::mem::replace(&mut self.mode, Mode::Edit);
        match mode {
            Mode::Confirm(pending) => match choice {
                0 => {
                    self.pending = Some(pending);
                    self.save(files);
                }
                1 => {
                    self.proceed(pending);
                }
                _ => {
                    self.pending = None;
                }
            },
            Mode::Overwrite(path) if choice == 0 => {
                self.write(&path, files);
            }
            _ => {
                self.pending = None;
            }
        }
    }

    pub fn action(&mut self, index: usize, files: &mut impl Files) {
        let keys = [
            Key::New,
            Key::Open,
            Key::Save,
            Key::SaveAs,
            Key::Undo,
            Key::Redo,
            Key::Find,
        ];
        if let Some(key) = keys.get(index) {
            self.key(*key, false, files);
        }
    }

    pub fn key(&mut self, key: Key, extend: bool, files: &mut impl Files) {
        if matches!(self.mode, Mode::Confirm(_) | Mode::Overwrite(_)) {
            match key {
                Key::Enter | Key::Character('s') => {
                    self.confirm(0, files);
                }
                Key::Character('d') if matches!(self.mode, Mode::Confirm(_)) => {
                    self.confirm(1, files);
                }
                Key::Escape | Key::Character('c') => {
                    self.confirm(2, files);
                }
                _ => {}
            }
            self.reveal();
            return;
        }
        let mode = core::mem::replace(&mut self.mode, Mode::Edit);
        match mode {
            Mode::Path { save, mut field } => match key {
                Key::Escape => {
                    self.pending = None;
                }
                Key::Enter => match normalize_path(&field.text) {
                    Ok(path) if save => match files.exists(&path) {
                        Ok(true) => {
                            self.mode = Mode::Overwrite(path);
                        }
                        Ok(false) => {
                            self.write(&path, files);
                        }
                        Err(error) => {
                            self.message(error, true);
                            self.mode = Mode::Path { save, field };
                        }
                    },
                    Ok(path) => {
                        if !self.open(&path, files) {
                            self.mode = Mode::Path { save, field };
                        }
                    }
                    Err(error) => {
                        self.message(error, true);
                        self.mode = Mode::Path { save, field };
                    }
                },
                _ => {
                    field.key(key);
                    self.mode = Mode::Path { save, field };
                }
            },
            Mode::Find(mut field) => match key {
                Key::Escape => {}
                Key::Enter | Key::FindNext | Key::FindPrevious => {
                    self.search = field.text.clone();
                    self.find(key == Key::FindPrevious);
                    self.mode = Mode::Find(field);
                }
                _ => {
                    field.key(key);
                    self.mode = Mode::Find(field);
                }
            },
            _ => {
                self.edit_key(key, extend, files);
            }
        }
        self.reveal();
    }

    fn edit_key(&mut self, key: Key, extend: bool, files: &mut impl Files) {
        let result = match key {
            Key::Character(c) => {
                let mut bytes = [0; 4];
                self.document.insert(c.encode_utf8(&mut bytes))
            }
            Key::Tab => self.document.insert("\t"),
            Key::Enter => self
                .document
                .insert(if self.document.crlf { "\r\n" } else { "\n" }),
            Key::Backspace => self.document.delete(false),
            Key::Delete => self.document.delete(true),
            Key::Paste => self.document.insert(&self.clipboard),
            Key::Cut => {
                self.clipboard = self.document.selected_text();
                self.document.insert("")
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.message(error.to_string(), true);
        }
        match key {
            Key::Left => {
                self.document.horizontal(false, extend);
            }
            Key::Right => {
                self.document.horizontal(true, extend);
            }
            Key::Up => {
                self.document.vertical(-1, extend);
            }
            Key::Down => {
                self.document.vertical(1, extend);
            }
            Key::PageUp => {
                self.document
                    .vertical(-(self.layout.rows.max(1) as isize), extend);
            }
            Key::PageDown => {
                self.document
                    .vertical(self.layout.rows.max(1) as isize, extend);
            }
            Key::Home => {
                self.document.select(
                    self.document
                        .line_start(self.document.line_for(self.document.cursor)),
                    extend,
                );
            }
            Key::End => {
                self.document.select(
                    self.document
                        .line_end(self.document.line_for(self.document.cursor)),
                    extend,
                );
            }
            Key::DocumentHome => {
                self.document.select(0, extend);
            }
            Key::DocumentEnd => {
                self.document.select(self.document.len(), extend);
            }
            Key::SelectAll => {
                self.document.select(0, false);
                self.document.select(self.document.len(), true);
            }
            Key::Copy => {
                self.clipboard = self.document.selected_text();
                self.message("Copied (editor clipboard)".to_string(), false);
            }
            Key::Undo => {
                self.document.undo();
            }
            Key::Redo => {
                self.document.redo();
            }
            Key::New => {
                self.request(Pending::New);
            }
            Key::Open => {
                self.request(Pending::Open);
            }
            Key::Close => {
                self.request(Pending::Close);
            }
            Key::Save => {
                self.save(files);
            }
            Key::SaveAs => {
                self.save_as();
            }
            Key::Find => {
                self.mode = Mode::Find(Field::new(&self.search));
            }
            Key::FindNext => {
                self.find(false);
            }
            Key::FindPrevious => {
                self.find(true);
            }
            _ => {}
        }
    }

    fn find(&mut self, previous: bool) {
        if self.search.is_empty() {
            self.mode = Mode::Find(Field::new(""));
            return;
        }
        let text = self.document.text();
        let (start, end) = self.document.selection();
        let found = if previous {
            text[..start]
                .rfind(&self.search)
                .or_else(|| text.rfind(&self.search))
        } else {
            text[end..]
                .find(&self.search)
                .map(|at| end + at)
                .or_else(|| text.find(&self.search))
        };
        if let Some(at) = found {
            self.document.select(at, false);
            self.document.select(at + self.search.len(), true);
            self.message(
                "Match found; Enter / F3 next, Esc return".to_string(),
                false,
            );
        } else {
            self.message("No matches".to_string(), false);
        }
    }

    pub fn reveal(&mut self) {
        self.layout = Layout::new(
            self.layout.width,
            self.layout.height,
            self.document.line_count(),
        );
        let line = self.document.line_for(self.document.cursor);
        let column = self.document.column(self.document.cursor);
        if line < self.top {
            self.top = line;
        }
        if line >= self.top + self.layout.rows.max(1) {
            self.top = line + 1 - self.layout.rows.max(1);
        }
        if column < self.left {
            self.left = column;
        }
        if column >= self.left + self.layout.columns.max(1) {
            self.left = column + 1 - self.layout.columns.max(1);
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        self.top = self.top.min(
            self.document
                .line_count()
                .saturating_sub(self.layout.rows.max(1)),
        );
    }

    pub fn wheel(&mut self, delta: i16) {
        if matches!(self.mode, Mode::Edit | Mode::Find(_)) {
            self.top = self.top.saturating_add_signed(-(delta as isize) * 3);
            self.clamp_scroll();
        }
    }

    pub fn pointer_move(&mut self, x: usize, y: usize) {
        if self.dragging {
            self.place_cursor(x, y, true);
        }
    }

    fn place_cursor(&mut self, x: usize, y: usize, extend: bool) {
        let line =
            (self.top + y.saturating_sub(EDIT_TOP) / LINE).min(self.document.line_count() - 1);
        let column = self.left + (x.saturating_sub(self.layout.text_x) + CELL / 2) / CELL;
        self.document
            .select(self.document.at_column(line, column), extend);
    }

    pub fn pointer_button(
        &mut self,
        x: usize,
        y: usize,
        pressed: bool,
        shift: bool,
        files: &mut impl Files,
    ) {
        if !pressed {
            self.dragging = false;
            return;
        }
        if matches!(self.mode, Mode::Confirm(_) | Mode::Overwrite(_)) {
            let (left, top, width) = self.dialog_bounds();
            if y >= top + 82 && y < top + 114 && x >= left && x < left + width {
                let choices = if matches!(self.mode, Mode::Confirm(_)) {
                    3
                } else {
                    2
                };
                self.confirm(((x - left) * choices / width).min(choices - 1), files);
            }
            return;
        }
        if matches!(self.mode, Mode::Path { .. } | Mode::Find(_)) && y >= TOOLBAR && y < EDIT_TOP {
            if x >= self.layout.width.saturating_sub(90) {
                self.key(Key::Enter, false, files);
            }
            return;
        }
        if !matches!(self.mode, Mode::Edit) {
            return;
        }
        if let Some(index) = self.layout.toolbar_button(x, y) {
            self.action(index, files);
        } else if y >= EDIT_TOP && y < self.layout.bottom {
            if x >= self.layout.width.saturating_sub(crate::layout::SCROLLBAR) {
                let track = self.layout.bottom.saturating_sub(EDIT_TOP).max(1);
                let max_top = self
                    .document
                    .line_count()
                    .saturating_sub(self.layout.rows.max(1));
                self.top = (y - EDIT_TOP).saturating_mul(max_top) / track;
            } else {
                self.place_cursor(x, y, shift);
                self.dragging = true;
            }
        }
    }

    pub fn dialog_bounds(&self) -> (usize, usize, usize) {
        let width = self.layout.width.saturating_sub(24).min(492);
        (
            (self.layout.width - width) / 2,
            self.layout.height.saturating_sub(136) / 2,
            width,
        )
    }

    pub fn title(&self, limit: usize) -> String {
        let path = self.path.as_deref().unwrap_or("Untitled");
        let name = path.rsplit('/').next().unwrap_or(path);
        let dirty = if self.document.dirty() { "* " } else { "" };
        // Keep enough room for Honoka's bounded window title and a UTF-8 boundary.
        let mut title = format!("{}{}", dirty, name);
        if self.path.is_none() {
            title.push_str(" - Text editor");
        }
        let mut end = title.len().min(limit);
        while !title.is_char_boundary(end) {
            end -= 1;
        }
        title.truncate(end);
        title
    }
}

pub fn normalize_path(input: &str) -> Result<String, String> {
    if input.is_empty() || input.len() > 255 || input.contains('\0') || input.ends_with('/') {
        return Err("Enter a file path, e.g. /alter/linux/tmp/note.txt".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => {
                parts.push(part);
            }
        }
    }
    if parts.is_empty() {
        return Err("A file name is required".to_string());
    }
    let path = format!("/{}", parts.join("/"));
    if path.len() > 255 {
        return Err("File path exceeds 255 bytes".to_string());
    }
    Ok(path)
}
