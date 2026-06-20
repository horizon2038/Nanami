#![no_std]
#![no_main]

use core::convert::Infallible;
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle, Rectangle, Triangle};
use embedded_graphics::text::{Baseline, Text};
use libnanami::{RequestError, Word};

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;
const SLOT_TIMER_NOTIFICATION: Word = 25;

const FRAME_DELAY_MS: Word = 33;
const WINDOW_X: Word = 640;
const WINDOW_Y: Word = 130;
const CONTENT_WIDTH: usize = 552;
const CONTENT_HEIGHT: usize = 326;

const ANIMATION_X: usize = 326;
const ANIMATION_Y: usize = 186;
const ANIMATION_W: usize = 176;
const ANIMATION_H: usize = 86;
const BALL_SIZE: u32 = 24;

struct HonokaFrameBuffer {
    vaddr: Word,
    width: usize,
    height: usize,
}

impl HonokaFrameBuffer {
    const fn new(vaddr: Word, width: usize, height: usize) -> Self {
        Self {
            vaddr,
            width,
            height,
        }
    }

    fn write_pixel(&mut self, x: usize, y: usize, color: Rgb888) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = y.saturating_mul(self.width).saturating_add(x);
        let packed = ((color.r() as u32) << 16) | ((color.g() as u32) << 8) | color.b() as u32;
        unsafe {
            core::ptr::write_volatile((self.vaddr + (index * 4) as Word) as *mut u32, packed);
        }
    }
}

impl DrawTarget for HonokaFrameBuffer {
    type Color = Rgb888;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            if point.x >= 0 && point.y >= 0 {
                self.write_pixel(point.x as usize, point.y as usize, color);
            }
        }
        Ok(())
    }
}

impl OriginDimensions for HonokaFrameBuffer {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[eg-test] panic: {}", info);
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[eg-test] start\n");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[eg-test] ipc tls failed: ", e))?;

    let (honoka_port, honoka_pid) = connect_honoka_service();
    let timer_port = connect_timer_service();
    libnanami::print!("[eg-test] services connected\n");

    libnanami::request_notification_port_create(
        SLOT_TIMER_NOTIFICATION,
        nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )
    .map_err(|e| log_error("[eg-test] timer notification create failed: ", e))?;

    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
        b"embedded-gfx",
    )
    .map_err(|e| log_error("[eg-test] create window failed: ", e))?;
    libnanami::println!("[eg-test] window created id={}", window_id);

    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[eg-test] present notification failed: ", e))?;

    let (shared_base, size_bytes) =
        nanami_services::gfx::honoka::honoka_attach_logical_framebuffer(honoka_port, window_id)
            .map_err(|e| log_error("[eg-test] attach framebuffer failed: ", e))?;
    let framebuffer =
        shared_base.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let pixel_bytes =
        size_bytes.saturating_sub(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    libnanami::println!(
        "[eg-test] logical framebuffer vaddr={:#x} bytes={:#x}",
        framebuffer,
        pixel_bytes
    );

    let mut display = HonokaFrameBuffer::new(framebuffer, CONTENT_WIDTH, CONTENT_HEIGHT);
    draw_static_scene(&mut display);
    present_rect(
        honoka_port,
        window_id,
        shared_base,
        present_notification,
        0,
        0,
        CONTENT_WIDTH,
        CONTENT_HEIGHT,
    );
    libnanami::print!("[eg-test] initial scene rendered\n");

    animate(
        &mut display,
        honoka_port,
        window_id,
        shared_base,
        present_notification,
        timer_port,
    )
}

