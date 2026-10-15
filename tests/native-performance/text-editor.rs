#![allow(dead_code)]
extern crate alloc;

#[path = "../../nanami/servers/apps/text-editor/src/app.rs"]
mod app;
#[path = "../../nanami/servers/apps/text-editor/src/document.rs"]
mod document;
#[path = "../../nanami/servers/apps/text-editor/src/input.rs"]
mod input;
#[path = "../../nanami/servers/apps/text-editor/src/layout.rs"]
mod layout;
#[path = "../../nanami/servers/apps/text-editor/src/render.rs"]
mod render;

use app::{Editor, Files, Mode, Pending};
use document::Document;
use input::{Key, Keyboard};
use std::collections::BTreeMap;

#[derive(Default)]
struct MemoryFiles {
    files: BTreeMap<String, String>,
    fail_read: bool,
    fail_write: bool,
    fail_stat: bool,
    writes: usize,
}

impl Files for MemoryFiles {
    fn read(&mut self, path: &str) -> Result<String, String> {
        if self.fail_read {
            return Err("Read failed".into());
        }
        self.files.get(path).cloned().ok_or("Not found".into())
    }
    fn exists(&mut self, path: &str) -> Result<bool, String> {
        if self.fail_stat {
            return Err("Stat failed".into());
        }
        Ok(self.files.contains_key(path))
    }
    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.writes += 1;
        if self.fail_write {
            return Err("Disk sync failed".into());
        }
        self.files.insert(path.into(), text.into());
        Ok(())
    }
}

fn type_text(editor: &mut Editor, files: &mut MemoryFiles, text: &str) {
    for c in text.chars() {
        editor.key(Key::Character(c), false, files);
    }
}

fn verify_index(doc: &Document) {
    let text = doc.text();
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.bytes().enumerate().filter_map(
            |(i, b)| {
                if b == b'\n' {
                    Some(i + 1)
                } else {
                    None
                }
            },
        ))
        .collect();
    assert_eq!(doc.line_count(), starts.len());
    for (i, &start) in starts.iter().enumerate() {
        assert_eq!(doc.line_start(i), start, "{} {:?}", i, text);
        assert_eq!(doc.line_for(start), i);
        let mut end = starts.get(i + 1).map(|at| at - 1).unwrap_or(text.len());
        if i + 1 < starts.len() && end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        assert_eq!(doc.line_end(i), end);
    }
}

#[test]
fn edits_keep_utf8_lines_and_crlf() {
    let mut doc = Document::new("one\r\n二\r\nthree").unwrap();
    assert!(doc.crlf);
    doc.select(5, false);
    doc.delete(false).unwrap();
    assert_eq!(doc.text(), "one二\r\nthree");
    doc.undo();
    assert_eq!(doc.text(), "one\r\n二\r\nthree");
    doc.select(5, false);
    doc.delete(true).unwrap();
    assert_eq!(doc.text(), "one\r\n\r\nthree");
    doc.insert("é🙂").unwrap();
    verify_index(&doc);
    doc.select(0, false);
    doc.select(doc.len(), true);
    doc.insert("\n\n").unwrap();
    assert_eq!(doc.line_count(), 3);
    doc.undo();
    verify_index(&doc);
    doc.redo();
    verify_index(&doc);
}

#[test]
fn randomized_gap_edits_match_string_oracle() {
    let mut doc = Document::new("").unwrap();
    let mut expected = String::new();
    let mut state = 0x189789af_u64;
    let replacements = ["x", "\n", "日本語", "🙂", "\t", "\r\n", "", "ab\ncd"];
    for _ in 0..6000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let boundaries: Vec<_> = expected
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(expected.len()))
            .collect();
        let a = boundaries[state as usize % boundaries.len()];
        let b = boundaries[(state >> 20) as usize % boundaries.len()];
        let (start, end) = (a.min(b), a.max(b));
        let inserted = replacements[(state >> 40) as usize % replacements.len()];
        doc.select(start, false);
        doc.select(end, true);
        doc.insert(inserted).unwrap();
        expected.replace_range(start..end, inserted);
        assert_eq!(doc.text(), expected);
        verify_index(&doc);
        doc.undo();
        doc.redo();
        assert_eq!(doc.text(), expected);
        verify_index(&doc);
    }
}

