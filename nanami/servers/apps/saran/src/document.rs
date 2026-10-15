use crate::bitmap::Image;
use alloc::{collections::VecDeque, vec::Vec};
use embedded_graphics::{
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{Ellipse, PrimitiveStyle, Rectangle},
};
use nanami_ui::Rect;

const HISTORY_BYTES: usize = 16 * 1024 * 1024;
const HISTORY_STEPS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Eraser,
    Line,
    Rectangle,
    Ellipse,
    Fill,
    Picker,
    Select,
}
impl Tool {
    pub const ALL: [Self; 8] = [
        Self::Brush,
        Self::Eraser,
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::Fill,
        Self::Picker,
        Self::Select,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Brush => "Brush",
            Self::Eraser => "Eraser",
            Self::Line => "Line",
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Fill => "Fill",
            Self::Picker => "Picker",
            Self::Select => "Select",
        }
    }
}
enum Change {
    Pixels {
        area: Rect,
        before: Vec<u32>,
        after: Vec<u32>,
    },
    Size {
        before: Image,
        after: Image,
    },
}
impl Change {
    fn cost(&self) -> usize {
        match self {
            Self::Pixels { before, after, .. } => (before.len() + after.len()) * 4,
            Self::Size { before, after } => (before.pixels.len() + after.pixels.len()) * 4,
        }
    }
    fn apply(&self, image: &mut Image, forward: bool) {
        match self {
            Self::Size { before, after } => {
                *image = if forward {
                    after.clone()
                } else {
                    before.clone()
                };
            }
            Self::Pixels {
                area,
                before,
                after,
            } => {
                let data = if forward { after } else { before };
                for row in 0..area.h {
                    let offset = (area.y as usize + row) * image.width + area.x as usize;
                    image.pixels[offset..offset + area.w]
                        .copy_from_slice(&data[row * area.w..(row + 1) * area.w]);
                }
            }
        }
    }
}
struct Edit {
    change: Change,
    before: u64,
    after: u64,
}
pub struct Document {
    pub image: Image,
    original: Option<Image>,
    undo: VecDeque<Edit>,
    redo: Vec<Edit>,
    cost: usize,
    revision: u64,
    saved: u64,
    next: u64,
}
impl Document {
    pub fn new(image: Image) -> Self {
        Self {
            image,
            original: None,
            undo: VecDeque::new(),
            redo: Vec::new(),
            cost: 0,
            revision: 0,
            saved: 0,
            next: 1,
        }
    }
    pub fn dirty(&self) -> bool {
        self.revision != self.saved
    }
    pub fn mark_saved(&mut self) {
        self.saved = self.revision;
    }
    pub fn begin(&mut self) {
        if self.original.is_none() {
            self.original = Some(self.image.clone());
        }
    }
    pub fn cancel(&mut self) {
        if let Some(original) = self.original.take() {
            self.image = original;
        }
    }
    pub fn restore(&mut self, area: Rect) {
        if let Some(original) = &self.original {
            let area = area.intersect(Rect::new(0, 0, self.image.width, self.image.height));
            for row in area.y as usize..area.y as usize + area.h {
                let start = row * self.image.width + area.x as usize;
                self.image.pixels[start..start + area.w]
                    .copy_from_slice(&original.pixels[start..start + area.w]);
            }
        }
    }
    pub fn commit(&mut self) -> bool {
        let Some(before) = self.original.take() else {
            return false;
        };
        let change = if before.width != self.image.width || before.height != self.image.height {
            Change::Size {
                before,
                after: self.image.clone(),
            }
        } else {
            let mut x0 = before.width;
            let mut y0 = before.height;
            let mut x1 = 0;
            let mut y1 = 0;
            for (i, (&a, &b)) in before.pixels.iter().zip(&self.image.pixels).enumerate() {
                if a != b {
                    let x = i % before.width;
                    let y = i / before.width;
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
            if x1 == 0 {
                return false;
            }
            let area = Rect::new(x0 as i32, y0 as i32, x1 - x0, y1 - y0);
            Change::Pixels {
                area,
                before: region(&before, area),
                after: region(&self.image, area),
            }
        };
        for edit in self.redo.drain(..) {
            self.cost -= edit.change.cost();
        }
        self.cost += change.cost();
        self.undo.push_back(Edit {
            change,
            before: self.revision,
            after: self.next,
        });
        self.revision = self.next;
        self.next += 1;
        while self.cost > HISTORY_BYTES || self.undo.len() > HISTORY_STEPS {
            if let Some(edit) = self.undo.pop_front() {
                self.cost -= edit.change.cost();
            } else {
                break;
            }
        }
        true
    }
    pub fn undo(&mut self) {
        self.cancel();
        if let Some(edit) = self.undo.pop_back() {
            edit.change.apply(&mut self.image, false);
            self.revision = edit.before;
            self.redo.push(edit);
        }
    }
    pub fn redo(&mut self) {
        self.cancel();
        if let Some(edit) = self.redo.pop() {
            edit.change.apply(&mut self.image, true);
            self.revision = edit.after;
            self.undo.push_back(edit);
        }
    }
    pub fn resize(&mut self, width: usize, height: usize, color: u32) -> Result<(), &'static str> {
        let mut next = Image::new(width, height, color)?;
        for y in 0..height.min(self.image.height) {
            let count = width.min(self.image.width);
            next.pixels[y * width..y * width + count].copy_from_slice(
                &self.image.pixels[y * self.image.width..y * self.image.width + count],
            );
        }
        self.begin();
        self.image = next;
        self.commit();
        Ok(())
    }
    pub fn fill(&mut self, x: usize, y: usize, color: u32) {
        let color = color & 0xffffff;
        let old = self.image.get(x, y);
        if old == color {
            return;
        }
        self.begin();
        let mut stack = alloc::vec![(x, y)];
        while let Some((x, y)) = stack.pop() {
            if self.image.get(x, y) != old {
                continue;
            }
            let mut left = x;
            while left > 0 && self.image.get(left - 1, y) == old {
                left -= 1;
            }
            let mut right = x;
            while right + 1 < self.image.width && self.image.get(right + 1, y) == old {
                right += 1;
            }
            for x in left..=right {
                self.image.set(x as i32, y as i32, color);
            }
            for adjacent in [
                y.checked_sub(1),
                y.checked_add(1).filter(|&row| row < self.image.height),
            ]
            .into_iter()
            .flatten()
            {
                let mut previous = false;
                for x in left..=right {
                    let same = self.image.get(x, adjacent) == old;
                    if same && !previous {
                        stack.push((x, adjacent));
                    }
                    previous = same;
                }
            }
        }
        self.commit();
    }
    pub fn clear(&mut self, area: Rect, color: u32) {
        self.begin();
        let area = area.intersect(Rect::new(0, 0, self.image.width, self.image.height));
        for y in area.y..area.y + area.h as i32 {
            for x in area.x..area.x + area.w as i32 {
                self.image.set(x, y, color);
            }
        }
        self.commit();
    }
    pub fn paste(&mut self, image: &Image, x: usize, y: usize) {
        self.begin();
        for row in 0..image.height.min(self.image.height.saturating_sub(y)) {
            let count = image.width.min(self.image.width.saturating_sub(x));
            if count > 0 {
                let dest = (y + row) * self.image.width + x;
                self.image.pixels[dest..dest + count]
                    .copy_from_slice(&image.pixels[row * image.width..row * image.width + count]);
            }
        }
        self.commit();
    }

    pub fn crop(&mut self, area: Rect) -> Result<(), &'static str> {
        let area = area.intersect(Rect::new(0, 0, self.image.width, self.image.height));
        if area.w == 0 || area.h == 0 {
            return Err("Select an image area to crop");
        }
        let next = Image {
            width: area.w,
            height: area.h,
            pixels: region(&self.image, area),
        };
        self.begin();
        self.image = next;
        self.commit();
        Ok(())
    }
    pub fn rotate(&mut self) {
        let mut next = Image::new(self.image.height, self.image.width, 0).unwrap();
        for y in 0..self.image.height {
            for x in 0..self.image.width {
                next.set(
                    (self.image.height - 1 - y) as i32,
                    x as i32,
                    self.image.get(x, y),
                );
            }
        }
        self.begin();
        self.image = next;
        self.commit();
    }
    pub fn flip_horizontal(&mut self) {
        self.begin();
        for row in self.image.pixels.chunks_mut(self.image.width) {
            row.reverse();
        }
        self.commit();
    }
}
pub fn region(image: &Image, area: Rect) -> Vec<u32> {
    let mut pixels = Vec::with_capacity(area.w * area.h);
    for y in area.y as usize..area.y as usize + area.h {
        pixels.extend_from_slice(
            &image.pixels
                [y * image.width + area.x as usize..y * image.width + area.x as usize + area.w],
        );
    }
    pixels
}
pub fn bounds(a: (i32, i32), b: (i32, i32)) -> Rect {
    Rect::new(
        a.0.min(b.0),
        a.1.min(b.1),
        (a.0 - b.0).unsigned_abs() as usize + 1,
        (a.1 - b.1).unsigned_abs() as usize + 1,
    )
}
pub fn shape(
    image: &mut Image,
    tool: Tool,
    a: (i32, i32),
    b: (i32, i32),
    size: usize,
    color: u32,
    filled: bool,
) {
    let area = bounds(a, b);
    let rgb = Rgb888::new((color >> 16) as u8, (color >> 8) as u8, color as u8);
    let style = if filled {
        PrimitiveStyle::with_fill(rgb)
    } else {
        PrimitiveStyle::with_stroke(rgb, size as u32)
    };
    match tool {
        Tool::Rectangle => {
            let _ = Rectangle::new(
                Point::new(area.x, area.y),
                Size::new(area.w as u32, area.h as u32),
            )
            .into_styled(style)
            .draw(image);
        }
        Tool::Ellipse => {
            let _ = Ellipse::new(
                Point::new(area.x, area.y),
                Size::new(area.w as u32, area.h as u32),
            )
            .into_styled(style)
            .draw(image);
        }
        _ => {
            stroke(image, a, b, size, color);
        }
    }
}
pub fn stroke(image: &mut Image, a: (i32, i32), b: (i32, i32), size: usize, color: u32) {
    let (mut x, mut y) = a;
    let dx = (b.0 - x).abs();
    let dy = -(b.1 - y).abs();
    let sx = if x < b.0 { 1 } else { -1 };
    let sy = if y < b.1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        for row in 0..size {
            for col in 0..size {
                let ox = col as i32 * 2 - size as i32 + 1;
                let oy = row as i32 * 2 - size as i32 + 1;
                if ox * ox + oy * oy <= (size * size) as i32 {
                    image.set(
                        x + col as i32 - size as i32 / 2,
                        y + row as i32 - size as i32 / 2,
                        color,
                    );
                }
            }
        }
        if (x, y) == b {
            break;
        }
        let twice = error * 2;
        if twice >= dy {
            error += dy;
            x += sx;
        }
        if twice <= dx {
            error += dx;
            y += sy;
        }
    }
}
