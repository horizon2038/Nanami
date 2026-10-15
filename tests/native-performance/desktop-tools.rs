#![allow(dead_code)]
extern crate alloc;
#[path = "../../nanami/servers/apps/saran/src/app.rs"]
mod app;
#[path = "../../nanami/servers/apps/saran/src/bitmap.rs"]
mod bitmap;
#[path = "../../nanami/servers/apps/saran/src/document.rs"]
mod document;
#[path = "../../nanami/servers/apps/performance-monitor/src/model.rs"]
mod model;
#[path = "../../nanami/servers/apps/performance-monitor/src/view.rs"]
mod monitor_view;
#[path = "../../nanami/servers/apps/saran/src/view.rs"]
mod view;
use app::{Dialog, Paint, Pending};
use bitmap::Image;
use document::{Document, Tool};
use nanami_ui::{Canvas, Files, Font, Key, Rect};
use std::collections::BTreeMap;

#[derive(Default)]
struct MemoryFiles {
    data: BTreeMap<String, Vec<u8>>,
    fail_read: bool,
    fail_write: bool,
    fail_stat: bool,
    writes: usize,
}
impl Files for MemoryFiles {
    fn read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        if self.fail_read {
            return Err("Read failed".into());
        }
        self.data.get(path).cloned().ok_or("Not found".into())
    }
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        self.writes += 1;
        if self.fail_write {
            return Err("Sync failed".into());
        }
        self.data.insert(path.into(), data.to_vec());
        Ok(())
    }
    fn exists(&mut self, path: &str) -> Result<bool, String> {
        if self.fail_stat {
            return Err("Stat failed".into());
        }
        Ok(self.data.contains_key(path))
    }
}
fn type_text(p: &mut Paint, files: &mut MemoryFiles, text: &str) {
    for c in text.chars() {
        p.key(Key::Character(c), false, files);
    }
}
fn painted() -> Paint {
    let mut p = Paint::new(912, 620);
    p.document.begin();
    document::stroke(&mut p.document.image, (5, 5), (80, 60), 4, 0x28486a);
    p.document.commit();
    p
}
fn point(p: &Paint, x: usize, y: usize) -> (i32, i32) {
    let origin = p.origin();
    (
        origin.0 + (x as i32 - p.pan.0 as i32) * p.zoom as i32 / 100,
        origin.1 + (y as i32 - p.pan.1 as i32) * p.zoom as i32 / 100,
    )
}

#[test]
fn ring_history_keeps_last_120_samples_and_skips_missing_data() {
    let mut h = model::History::new();
    for i in 0..400 {
        h.push(model::Sample {
            at_ms: i * 1000,
            memory: Some((1000, 1000 - i)),
            processes: Some((i, 3)),
            cores: None,
        });
    }
    assert_eq!(h.len, 120);
    assert_eq!(h.get(0).at_ms, 280000);
    assert_eq!(h.latest().unwrap().at_ms, 399000);
    assert_eq!(h.memory_stats(), Some((280, 399, 339)));
    h.push(model::Sample::default());
    assert_eq!(h.latest().unwrap().percentage(), None);
    assert_eq!(h.memory_stats(), Some((281, 399, 340)));
}
#[test]
fn monitor_percentages_handle_zero_full_and_large_totals() {
    for (total, free, expected) in [
        (0, 0, 0),
        (u64::MAX, 0, 100),
        (u64::MAX, u64::MAX, 0),
        (100, 40, 60),
        (100, 101, 0),
    ] {
        let s = model::Sample {
            memory: Some((total, free)),
            ..Default::default()
        };
        assert_eq!(s.percentage(), Some(expected));
    }
    assert_eq!(model::History::new().memory_stats(), None);
}
#[test]
fn pause_interval_tabs_and_refresh_do_not_append_samples() {
    let mut m = model::Monitor::new();
    m.refresh = false;
    assert!(m.key(Key::Character(' ')));
    assert!(m.paused);
    for _ in 0..10 {
        m.key(Key::Character('+'));
    }
    assert_eq!(m.interval_ms(), 500);
    for _ in 0..10 {
        m.key(Key::Character('-'));
    }
    assert_eq!(m.interval_ms(), 2000);
    m.key(Key::Character('2'));
    assert!(m.tab == model::Tab::Processes);
    m.key(Key::Character('3'));
    assert!(m.tab == model::Tab::System);
    m.key(Key::F5);
    assert!(m.refresh);
    assert_eq!(m.history.len, 0);
    assert!(!m.click(-1, -1));
    assert!(m.click(20, 12));
    assert!(!m.paused);
}
#[test]
fn monitor_render_clips_small_and_truncated_surfaces() {
    let font = Font::new();
    let mut m = model::Monitor::new();
    m.history.push(model::Sample {
        at_ms: 1000,
        memory: Some((4 << 30, 3 << 30)),
        processes: Some((19, 2)),
        cores: Some((64, u64::MAX)),
    });
    m.history.push(model::Sample {
        at_ms: 2000,
        ..m.history.get(0)
    });
    let system = monitor_view::System {
        architecture: "x86_64".into(),
        platform: "pc99".into(),
        kernel: "0.3.8-smp".into(),
        os: "0.5.1".into(),
    };
    for tab in [
        model::Tab::Memory,
        model::Tab::Processes,
        model::Tab::System,
    ] {
        m.tab = tab;
        for (w, h) in [(0, 0), (1, 1), (72, 32), (200, 160), (760, 600)] {
            let mut pixels = vec![0xdeadbeef; w * h + 16];
            monitor_view::draw(
                &mut Canvas::new(&mut pixels[..w * h], w, h),
                &font,
                &m,
                &system,
            );
            assert!(pixels[w * h..].iter().all(|&p| p == 0xdeadbeef));
            let expected = pixels.clone();
            monitor_view::draw(
                &mut Canvas::new(&mut pixels[..w * h], w, h),
                &font,
                &m,
                &system,
            );
            assert_eq!(pixels, expected);
            monitor_view::draw(
                &mut Canvas::new(&mut pixels[..w * h / 2], w, h),
                &font,
                &m,
                &system,
            );
        }
    }
}