#[test]
fn gap_growth_and_history_remain_bounded() {
    let mut doc = Document::new("").unwrap();
    let text = "a".repeat(document::MAX_BYTES);
    doc.insert(&text).unwrap();
    assert!(doc.insert("b").is_err());
    assert_eq!(doc.len(), document::MAX_BYTES);
    doc.select(0, false);
    doc.select(doc.len(), true);
    doc.insert("").unwrap();
    doc.undo();
    assert_eq!(doc.text(), text);
    assert!(Document::new(&"x".repeat(document::MAX_BYTES + 1)).is_err());
    assert!(Document::new("binary\0").is_err());
    let mut doc = Document::new("").unwrap();
    for _ in 0..400 {
        doc.insert("a").unwrap();
    }
    for _ in 0..500 {
        doc.undo();
    }
    assert_eq!(doc.len(), 272);
}

#[test]
fn saved_revision_survives_undo_redo_and_branching() {
    let mut doc = Document::new("a").unwrap();
    doc.insert("b").unwrap();
    doc.mark_saved();
    assert!(!doc.dirty());
    doc.insert("c").unwrap();
    assert!(doc.dirty());
    doc.undo();
    assert!(!doc.dirty());
    doc.undo();
    assert!(doc.dirty());
    doc.redo();
    assert!(!doc.dirty());
    doc.undo();
    doc.insert("d").unwrap();
    doc.redo();
    assert_eq!(doc.text(), "da");
    assert!(doc.dirty());
}

#[test]
fn tabs_and_preferred_columns_are_consistent() {
    let mut doc = Document::new("a\tbcd\n短\nabcdefg").unwrap();
    assert_eq!(doc.column(2), 4);
    assert_eq!(doc.at_column(0, 2), 1);
    assert_eq!(doc.at_column(0, 3), 2);
    doc.select(4, false); // column 6
    doc.vertical(1, false);
    assert_eq!(doc.column(doc.cursor), 1);
    doc.vertical(1, false);
    assert_eq!(doc.column(doc.cursor), 6);
    doc.horizontal(false, false);
    doc.vertical(-2, true);
    assert_eq!(doc.column(doc.cursor), 5);
}

#[test]
fn failed_open_keeps_current_document_and_path() {
    let mut files = MemoryFiles::default();
    let mut app = Editor::new(840, 536);
    app.path = Some("/original".into());
    type_text(&mut app, &mut files, "unsaved");
    assert!(!app.open("/absent", &mut files));
    assert_eq!(app.document.text(), "unsaved");
    assert!(app.document.dirty());
    assert_eq!(app.path.as_deref(), Some("/original"));
    assert!(app.error);
}

#[test]
fn save_as_cancel_and_overwrite_confirmation() {
    let mut files = MemoryFiles::default();
    files.files.insert("/existing".into(), "old".into());
    let mut app = Editor::new(840, 536);
    type_text(&mut app, &mut files, "new");
    app.key(Key::SaveAs, false, &mut files);
    type_text(&mut app, &mut files, "/existing");
    app.key(Key::Enter, false, &mut files);
    assert!(matches!(app.mode, Mode::Overwrite(_)));
    assert_eq!(files.writes, 0);
    app.key(Key::Escape, false, &mut files);
    assert!(app.document.dirty());
    assert_eq!(files.files["/existing"], "old");
    app.key(Key::SaveAs, false, &mut files);
    type_text(&mut app, &mut files, "/existing");
    app.key(Key::Enter, false, &mut files);
    app.key(Key::Enter, false, &mut files);
    assert_eq!(files.files["/existing"], "new");
    assert!(!app.document.dirty());
}

#[test]
fn close_saves_unnamed_document_before_exiting() {
    let mut files = MemoryFiles::default();
    let mut app = Editor::new(840, 536);
    type_text(&mut app, &mut files, "keep me");
    app.request(Pending::Close);
    app.key(Key::Enter, false, &mut files);
    assert!(!app.closed);
    assert!(matches!(app.mode, Mode::Path { save: true, .. }));
    type_text(&mut app, &mut files, "/note");
    app.key(Key::Enter, false, &mut files);
    assert!(app.closed);
    assert_eq!(files.files["/note"], "keep me");
}

