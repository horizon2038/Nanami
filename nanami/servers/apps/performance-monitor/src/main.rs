#![no_std]
#![no_main]

use core::convert::Infallible;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::{Rgb888, RgbColor};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use libnanami::{NanamiMemoryInfo, NanamiProcessInfo, RequestError, Word};

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;

const UPDATE_INTERVAL_MS: Word = 500;
const WINDOW_X: Word = 180;
const WINDOW_Y: Word = 120;
const CONTENT_WIDTH: usize = 520;
const CONTENT_HEIGHT: usize = 330;
const HISTORY_SAMPLES: usize = 56;

const BG: Rgb888 = Rgb888::new(20, 23, 26);
const BAND: Rgb888 = Rgb888::new(29, 33, 37);
const TEXT: Rgb888 = Rgb888::new(238, 240, 237);
const MUTED: Rgb888 = Rgb888::new(158, 166, 166);
const GRID: Rgb888 = Rgb888::new(60, 67, 69);
const GREEN: Rgb888 = Rgb888::new(73, 176, 124);
const AMBER: Rgb888 = Rgb888::new(224, 172, 78);
const RED: Rgb888 = Rgb888::new(218, 93, 78);

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

#[derive(Clone, Copy)]
struct Snapshot {
    memory: Option<NanamiMemoryInfo>,
    process: Option<NanamiProcessInfo>,
}

struct History {
    samples: [u8; HISTORY_SAMPLES],
    len: usize,
}

impl History {
    const fn new() -> Self {
        Self {
            samples: [0; HISTORY_SAMPLES],
            len: 0,
        }
    }

    fn push(&mut self, value: u8) {
        if self.len < self.samples.len() {
            self.samples[self.len] = value;
            self.len += 1;
            return;
        }
        self.samples.copy_within(1.., 0);
        self.samples[self.samples.len() - 1] = value;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[performance-monitor] panic: {}", info);
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[performance-monitor] ipc tls failed: ", e))?;
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    libnanami::ipc::bind_current_thread_notification(notification)
        .map_err(|e| log_error("[performance-monitor] bind notification failed: ", e))?;

    let (honoka_port, honoka_pid) = connect_honoka_service();
    let timer_port = connect_timer_service();

    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
        b"Performance Monitor",
    )
    .map_err(|e| log_error("[performance-monitor] create window failed: ", e))?;
    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[performance-monitor] present notification failed: ", e))?;

    let (width, height) =
        nanami_services::gfx::honoka::honoka_get_window_content_size(honoka_port, window_id)
            .map_err(|e| log_error("[performance-monitor] content size failed: ", e))?;
    let (shared_base, size_bytes) =
        nanami_services::gfx::honoka::honoka_attach_logical_framebuffer(honoka_port, window_id)
            .map_err(|e| log_error("[performance-monitor] framebuffer attach failed: ", e))?;
    let framebuffer =
        shared_base.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let pixel_bytes =
        size_bytes.saturating_sub(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let (input_base, _) =
        nanami_services::gfx::honoka::honoka_attach_input_queue(honoka_port, window_id)
            .map_err(|e| log_error("[performance-monitor] input queue failed: ", e))?;
    nanami_services::gfx::honoka::honoka_attach_input_notification(honoka_port, window_id)
        .map_err(|e| log_error("[performance-monitor] input notification failed: ", e))?;

    let width = width as usize;
    let expected_height = height as usize;
    let capacity_pixels = (pixel_bytes / 4) as usize;
    let drawable_height = if width == 0 {
        0
    } else {
        capacity_pixels.min(width.saturating_mul(expected_height)) / width
    };
    let mut display = HonokaFrameBuffer::new(framebuffer, width, drawable_height, capacity_pixels);
    let mut input_queue = nanami_services::input::InputEventQueue::new(input_base);
    let mut history = History::new();
    let mut update_count = 0usize;

    nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        UPDATE_INTERVAL_MS,
        libnanami::PROCESS_SLOT_NOTIFICATION,
    )
    .map_err(|e| log_error("[performance-monitor] timer start failed: ", e))?;

    loop {
        drain_input(&mut input_queue, honoka_port, window_id);
        let snapshot = read_snapshot();
        if let Some(memory) = snapshot.memory {
            history.push(percent(memory.used_bytes(), memory.total_bytes) as u8);
        }
        draw_monitor(&mut display, snapshot, &history, update_count);
        update_count = update_count.wrapping_add(1);
        present_full(
            honoka_port,
            window_id,
            shared_base,
            present_notification,
            width,
            drawable_height,
        );
        wait_update(&mut input_queue, honoka_port, window_id);
    }
}

fn read_snapshot() -> Snapshot {
    Snapshot {
        memory: libnanami::request_nanami_info_memory().ok(),
        process: libnanami::request_nanami_info_process().ok(),
    }
}

