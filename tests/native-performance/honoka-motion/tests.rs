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
    let mut compositor = Compositor::new(fb, font::TextRenderer, 0, 0, 0, theme).unwrap();
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
