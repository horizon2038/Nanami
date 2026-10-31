//! Same-VM before/after comparison of the real Honoka fbdev detection loop.
use nanami_runtime::Word;
#[derive(Clone, Copy)]
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
#[path = "../../../../nanami/servers/apps/honoka/src/app/fbdev_damage.rs"]
mod current;
#[path = "../../../native-performance/fbdev-reference.rs"]
mod reference;
pub fn benchmark(timer: Word) {
    const PIXELS: usize = 800 * 600;
    const BYTES: usize = PIXELS * 4;
    const FRAMES: usize = 120;
    let source = nanami_runtime::request_memory_create(BYTES).unwrap();
    let previous = nanami_runtime::request_memory_create(BYTES).unwrap();
    for sample in 0..3 {
        for workload in 0..3 {
            let mut expected_pixels = 0;
            for before in [true, false] {
                unsafe {
                    core::ptr::write_bytes(source as *mut u8, 0, BYTES);
                    core::ptr::write_bytes(previous as *mut u8, 0, BYTES);
                }
                let (start, hz) =
                    nanami_client::timer::timer_service_monotonic_ticks(timer).unwrap();
                let mut pixels = 0;
                for frame in 0..FRAMES {
                    unsafe {
                        if workload == 1 {
                            (source as *mut u32)
                                .add(frame % 600 * 800 + frame % 800)
                                .write_volatile(frame as u32 + 1);
                        } else if workload == 2 {
                            core::ptr::write_bytes(source as *mut u8, frame as u8 + 1, BYTES);
                        }
                        let changed = |r: Rect| pixels += r.width as usize * r.height as usize;
                        if before {
                            reference::capture(source, previous, 800, 800, 600, changed);
                        } else {
                            current::capture(source, previous, 800, 800, 600, changed);
                        }
                    }
                }
                let (end, _) = nanami_client::timer::timer_service_monotonic_ticks(timer).unwrap();
                if before {
                    expected_pixels = pixels;
                } else {
                    assert_eq!(pixels, expected_pixels);
                }
                nanami_runtime::println!(
                    "FRAME-PERF sample={} workload={} before={} frames={} ns={} damaged-pixels={}",
                    sample,
                    workload,
                    before,
                    FRAMES,
                    (end - start) as u128 * 1_000_000_000 / hz as u128,
                    pixels
                );
            }
        }
    }
    nanami_runtime::request_mapping_release(source, BYTES).unwrap();
    nanami_runtime::request_mapping_release(previous, BYTES).unwrap();
}
