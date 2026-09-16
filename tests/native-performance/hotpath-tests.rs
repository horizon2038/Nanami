#![allow(dead_code)]

extern crate alloc;
extern crate self as libnanami;
extern crate self as nanami_services;

use std::collections::VecDeque;
use std::hint::black_box;

pub type Word = usize;
#[derive(Debug)]
pub struct RequestError;
#[derive(Debug)]
pub struct NanamiError;
impl NanamiError {
    pub const INVALID_ARGUMENT: Self = Self;
}
pub mod constants {
    pub const CURSOR_SIZE: i32 = 16;
}
pub mod gfx {
    pub fn display_service_present(
        _: usize,
        _: usize,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(), super::RequestError> {
        panic!("an offscreen cache must never present");
    }
}

// Same dimensions as the native shell and terminal service.
const COLS: usize = 712 / 8;
const MAX_ROWS: usize = 128;
const RING_BYTES: usize = 4096;

#[path = "../../nanami/servers/apps/honoka/src/app/background.rs"]
mod background;
#[path = "../../nanami/servers/apps/honoka/src/app/framebuffer.rs"]
mod framebuffer;
#[path = "../../nanami/servers/apps/honoka/src/app/motion_damage.rs"]
mod motion_damage;
#[path = "motion-tests.rs"]
mod motion_tests;
#[path = "../../nanami/servers/apps/terminal-service/src/ring.rs"]
mod ring;
#[path = "../../nanami/servers/apps/shell/src/scrollback.rs"]
mod scrollback;

#[path = "baseline-ring.rs"]
mod baseline_ring;

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    *seed >> 16
}

#[test]
fn scrollback_matches_deque_through_wrap_pop_clear_and_replace() {
    let mut rows = scrollback::Scrollback::new();
    let mut expected = VecDeque::new();
    let mut seed = 7;
    let mut evictions = 0;
    for step in 0..20000 {
        let action = next(&mut seed) % 1000;
        let text = [step as u8; COLS];
        let colors = [step as u32 * 173; COLS];
        match action {
            0 => {
                rows.clear();
                expected.clear();
            }
            1..=150 => {
                rows.pop();
                expected.pop_back();
            }
            151..=300 if !expected.is_empty() => {
                let index = next(&mut seed) as usize % expected.len();
                rows.replace(index, text, colors);
                expected[index] = (text, colors);
            }
            _ => {
                let full = expected.len() == MAX_ROWS;
                if full {
                    expected.pop_front();
                    evictions += 1;
                }
                expected.push_back((text, colors));
                assert_eq!(rows.push(text, colors), full);
            }
        }
        assert_eq!(rows.len(), expected.len());
        for (index, (text, colors)) in expected.iter().enumerate() {
            assert_eq!(rows.line(index), text);
            assert_eq!(rows.colors(index), colors);
        }
    }
    assert!(evictions > MAX_ROWS * 10);
}

#[test]
fn terminal_bulk_io_matches_deque_with_partial_transfers() {
    let mut ring = ring::ByteRing::EMPTY;
    let mut expected = VecDeque::new();
    let mut seed = 17;
    for step in 0..20000 {
        let action = next(&mut seed) % 1000;
        let count = next(&mut seed) as usize % (RING_BYTES * 2);
        match action {
            0 => {
                ring.clear();
                expected.clear();
            }
            1..=100 => {
                let accepted = expected.len() < RING_BYTES;
                assert_eq!(ring.push(step as u8), accepted);
                if accepted {
                    expected.push_back(step as u8);
                }
            }
            101..=550 => {
                let source: Vec<_> = (0..count).map(|i| (i + step) as u8).collect();
                let done = unsafe { ring.write_from(source.as_ptr(), count) };
                assert_eq!(done, count.min(RING_BYTES - expected.len()));
                expected.extend(source[..done].iter().copied());
            }
            _ => {
                let mut destination = vec![0xa5; count + 2];
                let done = unsafe { ring.read_into(destination.as_mut_ptr().add(1), count) };
                assert_eq!(done, count.min(expected.len()));
                for &actual in &destination[1..1 + done] {
                    assert_eq!(Some(actual), expected.pop_front());
                }
                assert_eq!(destination[0], 0xa5);
                assert!(destination[1 + done..].iter().all(|x| *x == 0xa5));
            }
        }
        assert_eq!(ring.len, expected.len());
    }
}