#[test]
fn bmp_round_trip_preserves_all_colors_and_odd_row_padding() {
    for (w, h) in [(1, 1), (3, 5), (13, 7), (640, 400)] {
        let mut image = Image::new(w, h, 0).unwrap();
        for (i, p) in image.pixels.iter_mut().enumerate() {
            *p = (i as u32).wrapping_mul(17891) & 0xffffff;
        }
        let bytes = image.encode();
        assert_eq!(bytes.len(), 54 + (w * 3).div_ceil(4) * 4 * h);
        assert_eq!(Image::decode(&bytes).unwrap(), image);
    }
}
#[test]
fn bmp_32_bit_top_down_ignores_reserved_alpha() {
    let mut data = Image::new(2, 2, 0).unwrap().encode();
    data[22..26].copy_from_slice(&(-2i32).to_le_bytes());
    data[28..30].copy_from_slice(&32u16.to_le_bytes());
    data[54..].copy_from_slice(&[1, 2, 3, 0, 4, 5, 6, 255, 7, 8, 9, 0, 10, 11, 12, 255]);
    assert_eq!(
        Image::decode(&data).unwrap().pixels,
        [0x030201, 0x060504, 0x090807, 0x0c0b0a]
    );
}
#[test]
fn bmp_rejects_invalid_headers_compression_overflow_and_truncation() {
    let good = Image::new(3, 3, 0xffffff).unwrap().encode();
    for at in [0, 10, 14, 18, 22, 26, 28, 30] {
        let mut data = good.clone();
        data[at..at + 4].fill(0xff);
        if at == 22 {
            data[at..at + 4].fill(0);
        }
        assert!(Image::decode(&data).is_err(), "field {}", at);
    }
    for length in 0..good.len() {
        assert!(Image::decode(&good[..length]).is_err());
    }
    assert!(Image::new(0, 1, 0).is_err());
    assert!(Image::new(1025, 1024, 0).is_err());
}
#[test]
fn malformed_bmp_fuzz_never_panics() {
    let mut state = 73u64;
    for _ in 0..4000 {
        let mut bytes = Image::new(7, 5, 0x102030).unwrap().encode();
        for _ in 0..8 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let at = (state >> 32) as usize % bytes.len();
            bytes[at] = state as u8;
        }
        let _ = Image::decode(&bytes);
    }
}
#[test]
fn a_stroke_is_one_undo_and_saved_revision_survives_redo() {
    let mut d = Document::new(Image::new(32, 32, 0xffffff).unwrap());
    d.begin();
    document::stroke(&mut d.image, (1, 1), (20, 20), 4, 0x123456);
    d.commit();
    let changed = d.image.clone();
    assert!(d.dirty());
    d.mark_saved();
    assert!(!d.dirty());
    d.undo();
    assert!(d.image.pixels.iter().all(|&p| p == 0xffffff));
    assert!(d.dirty());
    d.redo();
    assert_eq!(d.image, changed);
    assert!(!d.dirty());
    d.begin();
    document::stroke(&mut d.image, (20, 1), (1, 20), 2, 0);
    d.cancel();
    assert_eq!(d.image, changed);
    assert!(!d.dirty());
}

