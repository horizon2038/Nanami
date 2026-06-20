#![no_std]
#![no_main]

use core::convert::Infallible;
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_graphics::image::GetPixel;
use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use libnanami::{RequestError, Word};
use tinybmp::Bmp;

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;

const WINDOW_X: Word = 260;
const WINDOW_Y: Word = 160;
const VIEW_PADDING: usize = 12;
const BUTTON_BAR_HEIGHT: usize = 54;
const MIN_CONTENT_WIDTH: usize = 420;
const ERROR_CONTENT_HEIGHT: usize = 180;

const IMAGE_BMP: &[u8] = include_bytes!("../assets/image.bmp");

struct HonokaFrameBuffer {
    vaddr: Word,
    width: usize,
    height: usize,
    capacity_pixels: usize,
}

impl HonokaFrameBuffer {
    const fn new(vaddr: Word, width: usize, height: usize, capacity_pixels: usize) -> Self {
        Self {
            vaddr,
            width,
            height,
            capacity_pixels,
        }
    }

    fn write_pixel(&mut self, x: usize, y: usize, color: Rgb888) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = y.saturating_mul(self.width).saturating_add(x);
        if index >= self.capacity_pixels {
            return;
        }
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
    libnanami::println!("[image-viewer] panic: {}", info);
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[image-viewer] start\n");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[image-viewer] ipc tls failed: ", e))?;

    let (honoka_port, honoka_pid) = connect_honoka_service();
    let timer_port = connect_timer_service();
    libnanami::print!("[image-viewer] services connected\n");

    let bmp = Bmp::<Rgb888>::from_slice(IMAGE_BMP).ok();
    let image_size = bmp
        .as_ref()
        .map(|image| image.size())
        .unwrap_or(Size::zero());
    let (content_width, content_height) = content_size_for_image(image_size);

    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        content_width as Word,
        content_height as Word,
        b"image-viewer",
    )
    .map_err(|e| log_error("[image-viewer] create window failed: ", e))?;
    libnanami::println!("[image-viewer] window created id={}", window_id);

    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[image-viewer] present notification failed: ", e))?;

    let (actual_content_width, actual_content_height) =
        nanami_services::gfx::honoka::honoka_get_window_content_size(honoka_port, window_id)
            .map_err(|e| log_error("[image-viewer] content size failed: ", e))?;
    let content_width = actual_content_width as usize;
    let content_height = actual_content_height as usize;

    let (shared_base, size_bytes) =
        nanami_services::gfx::honoka::honoka_attach_logical_framebuffer(honoka_port, window_id)
            .map_err(|e| log_error("[image-viewer] attach framebuffer failed: ", e))?;
    let framebuffer =
        shared_base.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let pixel_bytes =
        size_bytes.saturating_sub(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    libnanami::println!(
        "[image-viewer] logical framebuffer vaddr={:#x} bytes={:#x} content={}x{}",
        framebuffer,
        pixel_bytes,
        content_width,
        content_height
    );

    let capacity_pixels = (pixel_bytes / 4) as usize;
    let expected_pixels = content_width.saturating_mul(content_height);
    if capacity_pixels < expected_pixels {
        libnanami::println!(
            "[image-viewer] framebuffer truncated expected_pixels={} capacity_pixels={}",
            expected_pixels,
            capacity_pixels
        );
    }
    let drawable_height = if content_width == 0 {
        0
    } else {
        capacity_pixels.min(expected_pixels) / content_width
    };

    let mut display =
        HonokaFrameBuffer::new(framebuffer, content_width, drawable_height, capacity_pixels);
    draw_viewer(&mut display, bmp.as_ref(), image_size);
    present_rect(
        honoka_port,
        window_id,
        shared_base,
        present_notification,
        0,
        0,
        content_width,
        drawable_height,
    );
    libnanami::print!("[image-viewer] rendered\n");

    loop {
        let _ = nanami_services::timer::timer_service_sleep_milliseconds(timer_port, 1000);
    }
}

fn content_size_for_image(image_size: Size) -> (usize, usize) {
    if image_size == Size::zero() {
        return (MIN_CONTENT_WIDTH, ERROR_CONTENT_HEIGHT);
    }

    let image_width = image_size.width as usize;
    let image_height = image_size.height as usize;
    let content_width = image_width
        .saturating_add(VIEW_PADDING * 2)
        .max(MIN_CONTENT_WIDTH);
    let content_height = image_height
        .saturating_add(VIEW_PADDING * 2)
        .saturating_add(BUTTON_BAR_HEIGHT);
    (content_width, content_height)
}

fn draw_viewer(display: &mut HonokaFrameBuffer, image: Option<&Bmp<'_, Rgb888>>, image_size: Size) {
    let _ = display.clear(Rgb888::new(18, 22, 27));
    draw_shell(display, image_size);

    if let Some(bmp) = image {
        draw_scaled_image(display, bmp, image_size);
    } else {
        let error = MonoTextStyle::new(&FONT_10X20, Rgb888::new(231, 96, 80));
        let _ = Text::with_baseline(
            "failed to decode assets/image.bmp",
            Point::new(32, 52),
            error,
            Baseline::Top,
        )
        .draw(display);
    }

    draw_buttons(display);
}

