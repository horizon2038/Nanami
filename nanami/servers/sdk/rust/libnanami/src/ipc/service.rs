use a9n_abi::capability_call::ipc_port::MessageInfo;
use a9n_abi::CapabilityDescriptor;

use crate::{map_capability_error, RequestError, Word};

use super::tls::init_ipc_tls;
use super::types::{ServiceEvent, ServiceRequest, HARDWARE_CONTEXT_WORDS};

const INVALID_KERNEL_CALL_FAULT: Word = 5;
const INVALID_KERNEL_CALL_CONTEXT_START: usize = 7;

fn decode_service_event(info: MessageInfo, identifier: Word) -> Result<ServiceEvent, RequestError> {
    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    if info.is_fault() {
        let reason = ipc.get_message(4);
        let mut hardware_context = [0; HARDWARE_CONTEXT_WORDS];
        let hardware_context_count = if reason == INVALID_KERNEL_CALL_FAULT {
            usize::from(info.message_length())
                .saturating_sub(INVALID_KERNEL_CALL_CONTEXT_START)
                .min(HARDWARE_CONTEXT_WORDS)
        } else {
            0
        };
        for (index, value) in hardware_context
            .iter_mut()
            .take(hardware_context_count)
            .enumerate()
        {
            *value = ipc.get_message(INVALID_KERNEL_CALL_CONTEXT_START + index);
        }

        return Ok(ServiceEvent::Fault {
            identifier,
            reason,
            program_counter: ipc.get_message(5),
            fault_address: ipc.get_message(6),
            architecture_fault_code: if reason == INVALID_KERNEL_CALL_FAULT {
                0
            } else {
                ipc.get_message(7)
            },
            hardware_context,
            hardware_context_count,
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

pub fn service_fault_continue_receive_event(
    port_descriptor: CapabilityDescriptor,
    hardware_context: &[Word],
) -> Result<ServiceEvent, RequestError> {
    init_ipc_tls()?;

    if hardware_context.len() > HARDWARE_CONTEXT_WORDS {
        return Err(RequestError::InvalidArgument);
    }

    let ipc = a9n_abi::arch::ipc_buffer::get_ipc_buffer();
    for (index, value) in hardware_context.iter().copied().enumerate() {
        ipc.configure_message(4 + index, value);
    }

    let mut info = MessageInfo::normal(true, hardware_context.len() as u8, 0);
    let mut identifier = 0;
    a9n_abi::arch::ipc_port::reply_receive(port_descriptor, &mut info, &mut identifier)
        .map_err(map_capability_error)?;
    decode_service_event(info, identifier)
}
