#![allow(dead_code)]
mod framebuffer {
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub struct Rect {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
    }
    impl Rect {
        pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
            Self {
                x,
                y,
                width,
                height,
            }
        }
    }
}
use framebuffer::Rect;
#[path = "../../nanami/servers/apps/honoka/src/app/fbdev_damage.rs"]
mod damage;
#[path = "fbdev-reference.rs"]
mod reference;

fn capture(
    source: &[u32],
    captured: &mut [u32],
    stride: usize,
    width: usize,
    height: usize,
) -> Vec<Rect> {
    assert!(
        stride >= width && source.len() >= stride * height && captured.len() >= stride * height
    );
    let mut changed = Vec::new();
    unsafe {
        damage::capture(
            source.as_ptr() as usize,
            captured.as_mut_ptr() as usize,
            stride,
            width,
            height,
            |r| changed.push(r),
        );
    }
    changed
}
#[test]
fn an_idle_fbdev_surface_produces_no_redraws() {
    let source = vec![0x12345678; 800 * 600];
    let mut previous = source.clone();
    for _ in 0..60 {
        assert!(capture(&source, &mut previous, 800, 800, 600).is_empty());
    }
}
#[test]
fn one_pixel_damages_one_tile_and_capture_is_stable_after_producer_changes() {
    let mut source = vec![0; 800 * 600];
    let mut previous = source.clone();
    source[123 * 800 + 73] = 0x123456;
    assert_eq!(
        capture(&source, &mut previous, 800, 800, 600),
        [Rect::new(64, 112, 32, 16)]
    );
    assert_eq!(previous, source);
    source[123 * 800 + 73] = 0;
    assert_eq!(previous[123 * 800 + 73], 0x123456);
    assert_eq!(
        capture(&source, &mut previous, 800, 800, 600),
        [Rect::new(64, 112, 32, 16)]
    );
    assert_eq!(previous, source);
}
#[test]
fn adjacent_tiles_merge_but_distant_damage_stays_sparse() {
    let mut source = vec![0; 160 * 48];
    let mut previous = source.clone();
    source[1] = 1;
    source[33] = 2;
    source[129] = 3;
    assert_eq!(
        capture(&source, &mut previous, 160, 160, 48),
        [Rect::new(0, 0, 64, 16), Rect::new(128, 0, 32, 16)]
    );
}

#[test]
fn full_motion_merges_into_one_present_instead_of_one_ipc_per_strip() {
    let source = vec![0x12345678; 800 * 600];
    let mut previous = vec![0; source.len()];
    assert_eq!(
        capture(&source, &mut previous, 800, 800, 600),
        [Rect::new(0, 0, 800, 600)]
    );
    assert_eq!(previous, source);
}
#[test]
fn odd_dimensions_padding_and_partial_tiles_never_overwrite_neighbors() {
    for width in [1, 3, 31, 32, 33, 65] {
        for height in [1, 15, 16, 17] {
            let stride = width + 2;
            let mut source = vec![99; stride * height + 8];
            let mut previous = vec![0; source.len()];
            previous[stride * height..].fill(0xabcdef);
            let changed = capture(&source, &mut previous, stride, width, height);
            let area: i32 = changed.iter().map(|r| r.width * r.height).sum();
            assert_eq!(area as usize, width * height);
            for y in 0..height {
                assert_eq!(
                    &previous[y * stride..y * stride + width],
                    &source[y * stride..y * stride + width]
                );
                assert_eq!(&previous[y * stride + width..(y + 1) * stride], &[0, 0]);
            }
            assert_eq!(&previous[stride * height..], &[0xabcdef; 8]);
            source.fill(0);
        }
    }
}

#[test]
fn batched_capture_matches_reference_for_alignment_tail_padding_and_every_pixel() {
    for (width, height, stride) in [(800, 600, 800), (33, 17, 35), (19, 21, 22)] {
        for offset in 0..2 {
            let mut source = vec![0u32; stride * height + 8];
            let mut expected = source.clone();
            let mut actual = source.clone();
            for frame in 0..20 {
                for i in 0..width * height {
                    if (i + frame * 37) % 13 == 0 {
                        source[offset + i / width * stride + i % width] ^= 0xfedc_ba98;
                    }
                }
                let mut before = Vec::new();
                let mut after = Vec::new();
                unsafe {
                    reference::capture(
                        source.as_ptr().add(offset) as usize,
                        expected.as_mut_ptr().add(offset) as usize,
                        stride,
                        width,
                        height,
                        |r| before.push(r),
                    );
                    damage::capture(
                        source.as_ptr().add(offset) as usize,
                        actual.as_mut_ptr().add(offset) as usize,
                        stride,
                        width,
                        height,
                        |r| after.push(r),
                    );
                }
                assert_eq!(actual, expected);
                assert_eq!(after, before);
            }
        }
    }
}
#[test]
#[ignore = "host-only timing, not guest FPS"]
fn benchmark_idle_and_sparse_fbdev_capture() {
    use std::{hint::black_box, time::Instant};
    let mut source = vec![0u32; 800 * 600];
    let mut captured = source.clone();
    let start = Instant::now();
    let mut pixels = 0;
    for frame in 0..600 {
        source[frame % 600 * 800 + frame % 800] ^= 1;
        unsafe {
            damage::capture(
                black_box(source.as_ptr() as usize),
                black_box(captured.as_mut_ptr() as usize),
                800,
                800,
                600,
                |r| pixels += r.width as usize * r.height as usize,
            );
        }
    }
    println!(
        "fbdev 600 sparse frames: capture={:?} redrawn-pixels={} old-full-redraw-pixels={}",
        start.elapsed(),
        pixels,
        600 * 800 * 600
    );
}
