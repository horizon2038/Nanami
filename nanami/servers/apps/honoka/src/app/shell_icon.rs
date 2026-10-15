//! Pixel-aligned terminal launcher, sized for the existing 28 x 22 hit target.
use super::{
    contains_rect, fill_clipped, intersect_rect, rounded_row_span, Framebuffer, Rect, Theme,
    SHELL_ICON_RECT,
};

// A balanced > prompt with soft diagonal edges. The fixed mask avoids font
// initialization, temporary surfaces, and per-pixel geometric calculations.
const PROMPT: [[u8; 6]; 8] = [
    [128, 255, 128, 0, 0, 0],
    [0, 128, 255, 128, 0, 0],
    [0, 0, 128, 255, 128, 0],
    [0, 0, 0, 128, 255, 128],
    [0, 0, 0, 128, 255, 128],
    [0, 0, 128, 255, 128, 0],
    [0, 128, 255, 128, 0, 0],
    [128, 255, 128, 0, 0, 0],
];

pub(super) fn draw(framebuffer: &Framebuffer, dirty: Rect, theme: Theme) {
    let outer = SHELL_ICON_RECT;
    let Some(clip) = intersect_rect(outer, dirty) else {
        return;
    };
    let inner = Rect::new(outer.x + 1, outer.y + 1, outer.width - 2, outer.height - 2);
    for y in clip.y..clip.y + clip.height {
        if let Some((x0, x1)) = rounded_row_span(outer, 3, y) {
            fill_clipped(framebuffer, clip, x0, y, x1 - x0, 1, theme.title_bar);
            blend_clipped(framebuffer, clip, x0 - 1, y, theme.title_bar, 96);
            blend_clipped(framebuffer, clip, x1, y, theme.title_bar, 96);
        }
        if let Some((x0, x1)) = rounded_row_span(inner, 2, y) {
            let color = if y < outer.y + 6 {
                theme.menu_bar
            } else {
                theme.window_frame
            };
            fill_clipped(framebuffer, clip, x0, y, x1 - x0, 1, color);
        }
    }
    // A slim title strip makes the outline read as a terminal window rather
    // than a generic > button. All shades follow the selected Honoka theme.
    fill_clipped(
        framebuffer,
        clip,
        outer.x + 1,
        outer.y + 6,
        outer.width - 2,
        1,
        theme.menu_edge,
    );
    for x in [outer.x + 4, outer.x + 7, outer.x + 10] {
        blend_clipped(framebuffer, clip, x, outer.y + 3, theme.title_text, 112);
    }
    for (row, samples) in PROMPT.iter().enumerate() {
        for (column, &coverage) in samples.iter().enumerate() {
            if coverage != 0 {
                blend_clipped(
                    framebuffer,
                    clip,
                    outer.x + 6 + column as i32,
                    outer.y + 9 + row as i32,
                    theme.title_text,
                    coverage,
                );
            }
        }
    }
    fill_clipped(
        framebuffer,
        clip,
        outer.x + 16,
        outer.y + 16,
        6,
        2,
        theme.accent,
    );
}

fn blend_clipped(framebuffer: &Framebuffer, clip: Rect, x: i32, y: i32, color: u32, alpha: u8) {
    if contains_rect(clip, x, y) {
        framebuffer.blend_pixel(x, y, color, alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::ScreenInfo;
    use alloc::vec;
    use alloc::vec::Vec;

    const GUARD: u32 = 0xdeadbeef;

    fn canvas(width: usize, height: usize) -> (Framebuffer, Vec<u32>) {
        let stride = width + 3;
        let mut pixels = vec![GUARD; stride * height];
        let screen = ScreenInfo {
            width,
            height,
            stride_bytes: stride * 4,
            bits_per_pixel: 32,
            red_position: 16,
            red_size: 8,
            green_position: 8,
            green_size: 8,
            blue_position: 0,
            blue_size: 8,
        };
        let framebuffer =
            Framebuffer::new(1, pixels.as_mut_ptr() as usize, pixels.len() * 4, screen)
                .ok()
                .unwrap();
        (framebuffer, pixels)
    }

    #[test]
    fn prompt_cursor_and_rounded_frame_follow_both_themes() {
        for data in [
            include_bytes!("../../assets/themes/default.theme").as_slice(),
            include_bytes!("../../assets/themes/warm.theme").as_slice(),
        ] {
            let (framebuffer, pixels) = canvas(64, 40);
            let theme = super::super::parse_theme(&framebuffer, data).unwrap();
            let full = Rect::new(0, 0, 64, 40);
            framebuffer.fill_rect_clip(full, theme.menu_bar);
            draw(&framebuffer, full, theme);
            let at = |x, y| pixels[y * 67 + x];
            let x = SHELL_ICON_RECT.x as usize;
            let y = SHELL_ICON_RECT.y as usize;
            assert_eq!(at(x, y), theme.menu_bar); // transparent corner
            assert_eq!(at(x + 7, y + 9), theme.title_text);
            assert_eq!(at(x + 10, y + 12), theme.title_text);
            assert_eq!(at(x + 16, y + 16), theme.accent);
            assert_eq!(at(x + 21, y + 17), theme.accent);
            assert_eq!(at(x + 3, y + 18), theme.window_frame);
            for py in 0..40 {
                for px in 0..64 {
                    if !contains_rect(SHELL_ICON_RECT, px as i32, py as i32) {
                        assert_eq!(at(px, py), theme.menu_bar);
                    }
                }
                assert!(pixels[py * 67 + 64..(py + 1) * 67]
                    .iter()
                    .all(|&p| p == GUARD));
            }
        }
    }

    #[test]
    fn pixel_clipped_redraw_and_repeated_paints_match_the_whole_menu() {
        let (whole, expected) = canvas(64, 40);
        let (tiled, actual) = canvas(64, 40);
        let data = include_bytes!("../../assets/themes/default.theme");
        let theme = super::super::parse_theme(&whole, data).unwrap();
        let full = Rect::new(0, 0, 64, 40);
        super::super::draw_menu_bar(&whole, whole.screen(), theme, full);
        for _ in 0..2 {
            for y in 0..40 {
                for x in 0..64 {
                    super::super::draw_menu_bar(
                        &tiled,
                        tiled.screen(),
                        theme,
                        Rect::new(x, y, 1, 1),
                    );
                }
            }
            assert_eq!(
                actual, expected,
                "antialiasing must not accumulate or escape dirty regions"
            );
        }
    }

    #[test]
    fn small_framebuffers_keep_stride_padding_and_outside_rows_untouched() {
        for (width, height) in [(1, 1), (16, 10), (40, 30)] {
            let (framebuffer, pixels) = canvas(width, height);
            let theme = super::super::parse_theme(
                &framebuffer,
                include_bytes!("../../assets/themes/default.theme"),
            )
            .unwrap();
            draw(&framebuffer, Rect::new(0, 0, 64, 40), theme);
            for row in pixels.chunks(width + 3) {
                assert!(row[width..].iter().all(|&p| p == GUARD));
            }
        }
    }
}
