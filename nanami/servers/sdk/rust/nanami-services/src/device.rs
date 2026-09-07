use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub const DEVICE_MANAGER_SERVICE: &str = "device-manager";
pub const DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE: Word = 0xd001;

pub fn hpet_mmio_base(service_port: CapabilityDescriptor) -> Result<Word, RequestError> {
    let (status, base, _) = call_port(
        service_port,
        DEVICE_MANAGER_REQUEST_HPET_MMIO_BASE,
        0,
        0,
        0,
        0,
        1,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if base == 0 {
        return Err(RequestError::Protocol);
    }
    Ok(base)
}
