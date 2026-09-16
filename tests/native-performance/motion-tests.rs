use super::*;
use framebuffer::Rect;
use motion_damage::MotionDamage;

fn bounds(rect: Rect) -> (i32, i32, i32, i32) {
    (rect.x, rect.y, rect.width, rect.height)
}

#[test]
fn motion_damage_keeps_only_first_and_last_positions() {
    let mut damage = MotionDamage::default();
    assert!(!damage.is_pending());
    for x in 0..4096 {
        damage.update(Rect::new(x, 50, 22, 22), Rect::new(x + 1, 50, 22, 22));
    }
    assert!(damage.is_pending());
    let (first, last) = damage.take().unwrap();
    assert_eq!(bounds(first), (0, 50, 22, 22));
    assert_eq!(bounds(last), (4096, 50, 22, 22));
    assert_eq!(first.width * first.height + last.width * last.height, 968);
    assert!(!damage.is_pending());
    assert!(damage.take().is_none());
}

#[test]
fn motion_damage_restarts_at_the_last_drawn_position() {
    let mut damage = MotionDamage::default();
    let a = Rect::new(5, 7, 700, 400);
    let b = Rect::new(900, 700, 700, 400);
    let c = Rect::new(22, 13, 700, 400);
    damage.update(a, b);
    assert_eq!(bounds(damage.take().unwrap().1), bounds(b));
    damage.update(b, c);
    let (first, last) = damage.take().unwrap();
    assert_eq!(bounds(first), bounds(b));
    assert_eq!(bounds(last), bounds(c));
}

#[test]
fn ram_fill_clips_without_touching_stride_padding_or_other_rows() {
    let info = framebuffer::ScreenInfo {
        stride_bytes: 40,
        ..screen(7, 9)
    };
    let mut pixels = vec![0xdeadbeef; 90];
    let fb = canvas(&mut pixels, info);
    fb.fill_rect(-3, -3, 8, 8, 0x12345678);
    for y in 0..9 {
        for x in 0..10 {
            assert_eq!(
                pixels[y * 10 + x],
                if y < 5 && x < 5 {
                    0x12345678
                } else {
                    0xdeadbeef
                }
            );
        }
    }
}

#[test]
#[ignore = "host-only timing workload"]
fn ram_fill_benchmark() {
    let info = screen(2560, 1080);
    let mut pixels = vec![0u32; info.width * info.height];
    let fb = canvas(&mut pixels, info);
    let old = median_ns(20, || {
        let base = black_box(pixels.as_mut_ptr());
        let color = black_box(0x12345678);
        for index in 0..pixels.len() {
            unsafe {
                base.add(index).write_volatile(color);
            }
        }
    });
    let new = median_ns(20, || {
        fb.fill_rect(0, 0, 2560, 1080, black_box(0x12345678));
        black_box(&pixels);
    });
    std::println!(
        "2560x1080 compositor RAM fill: {old} -> {new} ns (not hardware VRAM throughput)"
    );
    assert!(pixels.iter().all(|&pixel| pixel == 0x12345678));
}
