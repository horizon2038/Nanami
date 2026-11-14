use super::*;
use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

pub fn net_service_settings(port: Word) -> Result<(Ipv4Settings, Word), RequestError> {
    for _ in 0..4 {
        let (mode, revision) = get(port, 0)?;
        let (address, netmask) = get(port, 1)?;
        let (gateway, dns) = get(port, 2)?;
        if get(port, 0)? == (mode, revision) {
            return Ok((
                Ipv4Settings {
                    mode,
                    address: unpack_ipv4(address),
                    netmask: unpack_ipv4(netmask),
                    gateway: unpack_ipv4(gateway),
                    dns: unpack_ipv4(dns),
                },
                revision,
            ));
        }
    }
    Err(RequestError::Status(NET_SERVICE_RESPONSE_CONFIG_CONFLICT))
}
fn get(port: Word, field: Word) -> Result<(Word, Word), RequestError> {
    let (status, a, b) = net_service_control_ex(port, NET_SERVICE_CONTROL_GET_SETTINGS, field, 0)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((a, b))
}
pub fn net_service_save_settings(
    port: Word,
    settings: Ipv4Settings,
    revision: Word,
) -> Result<Word, RequestError> {
    settings
        .validate_draft()
        .map_err(|_| RequestError::InvalidArgument)?;
    let (status, next, _) = call_port(
        port,
        NET_SERVICE_REQUEST_SET_SETTINGS,
        settings.mode,
        pack_ipv4(settings.address) as Word | ((pack_ipv4(settings.netmask) as Word) << 32),
        pack_ipv4(settings.gateway) as Word | ((pack_ipv4(settings.dns) as Word) << 32),
        revision,
        5,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(next)
}
pub fn net_service_reload(port: Word) -> Result<(), RequestError> {
    net_service_control(port, NET_SERVICE_CONTROL_RELOAD, 0, 0)
}
pub fn net_service_config_state(port: Word) -> Result<(Word, Word), RequestError> {
    let (status, state, mode) =
        net_service_control_ex(port, NET_SERVICE_CONTROL_GET_CONFIG_STATE, 0, 0)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok((state, mode))
}
