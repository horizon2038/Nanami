//! Use the current rooted lookup's type, never a cached pathname classification.
use super::*;
use nanami_client::vfs::{VFS_FILE_TYPE_DIRECTORY, VFS_FILE_TYPE_REGULAR, VFS_FILE_TYPE_SOCKET};

const KIND_FASTPATH: bool = match option_env!("NANAMI_READLINK_KIND_FASTPATH") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => true,
};

pub(super) fn sys_readlink(
    runtime: &mut Runtime, pid: Word, path: Word, output: Word, bytes: Word,
) -> Result<Word, i32> {
    sys_readlinkat(runtime, pid, LINUX_AT_FDCWD, path, output, bytes)
}

pub(super) fn sys_readlinkat(
    runtime: &mut Runtime, pid: Word, dirfd: Word, path: Word, output: Word, bytes: Word,
) -> Result<Word, i32> {
    let raw_bytes = read_c_string(runtime, pid, path)?;
    let resolved_bytes = if KIND_FASTPATH {
        let resolved = super::namespace::resolve_for_readlink(runtime, pid, dirfd, raw_bytes)?;
        // Preserve the existing lookup-before-output-validation error order.
        // A NULL nonempty output faults even for an existing regular file.
        if output == 0 && bytes != 0 { return Err(EFAULT); }
        if matches!(resolved.file_type, VFS_FILE_TYPE_REGULAR | VFS_FILE_TYPE_DIRECTORY | VFS_FILE_TYPE_SOCKET) {
            #[cfg(feature = "wait-shape")]
            super::wait_shape::path_step(runtime, pid, 2);
            // The successful rooted, no-follow lookup already observed a
            // non-link. Linearize this negative readlink at that observation;
            // do not walk the same path a second time just to obtain EINVAL.
            return Err(EINVAL);
        }
        resolved.bytes
    } else {
        super::namespace::resolve_at(runtime, pid, dirfd, raw_bytes, false)?
    };
    super::filesystem::readlink_from_current_path(runtime, pid, resolved_bytes, output, bytes)
}
