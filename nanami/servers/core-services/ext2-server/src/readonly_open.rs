//! Validate the complete reply before acquiring an owned read-only handle.
use super::path_walk::{Error, Kind};
use nanami_client::vfs::*;
use nanami_runtime::Word;

pub(super) fn complete(
    path: &[u8], kind: Option<Kind>, flags: Word, output: &mut [u8],
    acquire: impl FnOnce() -> Result<Option<Word>, Error>,
) -> Result<(Word, Word), Error> {
    if flags & !VFS_OPEN_DIRECTORY != 0 || path.first() != Some(&b'/') || path.contains(&0) {
        return Err(Error::Invalid);
    }
    let prefix = core::mem::size_of::<PathOpenResult>();
    let total = prefix.checked_add(path.len()).ok_or(Error::TooLong)?;
    if output.len() < total {
        return Err(Error::TooLong);
    }
    let file_type = match kind {
        Some(Kind::File) => VFS_FILE_TYPE_REGULAR,
        Some(Kind::Directory) => VFS_FILE_TYPE_DIRECTORY,
        Some(Kind::Symlink) => VFS_FILE_TYPE_SYMLINK,
        Some(Kind::Socket) => VFS_FILE_TYPE_SOCKET,
        None => VFS_FILE_TYPE_UNKNOWN,
    };
    let descriptor = if matches!(kind, Some(Kind::File | Kind::Directory))
        && (flags & VFS_OPEN_DIRECTORY == 0 || kind == Some(Kind::Directory))
    {
        acquire()?.unwrap_or(VFS_PATH_OPEN_NONE)
    } else {
        VFS_PATH_OPEN_NONE
    };
    // No fallible work after acquisition; no hidden retry or allocation.
    output[..prefix].copy_from_slice(&descriptor.to_ne_bytes());
    output[prefix..total].copy_from_slice(path);
    Ok((total, file_type))
}
