use super::*;
use compositor::Compositor;
use input::InputEvent;

#[test]
fn rgb24_rows_match_pixel_oracle_across_formats_padding_and_truncation() {
    let source: Vec<u8> = (0..51).map(|i| (i * 97 + 13) as u8).collect();
    for format in [(16, 8, 8, 8, 0, 8), (0, 8, 8, 8, 16, 8), (11, 5, 5, 6, 0, 5)] {
        for padding in [0, 3] {
            let stride = 19 + padding;
            let screen = framebuffer::ScreenInfo { width: 19, height: 4, stride_bytes: stride * 4,
                bits_per_pixel: 32, red_position: format.0, red_size: format.1,
                green_position: format.2, green_size: format.3, blue_position: format.4, blue_size: format.5 };
            for mapped in [stride * 4, stride * 8 - 1, stride * 16] {
                for (x, y) in [(0, 0), (2, 1), (-1, 1), (3, 1), (0, -1), (0, 4)] {
                    let mut actual = vec![0xabcdef12u32; stride * 4 + 8]; let mut expected = actual.clone();
                    let fb = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word + 16, mapped, screen).ok().unwrap();
                    let old = framebuffer::Framebuffer::new(1, expected.as_mut_ptr() as Word + 16, mapped, screen).ok().unwrap();
                    if !fb.blit_rgb24_row(x, y, &source) {
                        for (i, rgb) in source.chunks_exact(3).enumerate() { fb.put_pixel(x + i as i32, y, fb.color(rgb[0], rgb[1], rgb[2])); }
                    }
                    for (i, rgb) in source.chunks_exact(3).enumerate() { old.put_pixel(x + i as i32, y, old.color(rgb[0], rgb[1], rgb[2])); }
                    assert_eq!(actual, expected, "format={format:?} padding={padding} mapped={mapped} x={x} y={y}");
                }
            }
        }
    }
}