#[test]
fn terminal_full_empty_and_wrapped_bulk_io() {
    let mut ring = ring::ByteRing::EMPTY;
    let source: Vec<_> = (0..RING_BYTES).map(|i| i as u8).collect();
    let mut result = vec![0; RING_BYTES];
    unsafe {
        assert_eq!(ring.read_into(result.as_mut_ptr(), RING_BYTES), 0);
        assert_eq!(ring.write_from(source.as_ptr(), 0), 0);
        assert_eq!(ring.write_from(source.as_ptr(), RING_BYTES), RING_BYTES);
        assert_eq!(ring.write_from(source.as_ptr(), 1), 0);
        assert!(!ring.push(1));
        assert_eq!(ring.read_into(result.as_mut_ptr(), 13), 13);
        assert_eq!(&result[..13], &source[..13]);
        assert_eq!(ring.write_from(source.as_ptr(), 13), 13);
        assert_eq!(ring.read_into(result.as_mut_ptr(), RING_BYTES), RING_BYTES);
        assert_eq!(&result[..RING_BYTES - 13], &source[13..]);
        assert_eq!(&result[RING_BYTES - 13..], &source[..13]);
        ring.clear();
        assert_eq!(ring.len, 0);
    }
}

fn screen(width: usize, height: usize) -> framebuffer::ScreenInfo {
    framebuffer::ScreenInfo {
        width,
        height,
        stride_bytes: (width + 5) * 4,
        bits_per_pixel: 32,
        red_position: 16,
        red_size: 8,
        green_position: 8,
        green_size: 8,
        blue_position: 0,
        blue_size: 8,
    }
}

fn canvas(pixels: &mut [u32], screen: framebuffer::ScreenInfo) -> framebuffer::Framebuffer {
    framebuffer::Framebuffer::new(1, pixels.as_mut_ptr() as usize, pixels.len() * 4, screen)
        .unwrap()
}

fn paint_reference(fb: &framebuffer::Framebuffer, rect: framebuffer::Rect) {
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            fb.put_pixel(x, y, fb.color((x * 7) as u8, (y * 13) as u8, (x + y) as u8));
        }
    }
}

