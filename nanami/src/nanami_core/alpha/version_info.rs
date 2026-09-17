use alloc::{format, string::String};
use nun::{CapabilityError, InitInfo, Word};

pub(super) fn kernel_version(info: &InitInfo) -> String {
    let mut version = format!(
        "{}.{}.{}",
        info.kernel_major_version, info.kernel_minor_version, info.kernel_patch_version
    );
    for (separator, suffix) in [
        ('-', info.get_pre_release_string()),
        ('+', info.get_build_metadata_string()),
    ] {
        if !suffix.is_empty() {
            version.push(separator);
            version.push_str(suffix);
        }
    }
    version
}

// Two little-endian words per reply, terminated by NUL. A string ending at a
// chunk boundary has a final all-zero chunk. Names (selectors 4/5) retain their
// existing fixed-size wire format; versions (6/7) need not fit in 32 bytes.
pub(super) fn encode_chunk(text: &str, chunk: Word) -> Result<(Word, Word), CapabilityError> {
    const WORD_BYTES: usize = core::mem::size_of::<Word>();
    const CHUNK_BYTES: usize = 2 * WORD_BYTES;
    let start = chunk
        .checked_mul(CHUNK_BYTES)
        .filter(|&start| start <= text.len())
        .ok_or(CapabilityError::InvalidArgument)?;
    let source = &text.as_bytes()[start..];
    let mut bytes = [0; CHUNK_BYTES];
    let len = source.len().min(CHUNK_BYTES);
    bytes[..len].copy_from_slice(&source[..len]);
    let mut first = [0; WORD_BYTES];
    let mut second = [0; WORD_BYTES];
    first.copy_from_slice(&bytes[..WORD_BYTES]);
    second.copy_from_slice(&bytes[WORD_BYTES..]);
    Ok((Word::from_le_bytes(first), Word::from_le_bytes(second)))
}