fn draw_static_scene(display: &mut HonokaFrameBuffer) {
    let _ = display.clear(Rgb888::new(18, 22, 28));

    let panel = PrimitiveStyle::with_fill(Rgb888::new(31, 38, 48));
    let border = PrimitiveStyle::with_stroke(Rgb888::new(242, 196, 92), 2);
    let dim = PrimitiveStyle::with_stroke(Rgb888::new(87, 103, 120), 1);
    let accent = PrimitiveStyle::with_fill(Rgb888::new(214, 94, 72));
    let green = PrimitiveStyle::with_fill(Rgb888::new(88, 166, 126));
    let blue = PrimitiveStyle::with_fill(Rgb888::new(83, 113, 148));

    let _ = Rectangle::new(Point::new(18, 18), Size::new(492, 268))
        .into_styled(panel)
        .draw(display);
    let _ = Rectangle::new(Point::new(18, 18), Size::new(492, 268))
        .into_styled(border)
        .draw(display);

    let title = MonoTextStyle::new(&FONT_10X20, Rgb888::new(245, 238, 218));
    let small = MonoTextStyle::new(&FONT_6X10, Rgb888::new(180, 190, 196));
    let _ = Text::with_baseline(
        "embedded-graphics on Honoka",
        Point::new(34, 38),
        title,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "DrawTarget -> shared logical framebuffer",
        Point::new(36, 72),
        small,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "damage rect + notification present",
        Point::new(36, 88),
        small,
        Baseline::Top,
    )
    .draw(display);

    let _ = Line::new(Point::new(36, 118), Point::new(288, 118))
        .into_styled(PrimitiveStyle::with_stroke(Rgb888::new(242, 196, 92), 3))
        .draw(display);
    let _ = Rectangle::new(Point::new(38, 146), Size::new(72, 48))
        .into_styled(accent)
        .draw(display);
    let _ = Circle::new(Point::new(136, 140), 58)
        .into_styled(green)
        .draw(display);
    let _ = Triangle::new(
        Point::new(238, 142),
        Point::new(194, 218),
        Point::new(286, 218),
    )
    .into_styled(blue)
    .draw(display);

    let mut x = 38;
    while x < 290 {
        let _ = Line::new(Point::new(x, 238), Point::new(x + 26, 222))
            .into_styled(dim)
            .draw(display);
        x += 34;
    }

    draw_animation_region(display, 0);
}

fn animate(
    display: &mut HonokaFrameBuffer,
    honoka_port: Word,
    window_id: Word,
    damage_queue: Word,
    present_notification: Word,
    timer_port: Word,
) -> ! {
    let mut frame = 0usize;
    start_frame_timer(timer_port);
    loop {
        draw_animation_region(display, frame);
        present_rect(
            honoka_port,
            window_id,
            damage_queue,
            present_notification,
            ANIMATION_X,
            ANIMATION_Y,
            ANIMATION_W,
            ANIMATION_H,
        );
        frame = frame.wrapping_add(1);
        wait_frame_timer();
    }
}

fn draw_animation_region(display: &mut HonokaFrameBuffer, frame: usize) {
    let bg = PrimitiveStyle::with_fill(Rgb888::new(22, 31, 38));
    let border = PrimitiveStyle::with_stroke(Rgb888::new(242, 196, 92), 2);
    let _ = Rectangle::new(
        Point::new(ANIMATION_X as i32, ANIMATION_Y as i32),
        Size::new(ANIMATION_W as u32, ANIMATION_H as u32),
    )
    .into_styled(bg)
    .draw(display);
    let _ = Rectangle::new(
        Point::new(ANIMATION_X as i32, ANIMATION_Y as i32),
        Size::new(ANIMATION_W as u32, ANIMATION_H as u32),
    )
    .into_styled(border)
    .draw(display);

    let amplitude = ANIMATION_W.saturating_sub(BALL_SIZE as usize + 16);
    let x = ANIMATION_X + 8 + triangle_wave(frame, amplitude);
    let y = ANIMATION_Y + 24 + ((frame >> 2) & 0x0f);
    let color = Rgb888::new(214, 94u8.saturating_add(((frame * 3) & 0x3f) as u8), 72);
    let _ = Circle::new(Point::new(x as i32, y as i32), BALL_SIZE)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(display);

    let pulse = 16 + (frame & 0x3f) as u32;
    let _ = Rectangle::new(
        Point::new(
            (ANIMATION_X + 18) as i32,
            (ANIMATION_Y + ANIMATION_H - 20) as i32,
        ),
        Size::new(pulse, 6),
    )
    .into_styled(PrimitiveStyle::with_fill(Rgb888::new(88, 166, 126)))
    .draw(display);
}

fn present_rect(
    honoka_port: Word,
    window_id: Word,
    damage_queue: Word,
    present_notification: Word,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    push_damage_rect(damage_queue, x, y, width, height);
    let _ = libnanami::ipc::notification_notify(present_notification);
    let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
        honoka_port,
        window_id,
        x as Word,
        y as Word,
        width as Word,
        height as Word,
    );
}