#[test]
fn background_damage_is_pixel_exact_with_padding_and_translucent_overlay() {
    use framebuffer::Rect;
    for red_position in [0, 16] {
        let screen = framebuffer::ScreenInfo {
            red_position,
            blue_position: 16 - red_position,
            ..screen(71, 53)
        };
        let full = Rect::new(0, 0, screen.width as i32, screen.height as i32);
        let mut actual = vec![0xdeadbeef; screen.stride_bytes / 4 * screen.height];
        let mut expected = actual.clone();
        let destination = canvas(&mut actual, screen);
        let reference = canvas(&mut expected, screen);
        let cache =
            background::BackgroundCache::new(screen, |fb| paint_reference(fb, full)).unwrap();
        let mut seed = 193;
        for step in 0..1000 {
            let x = next(&mut seed) as usize % screen.width;
            let y = next(&mut seed) as usize % screen.height;
            let width = next(&mut seed) as usize % (screen.width - x) + 1;
            let height = next(&mut seed) as usize % (screen.height - y) + 1;
            let dirty = Rect::new(x as i32, y as i32, width as i32, height as i32);
            cache.paint(&destination, dirty);
            paint_reference(&reference, dirty);
            let color = destination.color(37, 109, 213);
            let opacity = (step % 256) as u8;
            destination.fill_rect_alpha(
                dirty.x,
                dirty.y,
                dirty.width,
                dirty.height,
                color,
                opacity,
            );
            reference.fill_rect_alpha(dirty.x, dirty.y, dirty.width, dirty.height, color, opacity);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn background_cache_budget_and_invalid_geometry_fall_back() {
    for geometry in [
        screen(0, 100),
        screen(100, 0),
        screen(16384, 16384),
        framebuffer::ScreenInfo {
            width: usize::MAX,
            ..screen(10, 10)
        },
    ] {
        assert!(background::BackgroundCache::new(geometry, |_| panic!("must not draw")).is_none());
    }
}

#[test]
#[ignore = "host-only timing workload"]
fn hotpath_benchmark() {
    for count in [1, 1024, RING_BYTES] {
        let mut ring = ring::ByteRing::EMPTY;
        let mut original = baseline_ring::ByteRing::EMPTY;
        let source = vec![37; count];
        let mut destination = vec![0; count];
        let old = median_ns(10000, || {
            let ring = black_box(&mut original);
            for &byte in black_box(&source) {
                if !ring.push(byte) {
                    break;
                }
            }
            for byte in black_box(&mut destination) {
                let Some(value) = ring.pop() else {
                    break;
                };
                *byte = value;
            }
        });
        let new = median_ns(10000, || unsafe {
            black_box(&mut ring).write_from(black_box(source.as_ptr()), count);
            black_box(&mut ring).read_into(black_box(destination.as_mut_ptr()), count);
        });
        std::println!("terminal {count} bytes write/read: {old} -> {new} ns/pair");
        assert_eq!(destination, source);
    }
    let mut rows = scrollback::Scrollback::new();
    for _ in 0..MAX_ROWS {
        rows.push([7; COLS], [17; COLS]);
    }
    let mut original = ShiftScrollback {
        rows: [[7; COLS]; MAX_ROWS],
        colors: [[17; COLS]; MAX_ROWS],
    };
    let old = median_ns(10000, || {
        black_box(&mut original).push(black_box([7; COLS]), black_box([17; COLS]));
    });
    let new = median_ns(10000, || {
        black_box(&mut rows).push(black_box([7; COLS]), black_box([17; COLS]));
    });
    std::println!("full scrollback: {old} -> {new} ns/line");

    // Same pixel conversion in both paths; cache initialization is excluded.
    let screen = screen(712, 396);
    let full = framebuffer::Rect::new(0, 0, 712, 396);
    let mut pixels = vec![0; screen.stride_bytes / 4 * screen.height];
    let fb = canvas(&mut pixels, screen);
    let cache = background::BackgroundCache::new(screen, |fb| paint_reference(fb, full)).unwrap();
    for cached in [false, true] {
        let ns = median_ns(100, || {
            if cached {
                black_box(&cache).paint(black_box(&fb), full);
            } else {
                paint_reference(black_box(&fb), full);
            }
            black_box(&pixels);
        });
        std::println!("background cached={cached}: {} us/frame", ns / 1000);
    }
}

fn median_ns(iterations: u128, mut operation: impl FnMut()) -> u128 {
    let mut samples = [0; 5];
    for sample in &mut samples {
        let start = std::time::Instant::now();
        for _ in 0..iterations {
            operation();
        }
        *sample = start.elapsed().as_nanos() / iterations;
    }
    samples.sort_unstable();
    samples[2]
}

// Original full-scrollback data movement, isolated from rendering and prompts.
struct ShiftScrollback {
    rows: [[u8; COLS]; MAX_ROWS],
    colors: [[u32; COLS]; MAX_ROWS],
}

impl ShiftScrollback {
    fn push(&mut self, line: [u8; COLS], colors: [u32; COLS]) {
        let mut i = 1;
        while i < MAX_ROWS {
            self.rows[i - 1] = self.rows[i];
            self.colors[i - 1] = self.colors[i];
            i += 1;
        }
        self.rows[MAX_ROWS - 1] = line;
        self.colors[MAX_ROWS - 1] = colors;
    }
}
