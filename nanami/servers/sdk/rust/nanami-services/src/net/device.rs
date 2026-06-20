use a9n_abi::CapabilityDescriptor;

use crate::{call_port, RequestError, Word, OS_RESPONSE_OK};

use super::constants::{
    NET_DEVICE_REQUEST_CONTROL, NET_DEVICE_REQUEST_RECV, NET_DEVICE_REQUEST_SEND,
};

pub fn net_device_send(
    device_port: CapabilityDescriptor,
    buffer_addr: Word,
    buffer_len: Word,
) -> Result<Word, RequestError> {
    let (status, detail0, _) = call_port(
        device_port,
        NET_DEVICE_REQUEST_SEND,
        buffer_addr,
        buffer_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(detail0)
}

pub fn net_device_recv(
    device_port: CapabilityDescriptor,
    buffer_addr: Word,
    buffer_len: Word,
) -> Result<Word, RequestError> {
    let (status, detail0, _) = call_port(
        device_port,
        NET_DEVICE_REQUEST_RECV,
        buffer_addr,
        buffer_len,
        0,
        0,
        3,
    )?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(detail0)
}

pub fn net_device_control(
    device_port: CapabilityDescriptor,
    control_code: Word,
    arg0: Word,
    arg1: Word,
) -> Result<(), RequestError> {
    let (status, _, _) = net_device_control_ex(device_port, control_code, arg0, arg1)?;
    if status != OS_RESPONSE_OK {
        return Err(RequestError::Status(status));
    }
    Ok(())
}

pub fn net_device_control_ex(
    device_port: CapabilityDescriptor,
    control_code: Word,
    arg0: Word,
    arg1: Word,
) -> Result<(Word, Word, Word), RequestError> {
    let (status, detail0, detail1) = call_port(
        device_port,
        NET_DEVICE_REQUEST_CONTROL,
        control_code,
        arg0,
        arg1,
        0,
        4,
    )?;
    Ok((status, detail0, detail1))
}
