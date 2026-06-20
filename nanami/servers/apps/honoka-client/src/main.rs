#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};
use libnanami::{RequestError, Word};

const SLOT_HONOKA_SERVICE: Word = 22;
const SLOT_HONOKA_PRESENT_NOTIFICATION: Word = 23;
const SLOT_TIMER_SERVICE: Word = 24;
const SLOT_TIMER_NOTIFICATION: Word = 25;
const FRAME_DELAY_MS: Word = 50;
const WINDOW_X: Word = 760;
const WINDOW_Y: Word = 120;
const CONTENT_WIDTH: usize = 512;
const CONTENT_HEIGHT: usize = 306;
const ANIMATION_X: usize = 310;
const ANIMATION_Y: usize = 190;
const ANIMATION_W: usize = 150;
const ANIMATION_H: usize = 70;
const BALL_SIZE: usize = 18;

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[honoka-client] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[honoka-client] start\n");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[honoka-client] ipc tls failed: ", e))?;

    let (honoka_port, honoka_pid) = connect_honoka_service();
    libnanami::print!("[honoka-client] connected honoka-service\n");
    let timer_port = connect_timer_service();
    libnanami::request_notification_port_create(
        SLOT_TIMER_NOTIFICATION,
        nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT,
    )
    .map_err(|e| log_error("[honoka-client] timer notification create failed: ", e))?;
    let window_id = nanami_services::gfx::honoka::honoka_create_window_with_title(
        honoka_port,
        WINDOW_X,
        WINDOW_Y,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
        b"Honoka Demo",
    )
    .map_err(|e| log_error("[honoka-client] create window failed: ", e))?;
    libnanami::print!("[honoka-client] window created id=");
    libnanami::print!("{}", window_id);
    libnanami::print!("\n");

    let present_notification = attach_honoka_present_notification(honoka_pid, window_id)
        .map_err(|e| log_error("[honoka-client] present notification failed: ", e))?;

    let (shared_base, size_bytes) =
        nanami_services::gfx::honoka::honoka_attach_logical_framebuffer(honoka_port, window_id)
            .map_err(|e| log_error("[honoka-client] attach framebuffer failed: ", e))?;
    let framebuffer =
        shared_base.saturating_add(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    let pixel_bytes =
        size_bytes.saturating_sub(nanami_services::gfx::honoka::HONOKA_DAMAGE_QUEUE_BYTES);
    libnanami::print!("[honoka-client] logical framebuffer vaddr=");
    libnanami::print!("{:#x}", framebuffer);
    libnanami::print!(" bytes=");
    libnanami::print!("{:#x}", pixel_bytes);
    libnanami::print!("\n");

    draw_demo(
        framebuffer,
        pixel_bytes as usize,
        CONTENT_WIDTH,
        CONTENT_HEIGHT,
    );
    push_damage_rect(shared_base, 0, 0, CONTENT_WIDTH, CONTENT_HEIGHT);
    let _ = libnanami::ipc::notification_notify(present_notification);
    let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
        honoka_port,
        window_id,
        0,
        0,
        CONTENT_WIDTH as Word,
        CONTENT_HEIGHT as Word,
    );
    libnanami::print!("[honoka-client] rendered\n");

    animate(
        honoka_port,
        window_id,
        shared_base,
        framebuffer,
        pixel_bytes as usize,
        present_notification,
        timer_port,
    )
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
                log_request_error("[honoka-client] waiting honoka-service: ", e);
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

