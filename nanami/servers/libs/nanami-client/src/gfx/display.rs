use super::*;
pub use nanami_uapi::gfx::display::*;

pub fn connector_info(
    port: CapabilityDescriptor,
    ordinal: Word,
) -> Result<Option<ConnectorInfo>, RequestError> {
    let (status, a, b) = call_port(
        port,
        DISPLAY_SERVICE_REQUEST_CONNECTOR_INFO,
        ordinal,
        0,
        0,
        0,
        2,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if a == 0 && b == 0 {
        return Ok(None);
    }
    ConnectorInfo::from_words(a, b)
        .map(Some)
        .ok_or(RequestError::Protocol)
}
pub fn read_edid(
    port: CapabilityDescriptor,
    ordinal: Word,
    buffer: &mut [u8],
) -> Result<usize, RequestError> {
    let info = connector_info(port, ordinal)?.ok_or(RequestError::InvalidArgument)?;
    let size = info.edid_bytes as usize;
    if size == 0 {
        return Err(RequestError::Unsupported);
    }
    if size > buffer.len() {
        return Err(RequestError::InvalidArgument);
    }
    for offset in (0..size).step_by(16) {
        let (status, a, b) = call_port(
            port,
            DISPLAY_SERVICE_REQUEST_READ_EDID,
            ordinal,
            offset,
            0,
            0,
            3,
        )?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&(a as u64).to_le_bytes());
        bytes[8..].copy_from_slice(&(b as u64).to_le_bytes());
        let count = (size - offset).min(16);
        buffer[offset..offset + count].copy_from_slice(&bytes[..count]);
    }
    Ok(size)
}
pub fn monitor_mode(
    port: CapabilityDescriptor,
    connector: Word,
    ordinal: Word,
) -> Result<Option<Timing>, RequestError> {
    let (status, a, h) = call_port(
        port,
        DISPLAY_SERVICE_REQUEST_MONITOR_MODE,
        connector,
        ordinal,
        0,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if a == MODE_END {
        if h != 0 {
            return Err(RequestError::Protocol);
        }
        return Ok(None);
    }
    let (status, v, reserved) = call_port(
        port,
        DISPLAY_SERVICE_REQUEST_MONITOR_MODE,
        connector,
        ordinal,
        1,
        0,
        4,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    if reserved != 0 {
        return Err(RequestError::Protocol);
    }
    Timing::from_words([a, h, v])
        .map(Some)
        .ok_or(RequestError::Protocol)
}
