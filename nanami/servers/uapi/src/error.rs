use crate::Word;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    InvalidArgument,
    Unsupported,
    Transport,
    Protocol,
    Status(Word),
}

impl core::fmt::Display for RequestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            RequestError::InvalidArgument => f.write_str("invalid-arg"),
            RequestError::Unsupported => f.write_str("unsupported"),
            RequestError::Transport => f.write_str("transport"),
            RequestError::Protocol => f.write_str("protocol"),
            RequestError::Status(status) => write!(f, "status={:#x}", status),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiError(pub Word);

impl NanamiError {
    pub const UNKNOWN: Self = Self(1);
    pub const INVALID_ARGUMENT: Self = Self(2);
    pub const UNSUPPORTED: Self = Self(3);
    pub const TRANSPORT: Self = Self(4);
    pub const PROTOCOL: Self = Self(5);

    #[inline(always)]
    pub const fn value(self) -> Word {
        self.0
    }
}

impl From<RequestError> for NanamiError {
    fn from(value: RequestError) -> Self {
        match value {
            RequestError::InvalidArgument => Self::INVALID_ARGUMENT,
            RequestError::Unsupported => Self::UNSUPPORTED,
            RequestError::Transport => Self::TRANSPORT,
            RequestError::Protocol => Self::PROTOCOL,
            RequestError::Status(code) => Self(code),
        }
    }
}

pub type NanamiResult = core::result::Result<(), NanamiError>;
