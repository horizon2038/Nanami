use crate::*;
use nun::{arch, CapabilityDescriptor, CapabilityError, MessageInfo, MessageSource};

const MAX_SERVICES: usize = 64;
const MAX_SERVICE_NAME_LEN: usize = 32;
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

pub const OS_REQUEST_IRQ_CONTROL: usize = 0x1001;
pub const OS_REQUEST_IO_PORT_CONTROL: usize = 0x1002;
pub const OS_REQUEST_SERVICE_REGISTER: usize = 0x1003;
pub const OS_REQUEST_PAGE_ALLOC: usize = 0x1004;
pub const OS_REQUEST_SERVICE_CONNECT: usize = 0x1005;
pub const OS_REQUEST_DMA_REQUEST: usize = 0x1006;
pub const OS_REQUEST_MMIO_REQUEST: usize = 0x1007;
pub const OS_REQUEST_SHARED_MEMORY_CREATE: usize = 0x1008;
pub const OS_REQUEST_SELF_PID: usize = 0x1009;
pub const OS_REQUEST_EXIT: usize = 0x100a;
pub const OS_REQUEST_INITIAL_FRAMEBUFFER_INFORMATION: usize = 0x100b;
pub const OS_REQUEST_NOTIFICATION_PORT_CREATE: usize = 0x100c;
pub const OS_REQUEST_NOTIFICATION_PORT_COPY: usize = 0x100d;
pub const OS_REQUEST_SHARED_FRAMEBUFFER_CREATE: usize = 0x100e;
pub const OS_REQUEST_HEAP_ALLOC: usize = 0x100f;
pub const OS_REQUEST_SERVICE_LIST: usize = 0x1010;
pub const OS_REQUEST_DEBUG_PING: usize = 0x10ff;

pub const OS_RESPONSE_OK: usize = 0;
pub const OS_RESPONSE_INVALID_ARGUMENT: usize = 1;
pub const OS_RESPONSE_PERMISSION_DENIED: usize = 2;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: usize = 3;
pub const OS_RESPONSE_ILLEGAL_OPERATION: usize = 4;
pub const OS_RESPONSE_FATAL: usize = 5;
pub const OS_RESPONSE_PONG_MAGIC: usize = 0x504f4e47;

pub const OS_SERVICE_NET_DEVICE: usize = 1;
pub const OS_SERVICE_NETWORK_SERVICE: usize = 2;
pub const OS_SERVICE_TIMER_SERVICE: usize = 3;
pub const OS_SERVICE_DISPLAY_SERVICE: usize = 4;
pub const OS_SERVICE_INPUT_SERVICE: usize = 5;
pub const OS_SERVICE_HONOKA_SERVICE: usize = 6;

#[derive(Clone, Copy)]
pub struct ServiceEntry {
    used: bool,
    owner_pid: usize,
    name_len: usize,
    name: [u8; MAX_SERVICE_NAME_LEN],
    port_descriptor: CapabilityDescriptor,
}

impl ServiceEntry {
    const EMPTY: Self = Self {
        used: false,
        owner_pid: usize::MAX,
        name_len: 0,
        name: [0; MAX_SERVICE_NAME_LEN],
        port_descriptor: 0,
    };
}

pub struct CommunicationManager {
    pub os_port: CapabilityDescriptor,
    services: [ServiceEntry; MAX_SERVICES],
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
            services: [ServiceEntry::EMPTY; MAX_SERVICES],
        }
    }

    pub fn register_service(
        &mut self,
        owner_pid: usize,
        name: &str,
        port_descriptor: CapabilityDescriptor,
    ) -> Result<(), CapabilityError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_SERVICE_NAME_LEN {
            return Err(CapabilityError::InvalidArgument);
        }

        if let Some(idx) = self.find_service_index(bytes) {
            self.services[idx].owner_pid = owner_pid;
            self.services[idx].port_descriptor = port_descriptor;
            return Ok(());
        }

        let mut i = 0;
        while i < MAX_SERVICES {
            if !self.services[i].used {
                self.services[i].used = true;
                self.services[i].owner_pid = owner_pid;
                self.services[i].name_len = bytes.len();
                self.services[i].name[..bytes.len()].copy_from_slice(bytes);
                self.services[i].port_descriptor = port_descriptor;
                return Ok(());
            }
            i += 1;
        }

        Err(CapabilityError::InvalidArgument)
    }

    pub fn resolve_service(&self, name: &str) -> Option<CapabilityDescriptor> {
        self.find_service_index(name.as_bytes())
            .map(|idx| self.services[idx].port_descriptor)
    }

    pub fn resolve_service_with_owner(&self, name: &str) -> Option<(CapabilityDescriptor, usize)> {
        self.find_service_index(name.as_bytes()).map(|idx| {
            (
                self.services[idx].port_descriptor,
                self.services[idx].owner_pid,
            )
        })
    }

    pub fn service_info_by_ordinal(&self, ordinal: usize) -> Option<(usize, usize)> {
        let mut seen = 0usize;
        let mut i = 0usize;
        while i < MAX_SERVICES {
            let e = self.services[i];
            if e.used {
                if seen == ordinal {
                    return Some((e.owner_pid, service_kind_from_name(&e.name[..e.name_len])));
                }
                seen += 1;
            }
            i += 1;
        }
        None
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
            Word::from(info)
        );
        arch::ipc_port::reply_receive(self.os_port, &mut info, &mut identifier)?;
        debug!(
        "[ipc.dbg] after reply_receive info={:#018x} source={:?} len={} transfer={} id={:#018x}",
        Word::from(info),
        info.source(),
        info.message_length(),
        info.transfer_count(),
        identifier
    );
        Ok(Self::decode_event(info, identifier))
    }

    fn find_service_index(&self, target: &[u8]) -> Option<usize> {
        let mut i = 0;
        while i < MAX_SERVICES {
            let e = self.services[i];
            if e.used && e.name_len == target.len() && e.name[..e.name_len] == target[..e.name_len]
            {
                return Some(i);
            }
            i += 1;
        }
        None
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

fn service_kind_from_name(name: &[u8]) -> usize {
    if name == b"net-device" {
        OS_SERVICE_NET_DEVICE
    } else if name == b"network-service" {
        OS_SERVICE_NETWORK_SERVICE
    } else if name == b"timer-service" {
        OS_SERVICE_TIMER_SERVICE
    } else if name == b"display_service" {
        OS_SERVICE_DISPLAY_SERVICE
    } else if name == b"input-service" {
        OS_SERVICE_INPUT_SERVICE
    } else if name == b"honoka-service" {
        OS_SERVICE_HONOKA_SERVICE
    } else {
        0
    }
}
