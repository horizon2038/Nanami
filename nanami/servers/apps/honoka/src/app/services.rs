use libnanami::Word;

use crate::constants::{
    CONNECT_RETRY_MS, SLOT_DISPLAY_SERVICE, SLOT_INPUT_SERVICE, SLOT_RTC_SERVICE,
    SLOT_TIMER_SERVICE,
};
use crate::framebuffer::{parse_screen_info, Framebuffer};
use crate::logging::{busy_delay, log_error, log_request_error};

pub struct ServicePorts {
    pub timer: Word,
    pub display: Word,
    pub input: Word,
    pub rtc: Word,
}

pub fn connect_services() -> Result<ServicePorts, libnanami::NanamiError> {
    let timer = connect_timer_service();
    let display = connect_display_service(timer)?;
    let input = connect_input_service(timer)?;
    let rtc = connect_rtc_service(timer)?;

    Ok(ServicePorts {
        timer,
        display,
        input,
        rtc,
    })
}

pub fn prepare_framebuffer(display_port: Word) -> Result<Framebuffer, libnanami::NanamiError> {
    let (detail0, detail1) = nanami_services::gfx::display_service_get_screen_info(display_port)
        .map_err(|e| log_error("[honoka] get screen info failed: ", e))?;
    let screen = parse_screen_info(detail0, detail1);

    if screen.bits_per_pixel != 32 {
        libnanami::println!("[honoka] unsupported bpp={}", screen.bits_per_pixel);
        return Err(libnanami::NanamiError::UNSUPPORTED);
    }

    let (framebuffer_vaddr, framebuffer_size) =
        nanami_services::gfx::display_service_prepare_shared_framebuffer(display_port)
            .map_err(|e| log_error("[honoka] prepare shared framebuffer failed: ", e))?;

    Framebuffer::new(framebuffer_vaddr, framebuffer_size, screen)
}

pub fn subscribe_input(input_port: Word) -> Result<(Word, Word), libnanami::NanamiError> {
    nanami_services::input::input_service_subscribe_shared(
        input_port,
        nanami_services::input::INPUT_SUBSCRIBE_ALL,
    )
    .map_err(|e| log_error("[honoka] input subscribe failed: ", e))
}

pub fn sleep_ms(timer_port: Word, milliseconds: Word) {
    let _ = nanami_services::timer::timer_service_sleep_milliseconds(timer_port, milliseconds);
}

fn connect_display_service(timer_port: Word) -> Result<Word, libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_display_service(SLOT_DISPLAY_SERVICE) {
            Ok(()) => {
                return Ok(libnanami::ipc::process_slot_descriptor(
                    SLOT_DISPLAY_SERVICE,
                ))
            }
            Err(e) => {
                log_request_error("[honoka] waiting display_service: ", e);
                sleep_ms(timer_port, CONNECT_RETRY_MS);
            }
        }
    }
}

fn connect_input_service(timer_port: Word) -> Result<Word, libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_input_service(SLOT_INPUT_SERVICE) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_INPUT_SERVICE)),
            Err(e) => {
                log_request_error("[honoka] waiting input-service: ", e);
                sleep_ms(timer_port, CONNECT_RETRY_MS);
            }
        }
    }
}

fn connect_rtc_service(timer_port: Word) -> Result<Word, libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_rtc_service(SLOT_RTC_SERVICE) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_RTC_SERVICE)),
            Err(e) => {
                log_request_error("[honoka] waiting rtc-service: ", e);
                sleep_ms(timer_port, CONNECT_RETRY_MS);
            }
        }
    }
}

fn connect_timer_service() -> Word {
    loop {
        match nanami_services::registry::connect_timer_service(SLOT_TIMER_SERVICE) {
            Ok(()) => return libnanami::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE),
            Err(e) => {
                log_request_error("[honoka] waiting timer-service: ", e);
                busy_delay();
            }
        }
    }
}
