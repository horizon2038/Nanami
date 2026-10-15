use crate::{
    bitmap::Image,
    document::{self, Document, Tool},
};
use alloc::{
    format,
    string::{String, ToString},
};
use nanami_ui::{Field, Files, Key, Rect};

pub const TOOLS_X: i32 = 8;
pub const TOOLS_Y: i32 = 110;
pub const CANVAS_X: i32 = 120;
pub const CANVAS_Y: i32 = 108;
pub const PALETTE: [u32; 24] = [
    0x000000, 0x777777, 0xbbbbbb, 0xffffff, 0x7e2424, 0xe04435, 0xe88a2d, 0xe4c441, 0x517a36,
    0x429859, 0x276a87, 0x377cc7, 0x333d85, 0x76538e, 0x9f456b, 0x85583a, 0x442c22, 0xb58b6b,
    0xe3c6a8, 0xf4e5bd, 0xa8b486, 0x86adb2, 0xaabdd3, 0xc8b5cb,
];
pub const FILE_BUTTONS: [(&str, usize); 11] = [
    ("New", 54),
    ("Open...", 84),
    ("Save", 62),
    ("Save as...", 110),
    ("Undo", 62),
    ("Redo", 62),
    ("Resize...", 100),
    ("Clear", 72),
    ("Crop", 62),
    ("Rotate", 82),
    ("Flip H", 80),
];
#[derive(Clone, Copy)]
pub enum Pending {
    New,
    Open,
    Close,
}
pub enum Dialog {
    None,
    Path { save: bool, field: Field },
    Confirm(Pending),
    Overwrite(String),
    Size { new: bool, field: Field },
    Color(Field),
}

