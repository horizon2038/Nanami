extern crate self as nanami_client;
extern crate self as nanami_runtime;
extern crate alloc;
type Word = usize;
mod vfs { pub use nanami_uapi::vfs::*; }
use vfs::*;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_walk.rs"]
mod path_walk;
use path_walk::{Error, Kind};
#[path = "../../nanami/servers/core-services/ext2-server/src/readonly_open.rs"]
mod readonly_open;

#[test]
fn normal_readonly_open_acquires_once_after_validating_the_whole_reply() {
    for kind in [Kind::File, Kind::Directory] {
        let mut output = [0xaa; 128]; let mut acquired = 0;
        let result = readonly_open::complete(b"/dir/file", Some(kind), 0, &mut output, || {
            acquired += 1; Ok(Some(7))
        }).unwrap();
        assert_eq!(acquired, 1);
        assert_eq!(result.0, std::mem::size_of::<PathOpenResult>() + 9);
        assert_eq!(&output[..8], &7usize.to_ne_bytes());
        assert_eq!(&output[8..17], b"/dir/file");
        assert_eq!(output[17], 0xaa);
    }
}

#[test]
fn short_output_or_invalid_flags_cannot_allocate_a_handle_or_publish_bytes() {
    for (path, flags, bytes) in [(b"/file".as_slice(), 0, 12), (b"file", 0, 128),
        (b"/file", VFS_OPEN_CREATE, 128), (b"/fi\0le", 0, 128)] {
        let mut output = vec![0xaa; bytes];
        assert!(readonly_open::complete(path, Some(Kind::File), flags, &mut output,
            || panic!("must validate before acquisition")).is_err());
        assert!(output.iter().all(|byte| *byte == 0xaa));
    }
}

#[test]
fn missing_symlink_socket_and_directory_mismatch_decline_without_acquiring() {
    for (kind, flags) in [(None, 0), (Some(Kind::Symlink), 0), (Some(Kind::Socket), 0),
        (Some(Kind::File), VFS_OPEN_DIRECTORY)] {
        let mut output = [0; 128];
        readonly_open::complete(b"/name", kind, flags, &mut output,
            || panic!("declined kinds must not open")).unwrap();
        assert_eq!(&output[..8], &VFS_PATH_OPEN_NONE.to_ne_bytes());
        assert_eq!(&output[8..13], b"/name");
    }
}

#[test]
fn failed_acquisition_does_not_publish_and_full_table_declines_cleanly() {
    let mut output = [0xaa; 128];
    assert_eq!(readonly_open::complete(b"/name", Some(Kind::File), 0, &mut output,
        || Err(Error::Io)), Err(Error::Io));
    assert!(output.iter().all(|byte| *byte == 0xaa));
    readonly_open::complete(b"/name", Some(Kind::Directory), VFS_OPEN_DIRECTORY,
        &mut output, || Ok(None)).unwrap();
    assert_eq!(&output[..8], &VFS_PATH_OPEN_NONE.to_ne_bytes());
}
