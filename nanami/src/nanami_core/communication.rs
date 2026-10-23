use crate::*;
pub use nanami_uapi::os::*;
use nun::{arch, CapabilityDescriptor, CapabilityError, MessageInfo, MessageSource};

pub mod bindings;
pub mod buffers;
pub mod events;
mod services;
use services::Services;
const FAULT_REASON_MR: usize = 4;
const FAULT_PC_MR: usize = 5;
const FAULT_ADDRESS_MR: usize = 6;
const FAULT_ARCH_CODE_MR: usize = 7;

const REQUEST_CODE_MR: usize = 4;
const REQUEST_ARG0_MR: usize = 5;
const REQUEST_ARG1_MR: usize = 6;
const REQUEST_ARG2_MR: usize = 7;
const REQUEST_ARG3_MR: usize = 8;

const RESPONSE_STATUS_MR: usize = 4;
const RESPONSE_DETAIL0_MR: usize = 5;
const RESPONSE_DETAIL1_MR: usize = 6;
const RESPONSE_MESSAGE_LENGTH: u8 = 3;
pub struct CommunicationManager {
    pub os_port: CapabilityDescriptor,
    pub(crate) services: Services,
    pub bindings: bindings::Bindings,
}

#[derive(Clone, Copy)]
pub struct KernelFaultEvent {
    pub identifier: usize,
    pub reason: usize,
    pub program_counter: usize,
    pub fault_address: usize,
    pub architecture_fault_code: usize,
}

#[derive(Clone, Copy)]
pub struct OsRequestEvent {
    pub identifier: usize,
    pub code: usize,
    pub arg0: usize,
    pub arg1: usize,
    pub arg2: usize,
    pub arg3: usize,
}

#[derive(Clone, Copy)]
pub struct NotificationEvent {
    pub identifier: usize,
    pub value: usize,
}

#[derive(Clone, Copy)]
pub enum CommunicationEvent {
    KernelFault(KernelFaultEvent),
    Notification(NotificationEvent),
    OsRequest(OsRequestEvent),
}

impl CommunicationManager {
    pub fn new(os_port: CapabilityDescriptor) -> Self {
        Self {
            os_port,
            services: Services::new(),
            bindings: bindings::Bindings::new(),
        }
    }

    pub fn register_service(
        &mut self,
        owner_pid: usize,
        name: &str,
        port_descriptor: CapabilityDescriptor,
    ) -> Result<(), CapabilityError> {
        self.services.register(owner_pid, name, port_descriptor)
    }

    pub fn resolve_service(&self, name: &str) -> Option<CapabilityDescriptor> {
        self.services.resolve(name).map(|(port, _)| port)
    }

    pub fn resolve_service_with_owner(&self, name: &str) -> Option<(CapabilityDescriptor, usize)> {
        self.services.resolve(name)
    }

    pub fn service_info_by_ordinal(&self, ordinal: usize, chunk: usize) -> Option<(usize, usize)> {
        self.services.info_chunk(ordinal, chunk)
    }

    pub fn receive_event(&mut self) -> Result<CommunicationEvent, CapabilityError> {
        let mut info = MessageInfo::normal(true, 0, 0);
        let mut identifier = 0usize;
        arch::ipc_port::receive(self.os_port, &mut info, &mut identifier)?;
        Ok(Self::decode_event(info, identifier))
    }

    pub fn reply_receive_status(
        &mut self,
        status: usize,
        detail0: usize,
        detail1: usize,
    ) -> Result<CommunicationEvent, CapabilityError> {
        let ipc_buffer = arch::ipc_buffer::get_ipc_buffer();
        ipc_buffer.configure_message(RESPONSE_STATUS_MR, status);
        ipc_buffer.configure_message(RESPONSE_DETAIL0_MR, detail0);
        ipc_buffer.configure_message(RESPONSE_DETAIL1_MR, detail1);

        let mut info = MessageInfo::normal(true, RESPONSE_MESSAGE_LENGTH, 0);
        let mut identifier = 0usize;
        debug!(
            "[ipc.dbg] before reply_receive info={:#018x}",
            usize::from(info)
        );
        arch::ipc_port::reply_receive(self.os_port, &mut info, &mut identifier)?;
        debug!(
        "[ipc.dbg] after reply_receive info={:#018x} source={:?} len={} transfer={} id={:#018x}",
        usize::from(info),
        info.source(),
        info.message_length(),
        info.transfer_count(),
        identifier
    );
        Ok(Self::decode_event(info, identifier))
    }

    pub fn reply_receive_fault_continue(
        &mut self,
        fault: KernelFaultEvent,
    ) -> Result<CommunicationEvent, CapabilityError> {
        let _ = fault;
        // Fault continuation must not copy reply payload into the faulted task's message registers.
        // A9N treats this zero-length reply as a resume request for the pending fault.
        let mut info = MessageInfo::normal(true, 0, 0);
        let mut identifier = 0usize;
        arch::ipc_port::reply_receive(self.os_port, &mut info, &mut identifier)?;
        Ok(Self::decode_event(info, identifier))
    }

    fn decode_event(info: MessageInfo, identifier: usize) -> CommunicationEvent {
        let ipc_buffer = arch::ipc_buffer::get_ipc_buffer();
        if info.is_fault() {
            return CommunicationEvent::KernelFault(KernelFaultEvent {
                identifier,
                reason: ipc_buffer.get_message(FAULT_REASON_MR),
                program_counter: ipc_buffer.get_message(FAULT_PC_MR),
                fault_address: ipc_buffer.get_message(FAULT_ADDRESS_MR),
                architecture_fault_code: ipc_buffer.get_message(FAULT_ARCH_CODE_MR),
            });
        }
        if info.is_notification() {
            let value = if info.message_length() >= 1 {
                ipc_buffer.get_message(REQUEST_CODE_MR)
            } else {
                0
            };
            return CommunicationEvent::Notification(NotificationEvent { identifier, value });
        }
        if !matches!(info.source(), MessageSource::Normal) {
            return CommunicationEvent::Notification(NotificationEvent {
                identifier,
                value: 0,
            });
        }

        let code = if info.message_length() >= 1 {
            ipc_buffer.get_message(REQUEST_CODE_MR)
        } else {
            0
        };
        let arg0 = if info.message_length() >= 2 {
            ipc_buffer.get_message(REQUEST_ARG0_MR)
        } else {
            0
        };
        let arg1 = if info.message_length() >= 3 {
            ipc_buffer.get_message(REQUEST_ARG1_MR)
        } else {
            0
        };
        let arg2 = if info.message_length() >= 4 {
            ipc_buffer.get_message(REQUEST_ARG2_MR)
        } else {
            0
        };
        let arg3 = if info.message_length() >= 5 {
            ipc_buffer.get_message(REQUEST_ARG3_MR)
        } else {
            0
        };

        CommunicationEvent::OsRequest(OsRequestEvent {
            identifier,
            code,
            arg0,
            arg1,
            arg2,
            arg3,
        })
    }
}
