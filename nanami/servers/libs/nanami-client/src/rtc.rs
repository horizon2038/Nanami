pub use nanami_uapi::rtc::*;

use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, OS_RESPONSE_OK};

pub fn rtc_service_read(
    rtc_service_port: CapabilityDescriptor,
) -> Result<RtcDateTime, RequestError> {
    let (status, date, time) =
        call_port(rtc_service_port, RTC_SERVICE_REQUEST_READ, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(unpack_rtc_datetime(date, time))
}
