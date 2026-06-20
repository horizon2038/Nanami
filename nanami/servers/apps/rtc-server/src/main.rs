#![no_std]
#![no_main]

use libnanami::ipc::{ServiceEvent, ServiceRequest};
use libnanami::{self, RequestError, Word};
use nanami_services::rtc::{pack_rtc_date, pack_rtc_time, RtcDateTime, RTC_SERVICE_REQUEST_READ};

const SLOT_IO_CMOS: Word = 16;
const SLOT_SERVICE_PORT: Word = 20;

const CMOS_INDEX_PORT: Word = 0x70;
const CMOS_DATA_PORT: Word = 0x71;
const CMOS_DISABLE_NMI: Word = 0x80;

const RTC_REG_SECONDS: u8 = 0x00;
const RTC_REG_MINUTES: u8 = 0x02;
const RTC_REG_HOURS: u8 = 0x04;
const RTC_REG_DAY: u8 = 0x07;
const RTC_REG_MONTH: u8 = 0x08;
const RTC_REG_YEAR: u8 = 0x09;
const RTC_REG_STATUS_A: u8 = 0x0a;
const RTC_REG_STATUS_B: u8 = 0x0b;
const RTC_REG_CENTURY: u8 = 0x32;

const STATUS_A_UPDATE_IN_PROGRESS: u8 = 0x80;
const STATUS_B_24_HOUR: u8 = 0x02;
const STATUS_B_BINARY: u8 = 0x04;
const HOUR_PM_BIT: u8 = 0x80;

#[derive(Clone, Copy, PartialEq, Eq)]
struct RawRtc {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    libnanami::print!("[rtc-server] panic\n");
    let _ = libnanami::request_exit();
    loop {}
}

fn nanami_main() -> libnanami::NanamiResult {
    libnanami::ipc::init_ipc_tls()
        .map_err(|e| log_error("[rtc-server] ipc tls init failed: ", e))?;

    nanami_services::registry::register_rtc_service()
        .map_err(|e| log_error("[rtc-server] service register failed: ", e))?;
    libnanami::print!("[rtc-server] service registered: rtc-service\n");

    libnanami::request_io_port(CMOS_INDEX_PORT, CMOS_DATA_PORT, SLOT_IO_CMOS)
        .map_err(|e| log_error("[rtc-server] request cmos io failed: ", e))?;

    let service_port = libnanami::ipc::process_slot_descriptor(SLOT_SERVICE_PORT);
    let io_desc = libnanami::ipc::process_slot_descriptor(SLOT_IO_CMOS);
    let mut pending_status = (libnanami::OS_RESPONSE_OK, 0, 0);
    let mut has_pending_reply = false;

    loop {
        let event = if has_pending_reply {
            has_pending_reply = false;
            match libnanami::ipc::service_reply_receive_event(
                service_port,
                pending_status.0,
                pending_status.1,
                pending_status.2,
            ) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[rtc-server] reply_receive failed: ", e)),
            }
        } else {
            match libnanami::ipc::service_receive_event(service_port) {
                Ok(e) => e,
                Err(e) => return Err(log_error("[rtc-server] receive failed: ", e)),
            }
        };

        match event {
            ServiceEvent::Request(request) => {
                pending_status = handle_request(io_desc, request);
                has_pending_reply = true;
            }
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault {
                identifier, reason, ..
            } => {
                libnanami::println!("[rtc-server] fault id={} reason={:#x}", identifier, reason);
            }
        }
    }
}

fn handle_request(io_desc: Word, request: ServiceRequest) -> (Word, Word, Word) {
    match request.code {
        RTC_SERVICE_REQUEST_READ => match read_rtc_datetime(io_desc) {
            Ok(dt) => (
                libnanami::OS_RESPONSE_OK,
                pack_rtc_date(dt),
                pack_rtc_time(dt),
            ),
            Err(e) => (map_request_error_to_status(e), 0, 0),
        },
        _ => (libnanami::OS_RESPONSE_INVALID_ARGUMENT, 0, 0),
    }
}

