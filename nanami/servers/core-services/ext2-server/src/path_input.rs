//! Copy request fields before filesystem I/O or reply writes can reuse SHM.
use super::path_walk::{Error, PATH_MAX};
use alloc::borrow::Cow;

fn check(capacity: usize, offset: usize, bytes: usize) -> Result<(), Error> {
    if bytes > PATH_MAX || offset.checked_add(bytes).is_none_or(|end| end > capacity) {
        return Err(Error::Invalid);
    }
    Ok(())
}

pub(super) fn path<'a>(
    base: usize, capacity: usize, offset: usize, bytes: usize, storage: &'a mut [u8],
) -> Result<Cow<'a, [u8]>, Error> {
    check(capacity, offset, bytes)?;
    let source = unsafe {
        core::slice::from_raw_parts((base + offset) as *const u8, bytes)
    };
    if bytes <= storage.len() {
        let output = &mut storage[..bytes];
        output.copy_from_slice(source);
        // Borrow only the owned caller storage, never session SHM across I/O.
        Ok(Cow::Borrowed(output))
    } else {
        Ok(Cow::Owned(source.to_vec()))
    }
}

pub(super) fn root(base: usize, capacity: usize, offset: usize, bytes: usize) -> Result<(), Error> {
    check(capacity, offset, bytes)?;
    if bytes == 0 { return Err(Error::Invalid); }
    // The caller consumes only this byte, then performs lookup against the
    // original session SHM. No slice/owned snapshot is retained across I/O.
    if unsafe { *((base + offset) as *const u8) } != b'/' { return Err(Error::Invalid); }
    Ok(())
}

pub(super) fn word(base: usize, capacity: usize, offset: usize, bytes: usize) -> Result<usize, Error> {
    check(capacity, offset, bytes)?;
    if bytes != core::mem::size_of::<usize>() { return Err(Error::Invalid); }
    // The complete scalar is copied before filesystem I/O or reply writes.
    Ok(usize::from_le(unsafe { core::ptr::read_unaligned((base + offset) as *const usize) }))
}
