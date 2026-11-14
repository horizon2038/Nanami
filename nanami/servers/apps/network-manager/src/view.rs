use crate::model::Model;
use nanami_ui::{Canvas, Font, Rect, ACCENT, CHROME, ERROR, INK, MUTED, PAPER, RULE};
pub fn field_rect(width: usize, index: usize) -> Rect {
    Rect::new(
        205,
        108 + index as i32 * 48,
        width.saturating_sub(230).min(340),
        30,
    )
}
pub fn draw(c: &mut Canvas<'_>, font: &Font, model: &Model) {
    let width = c.width;
    c.rect(Rect::new(0, 0, width, c.height), PAPER);
    c.rect(Rect::new(0, 0, width, 78), CHROME);
    c.text(
        font,
        20,
        14,
        "IPv4 configuration",
        INK,
        width.saturating_sub(40),
    );
    c.button(
        font,
        Rect::new(20, 40, 165, 28),
        "Automatic (DHCP)",
        model.mode == 0,
    );
    c.button(font, Rect::new(200, 40, 165, 28), "Manual", model.mode == 1);
    c.rect(Rect::new(0, 77, width, 1), RULE);
    for (index, label) in ["IP address", "Subnet mask", "Default gateway", "DNS server"]
        .iter()
        .enumerate()
    {
        let rect = field_rect(width, index);
        c.text(
            font,
            20,
            rect.y + 7,
            label,
            if model.mode == 1 { INK } else { MUTED },
            180,
        );
        c.field(font, rect, &model.fields[index]);
        if model.mode == 1 && model.focus == index {
            c.border(rect, ACCENT);
        }
    }
    c.text(
        font,
        20,
        308,
        "0.0.0.0 disables the optional gateway or DNS.",
        MUTED,
        width.saturating_sub(40),
    );
    c.button(
        font,
        Rect::new(20, 340, 140, 30),
        "Save & apply",
        model.edited,
    );
    c.button(font, Rect::new(175, 340, 140, 30), "Reload", false);
    c.button(font, Rect::new(330, 340, 140, 30), "Refresh", false);
    c.text(font, 20, 388, &model.active, INK, width.saturating_sub(40));
    c.text(
        font,
        20,
        414,
        &model.message,
        if model.error { ERROR } else { ACCENT },
        width.saturating_sub(40),
    );
    c.text(
        font,
        20,
        448,
        "Apply/reload closes existing connections. Ctrl+S: apply; F5: reload.",
        MUTED,
        width.saturating_sub(40),
    );
}