fn read_rtc_datetime(io_desc: Word) -> Result<RtcDateTime, RequestError> {
    let mut prev = read_raw_stable_candidate(io_desc)?;
    let mut tries = 0;
    while tries < 8 {
        let next = read_raw_stable_candidate(io_desc)?;
        if next == prev {
            return Ok(decode_raw(next));
        }
        prev = next;
        tries += 1;
    }
    Ok(decode_raw(prev))
}

fn read_raw_stable_candidate(io_desc: Word) -> Result<RawRtc, RequestError> {
    wait_until_not_updating(io_desc)?;
    Ok(RawRtc {
        second: cmos_read(io_desc, RTC_REG_SECONDS)?,
        minute: cmos_read(io_desc, RTC_REG_MINUTES)?,
        hour: cmos_read(io_desc, RTC_REG_HOURS)?,
        day: cmos_read(io_desc, RTC_REG_DAY)?,
        month: cmos_read(io_desc, RTC_REG_MONTH)?,
        year: cmos_read(io_desc, RTC_REG_YEAR)?,
        century: cmos_read(io_desc, RTC_REG_CENTURY).unwrap_or(0),
        status_b: cmos_read(io_desc, RTC_REG_STATUS_B)?,
    })
}

fn wait_until_not_updating(io_desc: Word) -> Result<(), RequestError> {
    let mut tries = 0;
    while tries < 1_000_000 {
        let status_a = cmos_read(io_desc, RTC_REG_STATUS_A)?;
        if (status_a & STATUS_A_UPDATE_IN_PROGRESS) == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
        tries += 1;
    }
    Err(RequestError::Transport)
}

fn decode_raw(raw: RawRtc) -> RtcDateTime {
    let binary = (raw.status_b & STATUS_B_BINARY) != 0;
    let second = decode_rtc_value(raw.second, binary);
    let minute = decode_rtc_value(raw.minute, binary);
    let mut hour = decode_rtc_value(raw.hour & !HOUR_PM_BIT, binary);
    let day = decode_rtc_value(raw.day, binary);
    let month = decode_rtc_value(raw.month, binary);
    let year = decode_rtc_value(raw.year, binary) as u16;
    let century = decode_rtc_value(raw.century, binary) as u16;

    if (raw.status_b & STATUS_B_24_HOUR) == 0 {
        if (raw.hour & HOUR_PM_BIT) != 0 && hour < 12 {
            hour += 12;
        } else if (raw.hour & HOUR_PM_BIT) == 0 && hour == 12 {
            hour = 0;
        }
    }

    let full_year = if century != 0 {
        century.saturating_mul(100).saturating_add(year)
    } else if year < 70 {
        2000 + year
    } else {
        1900 + year
    };

    RtcDateTime {
        year: full_year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

fn decode_rtc_value(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        ((value >> 4) * 10).saturating_add(value & 0x0f)
    }
}

fn cmos_read(io_desc: Word, register: u8) -> Result<u8, RequestError> {
    libnanami::io::io_write(
        io_desc,
        CMOS_INDEX_PORT,
        1,
        CMOS_DISABLE_NMI | register as Word,
    )?;
    Ok(libnanami::io::io_read(io_desc, CMOS_DATA_PORT, 1)? as u8)
}

fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => libnanami::OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Status(status) => status,
        RequestError::Unsupported => libnanami::OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Transport | RequestError::Protocol => libnanami::OS_RESPONSE_FATAL,
    }
}

fn log_error(prefix: &str, error: RequestError) -> libnanami::NanamiError {
    libnanami::print!("{}", prefix);
    libnanami::println!("{:?}", error);
    libnanami::NanamiError::from(error)
}

libnanami::nanami_entry!(nanami_main);
