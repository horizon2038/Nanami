//! Bounded ordinary-file reads through the POSIX service window.
use super::{Word, EIO};

pub(super) fn read(
    len: Word,
    capacity: Word,
    mut service_read: impl FnMut(Word) -> Result<(Word, Word), i32>,
    mut copy: impl FnMut(Word, Word, Word) -> Result<(), i32>,
) -> Result<Word, i32> {
    let mut done = 0;
    while done < len {
        let chunk = (len - done).min(capacity.max(1));
        let (bytes, source) = service_read(chunk)?;
        if bytes > chunk {
            return Err(EIO);
        }
        if bytes == 0 {
            break;
        }
        copy(done, source, bytes)?;
        done += bytes;
        // A successful short read is a complete read result. Do not issue
        // another service request merely to confirm EOF or fill the window.
        if bytes < chunk {
            break;
        }
    }
    Ok(done)
}
