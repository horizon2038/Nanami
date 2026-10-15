#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

#[path = "app/background.rs"]
pub mod background;
#[path = "app/compositor.rs"]
pub mod compositor;
#[path = "app/constants.rs"]
pub mod constants;
#[path = "app/font.rs"]
pub mod font;
#[path = "app/framebuffer.rs"]
pub mod framebuffer;
#[path = "app/info_panel.rs"]
pub mod info_panel;
#[path = "app/input.rs"]
pub mod input;
#[path = "app/logging.rs"]
pub mod logging;
#[path = "app/motion_damage.rs"]
mod motion_damage;
#[path = "app/profile.rs"]
mod profile;
#[path = "app/server.rs"]
pub mod server;
#[path = "app/services.rs"]
pub mod services;

use nanami_runtime::ipc::ServiceEvent;

use crate::compositor::Compositor;
use crate::constants::{
    MAX_COALESCED_MOUSE_MOVES, MAX_INPUT_EVENTS_PER_FRAME, SLOT_NOTIFICATION, SLOT_SERVICE_PORT,
};
use crate::font::TextRenderer;
use crate::input::{decode_input_event, InputEvent};
use crate::logging::log_error;
use crate::server::handle_request;
use crate::services::{connect_services, load_honoka_theme, prepare_framebuffer, subscribe_input};

const THEME_DATA_MAX: usize = 0x800;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[honoka] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {}
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    let (used, remaining, total) = nanami_runtime::heap::heap_stats();
    nanami_runtime::println!(
        "[honoka] allocation failed size={:#x} align={:#x} heap-used={:#x} heap-rem={:#x} heap-total={:#x}",
        layout.size(),
        layout.align(),
        used,
        remaining,
        total
    );
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::print!("[honoka] start\n");
    nanami_runtime::ipc::init_ipc_tls()
        .map_err(|e| log_error("[honoka] ipc tls init failed: ", e))?;
    nanami_runtime::print!("[honoka] ipc ready\n");
    let (heap_base, heap_size) = nanami_runtime::heap::init_heap(9 * 1024 * 1024)
        .map_err(|e| log_error("[honoka] heap init failed: ", e))?;
    nanami_runtime::println!(
        "[honoka] heap ready vaddr={:#x} bytes={:#x}",
        heap_base,
        heap_size
    );
    let text = TextRenderer::new();

    nanami_client::registry::register_honoka_service()
        .map_err(|e| log_error("[honoka] service register failed: ", e))?;
    nanami_runtime::print!("[honoka] service registered: honoka-service\n");

    let notification = nanami_runtime::ipc::process_slot_descriptor(SLOT_NOTIFICATION);
    nanami_runtime::ipc::bind_current_thread_notification(notification)
        .map_err(|e| log_error("[honoka] bind notification failed: ", e))?;
    nanami_runtime::print!("[honoka] notification bound\n");

    nanami_runtime::print!("[honoka] connect services start\n");
    let ports = connect_services()?;
    nanami_runtime::print!("[honoka] services connected\n");
    let framebuffer = prepare_framebuffer(ports.display)?;
    log_framebuffer_ready(&framebuffer);
    let (input_queue_vaddr, input_queue_bytes) = subscribe_input(ports.input)?;
    nanami_runtime::print!("[honoka] input subscribed\n");
    nanami_runtime::println!(
        "[honoka] input queue vaddr={:#x} bytes={:#x}",
        input_queue_vaddr,
        input_queue_bytes
    );

    let mut theme_data = [0u8; THEME_DATA_MAX];
    let theme_len = load_honoka_theme(&ports, &mut theme_data)?;
    nanami_runtime::println!("[honoka] theme loaded bytes={:#x}", theme_len);
    let mut compositor = Compositor::new(
        framebuffer,
        text,
        ports.exec,
        ports.exec_shm,
        ports.exec_shm_size,
        ports.timer,
        &theme_data[..theme_len],
        services::load_desktop_info(),
    )
    .ok_or_else(|| {
        nanami_runtime::print!("[honoka] invalid theme file\n");
        nanami_runtime::NanamiError::INVALID_ARGUMENT
    })?;
    refresh_clock(ports.rtc, &mut compositor);
    start_clock_timer(ports.timer);
    nanami_runtime::print!("[honoka] initial render start\n");
    while compositor.render_if_needed() {}
    nanami_runtime::print!("[honoka] initial render done\n");

    nanami_runtime::print!("[honoka] online\n");
    nanami_runtime::diagnostics::clock_info("honoka");
    let mut input_queue = nanami_client::input::InputEventQueue::new(input_queue_vaddr);
    run_event_loop(ports.rtc, &mut compositor, &mut input_queue)
}

