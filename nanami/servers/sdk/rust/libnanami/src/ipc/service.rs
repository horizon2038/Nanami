use a9n_abi::capability_call::ipc_port::MessageInfo;
use a9n_abi::CapabilityDescriptor;

use crate::{map_capability_error, RequestError, Word};

use super::tls::init_ipc_tls;
use super::types::{ServiceEvent, ServiceRequest};

fn decode_service_event(info: MessageInfo, identifier: Word) -> Result<ServiceEvent, RequestError> {
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    if info.is_fault() {
        return Ok(ServiceEvent::Fault {
            identifier,
            reason: ipc.get_message(4),
            program_counter: ipc.get_message(5),
            fault_address: ipc.get_message(6),
            architecture_fault_code: ipc.get_message(7),
        });
    }
    if info.is_notification() {
        let value = if info.message_length() >= 1 {
            ipc.get_message(4)
        } else {
            0
        };
        return Ok(ServiceEvent::Notification { identifier, value });
    }
    if !info.is_normal() {
        return Err(RequestError::Protocol);
    }

    let len = info.message_length();

    let code = if len >= 1 { ipc.get_message(4) } else { 0 };
    let arg0 = if len >= 2 { ipc.get_message(5) } else { 0 };
    let arg1 = if len >= 3 { ipc.get_message(6) } else { 0 };
    let arg2 = if len >= 4 { ipc.get_message(7) } else { 0 };
    let arg3 = if len >= 5 { ipc.get_message(8) } else { 0 };

    Ok(ServiceEvent::Request(ServiceRequest {
        identifier,
        code,
        arg0,
        arg1,
        arg2,
        arg3,
    }))
}

pub fn service_receive(
    port_descriptor: CapabilityDescriptor,
) -> Result<ServiceRequest, RequestError> {
    loop {
        match service_receive_event(port_descriptor)? {
            ServiceEvent::Request(req) => return Ok(req),
            ServiceEvent::Notification { .. } => {}
            ServiceEvent::Fault { .. } => return Err(RequestError::Protocol),
        }
    }
}

pub fn service_receive_event(
    port_descriptor: CapabilityDescriptor,
) -> Result<ServiceEvent, RequestError> {
    init_ipc_tls()?;

    let mut info = MessageInfo::normal(true, 0, 0);
    let mut identifier = 0;
    a9n_abi::arch::ipc_port::receive(port_descriptor, &mut info, &mut identifier)
        .map_err(map_capability_error)?;
    decode_service_event(info, identifier)
}

pub fn service_reply_receive(
    port_descriptor: CapabilityDescriptor,
    status: Word,
    detail0: Word,
    detail1: Word,
) -> Result<ServiceRequest, RequestError> {
    match service_reply_receive_event(port_descriptor, status, detail0, detail1)? {
        ServiceEvent::Request(req) => Ok(req),
        ServiceEvent::Fault { .. } => Err(RequestError::Protocol),
        ServiceEvent::Notification { .. } => loop {
            match service_receive_event(port_descriptor)? {
                ServiceEvent::Request(req) => return Ok(req),
                ServiceEvent::Notification { .. } => {}
                ServiceEvent::Fault { .. } => return Err(RequestError::Protocol),
            }
        },
    }
}

pub fn service_reply_receive_event(
    port_descriptor: CapabilityDescriptor,
    status: Word,
    detail0: Word,
    detail1: Word,
) -> Result<ServiceEvent, RequestError> {
    init_ipc_tls()?;

    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    ipc.configure_message(4, status);
    ipc.configure_message(5, detail0);
    ipc.configure_message(6, detail1);

    let mut info = MessageInfo::normal(true, 3, 0);
    let mut identifier = 0;
    a9n_abi::arch::ipc_port::reply_receive(port_descriptor, &mut info, &mut identifier)
        .map_err(map_capability_error)?;
    decode_service_event(info, identifier)
}
