//! Negoto management protocol. Payloads/replies are UTF-8 JSON in per-client shared memory.
use crate::Word;
pub const NEGOTO_REQUEST_ATTACH: Word = 0xd101;
pub const NEGOTO_REQUEST_COMMAND: Word = 0xd102;
pub const NEGOTO_REQUEST_DETACH: Word = 0xd103;
pub const NEGOTO_SHM_BYTES: Word = 64 * 1024;
pub const NEGOTO_COMMAND_MAX_BYTES: Word = 1024;
pub const NEGOTO_CONTAINER_MAX: usize = 8;
pub const NEGOTO_CLIENT_MAX: usize = 16;
