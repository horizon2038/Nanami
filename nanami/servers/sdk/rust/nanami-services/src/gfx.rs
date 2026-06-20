use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub mod honoka;

pub const DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO: Word = 0x5001;
pub const DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER: Word = 0x5002;

pub fn display_service_get_screen_info(
    display_service_port: CapabilityDescriptor,
) -> Result<(Word, Word), RequestError> {
    let (status, detail0, detail1) = call_port(
        display_service_port,
        DISPLAY_SERVICE_REQUEST_GET_SCREEN_INFO,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((detail0, detail1))
}

pub fn display_service_prepare_shared_framebuffer(
    display_service_port: CapabilityDescriptor,
) -> Result<(Word, Word), RequestError> {
    let (status, detail0, detail1) = call_port(
        display_service_port,
        DISPLAY_SERVICE_REQUEST_PREPARE_SHARED_FRAMEBUFFER,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((detail0, detail1))
}