#[test]
fn pnm_row_path_and_scaled_fallback_match_legacy_pixel_walk() {
    for width in [9usize, 17, 29] {
        let height = 13;
        let pixels: Vec<u8> = (0..width * height * 3).map(|i| (i * 73 + 41) as u8).collect();
        for red in [0, 16] {
            let screen = framebuffer::ScreenInfo { width: 19, height: 15, stride_bytes: 23 * 4, bits_per_pixel: 32,
                red_position: red, red_size: 8, green_position: 8, green_size: 8, blue_position: 16 - red, blue_size: 8 };
            let desktop = framebuffer::Rect::new(1, 2, 17, 12);
            for dirty in [desktop, framebuffer::Rect::new(4, 5, 8, 6), framebuffer::Rect::new(17, 13, 1, 1)] {
                for mapped in [23 * 4 * 15, 23 * 4 * 8 + 13] {
                    let mut actual = vec![0xdeadbeefu32; 23 * 15 + 8]; let mut expected = actual.clone();
                    let fb = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word + 16, mapped, screen).ok().unwrap();
                    let old = framebuffer::Framebuffer::new(1, expected.as_mut_ptr() as Word + 16, mapped, screen).ok().unwrap();
                    compositor::draw_pnm_for_test(&fb, desktop, dirty, width, height, &pixels);
                    for y in dirty.y..dirty.y + dirty.height {
                        for x in dirty.x..dirty.x + dirty.width {
                            let sx = (x - desktop.x) as usize * width / desktop.width as usize;
                            let sy = (y - desktop.y) as usize * height / desktop.height as usize;
                            let i = (sy * width + sx) * 3;
                            old.put_pixel(x, y, old.color(pixels[i], pixels[i + 1], pixels[i + 2]));
                        }
                    }
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn rgb888_translucent_rows_match_all_channel_pairs_and_alpha_values() {
    let mut solid = vec![0u32; 256];
    let mut blit = solid.clone();
    let mut source = solid.clone();
    let mut screen = framebuffer::ScreenInfo {
        width: 256, height: 1, stride_bytes: 1024, bits_per_pixel: 32,
        red_position: 16, red_size: 8, green_position: 8, green_size: 8,
        blue_position: 0, blue_size: 8,
    };
    for (red, blue) in [(16, 0), (0, 16)] {
        screen.red_position = red;
        screen.blue_position = blue;
        for alpha in 1..255u32 {
            for foreground in 0..256u32 {
                let color = 0x1300_0000 | foreground * 0x010101;
                source.fill(color);
                for background in 0..256u32 {
                    let pixel = 0xa500_0000 | (background << 16)
                        | ((255 - background) << 8) | (background ^ 0xa5);
                    solid[background as usize] = pixel;
                    blit[background as usize] = pixel;
                }
                let output = framebuffer::Framebuffer::new(1, solid.as_mut_ptr() as Word, 1024, screen).ok().unwrap();
                output.fill_rect_alpha(0, 0, 256, 1, color, alpha as u8);
                let output = framebuffer::Framebuffer::new(1, blit.as_mut_ptr() as Word, 1024, screen).ok().unwrap();
                output.blit_bgra32_from_alpha(0, 0, 256, 1, source.as_ptr() as Word,
                    1024, 256, 0, 0, alpha as u8);
                for background in 0..256u32 {
                    let mut expected = 0xa500_0000;
                    for (shift, value) in [(16, background), (8, 255 - background), (0, background ^ 0xa5)] {
                        let channel = (foreground * alpha + value * (255 - alpha) + 127) / 255;
                        expected |= channel << shift;
                    }
                    assert_eq!(solid[background as usize], expected);
                    assert_eq!(blit[background as usize], expected);
                }
            }
        }
    }
}

fn setup() -> (Compositor, Vec<u32>) {
    setup_with_vblank(false)
}

#[test]
fn client_only_damage_skips_decoration_rows_and_matches_unpruned_surface() {
    let (mut compositor, mut pixels) = setup();
    let content = framebuffer::Rect::new(84, 110, 400, 300);
    assert!(!compositor.render_surface_for_test(1, content, false));
    compositor.attach_logical_framebuffer_to_process(18, 1, 19).unwrap();
    assert!(compositor.render_surface_for_test(1, content, false));
    for (x, y) in [(80, 80), (-40, -20), (760, 570)] {
        compositor.move_window(18, 1, x, y).unwrap();
        for (width, height) in [(400usize, 300usize), (72, 32), (500, 400)] {
            compositor.resize_viewport(18, 1, width, height).unwrap();
            for opacity in [0, 1, 127, 254, 255] {
                compositor.set_window_opacity(18, 1, opacity).unwrap();
                for dirty in [
                    framebuffer::Rect::new(x + 4, y + 30, width as i32, height as i32),
                    framebuffer::Rect::new(x + 5, y + 31, 30, 20),
                    framebuffer::Rect::new(x, y, width as i32 + 8, height as i32 + 34),
                    framebuffer::Rect::new(x + 3, y + 30, 2, 20),
                    framebuffer::Rect::new(x + 4, y + 30 + height as i32 - 1, width as i32, 2),
                    framebuffer::Rect::new(x + 403, y + 31, 5, 20),
                    framebuffer::Rect::new(x + 4, y + 330, 20, 10),
                    framebuffer::Rect::new(x + 4, y + 30, 0, 0),
                ] {
                    for (i, pixel) in pixels.iter_mut().enumerate() {
                        *pixel = (i as u32).wrapping_mul(0x9e37_79b9) ^ 0xa5c3_817f;
                    }
                    let before = pixels.clone();
                    let skipped = compositor.render_surface_for_test(1, dirty, false);
                    let optimized = pixels.clone();
                    pixels.copy_from_slice(&before);
                    compositor.render_surface_for_test(1, dirty, true);
                    assert_eq!(pixels, optimized, "position={x},{y} size={width},{height} opacity={opacity} dirty={},{},{},{}", dirty.x, dirty.y, dirty.width, dirty.height);
                    if skipped { assert_eq!(optimized, before); }
                }
            }
        }
    }
}

#[test]
fn translucent_image_rows_match_scalar_blending_with_formats_and_prefix_failures() {
    for (rp, rs, gp, gs, bp, bs) in [(16, 8, 8, 8, 0, 8), (11, 5, 5, 6, 0, 5),
        (20, 12, 10, 10, 0, 10), (31, 7, 8, 0, 40, 8), (0, 32, 1, 31, 17, 9)] {
        for (screen_width, ss, ds, sx, dx, width) in [
            (11usize, 11usize, 11usize, 0usize, 0usize, 11usize), (13, 15, 16, 1, 1, 11)] {
            let screen = framebuffer::ScreenInfo { width: screen_width, height: 7, stride_bytes: ds * 4,
                bits_per_pixel: 32, red_position: rp, red_size: rs, green_position: gp,
                green_size: gs, blue_position: bp, blue_size: bs };
            let source: Vec<u32> = (0..ss * 8 + 8)
                .map(|i| (i as u32).wrapping_mul(0x9e3779b9) ^ 0xc379158f).collect();
            for source_bytes in [ss * 4 - 1, ss * 4 * 3 + 3, ss * 4 * 8] {
                for target_bytes in [1, ds * 4 * 3 + 2, ds * 4 * 7] {
                    for (sy, dy, height) in [(0usize, 0usize, 1usize), (1, 2, 4), (0, 0, 7), (1, 2, 7)] {
                        for alpha in 0..=254u8 {
                            let mut actual: Vec<u32> = (0..ds * 7 + 8)
                                .map(|i| (i as u32).wrapping_mul(0x8173b99d) ^ 0xa5c3817f).collect();
                            let mut expected = actual.clone();
                            let output = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word, target_bytes, screen).ok().unwrap();
                            let scalar = framebuffer::Framebuffer::new(1, expected.as_mut_ptr() as Word, target_bytes, screen).ok().unwrap();
                            output.blit_bgra32_from_alpha(dx as i32, dy as i32, width as i32, height as i32,
                                source.as_ptr() as Word, source_bytes, ss, sx, sy, alpha);
                            if alpha != 0 && dy + height <= 7 {
                                for row in 0..height {
                                    let from = (sy + row) * ss + sx;
                                    let to = (dy + row) * ds + dx;
                                    if (from + width) * 4 > source_bytes || (to + width) * 4 > target_bytes { break; }
                                    for x in 0..width {
                                        scalar.blend_pixel((dx + x) as i32, (dy + row) as i32, source[from + x], alpha);
                                    }
                                }
                            }
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn translucent_image_aliases_keep_forward_pixel_read_write_order() {
    let width = 7usize;
    let screen = framebuffer::ScreenInfo { width, height: 1, stride_bytes: width * 4,
        bits_per_pixel: 32, red_position: 16, red_size: 8, green_position: 8,
        green_size: 8, blue_position: 0, blue_size: 8 };
    for (source, destination) in [(0usize, 1usize), (1, 0), (0, 0)] {
        for alpha in [1u8, 127, 254] {
            let mut actual: Vec<u32> = (0..width + 1)
                .map(|i| (i as u32).wrapping_mul(0x8173b99d) ^ 0xa5c3817f).collect();
            let mut expected = actual.clone();
            let base = actual.as_mut_ptr() as Word;
            let fb = framebuffer::Framebuffer::new(1, base + destination * 4, width * 4, screen).ok().unwrap();
            let scalar = framebuffer::Framebuffer::new(1, expected.as_mut_ptr() as Word + destination * 4, width * 4, screen).ok().unwrap();
            fb.blit_bgra32_from_alpha(0, 0, width as i32, 1, base + source * 4, width * 4, width, 0, 0, alpha);
            for pixel in 0..width {
                let foreground = expected[source + pixel];
                scalar.blend_pixel(pixel as i32, 0, foreground, alpha);
            }
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn opaque_row_copy_matches_scalar_prefix_bounds_with_stride_and_offsets() {
    for (screen_width, src_stride, dst_stride, sx, dx, width) in [
        (17usize, 17usize, 17usize, 0usize, 0usize, 17usize),
        (17, 20, 20, 0, 0, 17), (17, 20, 17, 1, 0, 16),
        (17, 17, 20, 0, 1, 16), (1, 1, 1, 0, 0, 1),
    ] {
        let screen = framebuffer::ScreenInfo { width: screen_width, height: 9,
            stride_bytes: dst_stride * 4, bits_per_pixel: 32, red_position: 16,
            red_size: 8, green_position: 8, green_size: 8, blue_position: 0, blue_size: 8 };
        let source: Vec<u32> = (0..src_stride * 11 + 8)
            .map(|i| (i as u32).wrapping_mul(0x9e3779b9)).collect();
        for src_bytes in [0, 1, src_stride * 4 - 1, src_stride * 4 * 3 + 2, src_stride * 4 * 11] {
            for dst_bytes in [1, dst_stride * 4 * 4 - 1, dst_stride * 4 * 9] {
                for sy in [0usize, 2, 10] {
                    for dy in [0usize, 2] {
                        for height in [1usize, 4, 7] {
                            let mut actual = vec![0xc3a5817fu32; dst_stride * 9 + 8];
                            let mut expected = actual.clone();
                            let fb = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word, dst_bytes, screen).ok().unwrap();
                            fb.blit_bgra32_from_alpha(dx as i32, dy as i32, width as i32, height as i32,
                                source.as_ptr() as Word, src_bytes, src_stride, sx, sy, 255);
                            if src_bytes != 0 && dy + height <= 9 {
                                for row in 0..height {
                                    let from = (sy + row) * src_stride + sx;
                                    let to = (dy + row) * dst_stride + dx;
                                    if (from + width) * 4 > src_bytes || (to + width) * 4 > dst_bytes { break; }
                                    expected[to..to + width].copy_from_slice(&source[from..from + width]);
                                }
                            }
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn overlapping_whole_copy_extents_keep_sequential_disjoint_row_semantics() {
    let width = 7usize;
    let screen = framebuffer::ScreenInfo { width, height: 3, stride_bytes: width * 4,
        bits_per_pixel: 32, red_position: 16, red_size: 8, green_position: 8,
        green_size: 8, blue_position: 0, blue_size: 8 };
    for down in [false, true] {
        let mut actual: Vec<u32> = (0..width * 4).map(|i| i as u32 + 100).collect();
        let mut expected = actual.clone();
        let (source, destination) = if down { (0, width) } else { (width, 0) };
        let base = actual.as_mut_ptr() as Word;
        let fb = framebuffer::Framebuffer::new(1, base + destination * 4, width * 3 * 4, screen).ok().unwrap();
        fb.blit_bgra32_from_alpha(0, 0, width as i32, 3, base + source * 4,
            width * 3 * 4, width, 0, 0, 255);
        for row in 0..3 {
            let from = source + row * width;
            expected.copy_within(from..from + width, destination + row * width);
        }
        assert_eq!(actual, expected);
    }
}

#[test]
fn contiguous_solid_fill_matches_complete_row_oracle_with_clipping_and_padding() {
    for stride in [17usize, 20] {
        let screen = framebuffer::ScreenInfo { width: 17, height: 9, stride_bytes: stride * 4,
            bits_per_pixel: 32, red_position: 16, red_size: 8, green_position: 8,
            green_size: 8, blue_position: 0, blue_size: 8 };
        for mapped in [1, 3, 17 * 4 - 1, stride * 4, stride * 4 * 5 + 3,
            stride * 4 * 9 - 1, stride * 4 * 9] {
            for (x, y, width, height) in [(0i32, 0i32, 17i32, 9i32), (-3, -2, 40, 20),
                (0, 2, 17, 4), (1, 2, 16, 4), (0, 0, 8, 9), (17, 0, 4, 9),
                (0, 8, 17, 10), (0, 9, 17, 1), (0, 0, -1, 9),
                (i32::MAX, 0, 20, 2), (0, 0, i32::MAX, i32::MAX)] {
                for color in [0, 0xffffffff, 0x11335577] {
                    let mut actual: Vec<u32> = (0..stride * 9 + 8)
                        .map(|i| (i as u32).wrapping_mul(0x9e3779b9)).collect();
                    let mut expected = actual.clone();
                    let fb = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word, mapped, screen).ok().unwrap();
                    fb.fill_rect(x, y, width, height, color);
                    if width > 0 && height > 0 {
                        let x0 = x.clamp(0, 17) as usize;
                        let x1 = x.saturating_add(width).clamp(0, 17) as usize;
                        if x1 > x0 {
                            for row in y.clamp(0, 9)..y.saturating_add(height).clamp(0, 9) {
                                let offset = row as usize * stride * 4 + x0 * 4;
                                if offset + (x1 - x0) * 4 <= mapped {
                                    expected[offset / 4..offset / 4 + x1 - x0].fill(color);
                                }
                            }
                        }
                    }
                    assert_eq!(actual, expected, "stride={stride} mapped={mapped} rect={x},{y},{width},{height}");
                }
            }
        }
    }
}

#[test]
fn solid_alpha_rows_match_scalar_pixels_with_formats_clipping_and_truncated_buffers() {
    let formats = [(16, 8, 8, 8, 0, 8), (11, 5, 5, 6, 0, 5),
        (20, 12, 10, 10, 0, 10), (31, 7, 8, 0, 40, 8), (0, 32, 1, 31, 17, 9)];
    for (rp, rs, gp, gs, bp, bs) in formats {
        let screen = framebuffer::ScreenInfo { width: 17, height: 9, stride_bytes: 23 * 4,
            bits_per_pixel: 32, red_position: rp, red_size: rs, green_position: gp,
            green_size: gs, blue_position: bp, blue_size: bs };
        for mapped in [1, 3, 4, 23 * 4 + 7, 23 * 9 * 4 - 3, 23 * 9 * 4] {
            for (x, y, width, height) in [(-3i32, -2i32, 12i32, 7i32), (0, 0, 17, 9),
                (16, 8, 5, 3), (18, 3, 4, 5), (5, 4, -1, 3), (i32::MAX, 0, 12, 4),
                (0, 0, i32::MAX, i32::MAX)] {
                for alpha in 0..=255u8 {
                    let mut actual: Vec<u32> = (0..23 * 9 + 8)
                        .map(|i| (i as u32).wrapping_mul(0x9e3779b9) ^ 0xa5c3817f).collect();
                    let mut expected = actual.clone();
                    let output = framebuffer::Framebuffer::new(1, actual.as_mut_ptr() as Word, mapped, screen).ok().unwrap();
                    let scalar = framebuffer::Framebuffer::new(1, expected.as_mut_ptr() as Word, mapped, screen).ok().unwrap();
                    let color = 0x7fe182ab;
                    output.fill_rect_alpha(x, y, width, height, color, alpha);
                    if alpha == 255 {
                        scalar.fill_rect(x, y, width, height, color);
                    } else if width > 0 && height > 0 {
                        for py in y.clamp(0, 9)..y.saturating_add(height).clamp(0, 9) {
                            for px in x.clamp(0, 17)..x.saturating_add(width).clamp(0, 17) {
                                scalar.blend_pixel(px, py, color, alpha);
                            }
                        }
                    }
                    assert_eq!(actual, expected, "mapped={mapped} alpha={alpha} format={rp}/{rs},{gp}/{gs},{bp}/{bs}");
                }
            }
        }
    }
}

#[test]
fn opaque_content_partition_matches_full_composition_through_overlap_and_clipping() {
    let (mut compositor, pixels) = setup();
    let (base, _) = compositor.attach_logical_framebuffer_to_process(18, 1, 19).unwrap();
    for y in 0..300 {
        for x in 0..400 {
            unsafe { (base as *mut u32).add(y * 400 + x)
                .write(0x110000 | ((y as u32) << 8) | (x as u32 & 255)); }
        }
    }
    let second = compositor.create_window(18, 240, 120, 430, 360).unwrap();
    for opacity in [255, 128, 0, 254, 255] {
        compositor.set_window_opacity(18, second, opacity).unwrap();
        for (x, y, width, height) in [
            (240, 120, 430, 360), (-30, -20, 430, 360),
            (760, 570, 430, 360), (100, 90, 120, 100),
        ] {
            compositor.move_window(18, second, x, y).unwrap();
            compositor.resize_viewport(18, second, width, height).unwrap();
            compositor.render_if_needed();
            let partitioned = pixels.clone();
            compositor.render_reference_for_test();
            assert_eq!(pixels, partitioned, "opacity={opacity} x={x} y={y}");
        }
    }
    compositor.set_window_visible(18, second, false).unwrap();
    compositor.render_if_needed();
    let partitioned = pixels.clone();
    compositor.render_reference_for_test();
    assert_eq!(pixels, partitioned);
}

#[test]
fn opaque_window_frame_damage_avoids_background_under_its_content() {
    let (compositor, pixels) = setup();
    let dirty = framebuffer::Rect::new(80, 80, 400, 300);
    let partitioned_area = compositor.render_region_for_test(dirty, false);
    let partitioned = pixels.clone();
    let reference_area = compositor.render_region_for_test(dirty, true);
    assert_eq!(pixels, partitioned);
    assert_eq!(reference_area, 400 * 300);
    assert!(partitioned_area < reference_area / 4);
    println!("opaque frame background area: reference={reference_area} partitioned={partitioned_area}");
}

#[test]
fn opaque_multiple_windows_skip_background_beyond_the_first_partition() {
    let (mut compositor, pixels) = setup();
    compositor.create_window(18, 480, 80, 300, 300).unwrap();
    let dirty = framebuffer::Rect::new(80, 80, 700, 300);
    let (old_area, old_splits) = compositor.render_budget_for_test(dirty, 1);
    assert_eq!(old_splits, 1);
    let old_pixels = pixels.clone();
    let (area, splits) = compositor.render_budget_for_test(dirty, 32);
    assert!(splits > 1 && splits <= 32);
    assert!(pixels == old_pixels);
    let partitioned = pixels.clone();
    let reference = compositor.render_region_for_test(dirty, true);
    assert!(pixels == partitioned);
    assert!(area < old_area / 3, "old={old_area} new={area}");
    assert!(area < reference / 8);
    println!("MULTI-OPAQUE background old={old_area} new={area} reference={reference} splits={splits}");
}

#[test]
fn opaque_partition_budget_fallback_keeps_pixels_through_overlap_opacity_and_clipping() {
    let (mut compositor, mut pixels) = setup();
    let mut windows = Vec::new();
    for index in 0..10 {
        windows.push(compositor.create_window(18, 70 + index*43, 60 + index*29, 330, 240).unwrap());
    }
    for opacity in [255, 128, 0, 254] {
        for &id in windows.iter().step_by(3) { compositor.set_window_opacity(18, id, opacity).unwrap(); }
        for dirty in [framebuffer::Rect::new(0,0,800,600),framebuffer::Rect::new(65,50,630,480),
            framebuffer::Rect::new(-10,-10,100,100)] {
            let initial=pixels.clone();
            let reference_area=compositor.render_region_for_test(dirty,true);
            let expected=pixels.clone();
            for budget in [0,1,2,4,32] {
                pixels.copy_from_slice(&initial);
                let (area,used)=compositor.render_budget_for_test(dirty,budget);
                let mismatch=pixels.iter().zip(&expected).position(|(actual,expected)| actual!=expected);
                assert_eq!(mismatch,None,"opacity={opacity} budget={budget}");
                assert!(used<=budget);
                assert!(area<=reference_area);
            }
        }
    }
}

fn setup_with_vblank(vblank: bool) -> (Compositor, Vec<u32>) {
    let screen = framebuffer::ScreenInfo {
        width: 800,
        height: 600,
        stride_bytes: 3200,
        bits_per_pixel: 32,
        red_position: 16,
        red_size: 8,
        green_position: 8,
        green_size: 8,
        blue_position: 0,
        blue_size: 8,
    };
    let mut pixels = vec![0; screen.width * screen.height];
    let fb =
        framebuffer::Framebuffer::new(1, pixels.as_mut_ptr() as usize, pixels.len() * 4, screen)
            .ok()
            .unwrap()
            .with_present_modes(vblank, false);
    (setup_framebuffer(fb), pixels)
}

fn setup_framebuffer(fb: framebuffer::Framebuffer) -> Compositor {
    let theme = include_bytes!("../../../nanami/servers/apps/honoka/assets/themes/default.theme");
    let fields = [
        "Kernel Version: A9N v0.3.4",
        "Nanami Version: 0.1.0",
        "Architecture: x86_64",
        "Platform: pc99",
    ]
    .map(String::from);
    let mut compositor =
        Compositor::new(fb, font::TextRenderer, 0, 0, 0, 0, theme, fields).unwrap();
    compositor.create_window(18, 80, 80, 400, 300).unwrap();
    compositor.render_if_needed();
    PRESENTED.with(|rects| rects.borrow_mut().clear());
    compositor
}

fn gpu_screen() -> framebuffer::ScreenInfo {
    framebuffer::ScreenInfo {
        width: 800,
        height: 600,
        stride_bytes: 3200,
        bits_per_pixel: 32,
        red_position: 16,
        red_size: 8,
        green_position: 8,
        green_size: 8,
        blue_position: 0,
        blue_size: 8,
    }
}

#[test]
fn gpu_protocol_compositor_matches_cpu_pixels_through_motion_and_drag() {
    gpu_mock::reset();
    gpu_mock::FAKE.with(|fake| fake.borrow_mut().accelerated = gpu::CAP_ALL);
    let fb = framebuffer::Framebuffer::try_gpu(1, gpu_screen())
        .unwrap()
        .unwrap()
        .with_present_modes(true, false);
    let (mut cpu, expected) = setup_with_vblank(true);
    let mut accelerated = setup_framebuffer(fb);
    let compare = || {
        gpu_mock::FAKE.with(|fake| {
            let fake = fake.borrow();
            let pixels = unsafe {
                std::slice::from_raw_parts(expected.as_ptr() as *const u8, expected.len() * 4)
            };
            assert_eq!(&fake.presented[..pixels.len()], pixels);
        })
    };
    compare();
    for event in [
        move_by(-300, -210),
        button(true),
        move_by(40, 18),
        move_by(70, 30),
        button(false),
        move_by(200, 220),
        move_by(-150, -180),
    ] {
        cpu.process_input(event);
        accelerated.process_input(event);
        cpu.render_if_needed();
        accelerated.render_if_needed();
        compare();
    }
    gpu_mock::FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.fills > 0);
        assert!(fake.copies > 0);
        assert!(fake.presents > 1);
        // Drawing a desktop must not turn each text/cursor pixel into IPC.
        assert!(fake.calls.len() < 1000);
    });
}

#[test]
fn software_gpu_backend_uses_cpu_drawing_but_the_same_present_protocol() {
    gpu_mock::reset();
    let fb = framebuffer::Framebuffer::try_gpu(1, gpu_screen())
        .unwrap()
        .unwrap();
    let (_cpu, expected) = setup();
    let _software = setup_framebuffer(fb);
    gpu_mock::FAKE.with(|fake| {
        let fake = fake.borrow();
        let pixels = unsafe {
            std::slice::from_raw_parts(expected.as_ptr() as *const u8, expected.len() * 4)
        };
        assert_eq!(&fake.presented[..pixels.len()], pixels);
        assert_eq!((fake.fills, fake.copies), (0, 0));
        assert!(fake.presents > 0);
    });
}

#[test]
fn gpu_timeout_prevents_cpu_reuse_of_both_copy_surfaces() {
    gpu_mock::reset();
    gpu_mock::FAKE.with(|fake| fake.borrow_mut().accelerated = gpu::CAP_ALL);
    let destination = framebuffer::Framebuffer::try_gpu(1, gpu_screen())
        .unwrap()
        .unwrap();
    let source = destination.new_gpu_canvas().unwrap();
    gpu_mock::FAKE.with(|fake| fake.borrow_mut().fail_work = true);
    let rectangle = framebuffer::Rect::new(0, 0, 800, 600);
    destination.copy_rect_from(&source, rectangle);
    assert!(!destination.healthy() && !source.healthy());
    for buffer in [&source, &destination] {
        buffer.fill_rect(0, 0, 1, 1, 0xff00ff);
        buffer.put_pixel(0, 0, 0xff00ff);
        assert_eq!(unsafe { (buffer.vaddr() as *const u32).read() }, 0);
        assert!(buffer.present(&[rectangle]).is_err());
    }
    drop(source);
    drop(destination);
    gpu_mock::FAKE.with(|fake| {
        assert!(!fake
            .borrow()
            .calls
            .iter()
            .any(|call| call.0 == gpu::DESTROY))
    });
}

fn move_by(dx: i32, dy: i32) -> InputEvent {
    InputEvent::MouseMove { dx, dy }
}
fn button(pressed: bool) -> InputEvent {
    InputEvent::MouseButton { code: 1, pressed }
}

#[test]
fn native_scanout_presents_once_per_composited_frame_without_repainting_sparse_gaps() {
    let (mut fallback, fallback_pixels) = setup();
    fallback.process_input(move_by(-550, 180));
    fallback.render_if_needed();
    let sparse = PRESENTED.with(|rects| std::mem::take(&mut *rects.borrow_mut()));
    assert!(sparse.len() > 1);
    let (mut native, native_pixels) = setup_with_vblank(true);
    native.process_input(move_by(-550, 180));
    native.render_if_needed();
    assert_eq!(native_pixels, fallback_pixels);
    PRESENTED.with(|rects| {
        let rects = rects.borrow();
        assert_eq!(rects.len(), 1);
        let merged = rects[0];
        for rect in sparse {
            assert!(merged.x <= rect.x && merged.y <= rect.y);
            assert!(merged.x + merged.width >= rect.x + rect.width);
            assert!(merged.y + merged.height >= rect.y + rect.height);
        }
    });
    native.render_if_needed();
    PRESENTED.with(|rects| assert_eq!(rects.borrow().len(), 1));
}

#[test]
fn software_scanout_batches_disjoint_motion_damage_into_one_request() {
    let event = || move_by(-550, 180);
    let (mut fallback, fallback_pixels) = setup();
    fallback.process_input(event());
    fallback.render_if_needed();
    let sparse = PRESENTED.with(|rects| std::mem::take(&mut *rects.borrow_mut()));
    assert_eq!(sparse.len(), 2);

    gpu_mock::reset();
    gpu_mock::FAKE.with(|fake| fake.borrow_mut().batch_supported = true);
    let fb = framebuffer::Framebuffer::try_gpu(1, gpu_screen())
        .unwrap()
        .unwrap()
        .with_present_modes(false, true);
    let mut software = setup_framebuffer(fb);
    gpu_mock::FAKE.with(|fake| fake.borrow_mut().calls.clear());
    software.process_input(event());
    software.render_if_needed();
    gpu_mock::FAKE.with(|fake| {
        let fake = fake.borrow();
        assert_eq!(fake.calls.len(), 1);
        assert_eq!(fake.calls[0].0, gpu::PRESENT_BATCH);
        let expected: Vec<_> = sparse.iter().map(|rect| gpu::Rect {
            x: rect.x as Word,
            y: rect.y as Word,
            width: rect.width as Word,
            height: rect.height as Word,
        }).collect();
        assert_eq!(fake.batches, [expected]);
        assert!(fake.presented.chunks_exact(4).zip(fallback_pixels)
            .all(|(bytes, pixel)| bytes == pixel.to_ne_bytes()));
    });
}

#[test]
fn resize_gesture_proposes_dimensions_before_surface_commit() {
    let (mut compositor, _pixels) = setup();
    let (base, _) = compositor.attach_input_queue(18, 1).unwrap();
    let mut events = honoka_api::WindowEventQueue::new(base);
    while events.pop().is_some() {}
    // Stay inside the rounded corner, rather than the transparent outer pixel.
    compositor.process_input(move_by(-120, 106));
    compositor.process_input(button(true));
    compositor.process_input(move_by(100, 80));
    compositor.process_input(button(false));
    assert_eq!(compositor.window_content_size(18, 1).unwrap(), (400, 300));
    let event = events.pop().expect("resize notification");
    assert_eq!(
        input::unpack_input_event(event).0,
        input::INPUT_EVENT_KIND_WINDOW_RESIZE
    );
    assert_eq!(input::unpack_input_event(event).2, 500);
    assert_eq!(input::unpack_input_event(event).3, 380);
}

#[test]
fn native_resize_checks_owner_size_and_allocation_before_commit() {
    let (mut compositor, _pixels) = setup();
    let (old, old_bytes) = compositor.attach_logical_framebuffer(18, 1).unwrap();
    assert!(compositor.resize_surface(19, 1, 500, 400).is_err());
    assert!(compositor.resize_surface(18, 1, usize::MAX, 400).is_err());
    assert!(compositor.resize_surface(18, 1, 71, 32).is_err());
    FAIL_ALLOCATION.with(|fail| fail.set(true));
    assert!(compositor.resize_surface(18, 1, 500, 400).is_err());
    assert_eq!(compositor.window_content_size(18, 1).unwrap(), (400, 300));
    let (new, bytes) = compositor.resize_surface(18, 1, 72, 32).unwrap();
    assert_ne!(new, old);
    assert_eq!(bytes, 72 * 32 * 4 + honoka_api::HONOKA_DAMAGE_QUEUE_BYTES);
    assert_eq!(compositor.window_content_size(18, 1).unwrap(), (72, 32));
    RELEASED.with(|released| assert!(released.borrow().contains(&(old, old_bytes))));
    compositor.render_if_needed();
}

#[test]
fn full_input_ring_cannot_lose_latest_resize() {
    let (mut compositor, _pixels) = setup();
    let (base, _) = compositor.attach_input_queue(18, 1).unwrap();
    let mut input = input::InputEventQueue::new(base);
    for _ in 0..input::INPUT_EVENT_QUEUE_CAPACITY + 1 {
        input.push_with_event_kind(1, 1);
    }
    honoka_api::publish_resize(base, 500, 400);
    honoka_api::publish_resize(base, 600, 450);
    let mut events = honoka_api::WindowEventQueue::new(base);
    assert!(!events.is_empty());
    let event = events.pop().unwrap();
    assert_eq!(
        input::unpack_input_event(event).0,
        input::INPUT_EVENT_KIND_WINDOW_RESIZE
    );
    assert_eq!(
        (
            input::unpack_input_event(event).2,
            input::unpack_input_event(event).3
        ),
        (600, 450)
    );
    assert_ne!(
        input::unpack_input_event(events.pop().unwrap()).0,
        input::INPUT_EVENT_KIND_WINDOW_RESIZE
    );
}

#[test]
fn guest_viewport_resize_preserves_mapping_and_source_stride() {
    let (mut compositor, pixels) = setup();
    let (base, _) = compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    for y in 0..300 {
        for x in 0..400 {
            unsafe {
                (base as *mut u32)
                    .add(y * 400 + x)
                    .write(0x400000 | (y as u32) << 8 | (x as u32 & 255));
            }
        }
    }
    for (width, height) in [(150, 100), (500, 400), (400, 300)] {
        compositor.resize_viewport(18, 1, width, height).unwrap();
        compositor.render_if_needed();
        for y in 4..height.min(296) {
            for x in 4..width.min(396) {
                assert_eq!(
                    pixels[(80 + constants::TITLE_BAR_HEIGHT as usize + y) * 800 + 84 + x],
                    0x400000 | (y as u32) << 8 | (x as u32 & 255)
                );
            }
        }
    }
}

#[test]
fn guest_remap_keeps_mode_after_viewport_resize() {
    let (mut compositor, _pixels) = setup();
    let (_, bytes) = compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    compositor.resize_viewport(18, 1, 200, 100).unwrap();
    compositor.detach_logical_framebuffer(18, 1).unwrap();
    let (_, remapped_bytes) = compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    assert_eq!(remapped_bytes, bytes);
    assert_eq!(remapped_bytes, 400 * 300 * 4);
    assert_eq!(compositor.window_content_size(18, 1).unwrap(), (200, 100));
}

#[test]
fn resized_guest_pointer_uses_the_cropped_backing_extent_and_returns_without_drift() {
    let (mut compositor, _) = setup();
    compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    let (base, _) = compositor.attach_input_queue(18, 1).unwrap();
    let mut events = honoka_api::WindowEventQueue::new(base);
    // The guest backing remains 400x300 even in a larger viewport.
    compositor.resize_viewport(18, 1, 600, 450).unwrap();
    for (dx, dy, x, y) in [
        (0, 150, 399, 299),
        (-500, -330, 16, 10),
        (290, 260, 306, 270),
    ] {
        compositor.process_input(move_by(dx, dy));
        let mut last = None;
        while let Some(packed) = events.pop() {
            let event = input::unpack_input_event(packed);
            if event.0 == input::INPUT_EVENT_KIND_MOUSE_MOVE {
                last = Some((event.2, event.3));
            }
        }
        assert_eq!(last, Some((x, y)));
    }
}

#[test]
fn parallel_fbdev_tile_columns_merge_to_two_exact_strips() {
    let (mut compositor, pixels) = setup();
    let (source, _) = compositor.attach_logical_framebuffer_to_process(18, 1, 19).unwrap();
    compositor.render_if_needed();
    PRESENTED.with(|rects| rects.borrow_mut().clear());
    for row in (0..300).step_by(16) {
        for column in [1, 129] {
            unsafe { (source as *mut u32).add(row * 400 + column).write(0x123456); }
        }
    }
    compositor.invalidate_presented_logical_framebuffer(1);
    assert_eq!(compositor.pending_damage_for_test().len(), 2);
    compositor.render_if_needed();
    PRESENTED.with(|rects| {
        let rects = rects.borrow();
        assert_eq!(rects.len(), 2);
        for rect in rects.iter() {
            assert!(rect.x == 84 || rect.x == 84 + 128);
            assert_eq!((rect.y, rect.width, rect.height), (110, 32, 300));
        }
        assert_eq!(rects.iter().map(|r| r.width * r.height).sum::<i32>(), 2 * 32 * 300);
    });
    let actual = pixels.clone();
    compositor.render_reference_for_test();
    assert_eq!(pixels, actual);
}

#[test]
fn damage_strip_bridge_rechecks_earlier_entries_without_joining_l_shapes_or_gaps() {
    use framebuffer::Rect;
    let (mut compositor, _) = setup();
    for x in [0, 32, 16] {
        compositor.queue_damage_for_test(Rect::new(x, 0, 16, 16), false);
    }
    let damage = compositor.pending_damage_for_test();
    assert_eq!(damage.len(), 1);
    assert_eq!((damage[0].x, damage[0].y, damage[0].width, damage[0].height), (0, 0, 48, 16));
    compositor.queue_damage_for_test(Rect::new(0, 16, 16, 16), false);
    compositor.queue_damage_for_test(Rect::new(64, 0, 16, 16), false);
    assert_eq!(compositor.pending_damage_for_test().len(), 3);
}

#[test]
fn fbdev_capture_skips_idle_frames_and_presents_only_changed_tiles() {
    let (mut compositor, pixels) = setup();
    let (source, _) = compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    compositor.render_if_needed();
    PRESENTED.with(|rects| rects.borrow_mut().clear());
    compositor.invalidate_presented_logical_framebuffer(1);
    compositor.render_if_needed();
    PRESENTED.with(|rects| assert!(rects.borrow().is_empty()));
    unsafe {
        (source as *mut u32).add(50 * 400 + 73).write(0x123456);
    }
    compositor.invalidate_presented_logical_framebuffer(1);
    compositor.render_if_needed();
    assert_eq!(
        pixels[(80 + constants::TITLE_BAR_HEIGHT as usize + 50) * 800 + 84 + 73],
        0x123456
    );
    PRESENTED.with(|rects| {
        let rects = rects.borrow();
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].width * rects[0].height, 32 * 16);
    });
    compositor.detach_logical_framebuffer(18, 1).unwrap();
    RELEASED.with(|released| {
        assert_eq!(
            released
                .borrow()
                .iter()
                .filter(|(_, bytes)| *bytes == 400 * 300 * 4)
                .count(),
            2
        )
    });
}

#[test]
fn fbdev_capture_allocation_failure_keeps_the_existing_full_redraw_path() {
    let (mut compositor, pixels) = setup();
    FAIL_CAPTURE.with(|fail| fail.set(true));
    let (source, _) = compositor
        .attach_logical_framebuffer_to_process(18, 1, 19)
        .unwrap();
    unsafe {
        (source as *mut u32).add(50 * 400 + 73).write(0x654321);
    }
    compositor.invalidate_presented_logical_framebuffer(1);
    compositor.render_if_needed();
    assert_eq!(
        pixels[(80 + constants::TITLE_BAR_HEIGHT as usize + 50) * 800 + 84 + 73],
        0x654321
    );
    compositor.detach_logical_framebuffer(18, 1).unwrap();
}

#[test]
fn coalesced_present_notifications_update_both_surfaces_not_an_unrelated_window() {
    let (mut compositor, pixels) = setup();
    let second = compositor.create_window(18, 510, 80, 200, 100).unwrap();
    let third = compositor.create_window(18, 20, 400, 160, 100).unwrap();
    let surfaces = [(1, 400), (second, 200), (third, 160)].map(|(id, stride)| {
        let (source, _) = compositor
            .attach_logical_framebuffer_to_process(18, id, 19)
            .unwrap();
        (source, stride)
    });
    compositor.render_if_needed();
    let unnotified_pixel = pixels[435 * 800 + 29];
    for (index, &(source, stride)) in surfaces.iter().enumerate() {
        unsafe {
            (source as *mut u32)
                .add(5 * stride + 5)
                .write(0x123450 + index as u32);
        }
    }
    // Model A9N's OR coalescing, including clients that mint a low-32-bit ID.
    let identifier = honoka_api::HONOKA_NOTIFICATION_PRESENT | 1 | (second & 0xffff_ffff);
    compositor.invalidate_presented_logical_framebuffer(identifier & 0xffff_ffff);
    compositor.render_if_needed();
    assert_eq!(pixels[115 * 800 + 89], 0x123450, "first surface lost");
    assert_eq!(pixels[115 * 800 + 519], 0x123451, "second surface lost");
    assert_eq!(
        pixels[435 * 800 + 29], unnotified_pixel,
        "unnotified surface was captured"
    );
    // Full-handle callers and unrelated high category bits retain the same hint.
    compositor.invalidate_presented_logical_framebuffer(
        honoka_api::HONOKA_NOTIFICATION_PRESENT | honoka_api::HONOKA_NOTIFICATION_INPUT | third,
    );
    compositor.render_if_needed();
    assert_eq!(pixels[435 * 800 + 29], 0x123452);
}

#[test]
fn a_reused_present_hint_does_not_make_a_stale_control_handle_valid() {
    let (mut compositor, _pixels) = setup();
    compositor.destroy_window(18, 1).unwrap();
    let replacement = compositor.create_window(18, 80, 80, 400, 300).unwrap();
    assert_ne!(replacement, 1);
    assert!(compositor.window_content_size(18, 1).is_err());
    assert!(compositor.destroy_window(18, 1).is_err());
    assert!(compositor.window_content_size(19, replacement).is_err());
    assert_eq!(
        compositor.window_content_size(18, replacement).unwrap(),
        (400, 300)
    );
    // Late notifications are harmless hints for the current window in that bit.
    compositor.invalidate_presented_logical_framebuffer(1);
    assert!(compositor.window_content_size(18, replacement).is_ok());
}

fn compare(events: &[InputEvent], drag: bool) -> Vec<framebuffer::Rect> {
    let (mut batched, pixels) = setup();
    let (mut reference, expected) = setup();
    for compositor in [&mut batched, &mut reference] {
        if drag {
            compositor.process_input(move_by(-300, -210)); // title at (300,90)
            compositor.process_input(button(true));
            compositor.render_if_needed();
        }
    }
    for &event in events {
        reference.process_input(event);
        reference.render_if_needed();
    }
    PRESENTED.with(|rects| rects.borrow_mut().clear());
    for &event in events {
        batched.process_input(event);
    }
    assert!(batched.has_pending_render());
    batched.render_if_needed();
    assert!(!batched.has_pending_render());
    assert_eq!(
        pixels, expected,
        "batched motion differs from drawing each position"
    );
    PRESENTED.with(|rects| std::mem::take(&mut *rects.borrow_mut()))
}

#[test]
fn cursor_burst_draws_two_regions_with_pixel_exact_results() {
    let moves: Vec<_> = (0..64).map(|_| move_by(-3, 1)).collect();
    let damage = compare(&moves, false);
    assert_eq!(damage.len(), 2);
    assert_eq!(
        damage.iter().map(|r| r.width * r.height).sum::<i32>(),
        2 * (constants::CURSOR_WIDTH + 2) * (constants::CURSOR_HEIGHT + 2)
    );
}

#[test]
fn cursor_reversal_and_clipping_keep_sequential_positions() {
    compare(
        &[
            move_by(1000, 1000),
            move_by(-150, -160),
            move_by(-1000, -1000),
            move_by(80, 90),
        ],
        false,
    );
}

#[test]
fn dragged_outline_stays_sparse_instead_of_becoming_fullscreen_damage() {
    let moves: Vec<_> = (0..64).map(|_| move_by(2, 1)).collect();
    let damage = compare(&moves, true);
    assert!(damage.len() <= 10);
    assert!(damage.iter().map(|r| r.width * r.height).sum::<i32>() < 30_000);
}

#[test]
fn drag_release_erases_first_outline_and_places_window_at_final_position() {
    let mut events: Vec<_> = (0..32).map(|_| move_by(2, 1)).collect();
    events.push(button(false));
    events.push(move_by(100, 80));
    compare(&events, true);
}

#[test]
fn click_between_moves_preserves_transition_damage() {
    compare(
        &[
            move_by(-300, -150),
            button(true),
            button(false),
            move_by(70, 30),
        ],
        false,
    );
}

#[test]
fn repeated_clock_damage_stays_sparse_and_keeps_sequential_pixels() {
    let (mut batched, pixels) = setup();
    let (mut reference, expected) = setup();
    for second in 0..100 {
        batched.process_input(move_by(-2, 1));
        reference.process_input(move_by(-2, 1));
        batched.set_clock(1, 2, second);
        reference.set_clock(1, 2, second);
        reference.render_if_needed();
    }
    PRESENTED.with(|rects| rects.borrow_mut().clear());
    batched.render_if_needed();
    assert_eq!(pixels, expected);
    assert!(!batched.has_pending_render());
    PRESENTED.with(|rects| {
        let rects = rects.borrow();
        assert_eq!(rects.len(), 3); // One clock update and two cursor endpoints.
        assert!(rects.iter().map(|r| r.width * r.height).sum::<i32>() < 4000);
    });
    batched.render_if_needed();
    PRESENTED.with(|rects| assert_eq!(rects.borrow().len(), 3));
}

#[test]
fn contained_damage_preserves_exact_union_without_filling_sparse_gaps() {
    use framebuffer::Rect;
    let (mut compositor, _) = setup();
    let mut expected = vec![false; 80 * 60];
    let mut seed = 0x129345u64;
    for step in 0..128 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let rect = if step % 17 == 0 {
            Rect::new(-5, -3, 60, 40)
        } else {
            Rect::new((seed % 90) as i32 - 10, ((seed >> 16) % 70) as i32 - 10,
                ((seed >> 32) % 35) as i32, ((seed >> 48) % 25) as i32)
        };
        compositor.queue_damage_for_test(rect, false);
        for y in 0..60 {
            for x in 0..80 {
                let inside = |r: Rect| x >= r.x && y >= r.y
                    && x < r.x + r.width && y < r.y + r.height;
                expected[(y * 80 + x) as usize] |= inside(rect);
                assert_eq!(compositor.pending_damage_for_test().iter().any(|r| inside(*r)),
                    expected[(y * 80 + x) as usize], "step={step} x={x} y={y}");
            }
        }
    }
    // A superset replaces the entire contained prefix; further repeats cannot
    // overflow the queue into a full-screen repaint.
    let outer = Rect::new(0, 0, 128, 128);
    for _ in 0..1000 {
        compositor.queue_damage_for_test(outer, false);
        compositor.queue_damage_for_test(Rect::new(4, 5, 8, 9), false);
    }
    assert_eq!(compositor.pending_damage_for_test().len(), 1);
    let rect = compositor.pending_damage_for_test()[0];
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (0, 0, 128, 128));
}

#[test]
fn damage_dedup_matches_original_queue_with_translucent_windows_and_scanout_modes() {
    use framebuffer::Rect;
    for merged in [false, true] {
        for opacity in [1, 127, 254, 255] {
            let (mut optimized, pixels) = setup_with_vblank(merged);
            let (mut reference, expected) = setup_with_vblank(merged);
            optimized.set_window_opacity(18, 1, opacity).unwrap();
            reference.set_window_opacity(18, 1, opacity).unwrap();
            optimized.render_if_needed();
            reference.render_if_needed();
            // The same final window state, painted through independent queues.
            for index in 0..300 {
                let rect = match index % 5 {
                    0 => Rect::new(70, 70, 430, 330),
                    1 => Rect::new(100, 100, 100, 100),
                    2 => Rect::new(550, 30, 80, 40),
                    3 => Rect::new(-10, -10, 30, 30),
                    _ => Rect::new(80, 80, 1, 1),
                };
                optimized.queue_damage_for_test(rect, false);
                reference.queue_damage_for_test(rect, true);
            }
            PRESENTED.with(|rects| rects.borrow_mut().clear());
            optimized.render_if_needed();
            PRESENTED.with(|rects| assert_eq!(rects.borrow().len(), if merged { 1 } else { 3 }));
            reference.render_if_needed();
            assert_eq!(pixels, expected, "merged={merged} opacity={opacity}");
            assert!(!optimized.has_pending_render());
        }
    }
}