#[test]
fn undo_history_is_bounded_and_noop_edits_do_not_consume_it() {
    let mut d = Document::new(Image::new(4, 4, 0).unwrap());
    for value in 1..=100 {
        d.begin();
        d.image.set(0, 0, value);
        assert!(d.commit());
    }
    d.begin();
    assert!(!d.commit());
    for _ in 0..200 {
        d.undo();
    }
    assert_eq!(d.image.get(0, 0), 68);
    for _ in 0..200 {
        d.redo();
    }
    assert_eq!(d.image.get(0, 0), 100);
}
#[test]
fn stroke_all_directions_clips_and_covers_both_ends() {
    for end in [(0, 0), (0, 31), (31, 0), (31, 31), (16, 31), (31, 16)] {
        let mut image = Image::new(32, 32, 0xffffff).unwrap();
        document::stroke(&mut image, (16, 16), end, 1, 0);
        assert_eq!(image.get(16, 16), 0);
        assert_eq!(image.get(end.0 as usize, end.1 as usize), 0);
        document::stroke(&mut image, (-20, -20), (40, 40), 32, 0);
        assert_eq!(image.pixels.len(), 1024);
    }
}
#[test]
fn flood_fill_respects_boundaries_and_undo() {
    let mut d = Document::new(Image::new(20, 20, 0xffffff).unwrap());
    d.begin();
    document::shape(&mut d.image, Tool::Rectangle, (3, 3), (16, 16), 1, 0, false);
    d.commit();
    let outlined = d.image.clone();
    d.fill(6, 6, 0x112233);
    assert_eq!(d.image.get(6, 6), 0x112233);
    assert_eq!(d.image.get(0, 0), 0xffffff);
    assert_eq!(d.image.get(3, 3), 0);
    d.undo();
    assert_eq!(d.image, outlined);
    d.redo();
    assert_eq!(d.image.get(6, 6), 0x112233);
    let previous = d.image.clone();
    d.fill(6, 6, 0x112233);
    d.undo();
    assert_eq!(d.image, outlined);
    d.redo();
    assert_eq!(d.image, previous);
    d.fill(6, 6, 0xff112233); // ignored alpha must not turn a no-op into an endless fill
    assert_eq!(d.image, previous);
}
#[test]
fn canvas_resize_crops_extends_and_is_undoable() {
    let mut d = Document::new(Image::new(8, 6, 0x102030).unwrap());
    let original = d.image.clone();
    d.resize(12, 10, 0xffffff).unwrap();
    assert_eq!(d.image.get(7, 5), 0x102030);
    assert_eq!(d.image.get(8, 5), 0xffffff);
    d.undo();
    assert_eq!(d.image, original);
    d.redo();
    d.resize(2, 2, 0).unwrap();
    assert_eq!(d.image.pixels, vec![0x102030; 4]);
    d.undo();
    assert_eq!(d.image.width, 12);
    assert!(d.resize(0, 2, 0).is_err());
    assert_eq!(d.image.width, 12);
}

#[test]
fn crop_rotate_and_flip_preserve_orientation_and_undo() {
    let image = Image {
        width: 3,
        height: 2,
        pixels: vec![1, 2, 3, 4, 5, 6],
    };
    let mut d = Document::new(image.clone());
    d.rotate();
    assert_eq!(d.image.width, 2);
    assert_eq!(d.image.height, 3);
    assert_eq!(d.image.pixels, [4, 1, 5, 2, 6, 3]);
    d.undo();
    assert_eq!(d.image, image);
    d.redo();
    d.flip_horizontal();
    assert_eq!(d.image.pixels, [1, 4, 2, 5, 3, 6]);
    d.undo();
    assert_eq!(d.image.pixels, [4, 1, 5, 2, 6, 3]);
    d.undo();
    d.crop(Rect::new(1, 0, 2, 2)).unwrap();
    assert_eq!(d.image.pixels, [2, 3, 5, 6]);
    d.undo();
    assert_eq!(d.image, image);
    assert!(d.crop(Rect::new(10, 10, 3, 3)).is_err());
    assert_eq!(d.image, image);
}