fn draw_monitor(
    display: &mut HonokaFrameBuffer,
    snapshot: Snapshot,
    history: &History,
    update_count: usize,
) {
    let _ = display.clear(BG);
    let title = MonoTextStyle::new(&FONT_10X20, TEXT);
    let label = MonoTextStyle::new(&FONT_6X10, MUTED);
    let value = MonoTextStyle::new(&FONT_10X20, TEXT);
    let accent = MonoTextStyle::new(
        &FONT_6X10,
        if update_count & 1 == 0 { GREEN } else { MUTED },
    );

    let _ = Text::with_baseline(
        "Nanami Performance Monitor",
        Point::new(20, 18),
        title,
        Baseline::Top,
    )
    .draw(display);
    let _ = Text::with_baseline(
        "LIVE  500 ms",
        Point::new((display.width.saturating_sub(94)) as i32, 24),
        accent,
        Baseline::Top,
    )
    .draw(display);
    draw_separator(display, 20, 50, display.width.saturating_sub(40));

    let _ = Text::with_baseline("MEMORY", Point::new(20, 66), label, Baseline::Top).draw(display);
    match snapshot.memory {
        Some(memory) => draw_memory(display, memory, history, value, label),
        None => draw_unavailable(display, 94, "memory data unavailable", RED),
    }

    draw_separator(display, 20, 228, display.width.saturating_sub(40));
    let _ =
        Text::with_baseline("PROCESSES", Point::new(20, 244), label, Baseline::Top).draw(display);
    match snapshot.process {
        Some(process) => draw_processes(display, process, label),
        None => draw_unavailable(display, 272, "process data unavailable", RED),
    }
}

fn draw_memory(
    display: &mut HonokaFrameBuffer,
    memory: NanamiMemoryInfo,
    history: &History,
    value_style: MonoTextStyle<'_, Rgb888>,
    label_style: MonoTextStyle<'_, Rgb888>,
) {
    let used = memory.used_bytes();
    let usage = percent(used, memory.total_bytes);
    let mut main_value = TextBuffer::<48>::new();
    let _ = write!(
        main_value,
        "{} MiB / {} MiB",
        to_mib(used),
        to_mib(memory.total_bytes)
    );
    let _ = Text::with_baseline(
        main_value.as_str(),
        Point::new(20, 88),
        value_style,
        Baseline::Top,
    )
    .draw(display);

    let mut usage_text = TextBuffer::<16>::new();
    let _ = write!(usage_text, "{}% used", usage);
    let _ = Text::with_baseline(
        usage_text.as_str(),
        Point::new((display.width.saturating_sub(78)) as i32, 94),
        label_style,
        Baseline::Top,
    )
    .draw(display);

    let bar_x = 20usize;
    let bar_y = 120usize;
    let bar_width = display.width.saturating_sub(40);
    let _ = Rectangle::new(
        Point::new(bar_x as i32, bar_y as i32),
        Size::new(bar_width as u32, 12),
    )
    .into_styled(PrimitiveStyle::with_fill(GRID))
    .draw(display);
    let fill = bar_width.saturating_mul(usage.min(100) as usize) / 100;
    let color = if usage >= 85 {
        RED
    } else if usage >= 65 {
        AMBER
    } else {
        GREEN
    };
    let _ = Rectangle::new(
        Point::new(bar_x as i32, bar_y as i32),
        Size::new(fill as u32, 12),
    )
    .into_styled(PrimitiveStyle::with_fill(color))
    .draw(display);

    let mut free_text = TextBuffer::<32>::new();
    let _ = write!(free_text, "Free {} MiB", to_mib(memory.free_bytes));
    let _ = Text::with_baseline(
        free_text.as_str(),
        Point::new(20, 142),
        label_style,
        Baseline::Top,
    )
    .draw(display);
    draw_history(display, history, 20, 164, bar_width, 48, color);
}

