//! The production window-local console codecs; no host hardware or kernel calls.
#[path = "../../nanami/servers/apps/alter/shared/src/common/console.rs"]
mod console;
use console::*;
fn drain(mouse: &mut Ps2Mouse) -> Vec<u8> {
    std::iter::from_fn(|| mouse.pop()).collect()
}
#[test]
fn mediumraw_preserves_press_release_repeat_and_us_keymap() {
    assert_eq!(mediumraw(30, 1), Some(30));
    assert_eq!(mediumraw(30, 0), Some(0x9e));
    assert_eq!(mediumraw(30, 2), Some(30));
    assert_eq!(mediumraw(255, 1), None);
    assert_eq!(mediumraw(30, -1), None);
    assert_eq!(keymap(0, 30), b'a' as u16);
    assert_eq!(keymap(1, 30), b'A' as u16);
    assert_eq!(keymap(0, 2), b'1' as u16);
    assert_eq!(keymap(1, 2), b'!' as u16);
    assert_eq!(keymap(0, 29), 0x0702);
    assert_eq!(keymap(0, 103), 0x0603);
    assert_eq!(keymap(0, 28), 0x0201);
}
#[test]
fn ps2_initial_state_synchronizes_protocol_detection_without_pointer_motion() {
    let mut mouse = Ps2Mouse::EMPTY;
    mouse.announce();
    let expected = [8, 0, 0].repeat(5);
    assert_eq!(drain(&mut mouse), expected);
    mouse.announce();
    assert!(drain(&mut mouse).is_empty());
    assert!(mouse.command(0xf2));
    assert_eq!(drain(&mut mouse), [0xfa, 0]);
}
#[test]
fn ps2_ack_identification_and_wheel_negotiation_are_real_bytes() {
    let mut mouse = Ps2Mouse::EMPTY;
    assert!(mouse.command(0xf2));
    assert_eq!(drain(&mut mouse), [0xfa, 0]);
    for rate in [200, 100, 80] {
        assert!(mouse.command(0xf3));
        assert!(mouse.command(rate));
        assert_eq!(drain(&mut mouse), [0xfa]);
    }
    assert!(mouse.command(0xf2));
    assert_eq!(drain(&mut mouse), [0xfa, 3]);
    mouse.event(2, 8, 1);
    mouse.event(0, 0, 0);
    assert_eq!(drain(&mut mouse), [8, 0, 0, 255]);
    assert!(mouse.command(0xff));
    assert_eq!(drain(&mut mouse), [0xfa, 0xaa, 0]);
}
#[test]
fn ps2_motion_buttons_short_reads_and_large_deltas_preserve_order() {
    let mut mouse = Ps2Mouse::EMPTY;
    mouse.event(1, 0x110, 1);
    mouse.event(2, 0, -7);
    mouse.event(2, 1, 9);
    mouse.event(0, 0, 0);
    assert!(mouse.ready());
    assert_eq!(mouse.pop(), Some(0x39));
    assert_eq!(mouse.pop(), Some((-7i8) as u8));
    assert_eq!(mouse.pop(), Some((-9i8) as u8));
    assert!(!mouse.ready());
    mouse.event(2, 0, 4000);
    mouse.event(0, 0, 0);
    let bytes = drain(&mut mouse);
    assert_eq!(bytes.len() % 3, 0);
    assert_eq!(
        bytes
            .chunks(3)
            .map(|packet| packet[1] as i8 as i32)
            .sum::<i32>(),
        4000
    );
}
#[test]
fn mousedev_replaces_unread_command_replies_without_dropping_motion() {
    let mut first = Ps2Mouse::EMPTY;
    for _ in 0..100 {
        assert!(first.command(0xf4));
    }
    assert!(first.writable());
    assert_eq!(drain(&mut first), [0xfa]);
    first.event(2, 0, 7);
    first.event(0, 0, 0);
    assert!(first.command(0xf2));
    assert_eq!(first.pop(), Some(0xfa));
    assert!(first.command(0xe9));
    assert_eq!(drain(&mut first), [0xfa, 0x60, 3, 200, 8, 7, 0]);
    let second = Ps2Mouse::EMPTY;
    assert!(!second.ready());
    assert!(first.command(0xf2));
    assert_eq!(drain(&mut first), [0xfa, 0]);
}

#[test]
fn slow_mouse_reader_keeps_all_motion_and_final_button_release() {
    let mut mouse = Ps2Mouse::EMPTY;
    mouse.event(1, 0x110, 1);
    for i in 0..2000 {
        mouse.event(2, 0, if i < 1000 { 1 } else { -1 });
        mouse.event(2, 1, 1);
        mouse.event(0, 0, 0);
    }
    mouse.event(1, 0x110, 0);
    mouse.event(0, 0, 0);
    let packets = drain(&mut mouse);
    assert_eq!(packets.len() % 3, 0);
    assert_eq!(packets.chunks(3).map(|p| p[1] as i8 as i32).sum::<i32>(), 0);
    assert_eq!(
        packets.chunks(3).map(|p| p[2] as i8 as i32).sum::<i32>(),
        -2000
    );
    assert_eq!(packets[packets.len() - 3] & 7, 0);
    assert!(!mouse.ready());
}

#[test]
fn button_only_release_is_not_lost_when_the_byte_queue_is_full() {
    let mut mouse = Ps2Mouse::EMPTY;
    mouse.event(1, 0x110, 1);
    for _ in 0..30 {
        mouse.event(0, 0, 0);
    }
    mouse.event(1, 0x110, 0);
    mouse.event(0, 0, 0);
    let packets = drain(&mut mouse);
    assert_eq!(packets[packets.len() - 3] & 7, 0);
}