#[test]
fn failed_save_never_marks_clean_or_closes_later() {
    let mut files = MemoryFiles {
        fail_write: true,
        ..Default::default()
    };
    let mut app = Editor::new(840, 536);
    app.path = Some("/note".into());
    type_text(&mut app, &mut files, "keep me");
    app.request(Pending::Close);
    app.key(Key::Enter, false, &mut files);
    assert!(app.document.dirty());
    assert!(!app.closed);
    assert!(app.error);
    assert_eq!(app.document.text(), "keep me");
    files.fail_write = false;
    app.key(Key::Save, false, &mut files);
    assert!(!app.closed);
    assert!(!app.document.dirty());
}

#[test]
fn failed_stat_does_not_overwrite_and_cancel_discards_pending_close() {
    let mut files = MemoryFiles {
        fail_stat: true,
        ..Default::default()
    };
    let mut app = Editor::new(840, 536);
    type_text(&mut app, &mut files, "note");
    app.request(Pending::Close);
    app.key(Key::Enter, false, &mut files);
    type_text(&mut app, &mut files, "/note");
    app.key(Key::Enter, false, &mut files);
    assert!(matches!(app.mode, Mode::Path { .. }));
    assert_eq!(files.writes, 0);
    app.key(Key::Escape, false, &mut files);
    files.fail_stat = false;
    app.key(Key::Save, false, &mut files);
    type_text(&mut app, &mut files, "/note");
    app.key(Key::Enter, false, &mut files);
    assert!(!app.closed);
}

#[test]
fn discard_and_cancel_are_different() {
    let mut files = MemoryFiles::default();
    let mut app = Editor::new(840, 536);
    type_text(&mut app, &mut files, "dirty");
    app.request(Pending::New);
    app.key(Key::Escape, false, &mut files);
    assert_eq!(app.document.text(), "dirty");
    app.request(Pending::New);
    app.key(Key::Character('d'), false, &mut files);
    assert_eq!(app.document.text(), "");
    assert!(!app.document.dirty());
    assert_eq!(files.writes, 0);
}

#[test]
fn copy_cut_paste_undo_and_search_wrap() {
    let mut app = Editor::new(840, 536);
    let mut files = MemoryFiles::default();
    type_text(&mut app, &mut files, "alpha beta alpha");
    app.key(Key::SelectAll, false, &mut files);
    app.key(Key::Cut, false, &mut files);
    assert_eq!(app.document.text(), "");
    app.key(Key::Paste, false, &mut files);
    assert_eq!(app.document.text(), "alpha beta alpha");
    app.key(Key::Find, false, &mut files);
    type_text(&mut app, &mut files, "alpha");
    app.key(Key::Enter, false, &mut files);
    assert_eq!(app.document.selection(), (0, 5));
    app.key(Key::FindNext, false, &mut files);
    assert_eq!(app.document.selection(), (11, 16));
    app.key(Key::FindNext, false, &mut files);
    assert_eq!(app.document.selection(), (0, 5));
    app.key(Key::FindPrevious, false, &mut files);
    assert_eq!(app.document.selection(), (11, 16));
    app.key(Key::Escape, false, &mut files);
    app.key(Key::Character('A'), false, &mut files);
    assert_eq!(app.document.text(), "alpha beta A");
}

#[test]
fn search_wrap_also_finds_a_match_spanning_the_cursor() {
    for key in [Key::FindNext, Key::FindPrevious] {
        let mut app = Editor::new(840, 536);
        let mut files = MemoryFiles::default();
        type_text(&mut app, &mut files, "alpha");
        app.document.select(2, false);
        app.key(Key::Find, false, &mut files);
        type_text(&mut app, &mut files, "alpha");
        app.key(key, false, &mut files);
        assert_eq!(app.document.selection(), (0, 5));
    }
}

#[test]
fn pointer_drag_scroll_and_resize_preserve_text() {
    let mut app = Editor::new(840, 536);
    let mut files = MemoryFiles::default();
    app.document = Document::new(&"abcdef\n".repeat(200)).unwrap();
    let x = app.layout.text_x;
    app.pointer_button(
        x + layout::CELL,
        layout::EDIT_TOP + 2,
        true,
        false,
        &mut files,
    );
    app.pointer_move(x + layout::CELL * 4, layout::EDIT_TOP + 2);
    app.pointer_button(x, layout::EDIT_TOP, false, false, &mut files);
    assert_eq!(app.document.selected_text(), "bcd");
    app.wheel(-20);
    assert_eq!(app.top, 60);
    let before = app.document.text();
    for (width, height) in [(1, 1), (72, 32), (240, 150), (1000, 700), (840, 536)] {
        app.resize(width, height);
        assert!(app.top < app.document.line_count());
        assert_eq!(app.document.text(), before);
    }
}

