//! Real fb-server presentation code, with RAM replacing the hardware aperture.
#![allow(dead_code)]
extern crate alloc;
extern crate self as libnanami;
use core::sync::atomic::{fence, Ordering};
type Word = usize;
const OS_RESPONSE_PERMISSION_DENIED: Word = 5;
#[derive(Debug, PartialEq)]
enum RequestError {
    InvalidArgument,
    Status(Word),
}
#[path = "../../nanami/servers/apps/fb-server/src/damage.rs"]
mod damage;
#[path = "../../nanami/servers/apps/fb-server/src/presentation.rs"]
mod presentation;
use presentation::present_shared_framebuffer as present;

struct ScreenInfo {
    width: usize,
    height: usize,
    stride: usize,
    framebuffer_bytes: usize,
}
#[derive(Clone, Copy)]
struct SharedFramebuffer {
    owner_pid: Word,
    local_vaddr: Word,
    bytes: Word,
}
struct DisplayState {
    screen: ScreenInfo,
    hardware_vaddr: Word,
    shared: SharedFramebuffer,
    shadow: Option<damage::Shadow>,
}
struct Display {
    state: DisplayState,
    source: Vec<u8>,
    hardware: Vec<u8>,
}
impl Display {
    fn new(width: usize, height: usize, padding: usize, cached: bool) -> Self {
        let stride = width * 4 + padding;
        let bytes = stride * height;
        let mut source = vec![0xff; bytes];
        let mut hardware = vec![0xff; bytes + 32];
        hardware[..16].fill(0xac);
        hardware[16 + bytes..].fill(0xac);
        let state = DisplayState {
            screen: ScreenInfo {
                width,
                height,
                stride,
                framebuffer_bytes: bytes,
            },
            hardware_vaddr: hardware.as_mut_ptr() as usize + 16,
            shared: SharedFramebuffer {
                owner_pid: 11,
                local_vaddr: source.as_mut_ptr() as usize,
                bytes,
            },
            shadow: cached.then(|| damage::Shadow::new(bytes).unwrap()),
        };
        Self {
            state,
            source,
            hardware,
        }
    }
    fn publish(&mut self, x: usize, y: usize, w: usize, h: usize) -> (usize, usize) {
        present(&mut self.state, 11, pack(x, y), pack(w, h)).unwrap()
    }
    fn pixels(&self) -> &[u8] {
        &self.hardware[16..16 + self.source.len()]
    }
    fn assert_canaries(&self) {
        assert!(self.hardware[..16].iter().all(|&b| b == 0xac));
        assert!(self.hardware[16 + self.source.len()..]
            .iter()
            .all(|&b| b == 0xac));
    }
}
fn pack(a: usize, b: usize) -> usize {
    a | b << 32
}

#[test]
fn duplicate_frames_write_no_hardware_but_still_acknowledge_all_pixels() {
    let mut display = Display::new(640, 400, 0, true);
    display.source.fill(0x12);
    assert_eq!(display.publish(0, 0, 640, 400), (256_000, 1_024_000));
    for _ in 0..60 {
        assert_eq!(display.publish(0, 0, 640, 400), (256_000, 0));
    }
    assert_eq!(display.pixels(), display.source);
    display.assert_canaries();
}

#[test]
fn neighboring_changed_spans_coalesce_but_unchanged_gaps_do_not_write() {
    let mut shadow = damage::Shadow::new(320).unwrap();
    let mut source = vec![0xff; 320];
    source[..128].fill(1);
    source[256..].fill(2);
    let mut spans = Vec::new();
    assert_eq!(
        shadow.copy(0, &source, |start, bytes| spans.push((start, bytes.len()))),
        192
    );
    assert_eq!(spans, [(0, 128), (256, 64)]);
    assert_eq!(shadow.copy(0, &source, |_, _| panic!("duplicate write")), 0);
}

#[test]
fn sparse_changes_copy_only_changed_spans_without_touching_other_pixels() {
    let mut display = Display::new(640, 400, 16, true);
    display.source[5 * display.state.screen.stride + 40] = 0;
    assert_eq!(display.publish(0, 0, 640, 400), (256_000, 64));
    assert_eq!(display.pixels(), display.source);
    assert_eq!(display.publish(0, 0, 640, 400), (256_000, 0));
    display.assert_canaries();
}