fn draw_history(
    display: &mut HonokaFrameBuffer,
    history: &History,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: Rgb888,
) {
    let _ = Rectangle::new(
        Point::new(x as i32, y as i32),
        Size::new(width as u32, height as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(BAND))
    .draw(display);
    for row in [25usize, 50, 75] {
        let line_y = y + height.saturating_mul(row) / 100;
        let _ = Line::new(
            Point::new(x as i32, line_y as i32),
            Point::new((x + width.saturating_sub(1)) as i32, line_y as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(GRID, 1))
        .draw(display);
    }
    if history.len < 2 {
        return;
    }
    let step = width.saturating_sub(1) / history.samples.len().saturating_sub(1).max(1);
    let mut index = 1usize;
    while index < history.len {
        let x0 = x + (index - 1) * step;
        let x1 = x + index * step;
        let y0 = y + height.saturating_sub(1)
            - height.saturating_sub(1) * history.samples[index - 1] as usize / 100;
        let y1 = y + height.saturating_sub(1)
            - height.saturating_sub(1) * history.samples[index] as usize / 100;
        let _ = Line::new(
            Point::new(x0 as i32, y0 as i32),
            Point::new(x1 as i32, y1 as i32),
        )
        .into_styled(PrimitiveStyle::with_stroke(color, 2))
        .draw(display);
        index += 1;
    }
}

fn draw_processes(
    display: &mut HonokaFrameBuffer,
    process: NanamiProcessInfo,
    label_style: MonoTextStyle<'_, Rgb888>,
) {
    let columns = [
        (20usize, "RUNNING", process.running, GREEN),
        (190usize, "EXITED", process.exited, RED),
        (350usize, "TOTAL", process.total(), AMBER),
    ];
    for (x, label, count, color) in columns {
        let _ = Text::with_baseline(label, Point::new(x as i32, 270), label_style, Baseline::Top)
            .draw(display);
        let mut text = TextBuffer::<16>::new();
        let _ = write!(text, "{}", count);
        let count_style = MonoTextStyle::new(&FONT_10X20, color);
        let _ = Text::with_baseline(
            text.as_str(),
            Point::new(x as i32, 288),
            count_style,
            Baseline::Top,
        )
        .draw(display);
    }
}

fn draw_unavailable(display: &mut HonokaFrameBuffer, y: usize, message: &str, color: Rgb888) {
    let style = MonoTextStyle::new(&FONT_10X20, color);
    let _ =
        Text::with_baseline(message, Point::new(20, y as i32), style, Baseline::Top).draw(display);
}

fn draw_separator(display: &mut HonokaFrameBuffer, x: usize, y: usize, width: usize) {
    let _ = Line::new(
        Point::new(x as i32, y as i32),
        Point::new((x + width) as i32, y as i32),
    )
    .into_styled(PrimitiveStyle::with_stroke(GRID, 1))
    .draw(display);
}

fn present_full(
    honoka_port: Word,
    window_id: Word,
    damage_queue: Word,
    present_notification: Word,
    width: usize,
    height: usize,
) {
    push_damage_rect(damage_queue, 0, 0, width, height);
    let _ = libnanami::ipc::notification_notify(present_notification);
    let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
        honoka_port,
        window_id,
        0,
        0,
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
            Err(error) => {
                log_request_error("[performance-monitor] waiting honoka: ", error);
                busy_delay();
            }
        }
    }
}

fn connect_timer_service() -> Word {
    loop {
        match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE),
            Err(error) => {
                log_request_error("[performance-monitor] waiting timer: ", error);
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

fn wait_update(
    input_queue: &mut nanami_services::input::InputEventQueue,
    honoka_port: Word,
    window_id: Word,
) {
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);
    loop {
        match libnanami::ipc::notification_wait(notification) {
            Ok(identifier) => {
                if identifier & nanami_services::gfx::honoka::HONOKA_NOTIFICATION_INPUT != 0 {
                    drain_input(input_queue, honoka_port, window_id);
                }
                if identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT != 0 {
                    return;
                }
            }
            Err(error) => {
                log_request_error("[performance-monitor] timer wait failed: ", error);
                busy_delay();
                return;
            }
        }
    }
}

fn drain_input(
    input_queue: &mut nanami_services::input::InputEventQueue,
    honoka_port: Word,
    window_id: Word,
) {
    let mut drained = 0usize;
    while drained < 256 {
        let Some(packed) = input_queue.pop() else {
            break;
        };
        let (kind, _, _, _, _) = nanami_services::input::unpack_input_event(packed);
        if kind == nanami_services::input::INPUT_EVENT_KIND_WINDOW_CLOSE {
            let _ = nanami_services::gfx::honoka::honoka_destroy_window(honoka_port, window_id);
            let _ = libnanami::request_exit();
            loop {
                core::hint::spin_loop();
            }
        }
        drained += 1;
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
        let ptr = (base as usize + index * core::mem::size_of::<Word>()) as *const AtomicUsize;
        (*ptr).load(Ordering::SeqCst) as Word
    }
}

fn write_word(base: Word, index: usize, value: Word) {
    unsafe {
        let ptr = (base as usize + index * core::mem::size_of::<Word>()) as *const AtomicUsize;
        (*ptr).store(value as usize, Ordering::SeqCst);
    }
}

fn to_mib(bytes: Word) -> Word {
    bytes / (1024 * 1024)
}

fn percent(value: Word, total: Word) -> Word {
    if total == 0 {
        return 0;
    }
    value.saturating_mul(100) / total
}

struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuffer<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.bytes.len().saturating_sub(self.len);
        let count = value.len().min(available);
        self.bytes[self.len..self.len + count].copy_from_slice(&value.as_bytes()[..count]);
        self.len += count;
        if count == value.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

fn busy_delay() {
    let mut count = 0usize;
    while count < 400_000 {
        core::hint::spin_loop();
        count += 1;
    }
}

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    log_request_error(prefix, error);
    error.into()
}

fn log_request_error(prefix: &str, error: RequestError) {
    libnanami::println!("{}{}", prefix, error);
}

libnanami::nanami_entry!(nanami_main);
