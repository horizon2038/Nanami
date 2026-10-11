use crate::Word;

pub const EXEC_REQUEST_CONTROL: Word = 0xd101;

pub const EXEC_REQUEST_SPAWN_PATH: Word = 0xd102;

pub const EXEC_REQUEST_SPAWN_PATH_ARGUMENTS: Word = 0xd103;

pub const EXEC_REQUEST_PROCESS_STATUS: Word = 0xd104;

pub const EXEC_REQUEST_PROCESS_REAP: Word = 0xd105;

pub const EXEC_REQUEST_PROCESS_KILL: Word = 0xd106;

pub const EXEC_CONTROL_ATTACH_SHARED_MEMORY: Word = 1;

pub const EXEC_DEFAULT_SHM_BYTES: Word = 0x4000;

pub const EXEC_LAUNCH_FLAG_DIAGNOSTICS: Word = 1 << (Word::BITS - 1);
