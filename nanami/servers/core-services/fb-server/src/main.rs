#![no_std]
#![no_main]

extern crate alloc;
mod damage;
mod gpu;
mod presentation;
mod profile;
use presentation::present_shared_framebuffer;

use core::sync::atomic::{fence, Ordering};
use nanami_runtime::ipc::{ServiceEvent, ServiceRequest};
use nanami_runtime::{self, RequestError, Word};

const SLOT_SERVICE_PORT: Word = 20;
const PAGE_SIZE: Word = 4096;

#[derive(Clone, Copy)]
struct ScreenInfo {
    display_id: Word,
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

#[derive(Clone, Copy)]
struct SharedFramebuffer {
    owner_pid: Word,
    local_vaddr: Word,
    peer_vaddr: Word,
    bytes: Word,
}

impl SharedFramebuffer {
    const EMPTY: Self = Self {
        owner_pid: 0,
        local_vaddr: 0,
        peer_vaddr: 0,
        bytes: 0,
    };
}

struct DisplayState {
    screen: ScreenInfo,
    hardware_vaddr: Word,
    shared: SharedFramebuffer,
    shadow: Option<damage::Shadow>,
    profile: profile::Profile,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::print!("[fb-server] panic\n");
    let _ = nanami_runtime::request_exit();
    loop {}
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    if let Err(e) = nanami_runtime::ipc::init_ipc_tls() {
        return Err(log_error("[fb-server] ipc tls init failed: ", e));
    }

