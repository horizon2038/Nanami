#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::{RequestError, Word};

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;
const SLOT_TIMER_NOTIFICATION: Word = 25;
const SLOT_POSIX_SERVICE: Word = 26;

const WINDOW_X: Word = 90;
const WINDOW_Y: Word = 78;
const CONTENT_WIDTH: usize = 712;
const CONTENT_HEIGHT: usize = 396;
const COLS: usize = CONTENT_WIDTH / FONT_W;
const ROWS: usize = CONTENT_HEIGHT / FONT_H;
const FONT_W: usize = 8;
const FONT_H: usize = 12;
const MAX_LINE: usize = 96;
const MAX_ROWS: usize = 32;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[Saran] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    let (used, remaining, total) = libnanami::heap::heap_stats();
    libnanami::println!(
        "[Saran] allocation failed size={:#x} align={:#x} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        layout.size(),
        layout.align(),
        used,
        remaining,
        total
    );
    let _ = libnanami::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

pub struct Xorshift<const N: usize> {
    state: [u32; N],
    index: usize,
}

impl<const N: usize> Xorshift<N> {
    pub fn new(seed: [u32; N]) -> Self {
        assert!(N >= 3, "Xorshift requires N >= 3");
        assert!(
            seed.iter().any(|&value| value != 0),
            "Xorshift seed must not be all zero"
        );

        Self {
            state: seed,
            index: 0,
        }
    }

    pub fn next(&mut self) -> u32 {
        let current_index = self.index;
        let next_index = (current_index + 1) % N;
        let third_index = (current_index + 2) % N;

        let x = self.state[current_index];
        let y = self.state[next_index];
        let z = self.state[third_index];

        let t = x ^ (x << 11);
        let next_value = y ^ (y >> 19) ^ (z ^ (z >> 8)) ^ t;

        self.state[current_index] = next_value;
        self.index = next_index;

        next_value
    }
}

struct Saran<'a> {
    // capabilities
    honoka_port: Word,
    timer_port: Word,
    notification: Word,
    present_notification: Word,

    // framebuffer state
    surface: nanami_services::gfx::honoka::WindowSurface,

    // input state
    input_queue: &'a mut nanami_services::gfx::honoka::WindowEventQueue,
    mouse_pressed: bool,
    previous_mouse_x: Option<usize>,
    previous_mouse_y: Option<usize>,

    // window state
    window_id: Word,
}

impl<'a> Saran<'a> {
    fn new(
        honoka_port: Word,
        timer_port: Word,
        notification: Word,
        present_notification: Word,
        surface: nanami_services::gfx::honoka::WindowSurface,
        input_queue: &'a mut nanami_services::gfx::honoka::WindowEventQueue,
        window_id: Word,
    ) -> Self {
        libnanami::println!(
            "[Saran] Saran::new honoka_port={:#x} timer_port={:#x} notification={:#x} present_notification={:#x} framebuffer_vaddr={:#x} width={} height={} window_id={}",
            honoka_port,
            timer_port,
            notification,
            present_notification,
            surface.pixels(),
            surface.width,
            surface.height,
            window_id
        );
        Self {
            honoka_port,
            timer_port,
            notification,
            present_notification,
            surface,
            input_queue,
            mouse_pressed: false,
            previous_mouse_x: None,
            previous_mouse_y: None,
            window_id,
        }
    }
}

