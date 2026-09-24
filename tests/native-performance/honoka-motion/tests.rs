use super::*;
use compositor::Compositor;
use input::InputEvent;

fn setup() -> (Compositor, Vec<u32>) {
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
            .unwrap();
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
    (compositor, pixels)
}

fn move_by(dx: i32, dy: i32) -> InputEvent {
    InputEvent::MouseMove { dx, dy }
}
fn button(pressed: bool) -> InputEvent {
    InputEvent::MouseButton { code: 1, pressed }
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
    assert_eq!(damage.iter().map(|r| r.width * r.height).sum::<i32>(), 968);
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
fn many_dirty_rectangles_merge_and_keep_sequential_pixels() {
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
    PRESENTED.with(|rects| assert_eq!(rects.borrow().len(), 1));
    batched.render_if_needed();
    PRESENTED.with(|rects| assert_eq!(rects.borrow().len(), 1));
}
