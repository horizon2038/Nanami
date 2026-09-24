extern crate alloc;
#[path = "../../nanami/servers/apps/shell/src/editor.rs"]
mod editor;
#[path = "../../nanami/servers/apps/shell/src/terminal/mod.rs"]
mod terminal;
use terminal::Terminal;

fn feed(t: &mut Terminal, s: &[u8]) {
    for &b in s {
        t.process_byte(b);
    }
}
fn row(t: &Terminal, y: usize) -> Vec<u8> {
    t.row(y).iter().map(|c| c.byte).collect()
}

#[test]
fn cursor_addressing_and_partial_erase_do_not_append_lines() {
    let mut t = Terminal::new(8, 4);
    feed(&mut t, b"first\r\nsecond\x1b[1;3HX\x1b[K\x1b[3;2HY");
    assert_eq!(row(&t, 0), b"fiX     ");
    assert_eq!(row(&t, 1), b"second  ");
    assert_eq!(row(&t, 2), b" Y      ");
    feed(&mut t, b"\x1b[2;3H\x1b[1J");
    assert_eq!(row(&t, 0), b"        ");
    assert_eq!(row(&t, 1), b"   ond  ");
    assert_eq!(row(&t, 2), b" Y      ");
}

#[test]
fn scrolling_region_preserves_header_and_status_line() {
    let mut t = Terminal::new(6, 5);
    feed(
        &mut t,
        b"head\x1b[2;1Hone\x1b[3;1Htwo\x1b[4;1Hthree\x1b[5;1Hstatus",
    );
    feed(&mut t, b"\x1b[2;4r\x1b[4;1H\nnew");
    assert_eq!(row(&t, 0), b"head  ");
    assert_eq!(row(&t, 1), b"two   ");
    assert_eq!(row(&t, 2), b"three ");
    assert_eq!(row(&t, 3), b"new   ");
    assert_eq!(row(&t, 4), b"status");
    feed(&mut t, b"\x1b[2;1H\x1bM");
    assert_eq!(row(&t, 1), b"      ");
    assert_eq!(row(&t, 2), b"two   ");
    assert_eq!(row(&t, 4), b"status");
}

#[test]
fn insert_delete_lines_and_characters() {
    let mut t = Terminal::new(8, 4);
    feed(&mut t, b"abcdefgh\x1b[1;3H\x1b[2@XY");
    assert_eq!(row(&t, 0), b"abXYcdef");
    feed(&mut t, b"\x1b[1;3H\x1b[2P");
    assert_eq!(row(&t, 0), b"abcdef  ");
    feed(&mut t, b"\x1b[H\x1b[L");
    assert_eq!(row(&t, 0), b"        ");
    assert_eq!(row(&t, 1), b"abcdef  ");
    feed(&mut t, b"\x1b[M");
    assert_eq!(row(&t, 0), b"abcdef  ");
}

#[test]
fn wrapping_is_deferred_and_lf_does_not_imply_cr() {
    let mut t = Terminal::new(4, 3);
    feed(&mut t, b"abcd\x1b[31m");
    assert_eq!(t.cursor(), (3, 0));
    feed(&mut t, b"e\nf");
    assert_eq!(row(&t, 1), b"e   ");
    assert_eq!(row(&t, 2), b" f  ");
    feed(&mut t, b"\x1b[H1234\rZ");
    assert_eq!(row(&t, 0), b"Z234");
}

#[test]
fn alternate_screen_restores_content_cursor_and_resizes_both_screens() {
    let mut t = Terminal::new(10, 5);
    feed(&mut t, b"prompt\x1b[?1049h\x1b[2J\x1b[3;2Hvi\x1b[?25l");
    assert!(!t.cursor_visible);
    assert_eq!(row(&t, 2), b" vi       ");
    t.resize(12, 6);
    feed(&mut t, b"\x1b[?1049l\x1b[?25h");
    assert_eq!(row(&t, 0), b"prompt      ");
    assert_eq!(t.cursor(), (6, 0));
    assert!(t.cursor_visible);
}

#[test]
fn colors_background_reverse_and_reset_are_cell_attributes() {
    let mut t = Terminal::new(8, 2);
    feed(
        &mut t,
        b"\x1b[31;44mA\x1b[7mB\x1b[0mC\x1b[38;2;10;20;30;48;5;232mD",
    );
    assert_eq!((t.row(0)[0].fg, t.row(0)[0].bg), (0xcd0000, 0x0000ee));
    assert_eq!((t.row(0)[1].fg, t.row(0)[1].bg), (0x0000ee, 0xcd0000));
    assert_eq!(t.row(0)[2].fg, 0xe8e0cf);
    assert_eq!((t.row(0)[3].fg, t.row(0)[3].bg), (0x0a141e, 0x080808));
}