#[test]
fn keyboard_handles_modifiers_releases_extended_keys_and_caps() {
    let mut keyboard = Keyboard::default();
    assert_eq!(keyboard.event(0x1e, true), Some(Key::Character('a')));
    assert_eq!(keyboard.event(0x1e, false), None);
    keyboard.event(0x2a, true);
    assert_eq!(keyboard.event(0x1e, true), Some(Key::Character('A')));
    keyboard.event(0x11d, true);
    assert_eq!(keyboard.event(0x1f, true), Some(Key::SaveAs));
    keyboard.event(0x11d, false);
    keyboard.event(0x2a, false);
    assert_eq!(keyboard.event(0x14b, true), Some(Key::Left));
    assert_eq!(keyboard.event(0x120, true), None);
    keyboard.event(0x3a, true);
    keyboard.event(0x3a, true);
    keyboard.event(0x3a, false);
    assert_eq!(keyboard.event(0x1e, true), Some(Key::Character('A')));
    keyboard.event(0x2a, true);
    assert_eq!(keyboard.event(0x1e, true), Some(Key::Character('a')));
    keyboard.event(0x38, true);
    assert_eq!(keyboard.event(0x1e, true), None);
}

#[test]
fn normalize_paths_and_bound_titles() {
    assert_eq!(app::normalize_path("a/../b/./note").unwrap(), "/b/note");
    for path in ["", "/", "..", "directory/", "nul\0file"] {
        assert!(app::normalize_path(path).is_err());
    }
    assert!(app::normalize_path(&"a".repeat(255)).is_err());
    let mut app = Editor::new(840, 536);
    app.path = Some(format!("/{}", "界".repeat(60)));
    let title = app.title(nanami_uapi::gfx::honoka::HONOKA_WINDOW_TITLE_BYTES);
    assert!(title.len() <= nanami_uapi::gfx::honoka::HONOKA_WINDOW_TITLE_BYTES);
    assert!(title.is_char_boundary(title.len()));
}

#[test]
fn a_full_path_field_can_still_be_replaced_by_typing() {
    let mut app = Editor::new(840, 536);
    let mut files = MemoryFiles::default();
    app.mode = Mode::Path {
        save: true,
        field: app::Field::new(&"x".repeat(255)),
    };
    type_text(&mut app, &mut files, "/note");
    match &app.mode {
        Mode::Path { field, .. } => {
            assert_eq!(field.text, "/note");
        }
        _ => panic!("path field closed unexpectedly"),
    }
}

#[test]
fn render_is_deterministic_and_clipped_at_every_window_size() {
    let mut renderer = render::Renderer::new();
    let mut app = Editor::new(840, 536);
    let mut files = MemoryFiles::default();
    type_text(
        &mut app,
        &mut files,
        "Hello, Honoka.\n\tUTF-8: café 日本語 🙂\n",
    );
    for (width, height) in [(0, 0), (1, 1), (72, 32), (180, 96), (840, 536)] {
        app.resize(width, height);
        for mode in 0..5 {
            app.mode = match mode {
                0 => Mode::Edit,
                1 => Mode::Path {
                    save: true,
                    field: app::Field::new("/a/very/long/file/name/note.txt"),
                },
                2 => Mode::Find(app::Field::new("find")),
                3 => Mode::Confirm(Pending::Close),
                _ => Mode::Overwrite("/note".into()),
            };
            let mut pixels = vec![0xdeadbeef; width * height + 16];
            renderer.draw(&mut pixels[..width * height], &app);
            assert!(pixels[width * height..].iter().all(|&p| p == 0xdeadbeef));
            let first = pixels.clone();
            renderer.draw(&mut pixels[..width * height], &app);
            assert_eq!(pixels, first);
            // Also allow a truncated mapping: never rely solely on the geometry.
            renderer.draw(&mut pixels[..(width * height) / 2], &app);
        }
    }
}
