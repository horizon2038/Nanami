use alloc::{format, string::String};
use nanami_uapi::alpha::{name, KernelVersion};
use nun::{CapabilityError, Word};

pub(super) fn kernel_version(info: &KernelVersion) -> String {
    let mut version = format!("{}.{}.{}", info.major, info.minor, info.patch);
    for (separator, suffix) in [
        ('-', name(&info.pre_release)),
        ('+', name(&info.build_metadata)),
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