libnanami::nanami_entry!(nanami_main);
fn nanami_main() -> libnanami::NanamiResult {
    // init ipc-buffer (TLS)
    libnanami::println!("[Saran] Starting EBI Emulation ...");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[Saran] ipc tls failed: ", e))?;

    // init process notification
    let notification =
        libnanami::ipc::process_slot_descriptor(libnanami::PROCESS_SLOT_NOTIFICATION);

    // connect to honoka-service
    let (honoka_port, honoka_pid) = connect_honoka_service();
    libnanami::println!("[Saran] connected honoka-service");

    // connect to timer-service
    let timer_port = connect_timer_service();
    libnanami::request_notification_port_create(
        SLOT_TIMER_NOTIFICATION,
        nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )
    .map_err(|e| log_error("[honoka-client] timer notification create failed: ", e))?;

    // create window
    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
        b"Saran",
    )
    .map_err(|e| log_error("[Saran] create window failed: ", e))?;
    libnanami::println!("[Saran] window created id={}", window_id);

    // attach present notification
    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[honoka-client] present notification failed: ", e))?;

    let surface = nanami_services::gfx::honoka::WindowSurface::attach(honoka_port, window_id)
        .map_err(|e| log_error("[Saran] attach framebuffer failed: ", e))?;
    draw_paper(&surface);

    // attach input queue
    let (input_base, _input_bytes) =
        nanami_services::gfx::honoka::honoka_attach_input_queue(honoka_port, window_id)
            .map_err(|e| log_error("[shell] attach input queue failed: ", e))?;
    nanami_services::gfx::honoka::honoka_attach_input_notification(honoka_port, window_id)
        .map_err(|e| log_error("[shell] attach input notification failed: ", e))?;

    // create input event queue
    let mut input_queue = nanami_services::gfx::honoka::WindowEventQueue::new(input_base);

    // create Saran
    let mut saran = Saran::new(
        honoka_port,
        timer_port,
        notification,
        present_notification,
        surface,
        &mut input_queue,
        window_id,
    );

    // flush framebuffer
    let _ = libnanami::ipc::notification_notify(saran.present_notification);
    let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
        saran.honoka_port,
        saran.window_id,
        0 as Word,
        0 as Word,
        saran.surface.width as Word,
        saran.surface.height as Word,
    );

    // main loop
    loop {
        drain_input(&mut saran);
        let waited = libnanami::ipc::notification_wait(saran.notification)
            .map_err(|e| log_error("[shell] notification wait failed: ", e))?;
        if (waited & nanami_services::gfx::honoka::HONOKA_NOTIFICATION_INPUT) != 0 {
            drain_input(&mut saran);
        }
    }

    libnanami::println!("[Saran] Exiting ...");
    Ok(libnanami::request_exit()?)
}

fn connect_honoka_service() -> (Word, Word) {
    loop {
        match nanami_services::registry::connect_honoka_service_with_pid(SLOT_HONOKA_SERVICE) {
            Ok(pid) => {
                // port-descriptor, pid
                return (
                    libnanami::ipc::process_slot_descriptor(SLOT_HONOKA_SERVICE),
                    pid,
                );
            }
            Err(e) => {
                log_request_error("[Saran] waiting honoka-service: ", e);
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
                log_request_error("[honoka-client] waiting timer-service: ", e);
                busy_delay();
            }
        }
    }
}

fn draw_rect(
    vaddr: Word,
    fb_width: usize,
    fb_height: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: u32,
) {
    let x_end = x.saturating_add(width).min(fb_width);
    let y_end = y.saturating_add(height).min(fb_height);
    let mut yy = y;
    while yy < y_end {
        let mut xx = x;
        while xx < x_end {
            let index = yy.saturating_mul(fb_width).saturating_add(xx);
            unsafe {
                core::ptr::write_volatile((vaddr + (index * 4) as Word) as *mut u32, color);
            }
            xx += 1;
        }
        yy += 1;
    }
}

// bresenham's line algorithm
fn draw_line(
    vaddr: Word,
    fb_width: usize,
    fb_height: usize,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: u32,
) {
    let dx = x1 as isize - x0 as isize;
    let dy = y1 as isize - y0 as isize;
    let sx = if dx > 0 { 1 } else { -1 };
    let sy = if dy > 0 { 1 } else { -1 };
    let mut err = if dx.abs() > dy.abs() {
        dx.abs() / 2
    } else {
        -dy.abs() / 2
    };
    let mut x = x0 as isize;
    let mut y = y0 as isize;

    loop {
        if x >= 0 && (x as usize) < fb_width && y >= 0 && (y as usize) < fb_height {
            let index = (y as usize)
                .saturating_mul(fb_width)
                .saturating_add(x as usize);
            unsafe {
                core::ptr::write_volatile((vaddr + (index * 4) as Word) as *mut u32, color);
            }
        }
        if x == x1 as isize && y == y1 as isize {
            break;
        }
        let err2 = err;
        if err2 > -(dx.abs() as isize) {
            err -= dy.abs() as isize;
            x += sx;
        }
        if err2 < dy.abs() as isize {
            err += dx.abs() as isize;
            y += sy;
        }
    }
}

#[derive(Clone, Copy)]
pub enum InputEvent {
    Key { code: Word, pressed: bool },
    MouseMove { dx: i32, dy: i32 },
    MouseButton { code: Word, pressed: bool },
    MouseWheel { delta: i32 },
    WindowClose,
    Resize { width: usize, height: usize },
    Unknown,
}

