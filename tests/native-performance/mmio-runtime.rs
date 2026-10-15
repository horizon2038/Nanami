//! Production runtime MMIO request encoder; lengths include the request code.
#![allow(dead_code)]
pub use a9n_abi::capability_call::address_space::Attribute;
use std::cell::RefCell;
type Word = usize;
const OS_REQUEST_MMIO_REQUEST: Word = nanami_uapi::os::OS_REQUEST_MMIO_REQUEST;
const OS_RESPONSE_OK: Word = nanami_uapi::os::OS_RESPONSE_OK;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestError {
    InvalidArgument,
    Transport,
    Status(Word),
}
#[derive(Default)]
struct Fake {
    calls: Vec<Vec<Word>>,
    error: Option<RequestError>,
    status: Word,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
fn call_os_port(
    code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
    arg3: Word,
    length: u8,
) -> Result<(Word, Word, Word), RequestError> {
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        let words = [code, arg0, arg1, arg2, arg3];
        // Copy exactly what the real IPC length would transfer to Alpha.
        fake.calls.push(words[..length as usize].to_vec());
        if let Some(error) = fake.error {
            return Err(error);
        }
        Ok((fake.status, arg0, 0x10000000))
    })
}
#[path = "../../nanami/servers/libs/nanami-runtime/src/mmio.rs"]
mod mmio;
fn reset() {
    FAKE.with(|fake| *fake.borrow_mut() = Fake::default());
}

#[test]
fn default_mmio_request_transmits_uncached_without_losing_arg2() {
    reset();
    assert_eq!(
        mmio::request_mmio(0xfed00000, 4096),
        Ok((0xfed00000, 0x10000000))
    );
    FAKE.with(|fake| {
        assert_eq!(
            fake.borrow().calls,
            [vec![OS_REQUEST_MMIO_REQUEST, 0xfed00000, 4096, 0x20]]
        )
    });
}

#[test]
fn each_explicit_flag_reaches_the_last_transferred_register() {
    for flag in [Attribute::NORMAL, Attribute::BUFFERED, Attribute::UNCACHED] {
        reset();
        assert!(mmio::request_mmio_with_mapping(0x90000000, 8192, flag).is_ok());
        FAKE.with(|fake| {
            assert_eq!(
                fake.borrow().calls,
                [vec![OS_REQUEST_MMIO_REQUEST, 0x90000000, 8192, flag.bits()]]
            )
        });
    }
}

#[test]
fn invalid_requests_do_not_issue_ipc() {
    reset();
    assert_eq!(
        mmio::request_mmio(0, 4096),
        Err(RequestError::InvalidArgument)
    );
    assert_eq!(
        mmio::request_mmio(0x90000000, 0),
        Err(RequestError::InvalidArgument)
    );
    for attribute in [
        Attribute::ALL,
        Attribute::BUFFERED | Attribute::UNCACHED,
        Attribute::from_bits_retain(8),
        Attribute::from_bits_retain(0x100),
    ] {
        assert_eq!(
            mmio::request_mmio_with_mapping(0x90000000, 4096, attribute),
            Err(RequestError::InvalidArgument)
        );
    }
    FAKE.with(|fake| assert!(fake.borrow().calls.is_empty()));
}

#[test]
fn published_sdk_attributes_match_the_unchanged_kernel_word() {
    assert_eq!(
        core::mem::size_of::<Attribute>(),
        core::mem::size_of::<Word>()
    );
    assert_eq!(Attribute::ALL.bits(), 7);
    for rights in 0..8 {
        let rights = Attribute::from_bits(rights).unwrap();
        assert_eq!((rights | Attribute::NORMAL).bits(), rights.bits());
        assert_eq!((rights | Attribute::BUFFERED).bits(), rights.bits() | 0x10);
        assert_eq!((rights | Attribute::UNCACHED).bits(), rights.bits() | 0x20);
    }
}

#[test]
fn alpha_and_transport_errors_are_preserved() {
    reset();
    FAKE.with(|fake| fake.borrow_mut().status = 5);
    assert_eq!(
        mmio::request_mmio(0x90000000, 4096),
        Err(RequestError::Status(5))
    );
    FAKE.with(|fake| fake.borrow_mut().error = Some(RequestError::Transport));
    assert_eq!(
        mmio::request_mmio(0x90000000, 4096),
        Err(RequestError::Transport)
    );
}