pub struct Paint {
    pub document: Document,
    pub path: Option<String>,
    pub tool: Tool,
    pub size: usize,
    pub filled: bool,
    pub foreground: u32,
    pub background: u32,
    pub selection: Option<Rect>,
    pub dialog: Dialog,
    pub status: String,
    pub error: bool,
    pub closed: bool,
    pub width: usize,
    pub height: usize,
    pub zoom: usize,
    pub pan: (usize, usize),
    pending: Option<Pending>,
    clipboard: Option<Image>,
    drag: Option<Drag>,
}
struct Drag {
    start: (i32, i32),
    last: (i32, i32),
    color: u32,
    tool: Tool,
}
impl Paint {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            document: Document::new(Image::new(640, 400, 0xffffff).unwrap()),
            path: None,
            tool: Tool::Brush,
            size: 4,
            filled: false,
            foreground: 0x000000,
            background: 0xffffff,
            selection: None,
            dialog: Dialog::None,
            status: "Left: foreground   Right: background".into(),
            error: false,
            closed: false,
            width,
            height,
            zoom: 100,
            pan: (0, 0),
            pending: None,
            clipboard: None,
            drag: None,
        }
    }
    pub fn message(&mut self, message: String, error: bool) {
        self.status = message;
        self.error = error;
    }
    pub fn viewport(&self) -> Rect {
        Rect::new(
            CANVAS_X,
            CANVAS_Y,
            self.width.saturating_sub(CANVAS_X as usize + 12),
            self.height.saturating_sub(CANVAS_Y as usize + 36),
        )
    }
    pub fn origin(&self) -> (i32, i32) {
        let view = self.viewport();
        let w = self.document.image.width * self.zoom / 100;
        let h = self.document.image.height * self.zoom / 100;
        (
            view.x + view.w.saturating_sub(w) as i32 / 2,
            view.y + view.h.saturating_sub(h) as i32 / 2,
        )
    }
    pub fn image_point(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        let origin = self.origin();
        if !self.viewport().contains(x, y) || x < origin.0 || y < origin.1 {
            return None;
        }
        let px = (x - origin.0) as usize * 100 / self.zoom + self.pan.0;
        let py = (y - origin.1) as usize * 100 / self.zoom + self.pan.1;
        if px >= self.document.image.width || py >= self.document.image.height {
            return None;
        }
        Some((px as i32, py as i32))
    }
    pub fn resize_window(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.clamp_pan();
    }
    pub fn clamp_pan(&mut self) {
        let view = self.viewport();
        self.pan.0 = self.pan.0.min(
            self.document
                .image
                .width
                .saturating_sub((view.w * 100 / self.zoom).max(1)),
        );
        self.pan.1 = self.pan.1.min(
            self.document
                .image
                .height
                .saturating_sub((view.h * 100 / self.zoom).max(1)),
        );
    }
    pub fn change_zoom(&mut self, direction: i32) {
        self.finish_drag();
        let levels = [25, 50, 100, 200, 400, 800];
        let i = levels.iter().position(|&z| z == self.zoom).unwrap_or(2);
        let next = i
            .saturating_add_signed(direction as isize)
            .min(levels.len() - 1);
        let view = self.viewport();
        let center = (
            self.pan.0 + view.w * 50 / self.zoom,
            self.pan.1 + view.h * 50 / self.zoom,
        );
        self.zoom = levels[next];
        self.pan = (
            center.0.saturating_sub(view.w * 50 / self.zoom),
            center.1.saturating_sub(view.h * 50 / self.zoom),
        );
        self.clamp_pan();
    }
    pub fn wheel(&mut self, delta: i16, control: bool, shift: bool) {
        if !matches!(self.dialog, Dialog::None) {
            return;
        }
        if control {
            self.change_zoom(delta.signum() as i32);
            return;
        }
        let amount = -(delta as isize) * (3200 / self.zoom).max(1) as isize;
        if shift {
            self.pan.0 = self.pan.0.saturating_add_signed(amount);
        } else {
            self.pan.1 = self.pan.1.saturating_add_signed(amount);
        }
        self.clamp_pan();
    }
    pub fn open(&mut self, path: &str, files: &mut impl Files) -> bool {
        let opened = nanami_ui::normalize_path(path)
            .map_err(ToString::to_string)
            .and_then(|path| {
                let bytes = files.read(&path)?;
                let image = Image::decode(&bytes).map_err(ToString::to_string)?;
                Ok((path, image))
            });
        match opened {
            Ok((path, image)) => {
                self.document = Document::new(image);
                self.path = Some(path);
                self.selection = None;
                self.pan = (0, 0);
                self.zoom = 100;
                self.message("Opened BMP".into(), false);
                true
            }
            Err(e) => {
                self.message(e, true);
                false
            }
        }
    }
    pub fn request(&mut self, pending: Pending) {
        self.finish_drag();
        if self.document.dirty() {
            self.dialog = Dialog::Confirm(pending);
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
                self.dialog = Dialog::Size {
                    new: true,
                    field: Field::new("640 x 400"),
                };
            }
            Pending::Open => {
                self.dialog = Dialog::Path {
                    save: false,
                    field: Field::new(self.path.as_deref().unwrap_or("/")),
                };
            }
        }
    }
    fn save_as(&mut self) {
        self.dialog = Dialog::Path {
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
        match files.write(path, &self.document.image.encode()) {
            Ok(()) => {
                self.document.mark_saved();
                self.path = Some(path.into());
                self.dialog = Dialog::None;
                self.message("Saved BMP".into(), false);
                if let Some(p) = self.pending.take() {
                    self.proceed(p);
                }
            }
            Err(error) => {
                self.pending = None;
                self.message(error, true);
            }
        }
    }
    pub fn confirm(&mut self, choice: usize, files: &mut impl Files) {
        let dialog = core::mem::replace(&mut self.dialog, Dialog::None);
        match dialog {
            Dialog::Confirm(p) => match choice {
                0 => {
                    self.pending = Some(p);
                    self.save(files);
                }
                1 => {
                    self.proceed(p);
                }
                _ => {
                    self.pending = None;
                }
            },
            Dialog::Overwrite(path) if choice == 0 => {
                self.write(&path, files);
            }
            _ => {
                self.pending = None;
            }
        }
    }
    pub fn key(&mut self, key: Key, shift: bool, files: &mut impl Files) {
        if matches!(self.dialog, Dialog::Confirm(_) | Dialog::Overwrite(_)) {
            match key {
                Key::Enter | Key::Character('s' | 'S') => {
                    self.confirm(0, files);
                }
                Key::Character('d' | 'D') if matches!(self.dialog, Dialog::Confirm(_)) => {
                    self.confirm(1, files);
                }
                Key::Escape => {
                    self.confirm(2, files);
                }
                _ => {}
            }
            return;
        }
        let dialog = core::mem::replace(&mut self.dialog, Dialog::None);
        match dialog {
            Dialog::Path { save, mut field } => {
                if key == Key::Escape {
                    self.pending = None;
                    return;
                }
                if key != Key::Enter {
                    field.key(key);
                    self.dialog = Dialog::Path { save, field };
                    return;
                }
                match nanami_ui::normalize_path(&field.text) {
                    Ok(path) if save => {
                        if !path.to_ascii_lowercase().ends_with(".bmp") {
                            self.message("Use a .bmp file name".into(), true);
                            self.dialog = Dialog::Path { save, field };
                            return;
                        }
                        match files.exists(&path) {
                            Ok(true) => {
                                self.dialog = Dialog::Overwrite(path);
                            }
                            Ok(false) => {
                                self.write(&path, files);
                            }
                            Err(e) => {
                                self.message(e, true);
                                self.dialog = Dialog::Path { save, field };
                            }
                        }
                    }
                    Ok(path) => {
                        if !self.open(&path, files) {
                            self.dialog = Dialog::Path { save, field };
                        }
                    }
                    Err(e) => {
                        self.message(e.into(), true);
                        self.dialog = Dialog::Path { save, field };
                    }
                }
            }
            Dialog::Size { new, mut field } => {
                if key == Key::Escape {
                    return;
                }
                if key != Key::Enter {
                    field.key(key);
                    self.dialog = Dialog::Size { new, field };
                    return;
                }
                match dimensions(&field.text) {
                    Ok((w, h)) => {
                        if new {
                            self.document =
                                Document::new(Image::new(w, h, self.background).unwrap());
                            self.path = None;
                        } else {
                            let _ = self.document.resize(w, h, self.background);
                        }
                        self.selection = None;
                        self.pan = (0, 0);
                        self.clamp_pan();
                        self.message("Canvas size changed".into(), false);
                    }
                    Err(error) => {
                        self.message(error.into(), true);
                        self.dialog = Dialog::Size { new, field };
                    }
                }
            }
            Dialog::Color(mut field) => {
                if key == Key::Escape {
                    return;
                }
                if key != Key::Enter {
                    field.key(key);
                    self.dialog = Dialog::Color(field);
                    return;
                }
                let text = field.text.strip_prefix('#').unwrap_or(&field.text);
                match u32::from_str_radix(text, 16) {
                    Ok(color) if text.len() == 6 && text.bytes().all(|c| c.is_ascii_hexdigit()) => {
                        self.foreground = color;
                        self.message("Foreground color set".into(), false);
                    }
                    _ => {
                        self.message("Enter six hex digits, e.g. #204080".into(), true);
                        self.dialog = Dialog::Color(field);
                    }
                }
            }
            _ => {
                self.edit_key(key, shift, files);
            }
        }
    }
    fn edit_key(&mut self, key: Key, shift: bool, files: &mut impl Files) {
        if key == Key::Escape {
            self.document.cancel();
            self.drag = None;
            self.selection = None;
            return;
        }
        self.finish_drag();
        match key {
            Key::Control('n') => {
                self.request(Pending::New);
            }
            Key::Control('o') => {
                self.request(Pending::Open);
            }
            Key::Control('s') if shift => {
                self.save_as();
            }
            Key::Control('s') => {
                self.save(files);
            }
            Key::Control('w' | 'q') => {
                self.request(Pending::Close);
            }
            Key::Control('z') if shift => {
                self.document.redo();
                self.selection = None;
                self.clamp_pan();
            }
            Key::Control('z') => {
                self.document.undo();
                self.selection = None;
                self.clamp_pan();
            }
            Key::Control('y') => {
                self.document.redo();
                self.selection = None;
                self.clamp_pan();
            }
            Key::Control('a') => {
                self.tool = Tool::Select;
                self.selection = Some(Rect::new(
                    0,
                    0,
                    self.document.image.width,
                    self.document.image.height,
                ));
            }
            Key::Control('x') if shift => {
                self.crop_selection();
            }
            Key::Control('r') => {
                self.document.rotate();
                self.selection = None;
                self.pan = (0, 0);
                self.clamp_pan();
            }
            Key::Control('h') => {
                self.document.flip_horizontal();
            }
            Key::Control('c' | 'x') => {
                self.copy(key == Key::Control('x'));
            }
            Key::Control('v') => {
                if let Some(image) = &self.clipboard {
                    let (x, y) = self
                        .selection
                        .map(|r| (r.x.max(0) as usize, r.y.max(0) as usize))
                        .unwrap_or((0, 0));
                    self.document.paste(image, x, y);
                    self.selection = Some(Rect::new(
                        x as i32,
                        y as i32,
                        image.width.min(self.document.image.width - x),
                        image.height.min(self.document.image.height - y),
                    ));
                }
            }
            Key::Delete => {
                let area = self.selection.unwrap_or(Rect::new(
                    0,
                    0,
                    self.document.image.width,
                    self.document.image.height,
                ));
                self.document.clear(area, self.background);
            }
            Key::Control('=' | '+') => {
                self.change_zoom(1);
            }
            Key::Control('-') => {
                self.change_zoom(-1);
            }
            Key::Control('0') => {
                self.zoom = 100;
                self.pan = (0, 0);
            }
            Key::Character('b' | 'B') => {
                self.tool = Tool::Brush;
            }
            Key::Character('e' | 'E') => {
                self.tool = Tool::Eraser;
            }
            Key::Character('l' | 'L') => {
                self.tool = Tool::Line;
            }
            Key::Character('r' | 'R') => {
                self.tool = Tool::Rectangle;
            }
            Key::Character('o' | 'O') => {
                self.tool = Tool::Ellipse;
            }
            Key::Character('f' | 'F') => {
                self.tool = Tool::Fill;
            }
            Key::Character('i' | 'I') => {
                self.tool = Tool::Picker;
            }
            Key::Character('m' | 'M') => {
                self.tool = Tool::Select;
            }
            Key::Character('[') => {
                self.size = self.size.saturating_sub(1).max(1);
            }
            Key::Character(']') => {
                self.size = (self.size + 1).min(32);
            }
            Key::Character('x' | 'X') => {
                core::mem::swap(&mut self.foreground, &mut self.background);
            }
            Key::Left => {
                self.pan.0 = self.pan.0.saturating_sub(32);
            }
            Key::Right => {
                self.pan.0 += 32;
                self.clamp_pan();
            }
            Key::Up => {
                self.pan.1 = self.pan.1.saturating_sub(32);
            }
            Key::Down => {
                self.pan.1 += 32;
                self.clamp_pan();
            }
            _ => {}
        }
    }
    fn copy(&mut self, cut: bool) {
        if let Some(area) = self.selection {
            self.clipboard = Some(Image {
                width: area.w,
                height: area.h,
                pixels: document::region(&self.document.image, area),
            });
            if cut {
                self.document.clear(area, self.background);
            }
            self.message("Copied selection (Saran clipboard)".into(), false);
        }
    }
    fn crop_selection(&mut self) {
        if let Some(area) = self.selection {
            match self.document.crop(area) {
                Ok(()) => {
                    self.selection = None;
                    self.pan = (0, 0);
                    self.clamp_pan();
                    self.message("Cropped selection".into(), false);
                }
                Err(error) => {
                    self.message(error.into(), true);
                }
            }
        } else {
            self.message("Select an area to crop".into(), true);
        }
    }
    pub fn pointer_move(&mut self, x: i32, y: i32) -> bool {
        let Some(point) = self.image_point(x, y) else {
            return false;
        };
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if point == drag.last {
            self.drag = Some(drag);
            return false;
        }
        match drag.tool {
            Tool::Brush | Tool::Eraser => {
                document::stroke(
                    &mut self.document.image,
                    drag.last,
                    point,
                    self.size,
                    drag.color,
                );
            }
            Tool::Select => {
                self.selection = Some(document::bounds(drag.start, point));
            }
            _ => {
                self.document
                    .restore(expanded(document::bounds(drag.start, drag.last), self.size));
                document::shape(
                    &mut self.document.image,
                    drag.tool,
                    drag.start,
                    point,
                    self.size,
                    drag.color,
                    self.filled,
                );
            }
        }
        self.drag = Some(Drag {
            last: point,
            ..drag
        });
        true
    }
    pub fn finish_drag(&mut self) {
        if self.drag.take().is_some() {
            self.document.commit();
        }
    }
    pub fn pointer_button(
        &mut self,
        x: i32,
        y: i32,
        button: usize,
        pressed: bool,
        files: &mut impl Files,
    ) {
        if !matches!(button, 1 | 2) {
            return;
        }
        if !pressed {
            self.finish_drag();
            return;
        }
        if !matches!(self.dialog, Dialog::None) {
            let (left, top, width) = self.dialog_bounds();
            let choices = if matches!(self.dialog, Dialog::Confirm(_)) {
                3
            } else {
                2
            };
            if Rect::new(left, top + 112, width, 32).contains(x, y) {
                let choice = ((x - left) as usize * choices / width.max(1)).min(choices - 1);
                if matches!(self.dialog, Dialog::Confirm(_) | Dialog::Overwrite(_)) {
                    self.confirm(choice, files);
                } else {
                    self.key(
                        if choice == 0 { Key::Enter } else { Key::Escape },
                        false,
                        files,
                    );
                }
            }
            return;
        }
        self.finish_drag();
        if y < 34 && button == 1 {
            let mut left = 8;
            for (i, (_, width)) in FILE_BUTTONS.iter().enumerate() {
                if left + width > self.width {
                    break;
                }
                if Rect::new(left as i32, 3, *width, 28).contains(x, y) {
                    match i {
                        0 => {
                            self.request(Pending::New);
                        }
                        1 => {
                            self.request(Pending::Open);
                        }
                        2 => {
                            self.save(files);
                        }
                        3 => {
                            self.save_as();
                        }
                        4 => {
                            self.document.undo();
                            self.selection = None;
                            self.clamp_pan();
                        }
                        5 => {
                            self.document.redo();
                            self.selection = None;
                            self.clamp_pan();
                        }
                        6 => {
                            self.dialog = Dialog::Size {
                                new: false,
                                field: Field::new(&format!(
                                    "{} x {}",
                                    self.document.image.width, self.document.image.height
                                )),
                            };
                        }
                        7 => {
                            self.document.clear(
                                Rect::new(
                                    0,
                                    0,
                                    self.document.image.width,
                                    self.document.image.height,
                                ),
                                self.background,
                            );
                        }
                        8 => {
                            self.crop_selection();
                        }
                        9 => {
                            self.document.rotate();
                            self.selection = None;
                            self.pan = (0, 0);
                            self.clamp_pan();
                        }
                        10 => {
                            self.document.flip_horizontal();
                        }
                        _ => {}
                    }
                    return;
                }
                left += width;
            }
        }
        if (38..68).contains(&y) && button == 1 {
            match x {
                64..=95 => {
                    self.size = self.size.saturating_sub(1).max(1);
                }
                146..=177 => {
                    self.size = (self.size + 1).min(32);
                }
                192..=279 => {
                    self.filled = !self.filled;
                }
                294..=397 => {
                    self.dialog = Dialog::Color(Field::new(&format!("#{:06X}", self.foreground)));
                }
                414..=457 => {
                    self.change_zoom(-1);
                }
                466..=509 => {
                    self.change_zoom(1);
                }
                518..=593 => {
                    self.zoom = 100;
                    self.pan = (0, 0);
                }
                _ => {}
            }
            return;
        }
        if (74..98).contains(&y) && x >= 90 {
            let index = (x - 90) as usize / 26;
            if let Some(&color) = PALETTE.get(index) {
                if button == 1 {
                    self.foreground = color;
                } else {
                    self.background = color;
                }
            }
            return;
        }
        if button == 1 && (TOOLS_X..CANVAS_X - 8).contains(&x) && y >= TOOLS_Y {
            let index = (y - TOOLS_Y) as usize / 32;
            if let Some(&tool) = Tool::ALL.get(index) {
                self.tool = tool;
                self.selection = None;
            }
            return;
        }
        let view = self.viewport();
        let visible_x = (view.w * 100 / self.zoom).max(1);
        let visible_y = (view.h * 100 / self.zoom).max(1);
        if Rect::new(view.x, view.y + view.h as i32 + 2, view.w, 8).contains(x, y)
            && self.document.image.width > visible_x
        {
            self.pan.0 = (x - view.x) as usize * (self.document.image.width - visible_x)
                / view.w.saturating_sub(1).max(1);
            return;
        }
        if Rect::new(view.x + view.w as i32 + 2, view.y, 8, view.h).contains(x, y)
            && self.document.image.height > visible_y
        {
            self.pan.1 = (y - view.y) as usize * (self.document.image.height - visible_y)
                / view.h.saturating_sub(1).max(1);
            return;
        }
        let Some(point) = self.image_point(x, y) else {
            return;
        };
        let color = if button == 2 || self.tool == Tool::Eraser {
            self.background
        } else {
            self.foreground
        };
        match self.tool {
            Tool::Fill => {
                self.document
                    .fill(point.0 as usize, point.1 as usize, color);
            }
            Tool::Picker => {
                let color = self.document.image.get(point.0 as usize, point.1 as usize);
                if button == 1 {
                    self.foreground = color;
                } else {
                    self.background = color;
                }
            }
            Tool::Select => {
                self.selection = Some(Rect::new(point.0, point.1, 1, 1));
                self.drag = Some(Drag {
                    start: point,
                    last: point,
                    color,
                    tool: self.tool,
                });
            }
            _ => {
                self.selection = None;
                self.document.begin();
                document::shape(
                    &mut self.document.image,
                    self.tool,
                    point,
                    point,
                    self.size,
                    color,
                    self.filled,
                );
                self.drag = Some(Drag {
                    start: point,
                    last: point,
                    color,
                    tool: self.tool,
                });
            }
        }
    }
    pub fn dialog_bounds(&self) -> (i32, i32, usize) {
        let w = self.width.saturating_sub(24).min(540);
        (
            ((self.width - w) / 2) as i32,
            self.height.saturating_sub(170) as i32 / 2,
            w,
        )
    }
    pub fn title(&self) -> String {
        format!(
            "{}{}",
            if self.document.dirty() { "* " } else { "" },
            self.path
                .as_deref()
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or("Saran")
        )
    }
}
pub fn dimensions(text: &str) -> Result<(usize, usize), &'static str> {
    let (a, b) = text
        .split_once(['x', 'X'])
        .ok_or("Use WIDTH x HEIGHT, each 1..1024 pixels")?;
    let w = a.trim().parse::<usize>().map_err(|_| "Invalid width")?;
    let h = b.trim().parse::<usize>().map_err(|_| "Invalid height")?;
    if w == 0 || h == 0 || w > crate::bitmap::MAX_EDGE || h > crate::bitmap::MAX_EDGE {
        return Err("Canvas size must be 1..1024 pixels on each side");
    }
    Ok((w, h))
}
fn expanded(area: Rect, size: usize) -> Rect {
    Rect::new(
        area.x - size as i32,
        area.y - size as i32,
        area.w + size * 2,
        area.h + size * 2,
    )
}
