use crate::Word;

pub const SERVICE_PORT_SLOT: Word = 20;

pub const NET_DEVICE: &str = "net-device";

pub const NETWORK_SERVICE: &str = "network-service";

pub const TIMER_SERVICE: &str = "timer-service";

pub const DISPLAY_SERVICE: &str = "display_service";

pub const INPUT_SERVICE: &str = "input-service";

pub const HONOKA_SERVICE: &str = "honoka-service";

pub const RTC_SERVICE: &str = "rtc-service";

pub const BLOCK_DEVICE: &str = "block-device";

pub const VFS_SERVICE: &str = "vfs-service";

pub const POSIX_SERVICE: &str = "posix-service";

pub const TERMINAL_SERVICE: &str = "terminal-service";

pub const EXEC_SERVICE: &str = "exec-service";

pub const SERVICE_NAME_BYTES: usize = 24;

/// Local diagnostic information; the owner PID is not a cross-domain identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub owner_pid: Word,
    pub name_len: Word,
    pub name: [u8; SERVICE_NAME_BYTES],
}

impl ServiceInfo {
    pub fn name(&self) -> &str {
        self.name
            .get(..self.name_len)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }
}