#[test]
fn randomized_clipped_partial_and_overlapping_presents_match_full_copy() {
    let mut cached = Display::new(73, 31, 20, true);
    let mut reference = Display::new(73, 31, 20, false);
    let mut seed = 19u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    for _ in 0..3000 {
        for _ in 0..10 {
            let offset = next() % cached.source.len();
            let byte = next() as u8;
            cached.source[offset] = byte;
            reference.source[offset] = byte;
        }
        let x = next() % 73;
        let y = next() % 31;
        let w = next() % 100 + 1;
        let h = next() % 40 + 1;
        let (pixels, written) = cached.publish(x, y, w, h);
        let (expected_pixels, expected_written) = reference.publish(x, y, w, h);
        assert_eq!(pixels, expected_pixels);
        assert!(written <= expected_written);
        assert_eq!(cached.pixels(), reference.pixels());
        cached.assert_canaries();
        // Changes outside this rectangle (including row padding) stay pending.
    }
}

#[test]
fn partial_span_and_cursor_erase_restore_are_not_lost() {
    let mut display = Display::new(35, 3, 4, true);
    let offset = display.state.screen.stride + 7 * 4;
    display.source[offset..offset + 4].fill(0);
    assert_eq!(display.publish(7, 1, 1, 1), (1, 4));
    display.source[offset..offset + 4].fill(0xff);
    assert_eq!(display.publish(7, 1, 1, 1), (1, 4));
    assert_eq!(display.publish(0, 0, 35, 3), (105, 0));
    display.assert_canaries();
}

#[test]
fn unchanged_initial_white_and_allocation_failure_fallback() {
    let mut cached = Display::new(20, 3, 0, true);
    let mut uncached = Display::new(20, 3, 0, false);
    assert_eq!(cached.publish(0, 0, 20, 3), (60, 0));
    assert_eq!(uncached.publish(0, 0, 20, 3), (60, 240));
    assert!(damage::Shadow::new(usize::MAX).is_none());
}

#[test]
fn rejected_requests_do_not_write_or_advance_shadow() {
    let mut display = Display::new(20, 10, 0, true);
    display.source.fill(0);
    for (pid, pos, size) in [
        (0, 0, pack(1, 1)),
        (12, 0, pack(1, 1)),
        (11, pack(20, 0), pack(1, 1)),
        (11, 0, pack(0, 1)),
        (11, 0, pack(1, 0)),
    ] {
        assert!(present(&mut display.state, pid, pos, size).is_err());
    }
    display.state.shared.bytes = 81;
    assert!(present(&mut display.state, 11, 0, pack(20, 10)).is_err());
    assert!(display.pixels().iter().all(|&b| b == 0xff));
    display.state.shared.bytes = display.source.len();
    assert_eq!(display.publish(0, 0, 20, 10), (200, 800));
}

#[test]
fn invalid_or_overflowing_stride_does_not_escape_the_visible_shadow() {
    let mut display = Display::new(20, 10, 0, true);
    display.source.fill(0);
    display.state.screen.stride = usize::MAX;
    assert!(present(&mut display.state, 11, pack(0, 2), pack(1, 1)).is_err());
    display.state.screen.stride = 4;
    assert!(present(&mut display.state, 11, pack(19, 9), pack(1, 1)).is_err());
    assert!(display.pixels().iter().all(|&b| b == 0xff));
}

#[test]
#[ignore = "RAM-only microbenchmark; not hardware framebuffer throughput"]
fn framebuffer_benchmark() {
    use std::{hint::black_box, time::Instant};
    for cached in [false, true] {
        let mut display = Display::new(2560, 1080, 0, cached);
        let stride = display.state.screen.stride;
        // A 640x400 client producing 10/35/60 different frames per 60 presents.
        for fps in [0, 10, 35, 60] {
            let mut bytes = 0;
            let mut generation = 0;
            let start = Instant::now();
            for present in 0..600 {
                let next = present * fps / 60;
                if next != generation {
                    for y in 0..400 {
                        display.source[(100 + y) * stride + 100 * 4..(100 + y) * stride + 740 * 4]
                            .fill(next as u8);
                    }
                    generation = next;
                }
                bytes += black_box(display.publish(100, 100, 640, 400)).1;
            }
            println!(
                "cached={cached} source-fps={fps} elapsed-us={} hardware-bytes={bytes}",
                start.elapsed().as_micros()
            );
            display.assert_canaries();
        }
    }
}