#[test]
fn scrollbar_hits_follow_zoom_and_memory_units_keep_small_deltas_visible() {
    let mut p = Paint::new(500, 300);
    let mut files = MemoryFiles::default();
    p.zoom = 400;
    let view = p.viewport();
    p.pointer_button(
        view.x + view.w as i32 - 1,
        view.y + view.h as i32 + 3,
        1,
        true,
        &mut files,
    );
    assert_eq!(
        p.pan.0,
        p.document.image.width - (view.w * 100 / p.zoom).max(1)
    );
    p.pointer_button(
        view.x + view.w as i32 + 3,
        view.y + view.h as i32 - 1,
        1,
        true,
        &mut files,
    );
    assert_eq!(
        p.pan.1,
        p.document.image.height - (view.h * 100 / p.zoom).max(1)
    );
    assert_eq!(monitor_view::bytes(0), "0 B");
    assert_eq!(monitor_view::bytes(4096), "4.0 KiB");
    assert_eq!(monitor_view::bytes(1048576), "1.0 MiB");
}
#[test]
fn geometric_preview_shrinking_has_no_old_trails_and_escape_restores() {
    for tool in [Tool::Line, Tool::Rectangle, Tool::Ellipse] {
        let mut p = Paint::new(912, 620);
        p.tool = tool;
        p.size = 16;
        let original = p.document.image.clone();
        let mut files = MemoryFiles::default();
        let start = point(&p, 40, 40);
        let far = point(&p, 250, 200);
        let near = point(&p, 80, 70);
        p.pointer_button(start.0, start.1, 1, true, &mut files);
        p.pointer_move(far.0, far.1);
        p.pointer_move(near.0, near.1);
        let mut expected = original.clone();
        document::shape(
            &mut expected,
            tool,
            (40, 40),
            (80, 70),
            16,
            p.foreground,
            false,
        );
        assert_eq!(p.document.image, expected, "{:?}", tool);
        p.key(Key::Escape, false, &mut files);
        assert_eq!(p.document.image, original);
        assert!(!p.document.dirty());
    }
}
#[test]
fn palette_picker_eraser_and_selection_copy_paste() {
    let mut p = Paint::new(912, 620);
    let mut files = MemoryFiles::default();
    p.pointer_button(90 + 26 * 5 + 4, 80, 1, true, &mut files);
    assert_eq!(p.foreground, app::PALETTE[5]);
    p.pointer_button(90 + 26 * 2 + 4, 80, 2, true, &mut files);
    assert_eq!(p.background, app::PALETTE[2]);
    p.document.begin();
    p.document.image.set(20, 20, 0xabcdef);
    p.document.commit();
    p.tool = Tool::Picker;
    let xy = point(&p, 20, 20);
    p.pointer_button(xy.0, xy.1, 1, true, &mut files);
    assert_eq!(p.foreground, 0xabcdef);
    p.selection = Some(Rect::new(20, 20, 1, 1));
    p.key(Key::Control('x'), false, &mut files);
    assert_eq!(p.document.image.get(20, 20), p.background);
    p.selection = Some(Rect::new(30, 30, 1, 1));
    p.key(Key::Control('v'), false, &mut files);
    assert_eq!(p.document.image.get(30, 30), 0xabcdef);
    p.key(Key::Control('z'), false, &mut files);
    assert_eq!(p.document.image.get(30, 30), 0xffffff);
    p.tool = Tool::Eraser;
    p.size = 1;
    let xy = point(&p, 30, 30);
    p.pointer_button(xy.0, xy.1, 1, true, &mut files);
    p.pointer_button(xy.0, xy.1, 1, false, &mut files);
    assert_eq!(p.document.image.get(30, 30), p.background);
    p.tool = Tool::Brush;
    let xy = point(&p, 12, 12);
    p.pointer_button(xy.0, xy.1, 2, true, &mut files);
    p.pointer_button(xy.0, xy.1, 2, false, &mut files);
    assert_eq!(p.document.image.get(12, 12), p.background);
}
#[test]
fn viewport_zoom_and_window_resizes_never_change_pixels() {
    let mut p = painted();
    let original = p.document.image.clone();
    for (w, h) in [(72, 32), (1, 1), (300, 200), (912, 620), (1800, 1000)] {
        p.resize_window(w, h);
        for _ in 0..6 {
            p.change_zoom(1);
            p.wheel(-30, false, false);
            p.wheel(-30, false, true);
        }
        for _ in 0..6 {
            p.change_zoom(-1);
        }
        assert_eq!(p.document.image, original);
        assert!(p.pan.0 < p.document.image.width);
        assert!(p.pan.1 < p.document.image.height);
    }
}
#[test]
fn open_and_save_failures_preserve_current_image_and_dirty_state() {
    let mut p = painted();
    let original = p.document.image.clone();
    let mut files = MemoryFiles::default();
    p.path = Some("/original.bmp".into());
    assert!(!p.open("/missing", &mut files));
    assert_eq!(p.document.image, original);
    files.data.insert("/bad".into(), b"not a BMP".to_vec());
    assert!(!p.open("/bad", &mut files));
    assert_eq!(p.document.image, original);
    files.fail_write = true;
    p.request(Pending::Close);
    p.key(Key::Enter, false, &mut files);
    assert!(!p.closed);
    assert!(p.document.dirty());
    assert_eq!(p.document.image, original);
    files.fail_write = false;
    p.key(Key::Control('s'), false, &mut files);
    assert!(!p.closed);
    assert!(!p.document.dirty());
    assert_eq!(
        Image::decode(&files.data["/original.bmp"]).unwrap(),
        original
    );
}
#[test]
fn save_as_overwrite_is_confirmed_and_unsaved_close_can_save() {
    let mut p = painted();
    let mut files = MemoryFiles::default();
    files.data.insert("/existing.bmp".into(), b"old".to_vec());
    p.key(Key::Control('s'), true, &mut files);
    type_text(&mut p, &mut files, "/existing.bmp");
    p.key(Key::Enter, false, &mut files);
    assert!(matches!(p.dialog, Dialog::Overwrite(_)));
    assert_eq!(files.writes, 0);
    p.key(Key::Escape, false, &mut files);
    assert_eq!(files.data["/existing.bmp"], b"old");
    p.request(Pending::Close);
    p.key(Key::Enter, false, &mut files);
    type_text(&mut p, &mut files, "/new.bmp");
    p.key(Key::Enter, false, &mut files);
    assert!(p.closed);
    assert!(files.data.contains_key("/new.bmp"));
}
#[test]
fn dimensions_and_custom_color_inputs_are_validated() {
    assert_eq!(app::dimensions(" 800 x 600 ").unwrap(), (800, 600));
    for text in ["0x1", "1025x1", "1x-1", "1x2x3", "nonsense"] {
        assert!(app::dimensions(text).is_err());
    }
    let mut p = Paint::new(912, 620);
    let mut files = MemoryFiles::default();
    p.dialog = Dialog::Color(nanami_ui::Field::new("#204080"));
    p.key(Key::Enter, false, &mut files);
    assert_eq!(p.foreground, 0x204080);
    p.dialog = Dialog::Color(nanami_ui::Field::new("bogus"));
    p.key(Key::Enter, false, &mut files);
    assert!(p.error);
    assert_eq!(p.foreground, 0x204080);
}
#[test]
fn paint_render_and_dialogs_are_clipped_and_deterministic() {
    let font = Font::new();
    let mut p = painted();
    for (w, h) in [(0, 0), (1, 1), (72, 32), (250, 160), (912, 620)] {
        p.resize_window(w, h);
        for dialog in 0..6 {
            p.dialog = match dialog {
                0 => Dialog::None,
                1 => Dialog::Path {
                    save: true,
                    field: nanami_ui::Field::new("/file.bmp"),
                },
                2 => Dialog::Confirm(Pending::Close),
                3 => Dialog::Overwrite("/file.bmp".into()),
                4 => Dialog::Size {
                    new: false,
                    field: nanami_ui::Field::new("640 x 400"),
                },
                _ => Dialog::Color(nanami_ui::Field::new("#abcdef")),
            };
            let mut pixels = vec![0xdeadbeef; w * h + 16];
            view::draw(&mut Canvas::new(&mut pixels[..w * h], w, h), &font, &p);
            assert!(pixels[w * h..].iter().all(|&p| p == 0xdeadbeef));
            let expected = pixels.clone();
            view::draw(&mut Canvas::new(&mut pixels[..w * h], w, h), &font, &p);
            assert_eq!(pixels, expected);
            view::draw(&mut Canvas::new(&mut pixels[..w * h / 2], w, h), &font, &p);
        }
    }
}
