#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

#[path = "app/compositor.rs"]
pub mod compositor;
#[path = "app/constants.rs"]
pub mod constants;
#[path = "app/font.rs"]
pub mod font;
#[path = "app/framebuffer.rs"]
pub mod framebuffer;
#[path = "app/input.rs"]
pub mod input;
#[path = "app/logging.rs"]
pub mod logging;
#[path = "app/server.rs"]
pub mod server;
#[path = "app/services.rs"]
pub mod services;

use libnanami::ipc::ServiceEvent;

use crate::compositor::Compositor;
use crate::constants::{
    MAX_COALESCED_MOUSE_MOVES, MAX_INPUT_EVENTS_PER_FRAME, SLOT_NOTIFICATION, SLOT_SERVICE_PORT,
};
use crate::font::TextRenderer;
use crate::input::{decode_input_event, InputEvent};
use crate::logging::log_error;
use crate::server::handle_request;
use crate::services::{connect_services, prepare_framebuffer, subscribe_input};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    libnanami::println!("[honoka] panic: {}", info);
    let _ = libnanami::request_exit();
    loop {}
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    let (used, remaining, total) = libnanami::heap::heap_stats();
    libnanami::println!(
        "[honoka] allocation failed size={:#x} align={:#x} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
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

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::print!("[honoka] start\n");
    libnanami::ipc::init_ipc_tls().map_err(|e| log_error("[honoka] ipc tls init failed: ", e))?;
    libnanami::print!("[honoka] ipc ready\n");
    let (heap_base, heap_size) = libnanami::heap::init_heap(9 * 1024 * 1024)
        .map_err(|e| log_error("[honoka] heap init failed: ", e))?;
    libnanami::println!(
        "[honoka] heap ready vaddr={:#x} bytes={:#x}",
        heap_base,
        heap_size
    );
    let text = TextRenderer::new();

    nanami_services::registry::register_honoka_service()
        .map_err(|e| log_error("[honoka] service register failed: ", e))?;
    libnanami::print!("[honoka] service registered: honoka-service\n");

    let notification = libnanami::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    libnanami::ipc::bind_current_thread_notification(notification)
        .map_err(|e| log_error("[honoka] bind notification failed: ", e))?;
    libnanami::print!("[honoka] notification bound\n");

    libnanami::print!("[honoka] connect services start\n");
    let ports = connect_services()?;
    libnanami::print!("[honoka] services connected\n");
    let framebuffer = prepare_framebuffer(ports.display)?;
    log_framebuffer_ready(&framebuffer);
    let (input_queue_vaddr, input_queue_bytes) = subscribe_input(ports.input)?;
    libnanami::print!("[honoka] input subscribed\n");
    libnanami::println!(
        "[honoka] input queue vaddr={:#x} bytes={:#x}",
        input_queue_vaddr,
        input_queue_bytes
    );

    let mut compositor = Compositor::new(framebuffer, text);
    refresh_clock(ports.rtc, &mut compositor);
    start_clock_timer(ports.timer);
    libnanami::print!("[honoka] initial render start\n");
    while compositor.render_if_needed() {}
    libnanami::print!("[honoka] initial render done\n");

    libnanami::print!("[honoka] online\n");
    let mut input_queue = nanami_services::input::InputEventQueue::new(input_queue_vaddr);
    run_event_loop(ports.rtc, &mut compositor, &mut input_queue)
}

fn run_event_loop(
    rtc_port: libnanami::Word,
    compositor: &mut Compositor,
    input_queue: &mut nanami_services::input::InputEventQueue,
) -> libnanami::NanamiResult {
    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending_reply = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    loop {
        if !has_pending_reply {
            let processed = pump_input(input_queue, compositor);
            if processed != 0 {
                render_until_idle(compositor);
            } else if compositor.has_pending_render() {
                render_until_idle(compositor);
            }
        }

        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            has_pending_reply = false;
            libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_reply.0,
                pending_reply.1,
                pending_reply.2,
            )
            .map_err(|e| log_error("[honoka] reply_receive failed: ", e))?
        } else {
            libnanami::ipc::service_receive_event(service_port)
                .map_err(|e| log_error("[honoka] receive failed: ", e))?
        };

        match event {
            ServiceEvent::Request(request) => {
                pending_reply = handle_request(request, compositor);
                has_pending_reply = true;
                render_until_idle(compositor);
            }
            ServiceEvent::Notification { identifier, .. } => {
                handle_notification(identifier, rtc_port, input_queue, compositor);
                render_until_idle(compositor);
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::println!("[honoka] fault id={} reason={:#x}", identifier, reason);
            }
        }
    }
}