fn draw_scaled_image(display: &mut HonokaFrameBuffer, bmp: &Bmp<'_, Rgb888>, image_size: Size) {
    let src_width = image_size.width as usize;
    let src_height = image_size.height as usize;
    if src_width == 0 || src_height == 0 {
        return;
    }

    let viewport_width = display.width.saturating_sub(VIEW_PADDING * 2);
    let viewport_height = display
        .height
        .saturating_sub(BUTTON_BAR_HEIGHT)
        .saturating_sub(VIEW_PADDING * 2);
    let (draw_width, draw_height) =
        fit_size(src_width, src_height, viewport_width, viewport_height);
    if draw_width == 0 || draw_height == 0 {
        return;
    }

    let origin_x = VIEW_PADDING + viewport_width.saturating_sub(draw_width) / 2;
    let origin_y = VIEW_PADDING + viewport_height.saturating_sub(draw_height) / 2;

    for y in 0..draw_height {
        let src_y = y.saturating_mul(src_height) / draw_height;
        for x in 0..draw_width {
            let src_x = x.saturating_mul(src_width) / draw_width;
            if let Some(color) = bmp.pixel(Point::new(src_x as i32, src_y as i32)) {
                display.write_pixel(origin_x + x, origin_y + y, color);
            }
        }
    }
}

fn fit_size(
    src_width: usize,
    src_height: usize,
    max_width: usize,
    max_height: usize,
) -> (usize, usize) {
    if src_width == 0 || src_height == 0 || max_width == 0 || max_height == 0 {
        return (0, 0);
    }
    if src_width <= max_width && src_height <= max_height {
        return (src_width, src_height);
    }

    let height_by_width = ((src_height as u64) * (max_width as u64) / (src_width as u64)) as usize;
    if height_by_width <= max_height {
        return (max_width.max(1), height_by_width.max(1));
    }

    let width_by_height = ((src_width as u64) * (max_height as u64) / (src_height as u64)) as usize;
    (width_by_height.max(1), max_height.max(1))
}

fn draw_shell(display: &mut HonokaFrameBuffer, image_size: Size) {
    let panel = PrimitiveStyle::with_fill(Rgb888::new(27, 34, 42));
    let border = PrimitiveStyle::with_stroke(Rgb888::new(229, 183, 87), 2);
    let dim = PrimitiveStyle::with_stroke(Rgb888::new(69, 82, 96), 1);
    let button_top = display.height.saturating_sub(BUTTON_BAR_HEIGHT);

    let _ = Rectangle::new(
        Point::new(0, 0),
        Size::new(display.width as u32, display.height as u32),
    )
    .into_styled(panel)
    .draw(display);
    let _ = Rectangle::new(
        Point::new(0, 0),
        Size::new(display.width as u32, display.height as u32),
    )
    .into_styled(border)
    .draw(display);

    let _ = Rectangle::new(
        Point::new(0, button_top as i32),
        Size::new(display.width as u32, 1),
    )
    .into_styled(dim)
    .draw(display);

    if image_size != Size::zero() {
        let status = MonoTextStyle::new(&FONT_6X10, Rgb888::new(178, 188, 194));
        let mut text = SizeText::new();
        text.push_decimal(image_size.width as usize);
        text.push_str("x");
        text.push_decimal(image_size.height as usize);
        text.push_str(" BMP");
        let _ = Text::with_baseline(
            text.as_str(),
            Point::new(14, button_top as i32 + 8),
            status,
            Baseline::Top,
        )
        .draw(display);
    }
}

fn draw_buttons(display: &mut HonokaFrameBuffer) {
    let labels = ["Open", "Fit", "Info", "Close"];
    let button_top = display.height.saturating_sub(BUTTON_BAR_HEIGHT) + 24;
    let gap = 8usize;
    let side_padding = 14usize;
    let button_width = display
        .width
        .saturating_sub(side_padding * 2)
        .saturating_sub(gap * 3)
        / 4;
    let button_height = 22usize;
    let text = MonoTextStyle::new(&FONT_6X10, Rgb888::new(245, 238, 218));

    for (index, label) in labels.iter().enumerate() {
        let x = side_padding + index * (button_width + gap);
        let y = button_top;
        let fill = if index == 0 {
            Rgb888::new(74, 107, 132)
        } else {
            Rgb888::new(43, 54, 66)
        };
        let border = if index == 0 {
            Rgb888::new(229, 183, 87)
        } else {
            Rgb888::new(101, 116, 130)
        };

        let _ = Rectangle::new(
            Point::new(x as i32, y as i32),
            Size::new(button_width as u32, button_height as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(fill))
        .draw(display);
        let _ = Rectangle::new(
            Point::new(x as i32, y as i32),
            Size::new(button_width as u32, button_height as u32),
        )
        .into_styled(PrimitiveStyle::with_stroke(border, 1))
        .draw(display);

        let text_x = x + button_width.saturating_sub(label.len() * 6) / 2;
        let _ = Text::with_baseline(
            label,
            Point::new(text_x as i32, y as i32 + 6),
            text,
            Baseline::Top,
        )
        .draw(display);
    }
}

struct SizeText {
    buf: [u8; 32],
    len: usize,
}

impl SizeText {
    const fn new() -> Self {
        Self {
            buf: [0; 32],
            len: 0,
        }
    }

    fn push_str(&mut self, s: &str) {
        for byte in s.bytes() {
            if self.len < self.buf.len() {
                self.buf[self.len] = byte;
                self.len += 1;
            }
        }
    }

    fn push_decimal(&mut self, value: usize) {
        let mut digits = [0u8; 20];
        let mut n = value;
        let mut count = 0usize;
        loop {
            digits[count] = b'0' + (n % 10) as u8;
            count += 1;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            if self.len < self.buf.len() {
                self.buf[self.len] = digits[count];
                self.len += 1;
            }
        }
    }

    fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
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
                log_request_error("[image-viewer] waiting honoka-service: ", e);
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
                log_request_error("[image-viewer] waiting timer-service: ", e);
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
    write_word(base, entry, x as Word);
    write_word(base, entry + 1, y as Word);
    write_word(base, entry + 2, width as Word);
    write_word(base, entry + 3, height as Word);
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
