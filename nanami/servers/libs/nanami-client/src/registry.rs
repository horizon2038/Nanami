pub use nanami_uapi::registry::*;

use nanami_runtime::{self, RequestError, Word};

pub fn register_service(name: &str) -> Result<(), RequestError> {
    let _ = register_service_with_pid(name)?;
    Ok(())
}

pub fn register_service_with_pid(name: &str) -> Result<Word, RequestError> {
    nanami_runtime::register_service_by_name_with_pid(name, SERVICE_PORT_SLOT)
}

pub fn connect_service(name: &str, destination_slot: Word) -> Result<(), RequestError> {
    nanami_runtime::connect_service_by_name(name, destination_slot)
}

pub fn connect_service_with_pid(name: &str, destination_slot: Word) -> Result<Word, RequestError> {
    nanami_runtime::connect_service_by_name_with_pid(name, destination_slot)
}

pub fn register_net_device() -> Result<(), RequestError> {
    register_service(NET_DEVICE)
}

pub fn register_network_service() -> Result<(), RequestError> {
    register_service(NETWORK_SERVICE)
}

pub fn register_timer_service() -> Result<(), RequestError> {
    register_service(TIMER_SERVICE)
}

pub fn register_display_service() -> Result<(), RequestError> {
    register_service(DISPLAY_SERVICE)
}

pub fn register_input_service() -> Result<(), RequestError> {
    register_service(INPUT_SERVICE)
}

pub fn register_honoka_service() -> Result<(), RequestError> {
    register_service(HONOKA_SERVICE)
}

pub fn register_rtc_service() -> Result<(), RequestError> {
    register_service(RTC_SERVICE)
}

pub fn register_block_device() -> Result<(), RequestError> {
    register_service(BLOCK_DEVICE)
}

pub fn register_vfs_service() -> Result<(), RequestError> {
    register_service(VFS_SERVICE)
}

pub fn register_posix_service() -> Result<(), RequestError> {
    register_service(POSIX_SERVICE)
}

pub fn register_terminal_service() -> Result<(), RequestError> {
    register_service(TERMINAL_SERVICE)
}

pub fn register_exec_service() -> Result<(), RequestError> {
    register_service(EXEC_SERVICE)
}

pub fn connect_net_device_with_pid(destination_slot: Word) -> Result<Word, RequestError> {
    connect_service_with_pid(NET_DEVICE, destination_slot)
}

pub fn connect_network_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(NETWORK_SERVICE, destination_slot)
}

pub fn connect_timer_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(TIMER_SERVICE, destination_slot)
}

pub fn connect_rtc_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(RTC_SERVICE, destination_slot)
}

pub fn connect_block_device_with_pid(destination_slot: Word) -> Result<Word, RequestError> {
    connect_service_with_pid(BLOCK_DEVICE, destination_slot)
}

pub fn connect_block_device(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(BLOCK_DEVICE, destination_slot)
}

pub fn connect_vfs_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(VFS_SERVICE, destination_slot)
}

pub fn connect_posix_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(POSIX_SERVICE, destination_slot)
}

pub fn connect_terminal_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(TERMINAL_SERVICE, destination_slot)
}

pub fn connect_exec_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(EXEC_SERVICE, destination_slot)
}

pub fn connect_display_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(DISPLAY_SERVICE, destination_slot)
}

pub fn connect_input_service(destination_slot: Word) -> Result<(), RequestError> {
    connect_service(INPUT_SERVICE, destination_slot)
}

pub fn connect_input_service_with_pid(destination_slot: Word) -> Result<Word, RequestError> {
    connect_service_with_pid(INPUT_SERVICE, destination_slot)
}

pub fn connect_honoka_service_with_pid(destination_slot: Word) -> Result<Word, RequestError> {
    connect_service_with_pid(HONOKA_SERVICE, destination_slot)
}

pub fn service_name_from_kind(kind: Word) -> &'static [u8] {
    match kind {
        SERVICE_KIND_NET_DEVICE => b"net-device",
        SERVICE_KIND_NETWORK_SERVICE => b"network-service",
        SERVICE_KIND_TIMER_SERVICE => b"timer-service",
        SERVICE_KIND_DISPLAY_SERVICE => b"display_service",
        SERVICE_KIND_INPUT_SERVICE => b"input-service",
        SERVICE_KIND_HONOKA_SERVICE => b"honoka-service",
        SERVICE_KIND_RTC_SERVICE => b"rtc-service",
        SERVICE_KIND_BLOCK_DEVICE => b"block-device",
        SERVICE_KIND_VFS_SERVICE => b"vfs-service",
        SERVICE_KIND_POSIX_SERVICE => b"posix-service",
        SERVICE_KIND_TERMINAL_SERVICE => b"terminal-service",
        SERVICE_KIND_EXEC_SERVICE => b"exec-service",
        _ => b"unknown",
    }
}