fn handle_notification(
    identifier: libnanami::Word,
    rtc_port: libnanami::Word,
    input_queue: &mut nanami_services::input::InputEventQueue,
    compositor: &mut Compositor,
) -> usize {
    if is_present_notification(identifier) {
        compositor.invalidate_presented_logical_framebuffer(present_window_id(identifier));
    }
    if is_timer_notification(identifier) {
        compositor.drain_presented_logical_framebuffers();
        refresh_clock(rtc_port, compositor);
    }
    pump_input(input_queue, compositor)
}

fn start_clock_timer(timer_port: libnanami::Word) {
    if let Err(e) = nanami_services::timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        1000,
        SLOT_NOTIFICATION,
    ) {
        libnanami::println!("[honoka] clock timer start failed: {:?}", e);
    }
}

fn refresh_clock(rtc_port: libnanami::Word, compositor: &mut Compositor) {
    match nanami_services::rtc::rtc_service_read(rtc_port) {
        Ok(dt) => compositor.set_clock(dt.hour, dt.minute, dt.second),
        Err(e) => libnanami::println!("[honoka] rtc read failed: {:?}", e),
    }
}

fn is_timer_notification(identifier: libnanami::Word) -> bool {
    (identifier & nanami_services::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) != 0
}

fn is_present_notification(identifier: libnanami::Word) -> bool {
    (identifier & nanami_services::gfx::honoka::HONOKA_NOTIFICATION_PRESENT) != 0
}

fn present_window_id(identifier: libnanami::Word) -> libnanami::Word {
    identifier & 0xffff_ffff
}

fn render_until_idle(compositor: &mut Compositor) {
    while compositor.render_if_needed() {}
}

fn log_framebuffer_ready(framebuffer: &crate::framebuffer::Framebuffer) {
    let screen = framebuffer.screen();
    libnanami::println!(
        "[honoka] framebuffer ready vaddr={:#x} bytes={:#x} size={}x{} stride={} bpp={}",
        framebuffer.vaddr(),
        framebuffer.bytes(),
        screen.width,
        screen.height,
        screen.stride_bytes,
        screen.bits_per_pixel
    );
}

fn pump_input(
    input_queue: &mut nanami_services::input::InputEventQueue,
    compositor: &mut Compositor,
) -> usize {
    let mut processed = 0usize;
    let mut pending_dx = 0i32;
    let mut pending_dy = 0i32;
    let mut pending_moves = 0usize;
    while processed < MAX_INPUT_EVENTS_PER_FRAME {
        let Some(packed) = input_queue.pop() else {
            break;
        };
        let event = decode_input_event(packed);

        match event {
            InputEvent::MouseMove { dx, dy } => {
                pending_dx = pending_dx.saturating_add(dx);
                pending_dy = pending_dy.saturating_add(dy);
                pending_moves += 1;
                if pending_moves >= MAX_COALESCED_MOUSE_MOVES {
                    flush_mouse_move(compositor, &mut pending_dx, &mut pending_dy);
                    pending_moves = 0;
                }
            }
            _ => {
                flush_mouse_move(compositor, &mut pending_dx, &mut pending_dy);
                pending_moves = 0;
                compositor.process_input(event);
            }
        }
        processed += 1;
    }
    flush_mouse_move(compositor, &mut pending_dx, &mut pending_dy);
    processed
}

fn flush_mouse_move(compositor: &mut Compositor, dx: &mut i32, dy: &mut i32) {
    if *dx != 0 || *dy != 0 {
        compositor.process_input(InputEvent::MouseMove { dx: *dx, dy: *dy });
        *dx = 0;
        *dy = 0;
    }
}

libnanami::nanami_entry!(nanami_main);
