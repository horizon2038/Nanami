//! Managed Service identities and cooperative receiver handover, local to one Alpha.
use crate::Word;

pub const OS_REQUEST_PROVIDER_REGISTER: Word = 0x1025;
pub const OS_REQUEST_SERVICE_PUBLISH: Word = 0x1026;
pub const OS_REQUEST_SESSION_CONNECT: Word = 0x1027;
pub const OS_REQUEST_BINDING_CONTROL: Word = 0x1028;
pub const CONTROL_PROVIDER_LOOKUP: Word = 1;
pub const CONTROL_PROVIDER_BINDING: Word = 2;
pub const CONTROL_PREPARE: Word = 3;
pub const CONTROL_QUIESCE: Word = 4;
pub const CONTROL_QUIESCED: Word = 5;
pub const CONTROL_COMMIT: Word = 6;
pub const CONTROL_ABORT: Word = 7;
pub const CONTROL_STATE: Word = 8;
pub const CONTROL_CLOSE: Word = 9;
pub const CONTROL_PROVIDER_RETIRE: Word = 10;
pub const CONTROL_BUFFER_CREATE: Word = 11;
pub const CONTROL_BUFFER_VIEW: Word = 12;
pub const CONTROL_EVENT_CONNECT: Word = 13;
pub const CONTROL_PROVIDER_SESSION: Word = 14;
pub const BINDING_NOTIFICATION: Word = 1 << 61;
pub const SERVICE_RESOURCE_NOTIFICATION: Word = 1 << 60;
pub const SERVICE_WORK_NOTIFICATION: Word = 1;
pub const SESSION_IDENTIFIER_BIT: Word = 1 << (Word::BITS - 1);
/// Initial exchange profile: stateless request/reply, without attached shared
/// data queues, device state, or application event subscriptions.
pub const STATELESS_CONTROL_PROFILE: Word = 1;
/// Opaque persistent session buffers. No device DMA or event subscription is
/// implied. Layout/state validation belongs to the compatible Providers.
pub const MANAGED_BUFFER_PROFILE: Word = 2;
/// Persistent opaque buffers plus a Service-owned pending-work Notification.
/// Notifications are coalescing hints; Providers retain queue cursors in data.
pub const MANAGED_EVENT_PROFILE: Word = 3;
pub const SESSION_BUFFER_MAX_BYTES: Word = 64 * 1024;

/// Profile 3 packs two caller-selected root slots in the receiver argument.
/// Earlier profiles keep their original wire representation.
pub fn provider_slots(profile: Word, raw: Word, notification: Word) -> Option<(Word, Word)> {
    let (receiver, event) = if profile == MANAGED_EVENT_PROFILE {
        (raw & 0xff, raw >> 8)
    } else if matches!(profile, STATELESS_CONTROL_PROFILE | MANAGED_BUFFER_PROFILE) {
        (raw, 0)
    } else {
        return None;
    };
    if !(23..224).contains(&receiver)
        || !(23..224).contains(&notification)
        || receiver == notification
    {
        return None;
    }
    if profile == MANAGED_EVENT_PROFILE
        && (!(23..224).contains(&event) || event == receiver || event == notification)
    {
        return None;
    }
    Some((receiver, event))
}

#[derive(Clone, Copy, Debug)]
pub struct SessionBuffer {
    pub address: Word,
    pub bytes: Word,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderId(pub Word);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceId(pub Word);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(pub Word);

#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingPhase {
    Active = 0,
    Prepared = 1,
    Quiescing = 2,
    Quiesced = 3,
    Faulted = 4,
}

impl BindingPhase {
    /// Accepted work may still need its existing data view while quiescing.
    /// This does not authorize a new client allocation or a foreign Provider.
    pub fn permits_buffer_view(self) -> bool {
        matches!(self, Self::Active | Self::Prepared | Self::Quiescing)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BindingState {
    pub service: ServiceId,
    pub generation: Word,
    pub phase: BindingPhase,
}

impl BindingState {
    pub fn encode(self) -> Word {
        (self.generation << 3) | self.phase as Word
    }

    pub fn decode(service: ServiceId, raw: Word) -> Self {
        let phase = match raw & 7 {
            0 => BindingPhase::Active,
            1 => BindingPhase::Prepared,
            2 => BindingPhase::Quiescing,
            3 => BindingPhase::Quiesced,
            _ => BindingPhase::Faulted,
        };
        Self {
            service,
            generation: raw >> 3,
            phase,
        }
    }
}

/// The endpoint is stable across supported cooperative Provider changes.
/// No Provider PID is a persistent authority in this API.
#[derive(Clone, Copy, Debug)]
pub struct Connection {
    pub endpoint: Word,
    /// Close authority in the connecting Alpha. For an import this need not
    /// equal the upstream Session badge observed by its Provider.
    pub session: SessionId,
    pub service: ServiceId,
}
