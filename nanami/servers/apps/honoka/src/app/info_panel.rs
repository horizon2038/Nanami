use alloc::{string::String, vec::Vec};

use crate::constants::MENU_BAR_HEIGHT;
use crate::font::TextRenderer;
use crate::framebuffer::{Framebuffer, Rect, ScreenInfo};

const MARGIN: i32 = 8;
const PADDING: i32 = 8;
const LINE_HEIGHT: i32 = 18;

/// Static desktop information, behind application windows and below the clock.
/// Painted into BackgroundCache once; direct painting is the low-memory fallback.
pub struct InfoPanel {
    rect: Rect,
    lines: Vec<String>,
}

impl InfoPanel {
    pub fn new(screen: ScreenInfo, text: &TextRenderer, fields: [String; 4]) -> Self {
        let screen_width = screen.width.min(i32::MAX as usize) as i32;
        let screen_height = screen.height.min(i32::MAX as usize) as i32;
        let y = MENU_BAR_HEIGHT + MARGIN;
        let available_width = (screen_width - 2 * MARGIN).max(0);
        let available_height = (screen_height - y - MARGIN).max(0);
        if available_width == 0 || available_height == 0 {
            return Self {
                rect: Rect::EMPTY,
                lines: Vec::new(),
            };
        }
        let width = fields
            .iter()
            .map(|line| text.text_width(line.as_bytes()))
            .max()
            .unwrap_or(0)
            .saturating_add(2 * PADDING)
            .min(available_width);
        let inner_width = (width - 2 * PADDING).max(1);
        let mut lines = Vec::new();
        for field in fields {
            let mut start = 0;
            for (end, ch) in field.char_indices() {
                if end > start
                    && text.text_width(field[start..end + ch.len_utf8()].as_bytes()) > inner_width
                {
                    lines.push(String::from(&field[start..end]));
                    start = end;
                }
            }
            lines.push(String::from(&field[start..]));
        }
        let height = (lines.len() as i32 * LINE_HEIGHT + 2 * PADDING).min(available_height);
        Self {
            rect: Rect::new(screen_width - MARGIN - width, y, width, height),
            lines,
        }
    }

    pub fn draw(&self, framebuffer: &Framebuffer, text: &TextRenderer, dirty: Rect) {
        let Some(area) = intersection(self.rect, dirty) else {
            return;
        };
        // Deliberately independent of wallpaper and theme colors.
        let white = framebuffer.color(255, 255, 255);
        let black = framebuffer.color(0, 0, 0);
        framebuffer.fill_rect_clip(area, white);
        let inner = Rect::new(
            self.rect.x + PADDING,
            self.rect.y + PADDING,
            self.rect.width - 2 * PADDING,
            self.rect.height - 2 * PADDING,
        );
        let Some(clip) = intersection(inner, area) else {
            return;
        };
        for (row, line) in self.lines.iter().enumerate() {
            let y = inner.y + row as i32 * LINE_HEIGHT;
            if y >= clip.y + clip.height {
                break;
            }
            if y + LINE_HEIGHT <= clip.y {
                continue;
            }
            text.draw_title(
                framebuffer,
                clip,
                inner.x,
                y,
                line.as_bytes(),
                black,
                white,
                u8::MAX,
            );
        }
    }
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let width = (a.x + a.width).min(b.x + b.width) - x;
    let height = (a.y + a.height).min(b.y + b.height) - y;
    (width > 0 && height > 0).then(|| Rect::new(x, y, width, height))
}
