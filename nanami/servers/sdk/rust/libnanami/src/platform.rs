use crate::{
    call_os_port, request_nanami_info_smp, RequestError, Word, OS_REQUEST_NANAMI_INFO,
    OS_RESPONSE_OK,
};

mod name;

const NANAMI_INFO_ARCHITECTURE_NAME: Word = 4;
const NANAMI_INFO_PLATFORM_NAME: Word = 5;
const NAME_BYTES: usize = 32;

/// Environment supplied by A9N's init_info, not inferred from the client binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NanamiPlatformInfo {
    architecture_name: [u8; NAME_BYTES],
    platform_name: [u8; NAME_BYTES],
    pub core_count: Word,
}

impl NanamiPlatformInfo {
    pub fn architecture_name(&self) -> &str {
        name_string(&self.architecture_name).unwrap_or("")
    }

    pub fn platform_name(&self) -> &str {
        name_string(&self.platform_name).unwrap_or("")
    }
}

fn name_string(bytes: &[u8; NAME_BYTES]) -> Result<&str, RequestError> {
    name::decode_name(bytes).ok_or(RequestError::Protocol)
}

fn request_name(kind: Word) -> Result<[u8; NAME_BYTES], RequestError> {
    const WORD_BYTES: usize = core::mem::size_of::<Word>();
    const CHUNK_BYTES: usize = 2 * WORD_BYTES;
    let mut bytes = [0u8; NAME_BYTES];
    for (index, chunk) in bytes.chunks_exact_mut(CHUNK_BYTES).enumerate() {
        let (status, first, second) = call_os_port(OS_REQUEST_NANAMI_INFO, kind, index, 0, 0, 3)?;
        if status != OS_RESPONSE_OK {
            return Err(RequestError::Status(status));
        }
        chunk[..WORD_BYTES].copy_from_slice(&first.to_le_bytes());
        chunk[WORD_BYTES..].copy_from_slice(&second.to_le_bytes());
    }
    name_string(&bytes)?;
    Ok(bytes)
}

pub fn request_nanami_info_platform() -> Result<NanamiPlatformInfo, RequestError> {
    Ok(NanamiPlatformInfo {
        architecture_name: request_name(NANAMI_INFO_ARCHITECTURE_NAME)?,
        platform_name: request_name(NANAMI_INFO_PLATFORM_NAME)?,
        core_count: request_nanami_info_smp()?.online_cores,
    })
}