pub fn decode_input_event(packed: Word) -> InputEvent {
    let (kind, code, value0, value1, _) = nanami_services::input::unpack_input_event(packed);
    match kind {
        nanami_services::input::INPUT_EVENT_KIND_KEY => InputEvent::Key {
            code,
            pressed: value0 != 0,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_MOVE => InputEvent::MouseMove {
            dx: value0 as i32,
            dy: value1 as i32,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_BUTTON => InputEvent::MouseButton {
            code,
            pressed: value0 != 0,
        },
        nanami_services::input::INPUT_EVENT_KIND_MOUSE_WHEEL => InputEvent::MouseWheel {
            delta: value0 as i32,
        },
        nanami_services::input::INPUT_EVENT_KIND_WINDOW_CLOSE => InputEvent::WindowClose,
        nanami_services::input::INPUT_EVENT_KIND_WINDOW_RESIZE => InputEvent::Resize {
            width: value0 as u16 as usize,
            height: value1 as u16 as usize,
        },
        _ => InputEvent::Unknown,
    }
}

fn drain_input(saran: &mut Saran) {
    let mut drained = 0usize;
    let mut dirty = false;

    while drained < 256 {
        let Some(packed) = saran.input_queue.pop() else {
            break;
        };
        let event = decode_input_event(packed);
        match event {
            InputEvent::Resize { width, height } => {
                match saran.surface.resize_with(
                    saran.honoka_port,
                    saran.window_id,
                    width,
                    height,
                    |old, new| {
                        draw_paper(new);
                        let width = old.width.min(new.width).saturating_sub(40);
                        let height = old.height.min(new.height).saturating_sub(40);
                        for row in 20..20 + height {
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    (old.pixels() as *const u32).add(row * old.width + 20),
                                    (new.pixels() as *mut u32).add(row * new.width + 20),
                                    width,
                                );
                            }
                        }
                    },
                ) {
                    Ok(changed) => dirty |= changed,
                    Err(e) => log_request_error("[Saran] resize failed: ", e),
                }
                saran.previous_mouse_x = None;
                saran.previous_mouse_y = None;
            }
            InputEvent::MouseMove { dx, dy } => {
                libnanami::println!("[Saran] mouse move dx={} dy={}", dx, dy);
                if saran.mouse_pressed {
                    dirty = true;
                    // draw when mouse pressed
                    if dx >= 0 && dy >= 0 {
                        /*
                        draw_rect(
                            saran.surface.pixels(),
                            saran.surface.width,
                            saran.surface.height,
                            dx as usize,
                            dy as usize,
                            4,
                            4,
                            0xffff_0000, // red
                        );
                        */
                        draw_line(
                            saran.surface.pixels(),
                            saran.surface.width,
                            saran.surface.height,
                            saran.previous_mouse_x.unwrap_or(dx as usize),
                            saran.previous_mouse_y.unwrap_or(dy as usize),
                            dx as usize,
                            dy as usize,
                            0xff00_0000, // black
                        );
                    }

                    saran.previous_mouse_x = Some(dx as usize);
                    saran.previous_mouse_y = Some(dy as usize);
                }
            }
            InputEvent::MouseButton { code, pressed } => {
                libnanami::println!("[Saran] mouse button code={} pressed={}", code, pressed);
                saran.mouse_pressed = pressed;

                if !pressed {
                    saran.previous_mouse_x = None;
                    saran.previous_mouse_y = None;
                }
            }
            InputEvent::MouseWheel { delta } => {
                libnanami::println!("[Saran] mouse wheel delta={}", delta);
            }
            InputEvent::Key { code, pressed } => {
                libnanami::println!("[Saran] key code={} pressed={}", code, pressed);
            }
            InputEvent::WindowClose => {
                let _ = nanami_services::gfx::honoka::honoka_destroy_window(
                    saran.honoka_port,
                    saran.window_id,
                );
                let _ = libnanami::request_exit();
                loop {
                    core::hint::spin_loop();
                }
            }
            InputEvent::Unknown => {
                libnanami::println!("[Saran] unknown input event packed={:#x}", packed);
            }
        }
        drained += 1;
    }
    if dirty {
        let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
            saran.honoka_port,
            saran.window_id,
            0,
            0,
            saran.surface.width,
            saran.surface.height,
        );
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

fn draw_paper(surface: &nanami_services::gfx::honoka::WindowSurface) {
    draw_rect(
        surface.pixels(),
        surface.width,
        surface.height,
        0,
        0,
        surface.width,
        surface.height,
        0x0010_1418,
    );
    draw_rect(
        surface.pixels(),
        surface.width,
        surface.height,
        10,
        10,
        surface.width.saturating_sub(20),
        surface.height.saturating_sub(20),
        0x0077_7777,
    );
    draw_rect(
        surface.pixels(),
        surface.width,
        surface.height,
        20,
        20,
        surface.width.saturating_sub(40),
        surface.height.saturating_sub(40),
        0x00ff_ffff,
    );
}
