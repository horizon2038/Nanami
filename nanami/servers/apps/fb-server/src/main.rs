#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};

const SLOT_SERVICE_PORT: Word = 20;
const PAGE_SIZE: Word = 4096;

struct ScreenInfo {
    display_id: Word,
    framebuffer_phys: Word,
    width: Word,
    height: Word,
    stride: Word,
    bits_per_pixel: Word,
    red_position: Word,
    red_size: Word,
    green_position: Word,
    green_size: Word,
    blue_position: Word,
    blue_size: Word,
    framebuffer_bytes: Word,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[fb-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    if let Err(e) = libnanami::ipc::init_ipc_tls() {
        return Err(log_error("[fb-server] ipc tls init failed: ", e));
    }

    if let Err(e) = nanami_services::registry::register_display_service() {
        return Err(log_error("[fb-server] service register failed: ", e));
    }
    libnanami::print!("[fb-server] service registered: display_service\n");

    let (fb_phys, fb_size) = match libnanami::request_initial_framebuffer_information(
        libnanami::FRAMEBUFFER_INFORMATION_REGION,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer region request failed: ",
                e,
            ))
        }
    };

    let (fb_width, fb_height) = match libnanami::request_initial_framebuffer_information(
        libnanami::FRAMEBUFFER_INFORMATION_GEOMETRY,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer geometry request failed: ",
                e,
            ))
        }
    };

    let (fb_stride, fb_bpp) = match libnanami::request_initial_framebuffer_information(
        libnanami::FRAMEBUFFER_INFORMATION_FORMAT,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer format request failed: ",
                e,
            ))
        }
    };

    let (display_id, color_info) = match libnanami::request_initial_framebuffer_information(
        libnanami::FRAMEBUFFER_INFORMATION_COLOR_AND_ID,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer color request failed: ",
                e,
            ))
        }
    };

    let color = unpack_color_info(color_info);

    if fb_size == 0 {
        return Err(log_error(
            "[fb-server] framebuffer region request failed: ",
            RequestError::InvalidArgument,
        ));
    }

    // Map HW framebuffer in fb-server itself; compositor-facing path is shared memory.
    let fb_base = fb_phys & !(PAGE_SIZE - 1);
    let fb_offset = fb_phys - fb_base;
    let map_size = align_up(fb_offset.saturating_add(fb_size), PAGE_SIZE);

    let (_, mapped_base) = match libnanami::request_mmio(fb_base, map_size) {
        Ok(v) => v,
        Err(e) => return Err(log_error("[fb-server] framebuffer mmio map failed: ", e)),
    };

    let mapped_fb = mapped_base.saturating_add(fb_offset);
    let framebuffer_bytes = fb_size;

    let screen_info = ScreenInfo {
        display_id,
        framebuffer_phys: fb_phys,
        width: fb_width,
        height: fb_height,
        stride: fb_stride,
        bits_per_pixel: fb_bpp,
        red_position: color.0,
        red_size: color.1,
        green_position: color.2,
        green_size: color.3,
        blue_position: color.4,
        blue_size: color.5,
        framebuffer_bytes,
    };

    libnanami::print!("[fb-server] framebuffer mapped paddr=");
    libnanami::print!("{:#x}", fb_phys);
    libnanami::print!(" vaddr=");
    libnanami::print!("{:#x}", mapped_fb);
    libnanami::print!(" bytes=");
    libnanami::print!("{:#x}", framebuffer_bytes);
    libnanami::print!("\n");

    libnanami::println!(
        "[fb-server] resolution={}x{}, R={}:{}, G={}:{}, B={}:{}",
        screen_info.width,
        screen_info.height,
        screen_info.red_position,
        screen_info.red_position + screen_info.red_size,
        screen_info.green_position,
        screen_info.green_position + screen_info.green_size,
        screen_info.blue_position,
        screen_info.blue_position + screen_info.blue_size
    );

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending_status = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    fill_screen(&screen_info, mapped_fb);

    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[fb-server] reply_receive failed: ", e)),
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[fb-server] receive failed: ", e)),
            }
        };
        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(request) => {
                pending_status = handle_request(request, &screen_info);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::print!("[fb-server] fault id=");
                libnanami::print!("{}", identifier);
                libnanami::print!(" reason=");
                libnanami::print!("{:#x}", reason);
                libnanami::print!("\n");
            }
        }
    }
}

// draw white background:
fn fill_screen(screen_info: &ScreenInfo, mapped_address: usize) {
    // 1. calculate the total size of the framebuffer
    let screen_size_bytes = screen_info.framebuffer_bytes;

    // 2. create a slice to the framebuffer memory u8[size]
    let framebuffer = unsafe {
        core::slice::from_raw_parts_mut(mapped_address as *mut u8, screen_size_bytes as usize)
    };

    // 3. fill the framebuffer with white color (0xFFFFFFFF for 32bpp)
    for chunk in framebuffer.chunks_exact_mut((screen_info.bits_per_pixel / 8) as usize) {
        chunk.copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    }
}

fn handle_request(request: ServiceRequest, screen_info: &ScreenInfo) -> (Word, Word, Word) {
    match request.code {
        nanami_services::gfx::DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO => {
            let detail0 = pack_screen_info_detail0(screen_info);
            let detail1 = pack_screen_info_detail1(screen_info);
            (libnanami::OS_RESPONSE_OK, detail0, detail1)
        }
        nanami_services::gfx::DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER => {
            if request.identifier == 0 {
                return (libnanami::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }

            match libnanami::request_shared_framebuffer_memory(
                request.identifier,
                screen_info.framebuffer_phys,
                screen_info.framebuffer_bytes,
            ) {
                Ok((_local_vaddr, peer_vaddr)) => {
                    libnanami::print!("[fb-server] shared framebuffer ready pid=");
                    libnanami::print!("{}", request.identifier);
                    libnanami::print!(" vaddr=");
                    libnanami::print!("{:#x}", peer_vaddr);
                    libnanami::print!(" bytes=");
                    libnanami::print!("{:#x}", screen_info.framebuffer_bytes);
                    libnanami::print!("\n");
                    (
                        libnanami::OS_RESPONSE_OK,
                        peer_vaddr,
                        screen_info.framebuffer_bytes,
                    )
                }
                Err(e) => (map_request_error_to_status(e), 0, 0),
            }
        }
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn pack_screen_info_detail0(info: &ScreenInfo) -> Word {
    (info.display_id & 0xffff)
        | ((info.bits_per_pixel & 0xffff) << 16)
        | ((info.width & 0xffff) << 32)
        | ((info.height & 0xffff) << 48)
}

fn pack_screen_info_detail1(info: &ScreenInfo) -> Word {
    (info.stride & 0xffff_ffff)
        | ((info.red_position & 0x1f) << 32)
        | ((info.red_size & 0x1f) << 37)
        | ((info.green_position & 0x1f) << 42)
        | ((info.green_size & 0x1f) << 47)
        | ((info.blue_position & 0x1f) << 52)
        | ((info.blue_size & 0x1f) << 57)
}

fn unpack_color_info(packed: Word) -> (Word, Word, Word, Word, Word, Word) {
    (
        packed & 0x1f,
        (packed >> 5) & 0x1f,
        (packed >> 10) & 0x1f,
        (packed >> 15) & 0x1f,
        (packed >> 20) & 0x1f,
        (packed >> 25) & 0x1f,
    )
}

fn align_up(value: Word, align: Word) -> Word {
    if align == 0 {
        return value;
    }
    let mask = align - 1;
    if (value & mask) == 0 {
        value
    } else {
        (value + mask) & !mask
    }
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
        RequestError::Status(code) => code,
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
