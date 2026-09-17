//! Real Alpha formatter/encoder and SDK decoder, with only the IPC transport mocked.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;
pub use a9n_types::InitInfo;
use std::cell::RefCell;
pub type Word = usize;
#[derive(Debug, PartialEq)]
pub enum CapabilityError {
    InvalidArgument,
}
#[derive(Debug, PartialEq)]
pub enum RequestError {
    Protocol,
    Status(Word),
}
const OS_REQUEST_NANAMI_INFO: Word = 0x1023;
const OS_RESPONSE_OK: Word = 0;

#[path = "../../nanami/servers/sdk/rust/libnanami/src/version.rs"]
mod client;
#[path = "../../nanami/src/nanami_core/alpha/version_info.rs"]
mod encoder;

#[derive(Default)]
struct Server {
    text: String,
    queries: Vec<(Word, Word)>,
    status: Word,
    invalid_utf8: bool,
}
thread_local! { static SERVER: RefCell<Server> = RefCell::new(Server::default()); }
fn call_os_port(
    code: Word,
    kind: Word,
    index: Word,
    a: Word,
    b: Word,
    words: u8,
) -> Result<(Word, Word, Word), RequestError> {
    assert_eq!((code, a, b, words), (OS_REQUEST_NANAMI_INFO, 0, 0, 3));
    SERVER.with(|server| {
        let mut server = server.borrow_mut();
        server.queries.push((kind, index));
        let (first, second) = if server.invalid_utf8 {
            (0xff, 0)
        } else {
            encoder::encode_chunk(&server.text, index).unwrap()
        };
        Ok((server.status, first, second))
    })
}
fn serve(text: &str) {
    SERVER.with(|s| {
        *s.borrow_mut() = Server {
            text: text.into(),
            ..Server::default()
        }
    });
}

#[test]
fn kernel_version_preserves_runtime_numbers_and_optional_suffixes() {
    let mut info: InitInfo = unsafe { core::mem::zeroed() };
    info.kernel_major_version = 3;
    info.kernel_minor_version = 12;
    info.kernel_patch_version = 123;
    assert_eq!(encoder::kernel_version(&info), "3.12.123");
    info.kernel_pre_release[..3].copy_from_slice(b"smp");
    assert_eq!(encoder::kernel_version(&info), "3.12.123-smp");
    let metadata = b"x86-64-Release-123456";
    info.kernel_build_metadata[..metadata.len()].copy_from_slice(metadata);
    assert_eq!(
        encoder::kernel_version(&info),
        "3.12.123-smp+x86-64-Release-123456"
    );
    info.kernel_pre_release.fill(0);
    assert_eq!(
        encoder::kernel_version(&info),
        "3.12.123+x86-64-Release-123456"
    );
}

#[test]
fn version_round_trips_all_chunk_boundaries_without_truncation() {
    for len in 1..=256 {
        let text = "v".repeat(len);
        serve(&text);
        let mut buffer = vec![0; len];
        assert_eq!(client::request_kernel_version(&mut buffer).unwrap(), text);
        SERVER.with(|s| {
            let s = s.borrow();
            assert_eq!(s.queries.len(), len / 16 + 1);
            assert!(s.queries.iter().all(|&(kind, _)| kind == 6));
        });
    }
    serve("0.8.9-beta+core-not-honoka");
    assert_eq!(
        client::request_nanami_version(&mut [0; 128]).unwrap(),
        "0.8.9-beta+core-not-honoka"
    );
    SERVER.with(|s| assert!(s.borrow().queries.iter().all(|&(kind, _)| kind == 7)));
}

#[test]
fn invalid_replies_and_small_buffers_fail_without_silent_truncation() {
    serve("01234567890123456789012345678901");
    assert_eq!(
        client::request_kernel_version(&mut [0; 20]),
        Err(RequestError::Protocol)
    );
    serve("");
    assert_eq!(
        client::request_kernel_version(&mut [0; 20]),
        Err(RequestError::Protocol)
    );
    SERVER.with(|s| s.borrow_mut().invalid_utf8 = true);
    assert_eq!(
        client::request_kernel_version(&mut [0; 20]),
        Err(RequestError::Protocol)
    );
    serve("version");
    SERVER.with(|s| s.borrow_mut().status = 3);
    assert_eq!(
        client::request_nanami_version(&mut [0; 20]),
        Err(RequestError::Status(3))
    );
    assert_eq!(
        encoder::encode_chunk("short", 1),
        Err(CapabilityError::InvalidArgument)
    );
    assert_eq!(
        encoder::encode_chunk("short", usize::MAX),
        Err(CapabilityError::InvalidArgument)
    );
}
