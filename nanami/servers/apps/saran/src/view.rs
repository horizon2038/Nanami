use crate::{
    app::{Dialog, Paint, FILE_BUTTONS, PALETTE, TOOLS_X, TOOLS_Y},
    document::Tool,
};
use alloc::format;
use nanami_ui::{Canvas, Font, Rect, ACCENT, CHROME, ERROR, INK, MUTED, PAPER, RULE};

pub fn draw(c: &mut Canvas<'_>, font: &Font, p: &Paint) {
    let w = c.width;
    let h = c.height;
    c.rect(Rect::new(0, 0, w, h), CHROME);
    let mut x = 8;
    for (label, width) in FILE_BUTTONS {
        if x + width > w {
            break;
        }
        c.button(font, Rect::new(x as i32, 3, width, 28), label, false);
        x += width;
    }
    c.rect(Rect::new(0, 33, w, 1), RULE);
    c.text(font, 12, 45, "Size", MUTED, 48);
    for (x, width, label, active) in [
        (64, 32, "-", false),
        (146, 32, "+", false),
        (192, 88, "Filled", p.filled),
        (294, 104, "Color...", false),
        (414, 44, "-", false),
        (466, 44, "+", false),
        (518, 76, "1:1", p.zoom == 100),
    ] {
        if x + width <= w {
            c.button(font, Rect::new(x as i32, 38, width, 28), label, active);
        }
    }
    c.text(font, 106, 45, &format!("{}", p.size), INK, 38);
    c.text(
        font,
        608,
        45,
        &format!("{}%", p.zoom),
        MUTED,
        w.saturating_sub(608),
    );
    c.rect(Rect::new(14, 74, 24, 22), p.background);
    c.border(Rect::new(14, 74, 24, 22), MUTED);
    c.rect(Rect::new(8, 70, 24, 22), p.foreground);
    c.border(Rect::new(8, 70, 24, 22), INK);
    c.text(font, 44, 76, "F/B", MUTED, 38);
    for (i, color) in PALETTE.iter().enumerate() {
        let x = 90 + i * 26;
        if x + 22 > w {
            break;
        }
        c.rect(Rect::new(x as i32, 74, 22, 22), *color);
        c.border(Rect::new(x as i32, 74, 22, 22), RULE);
        if *color == p.foreground {
            c.border(Rect::new(x as i32 - 2, 72, 26, 26), ACCENT);
        }
    }
    c.rect(Rect::new(0, 104, w, 1), RULE);
    let bottom = h.saturating_sub(26);
    let old = c.clip;
    c.clip = Rect::new(0, 106, w, bottom.saturating_sub(106));
    for (i, tool) in Tool::ALL.iter().enumerate() {
        c.button(
            font,
            Rect::new(TOOLS_X, TOOLS_Y + i as i32 * 32, 104, 28),
            tool.label(),
            *tool == p.tool,
        );
    }
    let view = p.viewport();
    c.rect(view, 0xc9c6be);
    c.clip = view.intersect(old);
    let (origin_x, origin_y) = p.origin();
    let image = &p.document.image;
    // Nearest-neighbor presentation: never resample or resize document pixels.
    for y in view.y.max(origin_y)..(view.y + view.h as i32) {
        let source_y = (y - origin_y) as usize * 100 / p.zoom + p.pan.1;
        if source_y >= image.height {
            break;
        }
        for x in view.x.max(origin_x)..(view.x + view.w as i32) {
            let source_x = (x - origin_x) as usize * 100 / p.zoom + p.pan.0;
            if source_x >= image.width {
                break;
            }
            c.pixel(x, y, image.get(source_x, source_y));
        }
    }
    if let Some(area) = p.selection {
        let left = origin_x + (area.x - p.pan.0 as i32) * p.zoom as i32 / 100;
        let top = origin_y + (area.y - p.pan.1 as i32) * p.zoom as i32 / 100;
        let width = (area.w * p.zoom / 100).max(1);
        let height = (area.h * p.zoom / 100).max(1);
        c.border(
            Rect::new(left - 1, top - 1, width + 2, height + 2),
            0xffffff,
        );
        for x in 0..width {
            let color = if x / 4 % 2 == 0 { 0x101010 } else { 0xffffff };
            c.pixel(left + x as i32, top, color);
            c.pixel(left + x as i32, top + height as i32 - 1, color);
        }
        for y in 0..height {
            let color = if y / 4 % 2 == 0 { 0x101010 } else { 0xffffff };
            c.pixel(left, top + y as i32, color);
            c.pixel(left + width as i32 - 1, top + y as i32, color);
        }
    }
    c.clip = old;
    if view.w > 0 && view.h > 0 {
        let visible_x = (view.w * 100 / p.zoom).max(1);
        let visible_y = (view.h * 100 / p.zoom).max(1);
        if image.width > visible_x {
            let thumb = (view.w * visible_x / image.width).max(12).min(view.w);
            let x = view.x + ((view.w - thumb) * p.pan.0 / (image.width - visible_x)) as i32;
            c.rect(
                Rect::new(view.x, view.y + view.h as i32 + 2, view.w, 8),
                RULE,
            );
            c.rect(Rect::new(x, view.y + view.h as i32 + 2, thumb, 8), MUTED);
        }
        if image.height > visible_y {
            let thumb = (view.h * visible_y / image.height).max(12).min(view.h);
            let y = view.y + ((view.h - thumb) * p.pan.1 / (image.height - visible_y)) as i32;
            c.rect(
                Rect::new(view.x + view.w as i32 + 2, view.y, 8, view.h),
                RULE,
            );
            c.rect(Rect::new(view.x + view.w as i32 + 2, y, 8, thumb), MUTED);
        }
    }
    c.rect(Rect::new(0, bottom as i32, w, 26), CHROME);
    c.rect(Rect::new(0, bottom as i32, w, 1), RULE);
    let position = format!(
        "{} x {} px   {}%   #{:06X}",
        image.width, image.height, p.zoom, p.foreground
    );
    c.text(font, 12, bottom as i32 + 5, &position, MUTED, w);
    let state = if p.document.dirty() && !p.error {
        "Modified"
    } else {
        &p.status
    };
    let available = w.saturating_sub(position.len() * 9 + 32) / 9;
    let len = state.chars().count().min(available);
    c.text(
        font,
        w.saturating_sub(len * 9 + 12) as i32,
        bottom as i32 + 5,
        state,
        if p.error { ERROR } else { MUTED },
        len * 9,
    );
    if !matches!(p.dialog, Dialog::None) {
        dialog(c, font, p);
    }
}
fn dialog(c: &mut Canvas<'_>, font: &Font, p: &Paint) {
    let (left, top, width) = p.dialog_bounds();
    c.rect(Rect::new(left, top, width, 164), PAPER);
    c.border(Rect::new(left, top, width, 164), RULE);
    let (heading, label, field, choices): (&str, &str, Option<&nanami_ui::Field>, &[&str]) =
        match &p.dialog {
            Dialog::Path { save, field } => (
                if *save { "Save BMP" } else { "Open BMP" },
                "Path",
                Some(field),
                &["OK [Enter]", "Cancel [Esc]"],
            ),
            Dialog::Size { new, field } => (
                if *new {
                    "New canvas"
                } else {
                    "Resize canvas (crop / extend)"
                },
                "Size in pixels",
                Some(field),
                &["OK [Enter]", "Cancel [Esc]"],
            ),
            Dialog::Color(field) => (
                "Foreground color",
                "RGB hex",
                Some(field),
                &["OK [Enter]", "Cancel [Esc]"],
            ),
            Dialog::Confirm(_) => (
                "Save changes before continuing?",
                "",
                None,
                &["Save [Enter]", "Discard [D]", "Cancel [Esc]"],
            ),
            Dialog::Overwrite(_) => (
                "Replace the existing file?",
                "",
                None,
                &["Replace [Enter]", "Cancel [Esc]"],
            ),
            Dialog::None => return,
        };
    c.text(
        font,
        left + 16,
        top + 14,
        heading,
        INK,
        width.saturating_sub(32),
    );
    if let Some(field) = field {
        c.text(
            font,
            left + 16,
            top + 42,
            label,
            MUTED,
            width.saturating_sub(32),
        );
        c.field(
            font,
            Rect::new(left + 16, top + 64, width.saturating_sub(32), 30),
            field,
        );
    } else {
        let path = match &p.dialog {
            Dialog::Overwrite(path) => path.as_str(),
            _ => p.path.as_deref().unwrap_or("Untitled canvas"),
        };
        c.text(
            font,
            left + 16,
            top + 54,
            path,
            MUTED,
            width.saturating_sub(32),
        );
    }
    if p.error {
        c.text(
            font,
            left + 16,
            top + 146,
            &p.status,
            ERROR,
            width.saturating_sub(32),
        );
    }
    for (i, label) in choices.iter().enumerate() {
        c.button(
            font,
            Rect::new(
                left + (i * width / choices.len()) as i32 + 6,
                top + 112,
                (width / choices.len()).saturating_sub(12),
                32,
            ),
            label,
            i == 0,
        );
    }
}
