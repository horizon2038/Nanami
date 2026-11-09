extern crate self as a9n_abi;
pub use nanami_uapi as uapi;
pub type Word = usize;
pub type CapabilityDescriptor = Word;
pub const OS_RESPONSE_OK: Word = 0;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RequestError { Status(Word), Transport, Unsupported, Protocol }
mod constants { pub use nanami_uapi::vfs::*; pub use nanami_uapi::posix::*; }
use constants::*;
thread_local! {
    static REPLY: std::cell::Cell<Result<(Word,Word,Word),RequestError>> = const { std::cell::Cell::new(Ok((0,16,1))) };
    static CALL: std::cell::Cell<(Word,Word,Word)> = const { std::cell::Cell::new((0,0,0)) };
    static ARGS: std::cell::Cell<[Word;4]> = const { std::cell::Cell::new([0;4]) };
}
fn call_port(_: Word, code: Word, arg0: Word, arg1: Word, arg2: Word, arg3: Word, words: Word)
    -> Result<(Word,Word,Word),RequestError> {
    CALL.with(|call| call.set((code,arg0,words)));
    ARGS.with(|args| args.set([arg0,arg1,arg2,arg3]));
    REPLY.with(|reply| reply.get())
}
#[path = "../../nanami/servers/libs/nanami-client/src/vfs/namespace.rs"]
mod namespace;
#[path = "../../nanami/servers/libs/nanami-client/src/posix/directory.rs"]
mod directory;

#[test]
fn biased_directory_wire_carries_bias_and_returns_raw_vfs_cookie() {
    REPLY.with(|reply| reply.set(Ok((0,7,20))));
    assert_eq!(directory::posix_read_dir_biased(1,9,16,512,3), Ok((7,20)));
    CALL.with(|call| assert_eq!(call.get(),(POSIX_REQUEST_READ_DIR_BIASED,9,5)));
    ARGS.with(|args| assert_eq!(args.get(),[9,16,512,3]));
}

#[test]
fn genuine_biased_directory_errors_are_distinct_from_old_entry_rejection() {
    REPLY.with(|reply| reply.set(Ok((POSIX_READ_DIR_BIASED_ERROR,uapi::os::OS_RESPONSE_INVALID_ARGUMENT,0))));
    assert_eq!(directory::posix_read_dir_biased(1,9,16,512,3), Err(RequestError::Status(uapi::os::OS_RESPONSE_INVALID_ARGUMENT)));
    REPLY.with(|reply| reply.set(Ok((uapi::os::OS_RESPONSE_INVALID_ARGUMENT,0,0))));
    assert_eq!(directory::posix_read_dir_biased(1,9,16,512,3), Err(RequestError::Unsupported));
    REPLY.with(|reply| reply.set(Err(RequestError::Transport)));
    assert_eq!(directory::posix_read_dir_biased(1,9,16,512,3), Err(RequestError::Transport));
    REPLY.with(|reply| reply.set(Ok((POSIX_READ_DIR_BIASED_ERROR,0,0))));
    assert_eq!(directory::posix_read_dir_biased(1,9,16,512,3), Err(RequestError::Protocol));
}

#[test]
fn rooted_open_uses_a_distinct_request_and_retains_the_reply() {
    REPLY.with(|reply| reply.set(Ok((OS_RESPONSE_OK,16,VFS_FILE_TYPE_REGULAR))));
    assert!(matches!(namespace::posix_rooted_open_request(1,64),Ok((16,VFS_FILE_TYPE_REGULAR))));
    CALL.with(|call| assert_eq!(call.get(),(uapi::posix::POSIX_REQUEST_OPEN_ROOTED,64,2)));
}

#[test]
fn only_legacy_rejection_of_the_new_entry_is_mapped_to_unsupported() {
    REPLY.with(|reply| reply.set(Ok((uapi::os::OS_RESPONSE_INVALID_ARGUMENT,0,0))));
    assert!(matches!(namespace::posix_rooted_open_request(1,64),
        Err(namespace::PathError::Filesystem(VFS_PATH_ERROR_UNSUPPORTED))));
    assert!(matches!(namespace::posix_path_request(1,64),
        Err(namespace::PathError::Transport(RequestError::Status(uapi::os::OS_RESPONSE_INVALID_ARGUMENT)))));
    CALL.with(|call| assert_eq!(call.get().0,uapi::posix::POSIX_REQUEST_PATH));
}

#[test]
fn transport_and_filesystem_errors_are_not_fallback_signals() {
    REPLY.with(|reply| reply.set(Err(RequestError::Transport)));
    assert!(matches!(namespace::posix_rooted_open_request(1,64),Err(namespace::PathError::Transport(RequestError::Transport))));
    REPLY.with(|reply| reply.set(Ok((VFS_PATH_RESPONSE_ERROR,VFS_PATH_ERROR_IO,0))));
    assert!(matches!(namespace::posix_rooted_open_request(1,64),Err(namespace::PathError::Filesystem(VFS_PATH_ERROR_IO))));
}