    let (fb_phys, fb_size) = match nanami_runtime::request_initial_framebuffer_information(
        nanami_runtime::FRAMEBUFFER_INFORMATION_REGION,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer region request failed: ",
                e,
            ))
        }
    };

    let (fb_width, fb_height) = match nanami_runtime::request_initial_framebuffer_information(
        nanami_runtime::FRAMEBUFFER_INFORMATION_GEOMETRY,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer geometry request failed: ",
                e,
            ))
        }
    };

    let (fb_stride, fb_bpp) = match nanami_runtime::request_initial_framebuffer_information(
        nanami_runtime::FRAMEBUFFER_INFORMATION_FORMAT,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Err(log_error(
                "[fb-server] framebuffer format request failed: ",
                e,
            ))
        }
    };

    let (display_id, color_info) = match nanami_runtime::request_initial_framebuffer_information(
        nanami_runtime::FRAMEBUFFER_INFORMATION_COLOR_AND_ID,
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

    if fb_size == 0 || fb_width == 0 || fb_height == 0 || fb_bpp != 32 {
        return Err(log_error(
            "[fb-server] framebuffer region request failed: ",
            RequestError::InvalidArgument,
        ));
    }

    // Map HW framebuffer in fb-server itself; compositor-facing path is shared memory.
    let fb_base = fb_phys & !(PAGE_SIZE - 1);
    let fb_offset = fb_phys - fb_base;
    let map_size = align_up(fb_offset.saturating_add(fb_size), PAGE_SIZE);

    let (_, mapped_base) = match nanami_runtime::request_mmio_with_mapping(
        fb_base,
        map_size,
        nanami_runtime::Attribute::BUFFERED,
    ) {
        Ok(v) => v,
        Err(e) => return Err(log_error("[fb-server] framebuffer mmio map failed: ", e)),
    };

    let mapped_fb = mapped_base.saturating_add(fb_offset);
    let framebuffer_bytes = fb_size;
    let stride_bytes = normalize_stride_bytes(fb_stride, fb_width, fb_bpp)
        .ok_or(nanami_runtime::NanamiError::INVALID_ARGUMENT)?;
    let visible_bytes = stride_bytes.checked_mul(fb_height);
    if visible_bytes.is_none_or(|bytes| bytes > framebuffer_bytes) {
        return Err(log_error(
            "[fb-server] framebuffer stride exceeds mapped region: ",
            RequestError::InvalidArgument,
        ));
    }

    let screen_info = ScreenInfo {
        display_id,
        width: fb_width,
        height: fb_height,
        stride: stride_bytes,
        bits_per_pixel: fb_bpp,
        red_position: color.0,
        red_size: color.1,
        green_position: color.2,
        green_size: color.3,
        blue_position: color.4,
        blue_size: color.5,
        framebuffer_bytes,
    };

    nanami_runtime::print!("[fb-server] framebuffer mapped paddr=");
    nanami_runtime::print!("{:#x}", fb_phys);
    nanami_runtime::print!(" vaddr=");
    nanami_runtime::print!("{:#x}", mapped_fb);
    nanami_runtime::print!(" bytes=");
    nanami_runtime::print!("{:#x}", framebuffer_bytes);
    nanami_runtime::print!("\n");

    nanami_runtime::println!(
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

    let service_port = nanami_runtime::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let mut pending_status = (nanami_runtime::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;
    let mut gpu_service = nanami_gpu::Service::default();

    fill_screen(&screen_info, mapped_fb);
    let mut display = DisplayState {
        screen: screen_info,
        hardware_vaddr: mapped_fb,
        shared: SharedFramebuffer::EMPTY,
        shadow: if option_env!("NANAMI_FB_CACHE") == Some("0") {
            None
        } else {
            damage::Shadow::new(visible_bytes.unwrap())
        },
        profile: profile::Profile::new(),
    };
    nanami_runtime::println!(
        "[fb-server] unchanged-pixel cache bytes={:#x}",
        if display.shadow.is_some() {
            visible_bytes.unwrap()
        } else {
            0
        }
    );

    // Publish only a usable backend. A client connecting to an uninitialized
    // service would otherwise block forever if mapping/validation failed.
    if let Err(e) = nanami_client::registry::register_display_service() {
        return Err(log_error("[fb-server] service register failed: ", e));
    }
    nanami_runtime::print!("[fb-server] service registered: display_service\n");
    if let Err(error) = nanami_client::registry::register_service(nanami_client::gpu::GPU_SERVICE) {
        nanami_runtime::println!("[fb-server] GPU service alias unavailable: {}", error);
    }

    loop {
        let used_reply_receive = has_pending_reply;
        let event = if used_reply_receive {
            match nanami_runtime::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[fb-server] reply_receive failed: ", e)),
            }
        } else {
            match nanami_runtime::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[fb-server] receive failed: ", e)),
            }
        };
        if used_reply_receive {
            has_pending_reply = false;
        }

        match event {
            ServiceEvent::Request(request) => {
                pending_status = if nanami_gpu::Service::handles(request.code) {
                    match gpu_service.request(
                        request.identifier,
                        request.code,
                        [request.arg0, request.arg1, request.arg2, request.arg3],
                        &mut gpu::Adapter(&mut display),
                    ) {
                        Ok((a, b)) => (nanami_runtime::OS_RESPONSE_OK, a, b),
                        Err(error) => (map_request_error_to_status(error), 0, 0),
                    }
                } else {
                    handle_request(request, &mut display)
                };
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                nanami_runtime::print!("[fb-server] fault id=");
                nanami_runtime::print!("{}", identifier);
                nanami_runtime::print!(" reason=");
                nanami_runtime::print!("{:#x}", reason);
                nanami_runtime::print!("\n");
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
    presentation::complete_framebuffer_writes();
}

fn handle_request(request: ServiceRequest, display: &mut DisplayState) -> (Word, Word, Word) {
    match request.code {
        nanami_client::gfx::DISPLAY_SERVICE_REQUEST_GET_CAPABILITIES => (
            nanami_runtime::OS_RESPONSE_OK,
            nanami_client::gfx::DISPLAY_CAP_GPU_INTERFACE,
            0,
        ),
        nanami_client::gfx::DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO => {
            let detail0 = pack_screen_info_detail0(&display.screen);
            let detail1 = pack_screen_info_detail1(&display.screen);
            (nanami_runtime::OS_RESPONSE_OK, detail0, detail1)
        }
        nanami_client::gfx::DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER => {
            if request.identifier == 0 {
                return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }
            if display.shared.owner_pid == request.identifier && display.shared.local_vaddr != 0 {
                return (
                    nanami_runtime::OS_RESPONSE_OK,
                    display.shared.peer_vaddr,
                    display.shared.bytes,
                );
            }
            if display.shared.owner_pid != 0 && display.shared.owner_pid != request.identifier {
                return (nanami_runtime::OS_RESPONSE_PERMISSION_DENIED, 0, 0);
            }

            match nanami_runtime::request_shared_memory(
                request.identifier,
                display.screen.framebuffer_bytes,
            ) {
                Ok((local_vaddr, peer_vaddr)) => {
                    unsafe {
                        core::ptr::write_bytes(
                            local_vaddr as *mut u8,
                            0,
                            display.screen.framebuffer_bytes as usize,
                        );
                    }
                    display.shared = SharedFramebuffer {
                        owner_pid: request.identifier,
                        local_vaddr,
                        peer_vaddr,
                        bytes: display.screen.framebuffer_bytes,
                    };
                    nanami_runtime::println!(
                        "[fb-server] shared framebuffer ready pid={} vaddr={:#x} bytes={:#x}",
                        request.identifier,
                        peer_vaddr,
                        display.screen.framebuffer_bytes
                    );
                    (
                        nanami_runtime::OS_RESPONSE_OK,
                        peer_vaddr,
                        display.screen.framebuffer_bytes,
                    )
                }
                Err(error) => (map_request_error_to_status(error), 0, 0),
            }
        }
        nanami_client::gfx::DISPLAY_SERVICE_REQUEST_PRESENT => {
            let before = display.profile.now();
            let result =
                present_shared_framebuffer(display, request.identifier, request.arg0, request.arg1);
            match result {
                Ok((pixels, written)) => {
                    display.profile.record(before, pixels, written);
                    (nanami_runtime::OS_RESPONSE_OK, pixels, 0)
                }
                Err(error) => (map_request_error_to_status(error), 0, 0),
            }
        }
        _ => (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
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

fn normalize_stride_bytes(stride: Word, width: Word, bits_per_pixel: Word) -> Option<Word> {
    let bytes_per_pixel = bits_per_pixel.checked_add(7)?.checked_div(8)?;
    if stride == 0 || width == 0 || bytes_per_pixel == 0 {
        return None;
    }
    let minimum_row_bytes = width.checked_mul(bytes_per_pixel)?;
    if stride >= minimum_row_bytes {
        Some(stride)
    } else {
        stride.checked_mul(bytes_per_pixel)
    }
}

fn map_request_error_to_status(err: RequestError) -> Word {
    match err {
        RequestError::InvalidArgument => nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => nanami_runtime::OS_RESPONSE_FATAL,
        RequestError::Status(code) => code,
    }
}

fn log_error(prefix: &str, err: RequestError) -> nanami_runtime::NanamiError {
    log_request_error(prefix, err);
    err.into()
}

fn log_request_error(prefix: &str, err: RequestError) {
    nanami_runtime::println!("{}{}", prefix, err);
}

nanami_runtime::nanami_entry!(nanami_main);
