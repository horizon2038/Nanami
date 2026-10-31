//! Advisory file-cache hints; no change to file contents or descriptor offsets.
use super::{LinuxFileKind, Runtime, Word, EBADF, EINVAL, ESPIPE};

pub(super) fn advise(
    runtime: &Runtime,
    pid: Word,
    fd: Word,
    _offset: Word,
    length: Word,
    advice: Word,
) -> Result<Word, i32> {
    // The Linux ABI declares fd/advice as int, unlike the 64-bit file range.
    let file = runtime.linux_file(pid, fd as u32 as Word).ok_or(EBADF)?;
    const O_PATH: Word = 0x20_0000;
    if file.has_posix_backend() && file.resource & O_PATH != 0 {
        return Err(EBADF);
    }
    if matches!(
        file.kind,
        LinuxFileKind::PipeRead | LinuxFileKind::PipeWrite
    ) {
        return Err(ESPIPE);
    }
    if (length as i64) < 0 || advice as u32 > 5 {
        return Err(EINVAL);
    }
    // NORMAL, RANDOM, SEQUENTIAL, WILLNEED, DONTNEED and NOREUSE are hints,
    // not requirements. The current cache has no per-file readahead/LRU policy.
    // In particular DONTNEED must not discard dirty data or shared-page leases.
    // Like Linux generic_fadvise, do not reject negative offsets or range
    // overflow: accepting a hint requires no arithmetic on this range here.
    Ok(0)
}