fn draw_demo(vaddr: Word, size_bytes: usize, width: usize, height: usize) {
    if vaddr == 0 || width == 0 || height == 0 {
        return;
    }
    let pixels = size_bytes / 4;
    let max_pixels = width.saturating_mul(height).min(pixels);
    let mut y = 0usize;
    while y < height {
        let mut x = 0usize;
        while x < width {
            let index = y.saturating_mul(width).saturating_add(x);
            if index >= max_pixels {
                return;
            }
            let color = demo_pixel(x, y, width, height);
            unsafe {
                core::ptr::write_volatile((vaddr + (index * 4) as Word) as *mut u32, color);
            }
            x += 1;
        }
        y += 1;
    }

    draw_rect(vaddr, width, height, 0, 0, width, 4, 0x0030_1d16);
    draw_rect(
        vaddr,
        width,
        height,
        0,
        height.saturating_sub(4),
        width,
        4,
        0x0030_1d16,
    );
    draw_rect(vaddr, width, height, 0, 0, 4, height, 0x0030_1d16);
    draw_rect(
        vaddr,
        width,
        height,
        width.saturating_sub(4),
        0,
        4,
        height,
        0x0030_1d16,
    );

    draw_rect(
        vaddr,
        width,
        height,
        34,
        42,
        width.saturating_sub(68),
        3,
        0x00f5_d08a,
    );
    draw_rect(vaddr, width, height, 34, 70, 170, 4, 0x00f7_efe3);
    draw_rect(vaddr, width, height, 34, 92, 270, 4, 0x00f7_efe3);
    draw_rect(vaddr, width, height, 34, 114, 220, 4, 0x00f7_efe3);

    draw_rect(
        vaddr,
        width,
        height,
        34,
        height.saturating_sub(82),
        90,
        36,
        0x00d7_6148,
    );
    draw_rect(
        vaddr,
        width,
        height,
        136,
        height.saturating_sub(82),
        120,
        36,
        0x008c_a67e,
    );
    draw_rect(
        vaddr,
        width,
        height,
        268,
        height.saturating_sub(82),
        150,
        36,
        0x005a_6f80,
    );
    redraw_animation_region(vaddr, size_bytes, width, height, 0);
}

fn demo_pixel(x: usize, y: usize, width: usize, height: usize) -> u32 {
    let fx = if width <= 1 {
        0
    } else {
        x.saturating_mul(255) / (width - 1)
    } as u32;
    let fy = if height <= 1 {
        0
    } else {
        y.saturating_mul(255) / (height - 1)
    } as u32;
    let stripe = if ((x / 18) + (y / 18)) & 1 == 0 {
        18
    } else {
        0
    };
    let r = 70u32.saturating_add(fx / 3).saturating_add(stripe);
    let g = 88u32.saturating_add(fy / 4).saturating_add(stripe / 2);
    let b = 104u32.saturating_add((255 - fx) / 5);
    (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
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

fn animate(
    honoka_port: Word,
    window_id: Word,
    damage_queue: Word,
    framebuffer: Word,
    size_bytes: usize,
    present_notification: Word,
    timer_port: Word,
) -> ! {
    let mut frame = 0usize;
    start_frame_timer(timer_port);
    loop {
        redraw_animation_region(
            framebuffer,
            size_bytes,
            CONTENT_WIDTH,
            CONTENT_HEIGHT,
            frame,
        );
        push_damage_rect(
            damage_queue,
            ANIMATION_X,
            ANIMATION_Y,
            ANIMATION_W,
            ANIMATION_H,
        );
        let _ = libnanami::ipc::notification_notify(present_notification);
        let _ = nanami_services::gfx::honoka::honoka_invalidate_logical_framebuffer(
            honoka_port,
            window_id,
            ANIMATION_X as Word,
            ANIMATION_Y as Word,
            ANIMATION_W as Word,
            ANIMATION_H as Word,
        );
        frame = frame.wrapping_add(1);
        wait_frame_timer();
    }
}

fn redraw_animation_region(
    vaddr: Word,
    size_bytes: usize,
    fb_width: usize,
    fb_height: usize,
    frame: usize,
) {
    draw_rect(
        vaddr,
        fb_width,
        fb_height,
        ANIMATION_X,
        ANIMATION_Y,
        ANIMATION_W,
        ANIMATION_H,
        0x001f_2a30,
    );
    draw_rect(
        vaddr,
        fb_width,
        fb_height,
        ANIMATION_X,
        ANIMATION_Y,
        ANIMATION_W,
        2,
        0x00f5_d08a,
    );

    let max_x = ANIMATION_W.saturating_sub(BALL_SIZE + 8);
    let x = ANIMATION_X + 4 + triangle_wave(frame, max_x);
    let y = ANIMATION_Y + 22 + ((frame >> 2) & 0x0f);
    let color = 0x00d7_6148 ^ (((frame as u32) & 0x1f) << 10);
    draw_rect(
        vaddr, fb_width, fb_height, x, y, BALL_SIZE, BALL_SIZE, color,
    );

    let pixels = size_bytes / 4;
    if pixels == 0 {
        return;
    }
    let pulse_w = 12 + (frame & 0x1f);
    draw_rect(
        vaddr,
        fb_width,
        fb_height,
        ANIMATION_X + 14,
        ANIMATION_Y + ANIMATION_H - 18,
        pulse_w,
        5,
        0x008c_a67e,
    );
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
        log_request_error("[honoka-client] frame timer start failed: ", e);
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
                log_request_error("[honoka-client] frame timer wait failed: ", e);
                busy_delay();
                return;
            }
        }
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
