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
#[path = "fbdev-capture-baseline.rs"]
mod baseline;

#[test]
fn clean_screen_halves_compared_groups_without_changing_samples_or_capture() {
    let source = vec![0x1234_5678_9abc_def0u64; 800 * 600 / 2];
    let mut captured = source.clone();
    damage::take_compared_groups();
    damage::take_row_pixels();
    unsafe {
        let input = core::slice::from_raw_parts(source.as_ptr().cast::<u32>(), 800 * 600);
        let output = core::slice::from_raw_parts_mut(captured.as_mut_ptr().cast::<u32>(), 800 * 600);
        assert!(capture(input, output, 800, 800, 600).is_empty());
        assert_eq!(input, output);
    }
    let groups = damage::take_compared_groups();
    let (sampled, compared) = damage::take_row_pixels();
    assert_eq!(groups, 30000);
    assert_eq!((sampled, compared), (480000, 480000));
    assert_eq!(source, captured);
    println!("FBDEV-CLEAN groups_before=60000 groups_after={groups} sampled={sampled} compared={compared} bytes_and_damage_equal=true");
}

#[test]
fn adjacent_known_dirty_tiles_form_one_sampled_span_without_changing_comparisons() {
    let width = 800;
    let height = 16;
    let mut source = vec![0u64; width * height / 2];
    let mut output = source.clone();
    unsafe {
        let input = core::slice::from_raw_parts_mut(source.as_mut_ptr().cast::<u32>(), width * height);
        let captured = core::slice::from_raw_parts_mut(output.as_mut_ptr().cast::<u32>(), width * height);
        for row in input.chunks_mut(width) { row[32..768].fill(0x1234_5678); }
        damage::take_known_spans();
        damage::take_row_pixels();
        assert_eq!(capture(input, captured, width, width, height), [Rect::new(32, 0, 736, 16)]);
        let (sampled, compared) = damage::take_row_pixels();
        let spans = damage::take_known_spans();
        assert_eq!((sampled, compared), (12800, 1760));
        assert_eq!(spans, 15);
        assert_eq!(captured, input);
        println!("FBDEV-DIRTY-SPANS known_spans_before=345 known_spans_after={spans} sampled={sampled} compared={compared}");
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn joined_sampled_spans_stop_at_readonly_and_protected_page_edges() {
    use core::ffi::c_void;
    unsafe extern "C" {
        fn mmap(address: *mut c_void, bytes: usize, prot: i32, flags: i32, fd: i32, offset: isize) -> *mut c_void;
        fn mprotect(address: *mut c_void, bytes: usize, prot: i32) -> i32;
        fn munmap(address: *mut c_void, bytes: usize) -> i32;
    }
    struct Guarded { allocation: *mut c_void, bytes: usize, payload: *mut u32, pages: usize }
    impl Guarded {
        unsafe fn new(pixels: usize) -> Self {
            let payload_bytes = pixels * 4;
            let pages = payload_bytes.next_multiple_of(4096);
            let bytes = pages + 8192;
            let allocation = mmap(core::ptr::null_mut(), bytes, 0, 0x22, -1, 0);
            assert_ne!(allocation as isize, -1);
            let first = allocation.cast::<u8>().add(4096);
            assert_eq!(mprotect(first.cast(), pages, 3), 0);
            first.write_bytes(0xa5, pages);
            let payload = first.add(pages - payload_bytes).cast();
            Self { allocation, bytes, payload, pages }
        }
    }
    impl Drop for Guarded {
        fn drop(&mut self) { assert_eq!(unsafe { munmap(self.allocation, self.bytes) }, 0); }
    }
    for width in [511, 512, 513, 799, 800, 801, 1023] {
        unsafe {
            let source = Guarded::new(width * 2);
            let captured = Guarded::new(width * 2);
            for index in 0..width * 2 {
                source.payload.add(index).write(index as u32 + 1);
                captured.payload.add(index).write(0);
            }
            assert_eq!(mprotect(source.allocation.cast::<u8>().add(4096).cast(), source.pages, 1), 0);
            let mut dirty = Vec::new();
            damage::capture(source.payload as usize, captured.payload as usize,
                width, width, 2, |rect| dirty.push(rect));
            assert_eq!(dirty, [Rect::new(0, 0, width as i32, 2)]);
            for index in 0..width * 2 {
                assert_eq!(captured.payload.add(index).read(), index as u32 + 1);
            }
            let prefix = captured.pages - width * 2 * 4;
            assert!(core::slice::from_raw_parts(captured.allocation.cast::<u8>().add(4096), prefix)
                .iter().all(|&byte| byte == 0xa5));
        }
    }
}

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
fn independently_aligned_rows_preserve_samples_padding_and_damage_order() {
    for width in [0usize, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33] {
        for padding in [1, 2] {
            for source_offset in 0..2 {
                for capture_offset in 0..2 {
                    let height = 17;
                    let stride = width + padding;
                    let words = (stride * height + 10).div_ceil(2);
                    // u64 storage guarantees both possible row alignments.
                    let mut source = vec![0u64; words];
                    let mut expected = vec![0xdead_beef_dead_beefu64; words];
                    let mut actual = expected.clone();
                    for frame in 0..4 {
                        unsafe {
                            let input = (source.as_mut_ptr() as *mut u32).add(source_offset);
                            for y in 0..height {
                                for x in 0..width {
                                    if (x + y + frame) % 3 == 0 {
                                        *input.add(y * stride + x) ^= 0x1234_5678;
                                    }
                                }
                            }
                            let mut before = Vec::new();
                            let mut after = Vec::new();
                            baseline::capture(input as usize,
                                (expected.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                                stride, width, height, |r| before.push(r));
                            damage::capture(input as usize,
                                (actual.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                                stride, width, height, |r| after.push(r));
                            assert_eq!(actual, expected);
                            assert_eq!(after, before);
                            damage::capture(input as usize,
                                (actual.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                                stride, width, height, |_| panic!("unchanged samples damaged"));
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn an_idle_fbdev_surface_produces_no_redraws() {
    let source = vec![0x12345678; 800 * 600];
    let mut previous = source.clone();
    damage::take_result_tile_checks();
    for _ in 0..60 {
        assert!(capture(&source, &mut previous, 800, 800, 600).is_empty());
    }
    println!("FBDEV-MASK idle60-result-tiles={}", damage::take_result_tile_checks());
}
#[test]
fn one_pixel_damages_one_tile_and_capture_is_stable_after_producer_changes() {
    let mut source = vec![0; 800 * 600];
    let mut previous = source.clone();
    source[123 * 800 + 73] = 0x123456;
    damage::take_result_tile_checks();
    assert_eq!(
        capture(&source, &mut previous, 800, 800, 600),
        [Rect::new(64, 112, 32, 16)]
    );
    println!("FBDEV-BIT-RUN sparse-result-steps={}", damage::take_result_tile_checks());
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
fn every_sixteen_tile_mask_and_high_bit_runs_keep_exact_damage_order() {
    fn check(width: usize, masks: impl Iterator<Item=u64>) {
        let mut source = vec![0u32; width];
        let mut expected = source.clone();
        let mut actual = source.clone();
        let mut before = Vec::new();
        let mut after = Vec::new();
        for mask in masks {
            source.fill(0); expected.fill(0); actual.fill(0);
            before.clear(); after.clear();
            for tile in 0..width.div_ceil(32) {
                if mask & (1u64 << tile) != 0 { source[tile * 32] = 1; }
            }
            unsafe {
                baseline::capture(source.as_ptr() as usize, expected.as_mut_ptr() as usize,
                    width, width, 1, |rect| before.push(rect));
                damage::capture(source.as_ptr() as usize, actual.as_mut_ptr() as usize,
                    width, width, 1, |rect| after.push(rect));
            }
            assert_eq!(actual, expected, "width={width} mask={mask:#x}");
            assert_eq!(after, before, "width={width} mask={mask:#x}");
        }
    }
    check(16 * 32, 0u64..=65535);
    for width in [2047, 2048] {
        check(width, (0..64).flat_map(|shift| [
            1u64.rotate_left(shift), 0x00ff_ffffu64.rotate_left(shift),
            0xaaaa_aaaau64.rotate_left(shift),
        ]).chain([0, u64::MAX]));
    }
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
    damage::take_result_tile_checks();
    assert_eq!(
        capture(&source, &mut previous, 800, 800, 600),
        [Rect::new(0, 0, 800, 600)]
    );
    assert_eq!(previous, source);
    println!("FBDEV-MASK full-result-tiles={}", damage::take_result_tile_checks());
    // A dirty full block followed by a clean full block must flush at the
    // exact boundary, then start a separate run in the partial final block.
    let width = 4097;
    let height = 17;
    let mut source = vec![0u32; width * height];
    for row in source.chunks_mut(width) {
        row[..2048].fill(1);
        row[4096] = 1;
    }
    let mut previous = vec![0; source.len()];
    assert_eq!(capture(&source, &mut previous, width, width, height), [
        Rect::new(0, 0, 2048, 16), Rect::new(4096, 0, 1, 16),
        Rect::new(0, 16, 2048, 1), Rect::new(4096, 16, 1, 1),
    ]);
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

#[test]
fn capture_preserves_damage_order_and_bytes_on_wide_surfaces() {
    for width in [0, 1, 31, 32, 33, 1023, 1024, 1025, 2047, 2048, 2049, 4097] {
        for offset in 0..2 {
            let height = 33;
            let stride = width + 3;
            let mut source = vec![0u32; stride * height + 8];
            let mut expected = source.clone();
            let mut actual = source.clone();
            for frame in 0..12 {
                for row in 0..height {
                    for column in 0..width {
                        if (column / 32 + row / 16 + frame) % 3 == 0 {
                            source[offset + row * stride + column] ^= 0x89ab_cdef;
                        }
                    }
                }
                let mut before = Vec::new();
                let mut after = Vec::new();
                unsafe {
                    baseline::capture(
                        source.as_ptr().add(offset) as usize,
                        expected.as_mut_ptr().add(offset) as usize,
                        stride,
                        width,
                        height,
                        |rect| before.push(rect),
                    );
                    damage::capture(
                        source.as_ptr().add(offset) as usize,
                        actual.as_mut_ptr().add(offset) as usize,
                        stride,
                        width,
                        height,
                        |rect| after.push(rect),
                    );
                }
                assert_eq!(
                    actual, expected,
                    "width={width} offset={offset} frame={frame}"
                );
                assert_eq!(after, before, "width={width} offset={offset} frame={frame}");
            }
        }
    }
}

#[test]
fn known_dirty_bands_preserve_all_samples_alignment_guards_and_rectangle_order() {
    for width in [1usize, 31, 32, 33, 65, 800, 2047, 2048, 2049, 4097] {
        for source_offset in 0..2 {
            for capture_offset in 0..2 {
                for dirty_row in [0, 8, 15] {
                    let height = 33;
                    let stride = width + 1;
                    let words = (stride * height + 10).div_ceil(2);
                    let mut source = vec![0u64; words];
                    let mut expected = vec![0u64; words];
                    unsafe {
                        let input = (source.as_mut_ptr() as *mut u32).add(source_offset);
                        let output = (expected.as_mut_ptr() as *mut u32).add(capture_offset);
                        for row in 0..height {
                            *output.add(row * stride + width) = 0xdead_beef;
                            for left in (0..width).step_by(32) {
                                if row % 16 == dirty_row { *input.add(row * stride + left) = 0x1234_5678; }
                            }
                        }
                        *output.add(stride * height) = 0xbeef_dead;
                        let mut actual = expected.clone();
                        let mut before = Vec::new();
                        let mut after = Vec::new();
                        baseline::capture(input as usize, output as usize,
                            stride, width, height, |r| before.push(r));
                        damage::capture(input as usize,
                            (actual.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                            stride, width, height, |r| after.push(r));
                        assert_eq!(actual, expected, "width={width} source={source_offset} capture={capture_offset} row={dirty_row}");
                        assert_eq!(after, before);
                        damage::capture(input as usize,
                            (actual.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                            stride, width, height, |_| panic!("idle band damaged"));
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }
    let mut source = vec![1u32; 800 * 600];
    let mut captured = vec![0u32; source.len()];
    for mode in ["full", "stripe", "partial", "partial-stripe", "sparse", "idle"] {
        captured.fill(0);
        source.fill(if mode == "full" { 1 } else { 0 });
        if mode == "stripe" {
            for top in (0..600).step_by(16) {
                for left in (0..800).step_by(32) { source[top * 800 + left] = 1; }
            }
        } else if mode == "partial" {
            for row in source.chunks_mut(800) { row[..400].fill(1); }
        } else if mode == "partial-stripe" {
            for top in (0..600).step_by(16) {
                for left in (0..800).step_by(64) { source[top * 800 + left] = 1; }
            }
        } else if mode == "sparse" { source[123 * 800 + 73] = 1; }
        damage::take_row_pixels();
        let _ = capture(&source, &mut captured, 800, 800, 600);
        let (sampled, compared) = damage::take_row_pixels();
        assert_eq!(sampled, 800 * 600);
        assert_eq!(captured, source);
        println!("FBDEV-KNOWN mode={mode} sampled={sampled} compared={compared}");
    }
}

#[test]
fn partially_dirty_blocks_keep_clean_tiles_guards_and_growing_mask_order() {
    for width in [33usize, 65, 800, 2047, 2048, 2049, 4097] {
        for source_offset in 0..2 {
            for capture_offset in 0..2 {
                let height = 33;
                let stride = width + 1;
                let words = (stride * height + 10).div_ceil(2);
                let mut source = vec![0u64; words];
                let mut expected = vec![0u64; words];
                unsafe {
                    let input = (source.as_mut_ptr() as *mut u32).add(source_offset);
                    let output = (expected.as_mut_ptr() as *mut u32).add(capture_offset);
                    for row in 0..height { *output.add(row * stride + width) = 0xdead_beef; }
                    *output.add(stride * height) = 0xbeef_dead;
                    let mut actual = expected.clone();
                    for frame in 0..4 {
                        source.fill(0);
                        for row in 0..height {
                            for left in (0..width).step_by(32) {
                                let tile = left / 32;
                                if tile % 3 != 1 && row % 16 == (tile % 3) * 4 + frame {
                                    *input.add(row * stride + left) = 0x1234_5678 + frame as u32;
                                }
                            }
                        }
                        let mut before = Vec::new();
                        let mut after = Vec::new();
                        baseline::capture(input as usize, output as usize,
                            stride, width, height, |r| before.push(r));
                        damage::capture(input as usize,
                            (actual.as_mut_ptr() as *mut u32).add(capture_offset) as usize,
                            stride, width, height, |r| after.push(r));
                        assert_eq!(actual, expected, "width={width} source={source_offset} capture={capture_offset} frame={frame}");
                        assert_eq!(after, before);
                    }
                }
            }
        }
    }
}

#[test]
fn contiguous_dirty_runs_match_oracle_at_mask_and_block_boundaries() {
    for width in [65usize, 2047, 2048, 2049, 4097] {
        for length in [1u32, 2, 3, 8, 31, 32, 63, 64] {
            for rotation in 0..64 {
                let mask = (u64::MAX >> (64 - length)).rotate_left(rotation);
                let height = 17;
                let stride = width + 1;
                let mut source = vec![0u32; stride * height + 1];
                let mut expected = vec![0u32; source.len()];
                for row in 0..height {
                    expected[row * stride + width] = 0xdead_beef;
                    if row % 16 == 0 {
                        for left in (0..width).step_by(32) {
                            if mask & (1u64 << ((left / 32) % 64)) != 0 {
                                source[row * stride + left] = 0x1234_5678;
                            }
                        }
                    }
                }
                *expected.last_mut().unwrap() = 0xbeef_dead;
                let mut actual = expected.clone();
                let mut before = Vec::new();
                let mut after = Vec::new();
                unsafe {
                    baseline::capture(source.as_ptr() as usize, expected.as_mut_ptr() as usize,
                        stride, width, height, |r| before.push(r));
                    damage::capture(source.as_ptr() as usize, actual.as_mut_ptr() as usize,
                        stride, width, height, |r| after.push(r));
                }
                assert_eq!(actual, expected, "width={width} length={length} rotation={rotation}");
                assert_eq!(after, before);
            }
        }
    }
}