#[test]
fn split_sequences_osc_and_reports_do_not_leak_escape_text() {
    let mut t = Terminal::new(8, 4);
    for s in [
        &b"\x1b["[..],
        b"2;",
        b"3H",
        b"\x1b]title",
        b"\x1b\\",
        b"\x1b[6n",
    ] {
        feed(&mut t, s);
    }
    let (reply, len) = t.take_response();
    assert_eq!(&reply[..len], b"\x1b[2;3R");
    assert_eq!(row(&t, 0), b"        ");
    feed(
        &mut t,
        b"\x1b[999999999999999999999999999999;999999999999H!",
    );
    assert_eq!(t.row(3)[7].byte, b'!');
}

#[test]
fn origin_mode_and_large_moves_stay_within_margins() {
    let mut t = Terminal::new(8, 5);
    feed(&mut t, b"\x1b[2;4r\x1b[?6h\x1b[1;1HX\x1b[999BY\x1b[999AZ");
    assert_eq!(t.row(1)[0].byte, b'X');
    assert_eq!(t.row(3)[1].byte, b'Y');
    assert_eq!(t.row(1)[2].byte, b'Z');
    t.resize(3, 1);
    assert!(t.cursor().0 < 3 && t.cursor().1 == 0);
}

#[test]
fn extended_usb_arrows_and_application_cursor_keys() {
    assert_eq!(terminal::keys::sequence(0x148, false), b"\x1b[A");
    assert_eq!(terminal::keys::sequence(0x14b, true), b"\x1bOD");
    assert_eq!(terminal::keys::sequence(0x153, false), b"\x1b[3~");
    assert_eq!(terminal::keys::sequence(0x01, false), b"\x1b");
}

#[test]
fn command_editing_inserts_and_deletes_at_cursor() {
    let mut e = editor::LineEditor::<8, 3>::new();
    for b in b"ac" {
        e.insert(*b);
    }
    e.cursor = 1;
    e.insert(b'b');
    assert_eq!(&e.bytes[..e.len], b"abc");
    e.backspace();
    assert_eq!(&e.bytes[..e.len], b"ac");
    e.delete();
    assert_eq!(&e.bytes[..e.len], b"a");
    e.cursor = 0;
    e.backspace();
    assert_eq!(&e.bytes[..e.len], b"a");
}

#[test]
fn history_wrap_and_draft_restore_do_not_overwrite_history() {
    let mut e = editor::LineEditor::<8, 2>::new();
    for b in b"abc" {
        e.insert(*b);
        e.remember();
        e.clear();
    }
    e.insert(b'x');
    e.cursor = 0;
    e.previous();
    assert_eq!(&e.bytes[..e.len], b"c");
    e.previous();
    e.previous();
    assert_eq!(&e.bytes[..e.len], b"b");
    e.insert(b'!');
    e.next();
    assert_eq!(&e.bytes[..e.len], b"c");
    e.next();
    assert_eq!(&e.bytes[..e.len], b"x");
    assert_eq!(e.cursor, 0);
    assert_eq!(e.visible_start(3), 0);
}

#[test]
fn terminal_scrollback_is_bounded_resize_safe_and_separate_from_alternate_screen() {
    let mut t = Terminal::new(8, 3);
    for _ in 0..200 {
        feed(&mut t, b"line\r\n");
    }
    assert_eq!(t.history_len(), 128);
    assert!(t.scroll(2));
    assert!(!t.at_bottom());
    assert_eq!(
        t.view_row(0).iter().map(|c| c.byte).collect::<Vec<_>>(),
        b"line    "
    );
    assert!(t.scroll(i16::MIN));
    assert!(t.at_bottom());
    t.resize(4, 2);
    assert_eq!(t.history_len(), 128);
    feed(&mut t, b"\x1b[31m\x1b[?1049h\x1b[32m\x1b7");
    for _ in 0..200 {
        feed(&mut t, b"x\r\n");
    }
    assert_eq!(t.history_len(), 0);
    assert!(!t.scroll(1));
    feed(&mut t, b"\x1b[?1049lX");
    assert_eq!(t.history_len(), 128);
    assert_eq!(t.row(t.cursor().1)[0].fg, 0xcd0000);
    feed(&mut t, b"\x1b[3J");
    assert_eq!(t.history_len(), 0);
}
