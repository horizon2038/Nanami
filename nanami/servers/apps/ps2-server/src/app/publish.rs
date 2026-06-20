use libnanami::Word;

use crate::state::Ps2Server;

pub fn publish_pending_events(server: &mut Ps2Server) {
    let published_keys = publish_keyboard_events(server);
    server.published_count = server.published_count.wrapping_add(published_keys);
    server.key_event_count = 0;

    let had_mouse_buttons = server.mouse_batch.button_count != 0;
    let published_mouse = publish_mouse_batch(server);
    server.published_count = server.published_count.wrapping_add(published_mouse);
    server.mouse_batch.clear();

    if should_notify_input(published_keys, published_mouse, had_mouse_buttons) {
        let _ = libnanami::ipc::notification_notify(server.input_notification);
    }
}

fn should_notify_input(
    published_keys: usize,
    published_mouse: usize,
    had_mouse_buttons: bool,
) -> bool {
    if published_keys != 0 || had_mouse_buttons {
        return true;
    }
    published_mouse != 0
}

fn publish_keyboard_events(server: &mut Ps2Server) -> usize {
    let mut published = 0usize;
    let mut queue = nanami_services::input::InputEventQueue::new(server.input_queue_vaddr);
    let mut i = 0usize;
    while i < server.key_event_count {
        let (code, value) = server.key_events[i];
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_KEY,
            code,
            value as i16,
            0,
            next_sequence(server),
        );
        queue.push_with_event_kind(nanami_services::input::INPUT_EVENT_KIND_KEY, packed);
        published = published.wrapping_add(1);
        i += 1;
    }
    published
}

fn publish_mouse_batch(server: &mut Ps2Server) -> usize {
    let mut published = 0usize;
    let mut queue = nanami_services::input::InputEventQueue::new(server.input_queue_vaddr);

    let dx = server.mouse_batch.dx;
    let dy = server.mouse_batch.dy;
    if dx != 0 || dy != 0 {
        let move_dx = clamp_i32_to_i16(dx);
        let move_dy = clamp_i32_to_i16(dy);
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE,
            0,
            move_dx,
            move_dy,
            next_sequence(server),
        );
        queue.push_with_event_kind(nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE, packed);
        published = published.wrapping_add(1);
    }

    let mut i = 0usize;
    let button_count = server.mouse_batch.button_count;
    while i < button_count {
        let button = server.mouse_batch.buttons[i];
        let packed = nanami_services::input::pack_input_event(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON,
            button.code,
            button.pressed as i16,
            0,
            next_sequence(server),
        );
        queue.push_with_event_kind(
            nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON,
            packed,
        );
        published = published.wrapping_add(1);
        i += 1;
    }

    published
}

fn next_sequence(server: &mut Ps2Server) -> Word {
    let flags = server.input_sequence & 0xff;
    server.input_sequence = server.input_sequence.wrapping_add(1);
    flags
}

fn clamp_i32_to_i16(value: i32) -> i16 {
    if value > i16::MAX as i32 {
        i16::MAX
    } else if value < i16::MIN as i32 {
        i16::MIN
    } else {
        value as i16
    }
}
