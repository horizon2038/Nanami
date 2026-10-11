pub use nanami_uapi::usb::*;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};
use a9n_abi::CapabilityDescriptor;

/// Return the number of initialized controllers and boot HID interfaces.
pub fn usb_service_info(port: CapabilityDescriptor) -> Result<(Word, Word), RequestError> {
    let (status, controllers, interfaces) =
        call_port(port, USB_SERVICE_REQUEST_INFO, 0, 0, 0, 0, 1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((controllers, interfaces))
}
