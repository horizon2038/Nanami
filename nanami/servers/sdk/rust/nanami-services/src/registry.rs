use libnanami::{self, RequestError, Word};

pub const SERVICE_PORT_SLOT: Word = 20;

pub const NET_DEVICE: &str = "net-device";
pub const NETWORK_SERVICE: &str = "network-service";
pub const TIMER_SERVICE: &str = "timer-service";
pub const DISPLAY_SERVICE: &str = "display_service";
pub const INPUT_SERVICE: &str = "input-service";
pub const HONOKA_SERVICE: &str = "honoka-service";
pub const RTC_SERVICE: &str = "rtc-service";

pub const SERVICE_KIND_NET_DEVICE: Word = 1;
pub const SERVICE_KIND_NETWORK_SERVICE: Word = 2;
pub const SERVICE_KIND_TIMER_SERVICE: Word = 3;
pub const SERVICE_KIND_DISPLAY_SERVICE: Word = 4;
pub const SERVICE_KIND_INPUT_SERVICE: Word = 5;
pub const SERVICE_KIND_HONOKA_SERVICE: Word = 6;
pub const SERVICE_KIND_RTC_SERVICE: Word = 7;

pub fn register_service(name: &str) -> Result<(), RequestError> {
    let _ = register_service_with_pid(name)?;
    Ok(())
}

pub fn register_service_with_pid(name: &str) -> Result<Word, RequestError> {
    libnanami::register_service_by_name_with_pid(name, SERVICE_PORT_SLOT)
}

pub fn connect_service(name: &str, destination_slot: Word) -> Result<(), RequestError> {
    libnanami::connect_service_by_name(name, destination_slot)
}

pub fn connect_service_with_pid(name: &str, destination_slot: Word) -> Result<Word, RequestError> {
    libnanami::connect_service_by_name_with_pid(name, destination_slot)
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
        _ => b"unknown",
    }
}
