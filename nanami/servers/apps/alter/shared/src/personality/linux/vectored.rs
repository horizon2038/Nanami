use super::{EFAULT, EINVAL, EIO};

/// Gather stream writes into the existing service buffer, without another allocation.
/// Commit successfully gathered bytes even if a later iovec cannot be copied.
pub(super) fn writev(
    mut iovecs: impl Iterator<Item = (usize, usize)>,
    capacity: usize,
    mut copy_into: impl FnMut(usize, usize, usize) -> Result<(), i32>,
    mut write: impl FnMut(usize) -> Result<usize, i32>,
) -> Result<usize, i32> {
    if capacity == 0 {
        return Err(EIO);
    }
    let mut base = 0usize;
    let mut len = 0usize;
    let mut offset = 0usize;
    let mut total = 0usize;
    loop {
        let mut staged = 0usize;
        let mut copy_error = None;
        while staged < capacity {
            if offset == len {
                let Some(next) = iovecs.next() else { break };
                (base, len) = next;
                offset = 0;
                if len == 0 {
                    continue;
                }
            }
            let chunk = (len - offset).min(capacity - staged);
            let result = base
                .checked_add(offset)
                .ok_or(EFAULT)
                .and_then(|source| copy_into(source, staged, chunk));
            if let Err(error) = result {
                copy_error = Some(error);
                break;
            }
            staged += chunk;
            offset += chunk;
        }
        if staged != 0 {
            let result = write(staged).and_then(|written| {
                if written <= staged {
                    Ok(written)
                } else {
                    Err(EIO)
                }
            });
            match result {
                Ok(written) => {
                    total = total.checked_add(written).ok_or(EINVAL)?;
                    if written < staged {
                        return Ok(total);
                    }
                }
                Err(_) if total != 0 => return Ok(total),
                Err(error) => return Err(error),
            }
        }
        if let Some(error) = copy_error {
            return if total != 0 { Ok(total) } else { Err(error) };
        }
        if staged < capacity {
            return Ok(total);
        }
    }
}
