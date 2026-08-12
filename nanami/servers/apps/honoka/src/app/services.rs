use libnanami::Word;

use crate::constants::{
    CONNECT_RETRY_MS, SLOT_DISPLAY_SERVICE, SLOT_EXEC_SERVICE, SLOT_INPUT_SERVICE,
    SLOT_RTC_SERVICE, SLOT_TIMER_SERVICE, SLOT_VFS_SERVICE,
};
use crate::framebuffer::{parse_screen_info, Framebuffer};
use crate::logging::{busy_delay, log_error, log_request_error};

pub struct ServicePorts {
    pub timer: Word,
    pub display: Word,
    pub input: Word,
    pub rtc: Word,
    pub exec: Word,
    pub exec_shm: Word,
    pub exec_shm_size: Word,
    pub vfs: Word,
    pub vfs_shm: Word,
    pub vfs_shm_size: Word,
}

const VFS_SHM_BYTES: Word = 0x1000;
const VFS_READ_OFFSET: usize = 0x200;
const HONOKA_CONFIG_PATH: &[u8] = b"/.honoka/config";
const HONOKA_THEME_DIRECTORY: &[u8] = b"/.honoka/";
const HONOKA_THEME_SUFFIX: &[u8] = b".theme";
const HONOKA_THEME_PATH_MAX: usize = 128;
const HONOKA_CONFIG_MAX: usize = 256;

pub fn connect_services() -> Result<ServicePorts, libnanami::NanamiError> {
    let timer = connect_timer_service();
    let display = connect_display_service(timer)?;
    let input = connect_input_service(timer)?;
    let rtc = connect_rtc_service(timer)?;
    let exec = connect_exec_service(timer)?;
    let (exec_shm, exec_shm_size) = nanami_services::exec::exec_attach_shared_memory(
        exec,
        nanami_services::exec::EXEC_DEFAULT_SHM_BYTES,
    )
    .map_err(|e| log_error("[honoka] exec shm attach failed: ", e))?;
    let vfs = connect_vfs_service(timer)?;
    let (vfs_shm, vfs_shm_size) =
        nanami_services::vfs::vfs_attach_shared_memory(vfs, VFS_SHM_BYTES)
            .map_err(|e| log_error("[honoka] vfs shm attach failed: ", e))?;

    Ok(ServicePorts {
        timer,
        display,
        input,
        rtc,
        exec,
        exec_shm,
        exec_shm_size,
        vfs,
        vfs_shm,
        vfs_shm_size,
    })
}

pub fn load_honoka_theme(
    ports: &ServicePorts,
    output: &mut [u8],
) -> Result<usize, libnanami::NanamiError> {
    let mut config = [0u8; HONOKA_CONFIG_MAX];
    let config_len = read_vfs_file(
        ports.vfs,
        ports.vfs_shm,
        ports.vfs_shm_size,
        HONOKA_CONFIG_PATH,
        &mut config,
    )?;
    let theme_name = configured_theme_name(&config[..config_len])
        .ok_or(libnanami::NanamiError::INVALID_ARGUMENT)?;
    let mut path = [0u8; HONOKA_THEME_PATH_MAX];
    let path_len = HONOKA_THEME_DIRECTORY
        .len()
        .checked_add(theme_name.len())
        .filter(|len| *len <= path.len())
        .ok_or(libnanami::NanamiError::INVALID_ARGUMENT)?;
    path[..HONOKA_THEME_DIRECTORY.len()].copy_from_slice(HONOKA_THEME_DIRECTORY);
    path[HONOKA_THEME_DIRECTORY.len()..path_len].copy_from_slice(theme_name);
    read_vfs_file(
        ports.vfs,
        ports.vfs_shm,
        ports.vfs_shm_size,
        &path[..path_len],
        output,
    )
}

fn read_vfs_file(
    vfs_port: Word,
    vfs_shm: Word,
    vfs_shm_size: Word,
    path: &[u8],
    output: &mut [u8],
) -> Result<usize, libnanami::NanamiError> {
    if path.is_empty() || path.len() > VFS_READ_OFFSET || VFS_READ_OFFSET >= vfs_shm_size as usize {
        return Err(libnanami::NanamiError::INVALID_ARGUMENT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), vfs_shm as *mut u8, path.len());
    }
    let handle = nanami_services::vfs::vfs_open(vfs_port, 0, path.len() as Word)
        .map_err(|e| log_error("[honoka] theme open failed: ", e))?;
    let read_capacity = output.len().min(vfs_shm_size as usize - VFS_READ_OFFSET);
    let read_result = nanami_services::vfs::vfs_read(
        vfs_port,
        handle,
        0,
        read_capacity as Word,
        VFS_READ_OFFSET as Word,
    );
    let close_result = nanami_services::vfs::vfs_close(vfs_port, handle);
    let bytes = read_result.map_err(|e| log_error("[honoka] theme read failed: ", e))? as usize;
    close_result.map_err(|e| log_error("[honoka] theme close failed: ", e))?;
    if bytes == 0 || bytes > read_capacity {
        return Err(libnanami::NanamiError::INVALID_ARGUMENT);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            (vfs_shm as usize + VFS_READ_OFFSET) as *const u8,
            output.as_mut_ptr(),
            bytes,
        );
    }
    Ok(bytes)
}

fn configured_theme_name(config: &[u8]) -> Option<&[u8]> {
    let mut start = 0usize;
    while start < config.len() {
        let mut end = start;
        while end < config.len() && config[end] != b'\n' {
            end += 1;
        }
        let line = trim_ascii(&config[start..end]);
        if !line.is_empty() && line[0] != b';' {
            if let Some(separator) = line.iter().position(|byte| *byte == b'=') {
                let key = trim_ascii(&line[..separator]);
                let value = trim_ascii(&line[separator + 1..]);
                if key == b"theme" && valid_theme_name(value) {
                    return Some(value);
                }
            }
        }
        start = end.saturating_add(1);
    }
    None
}

fn valid_theme_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.ends_with(HONOKA_THEME_SUFFIX)
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'-' | b'_'))
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(|byte| byte.is_ascii_whitespace()) {
        value = &value[1..];
    }
    while value.last().is_some_and(|byte| byte.is_ascii_whitespace()) {
        value = &value[..value.len() - 1];
    }
    value
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

    Framebuffer::new(display_port, framebuffer_vaddr, framebuffer_size, screen)
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

fn connect_exec_service(timer_port: Word) -> Result<Word, libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_exec_service(SLOT_EXEC_SERVICE) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_EXEC_SERVICE)),
            Err(e) => {
                log_request_error("[honoka] waiting exec-service: ", e);
                sleep_ms(timer_port, CONNECT_RETRY_MS);
            }
        }
    }
}

fn connect_vfs_service(timer_port: Word) -> Result<Word, libnanami::NanamiError> {
    loop {
        match nanami_services::registry::connect_vfs_service(SLOT_VFS_SERVICE) {
            Ok(()) => return Ok(libnanami::ipc::process_slot_descriptor(SLOT_VFS_SERVICE)),
            Err(e) => {
                log_request_error("[honoka] waiting vfs-service: ", e);
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