fn connect_honoka_service() -> (Word, Word) {
    loop {
        match nanami_services::registry::connect_honoka_service_with_pid(SLOT_HONOKA_SERVICE) {
            Ok(pid) => {
                return (
                    libnanami::ipc::process_slot_descriptor(SLOT_HONOKA_SERVICE),
                    pid,
                )
            }
            Err(e) => {
                log_request_error("[eg-test] waiting honoka-service: ", e);
                busy_delay();
            }
        }
    }
}

fn attach_honoka_present_notification(
    honoka_pid: Word,
    window_id: Word,
) -> Result<Word, RequestError> {
    libnanami::request_notification_port_copy(
        honoka_pid,
        libnanami::PROCESS_SLOT_NOTIFICATION,
        SLOT_HONOKA_PRESENT_NOTIFICATION,
        nanami_services::gfx::honoka::HONOKA_NOTIFICATION_PRESENT | (window_id & 0xffff_ffff),
    )?;
    Ok(libnanami::ipc::process_slot_descriptor(
        SLOT_HONOKA_PRESENT_NOTIFICATION,
    ))
}

fn connect_timer_service() -> Word {
    loop {
        match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE),
            Err(e) => {
                log_request_error("[eg-test] waiting timer-service: ", e);
                busy_delay();
            }
        }
    }
}

fn push_damage_rect(base: Word, x: usize, y: usize, width: usize, height: usize) {
    if read_word(base, 0) != nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_MAGIC {
        return;
    }
    let capacity = read_word(base, 1) as usize;
    if capacity == 0 || capacity > nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_CAPACITY {
        return;
    }

    let entry = nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_HEADER_WORDS;
    let pending = read_word(base, 4) != 0;
    let (mut out_x, mut out_y, mut out_w, mut out_h) = (x, y, width, height);
    if pending {
        let old_x = read_word(base, entry) as usize;
        let old_y = read_word(base, entry + 1) as usize;
        let old_w = read_word(base, entry + 2) as usize;
        let old_h = read_word(base, entry + 3) as usize;
        let x2 = x.saturating_add(width).max(old_x.saturating_add(old_w));
        let y2 = y.saturating_add(height).max(old_y.saturating_add(old_h));
        out_x = x.min(old_x);
        out_y = y.min(old_y);
        out_w = x2.saturating_sub(out_x);
        out_h = y2.saturating_sub(out_y);
    }
    write_word(base, entry, out_x as Word);
    write_word(base, entry + 1, out_y as Word);
    write_word(base, entry + 2, out_w as Word);
    write_word(base, entry + 3, out_h as Word);
    write_word(base, 4, read_word(base, 4).wrapping_add(1).max(1));
}

fn read_word(base: Word, index: usize) -> Word {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).load(Ordering::SeqCst) as Word
    }
}

fn write_word(base: Word, index: usize, value: Word) {
    unsafe {
        let ptr = (base as usize + word_offset(index) as usize) as *const AtomicUsize;
        (*ptr).store(value as usize, Ordering::SeqCst);
    }
}

const fn word_offset(index: usize) -> Word {
    (index * core::mem::size_of::<Word>()) as Word
}

fn start_frame_timer(timer_port: Word) {
    if let Err(e) = nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        FRAME_DELAY_MS,
        SLOT_TIMER_NOTIFICATION,
    ) {
        log_request_error("[eg-test] frame timer start failed: ", e);
        busy_delay();
    }
}

fn wait_frame_timer() {
    let notification = libnanami::ipc::process_slot_descriptor(SLOT_TIMER_NOTIFICATION);
    loop {
        match libnanami::ipc::notification_wait(notification) {
            Ok(identifier) => {
                if (identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) != 0 {
                    return;
                }
            }
            Err(e) => {
                log_request_error("[eg-test] frame timer wait failed: ", e);
                busy_delay();
                return;
            }
        }
    }
}

fn triangle_wave(frame: usize, amplitude: usize) -> usize {
    if amplitude == 0 {
        return 0;
    }
    let period = amplitude.saturating_mul(2);
    let t = frame.wrapping_mul(5) % period;
    if t < amplitude {
        t
    } else {
        period - t
    }
}

fn busy_delay() {
    let mut i = 0usize;
    while i < 400_000 {
        core::hint::spin_loop();
        i += 1;
    }
}

fn log_error(prefix: &str, err: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

fn log_request_error(prefix: &str, err: RequestError) {
    libnanami::println!("{}{}", prefix, err);
}

libnanami::nanami_entry!(nanami_main);
