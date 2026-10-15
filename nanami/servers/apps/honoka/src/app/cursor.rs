//! Conventional arrow with a fixed tip/hotspot and antialiased one-pixel outline.
//! Coverage is evaluated at compile time, never in the pointer motion hot path.
use super::{Framebuffer, Rect};
use crate::constants::{CURSOR_HEIGHT, CURSOR_WIDTH};

const OUTER: [(i32, i32); 7] = [
    (0, 0),
    (0, 19),
    (5, 15),
    (9, 24),
    (13, 22),
    (9, 14),
    (18, 14),
];
const INNER: [(i32, i32); 7] = [
    (1, 3),
    (1, 17),
    (5, 13),
    (10, 22),
    (12, 21),
    (8, 13),
    (15, 13),
];

const fn inside(polygon: &[(i32, i32); 7], x: i32, y: i32) -> bool {
    let mut inside = false;
    let mut i = 0;
    let mut j = 6;
    while i < 7 {
        let (xi, yi) = (polygon[i].0 * 8, polygon[i].1 * 8);
        let (xj, yj) = (polygon[j].0 * 8, polygon[j].1 * 8);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
        i += 1;
    }
    inside
}

const fn mask() -> [[(u8, u8); CURSOR_WIDTH as usize]; CURSOR_HEIGHT as usize] {
    let mut result = [[(0, 0); CURSOR_WIDTH as usize]; CURSOR_HEIGHT as usize];
    let mut y = 0;
    while y < CURSOR_HEIGHT as usize {
        let mut x = 0;
        while x < CURSOR_WIDTH as usize {
            let mut outer = 0;
            let mut inner = 0;
            let mut sy = 0;
            while sy < 4 {
                let mut sx = 0;
                while sx < 4 {
                    let px = x as i32 * 8 + sx * 2 + 1;
                    let py = y as i32 * 8 + sy * 2 + 1;
                    if inside(&OUTER, px, py) {
                        outer += 1;
                    }
                    if inside(&INNER, px, py) {
                        inner += 1;
                    }
                    sx += 1;
                }
                sy += 1;
            }
            result[y][x] = ((outer * 255 / 16) as u8, (inner * 255 / 16) as u8);
            x += 1;
        }
        y += 1;
    }
    result
}

const MASK: [[(u8, u8); CURSOR_WIDTH as usize]; CURSOR_HEIGHT as usize] = mask();

pub(super) fn draw(fb: &Framebuffer, x: i32, y: i32, fill: u32, outline: u32, clip: Rect) {
    for (row, samples) in MASK.iter().enumerate() {
        for (column, &(outer, inner)) in samples.iter().enumerate() {
            let px = x + column as i32;
            let py = y + row as i32;
            if outer == 0
                || px < clip.x
                || py < clip.y
                || px >= clip.x.saturating_add(clip.width)
                || py >= clip.y.saturating_add(clip.height)
            {
                continue;
            }
            // Mix the outline/fill first, then composite once over the restored
            // background. Repainting a clipped region cannot darken old edges.
            let ratio = (inner as u32 * 255 / outer as u32).min(255);
            let mut color = 0;
            for shift in [0, 8, 16] {
                let a = (fill >> shift) & 255;
                let b = (outline >> shift) & 255;
                color |= ((a * ratio + b * (255 - ratio) + 127) / 255) << shift;
            }
            fb.blend_pixel(px, py, color, outer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framebuffer::ScreenInfo;
    use alloc::vec;

    #[test]
    fn arrow_has_an_outline_a_tail_and_a_tip_at_the_hotspot() {
        assert!(MASK[0][0].0 > 0);
        assert_eq!(MASK[5][0], (255, 0));
        assert_eq!(MASK[8][3], (255, 255));
        assert!(MASK[22].iter().any(|&(a, _)| a != 0));
        assert_eq!(MASK[23][17], (0, 0));
    }

    #[test]
    fn clipped_paint_matches_full_paint_and_respects_stride_padding() {
        let (width, height, stride) = (36, 34, 39);
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
        let mut expected = vec![0x797979; stride * height];
        let mut actual = expected.clone();
        let full = Framebuffer::new(
            1,
            expected.as_mut_ptr() as usize,
            expected.len() * 4,
            screen,
        )
        .ok()
        .unwrap();
        let tiled = Framebuffer::new(1, actual.as_mut_ptr() as usize, actual.len() * 4, screen)
            .ok()
            .unwrap();
        draw(
            &full,
            5,
            4,
            0xffffff,
            0x171717,
            Rect::new(0, 0, width as i32, height as i32),
        );
        for y in 0..height {
            for x in 0..width {
                draw(
                    &tiled,
                    5,
                    4,
                    0xffffff,
                    0x171717,
                    Rect::new(x as i32, y as i32, 1, 1),
                );
            }
        }
        assert_eq!(actual, expected);
        for row in actual.chunks(stride) {
            assert!(row[width..].iter().all(|&p| p == 0x797979));
        }
        draw(&tiled, -9, -9, 0xffffff, 0x171717, Rect::new(0, 0, 1, 1));
        for row in actual.chunks(stride) {
            assert!(row[width..].iter().all(|&p| p == 0x797979));
        }
    }
}