fn run_event_loop(
    rtc_port: nanami_runtime::Word,
    compositor: &mut Compositor,
    input_queue: &mut nanami_client::input::InputEventQueue,
) -> nanami_runtime::NanamiResult {
    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending_reply = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
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
            nanami_runtime::ipc::service_reply_receive_event(
                service_port,
                pending_reply.0,
                pending_reply.1,
                pending_reply.2,
            )
            .map_err(|e| log_error("[honoka] reply_receive failed: ", e))?
        } else {
            nanami_runtime::ipc::service_receive_event(service_port)
                .map_err(|e| log_error("[honoka] receive failed: ", e))?
        };

        match event {
            ServiceEvent::Request(request) => {
                pending_reply = handle_request(request, compositor);
                has_pending_reply = true;
                render_until_idle(compositor);
            }
            ServiceEvent::Notification { identifier, .. } => {
                let received = nanami_runtime::diagnostics::stamp();
                handle_notification(identifier, rtc_port, input_queue, compositor);
                render_until_idle(compositor);
                nanami_runtime::diagnostics::wake(
                    nanami_runtime::diagnostics::InputStage::Honoka,
                    identifier,
                    received,
                );
            }
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                nanami_runtime::println!("[honoka] fault id={} reason={:#x}", identifier, reason);
            }
        }
    }
}

fn handle_notification(
    identifier: nanami_runtime::Word,
    rtc_port: nanami_runtime::Word,
    input_queue: &mut nanami_client::input::InputEventQueue,
    compositor: &mut Compositor,
) -> usize {
    if is_present_notification(identifier) {
        compositor.invalidate_presented_logical_framebuffer(present_window_id(identifier));
    }
    if is_timer_notification(identifier) {
        compositor.reap_dead_windows();
        compositor.drain_presented_logical_framebuffers();
        refresh_clock(rtc_port, compositor);
    }
    pump_input(input_queue, compositor)
}

fn start_clock_timer(timer_port: nanami_runtime::Word) {
    if let Err(e) = nanami_client::timer::timer_service_interval_on_notification_milliseconds(
        timer_port,
        1000,
        SLOT_NOTIFICATION,
    ) {
        nanami_runtime::println!("[honoka] clock timer start failed: {:?}", e);
    }
}

fn refresh_clock(rtc_port: nanami_runtime::Word, compositor: &mut Compositor) {
    match nanami_client::rtc::rtc_service_read(rtc_port) {
        Ok(dt) => compositor.set_clock(dt.hour, dt.minute, dt.second),
        Err(e) => nanami_runtime::println!("[honoka] rtc read failed: {:?}", e),
    }
}

fn is_timer_notification(identifier: nanami_runtime::Word) -> bool {
    (identifier & nanami_client::timer::TIMER_NOTIFICATION_IDENTIFIER_BIT) != 0
}

fn is_present_notification(identifier: nanami_runtime::Word) -> bool {
    (identifier & nanami_client::gfx::honoka::HONOKA_NOTIFICATION_PRESENT) != 0
}

fn present_window_id(identifier: nanami_runtime::Word) -> nanami_runtime::Word {
    identifier & 0xffff_ffff
}

fn render_until_idle(compositor: &mut Compositor) {
    let before = nanami_runtime::diagnostics::stamp();
    while compositor.render_if_needed() {}
    nanami_runtime::diagnostics::rpc(
        nanami_runtime::diagnostics::RpcStage::HonokaRender,
        before,
        nanami_runtime::diagnostics::stamp(),
        0,
        false,
    );
}

fn log_framebuffer_ready(framebuffer: &crate::framebuffer::Framebuffer) {
    let screen = framebuffer.screen();
    nanami_runtime::println!(
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
    input_queue: &mut nanami_client::input::InputEventQueue,
    compositor: &mut Compositor,
) -> usize {
    let mut processed = 0usize;
    let mut pending_dx = 0i32;
    let mut pending_dy = 0i32;
    let mut pending_moves = 0usize;
    let mut trace_keys = 0;
    let mut trace_tag = 0;
    let mut trace_before = None;
    while processed < MAX_INPUT_EVENTS_PER_FRAME {
        let Some(packed) = input_queue.pop() else {
            break;
        };
        let event = decode_input_event(packed);
        if nanami_runtime::diagnostics::ENABLED
            && packed & 255 == nanami_client::input::INPUT_EVENT_KIND_KEY
        {
            if trace_keys == 0 {
                trace_before = nanami_runtime::diagnostics::stamp();
            }
            trace_keys += 1;
            trace_tag = packed >> 56;
        }

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
    nanami_runtime::diagnostics::keys(
        nanami_runtime::diagnostics::InputStage::Honoka,
        trace_keys,
        trace_tag,
        trace_before,
        0,
        0,
    );
    processed
}

fn flush_mouse_move(compositor: &mut Compositor, dx: &mut i32, dy: &mut i32) {
    if *dx != 0 || *dy != 0 {
        compositor.process_input(InputEvent::MouseMove { dx: *dx, dy: *dy });
        *dx = 0;
        *dy = 0;
    }
}

nanami_runtime::nanami_entry!(nanami_main);
