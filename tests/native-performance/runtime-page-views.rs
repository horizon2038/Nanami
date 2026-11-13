//! Execute the production request encoder and response validation.
#![allow(dead_code)]
use std::cell::RefCell;
use nanami_uapi::os::*;
use nanami_uapi::RequestError;
type Word = usize;
#[derive(Default)]
struct Fake {
    calls: Vec<[Word; 5]>,
    entries: Vec<(Word, Word)>,
    locals: Vec<Word>,
    reply: Option<Result<(Word, Word, Word), RequestError>>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
fn call_os_port(code: Word, pid: Word, pointer: Word, count: Word, zero: Word, length: u8)
    -> Result<(Word, Word, Word), RequestError>
{
    assert_eq!(length, 5);
    FAKE.with_borrow_mut(|fake| {
        fake.calls.push([code, pid, pointer, count, zero]);
        if code == OS_REQUEST_MAPPING_RELEASE_VIEWS {
            assert_eq!((count, zero), (0, 0));
            fake.locals = unsafe { core::slice::from_raw_parts(pid as *const Word, pointer) }.to_vec();
            return fake.reply.unwrap_or(Ok((OS_RESPONSE_OK, (1 << pointer) - 1, pointer)));
        }
        let entries = unsafe { core::slice::from_raw_parts(pointer as *const OsPageViewEntry, count) };
        fake.entries = entries.iter().map(|entry| (entry.target, entry.local)).collect();
        fake.reply.unwrap_or(Ok((OS_RESPONSE_OK, count, 0)))
    })
}
#[path = "../../nanami/servers/libs/nanami-runtime/src/page_views.rs"]
mod production;

fn entries() -> [OsPageViewEntry; 2] {
    [OsPageViewEntry { target: 0x2000, local: 0x8000 }, OsPageViewEntry { target: 0x3000, local: 0xc000 }]
}
fn reset() { FAKE.with_borrow_mut(|fake| *fake = Fake::default()); }

#[test]
fn both_view_vectors_transmit_the_whole_descriptor_in_one_five_word_request() {
    for writable in [false, true] {
        reset();
        let entries = entries();
        let result = if writable { production::request_process_write_views(7, &entries) }
            else { production::request_process_read_views(7, &entries) };
        assert_eq!(result, Ok((2, 0)));
        FAKE.with_borrow(|fake| {
            assert_eq!(fake.calls, [[if writable { OS_REQUEST_PROCESS_WRITE_VIEWS } else { OS_REQUEST_PROCESS_READ_VIEWS }, 7, entries.as_ptr() as Word, 2, 0]]);
            assert_eq!(fake.entries, [(0x2000, 0x8000), (0x3000, 0xc000)]);
        });
    }
}

#[test]
fn view_vector_rejects_invalid_shapes_without_issuing_ipc() {
    for invalid in 0..7 {
        reset();
        let mut entries = entries().to_vec();
        let mut pid = 7;
        match invalid {
            0 => pid = 0,
            1 => entries.clear(),
            2 => entries.resize(OS_PAGE_VIEW_VECTOR_MAX + 1, entries[0]),
            3 => entries[0].target += 1,
            4 => entries[0].local = 0,
            5 => entries[0].target = Word::MAX & !4095,
            _ => entries[0].local = Word::MAX & !4095,
        }
        assert_eq!(production::request_process_read_views(pid, &entries), Err(RequestError::InvalidArgument));
        FAKE.with_borrow(|fake| assert!(fake.calls.is_empty()));
    }
}

#[test]
fn view_vector_preserves_prefix_status_but_rejects_untrustworthy_completion() {
    for reply in [(0, 3, 0), (0, 1, 0), (0, 2, OS_RESPONSE_FATAL), (0, 0, 0)] {
        reset();
        FAKE.with_borrow_mut(|fake| fake.reply = Some(Ok(reply)));
        assert_eq!(production::request_process_write_views(7, &entries()), Err(RequestError::Protocol));
    }
    reset();
    FAKE.with_borrow_mut(|fake| fake.reply = Some(Ok((0, 1, OS_RESPONSE_PERMISSION_DENIED))));
    assert_eq!(production::request_process_write_views(7, &entries()), Ok((1, OS_RESPONSE_PERMISSION_DENIED)));
    for error in [RequestError::Transport, RequestError::Status(OS_RESPONSE_PERMISSION_DENIED)] {
        reset();
        FAKE.with_borrow_mut(|fake| fake.reply = Some(if error == RequestError::Transport { Err(error) }
            else { Ok((OS_RESPONSE_PERMISSION_DENIED, 0, 0)) }));
        assert_eq!(production::request_process_write_views(7, &entries()), Err(error));
    }
}

#[test]
fn release_vector_transmits_exact_local_pages_and_validates_each_acknowledgement() {
    let locals = [0x8000, 0xc000, 0x10000];
    for mask in 0..8 {
        reset();
        FAKE.with_borrow_mut(|fake| fake.reply = Some(Ok((0, mask, 3))));
        assert_eq!(production::request_mapping_release_views(&locals), Ok(mask));
        FAKE.with_borrow(|fake| {
            assert_eq!(fake.calls, [[OS_REQUEST_MAPPING_RELEASE_VIEWS, locals.as_ptr() as Word, 3, 0, 0]]);
            assert_eq!(fake.locals, locals);
        });
    }
    for reply in [(0, 8, 3), (0, 7, 2), (0, 7, 4)] {
        reset();
        FAKE.with_borrow_mut(|fake| fake.reply = Some(Ok(reply)));
        assert_eq!(production::request_mapping_release_views(&locals), Err(RequestError::Protocol));
    }
    for error in [RequestError::Transport, RequestError::Status(OS_RESPONSE_FATAL)] {
        reset();
        FAKE.with_borrow_mut(|fake| fake.reply = Some(if error == RequestError::Transport { Err(error) }
            else { Ok((OS_RESPONSE_FATAL, 7, 3)) }));
        assert_eq!(production::request_mapping_release_views(&locals), Err(error));
    }
}

#[test]
fn release_vector_rejects_bad_local_pages_before_ipc() {
    for locals in [vec![], vec![0x8000; OS_PAGE_VIEW_VECTOR_MAX + 1], vec![0x8000, 0x8000],
        vec![0], vec![0x8001], vec![Word::MAX & !4095]] {
        reset();
        assert_eq!(production::request_mapping_release_views(&locals), Err(RequestError::InvalidArgument));
        FAKE.with_borrow(|fake| assert!(fake.calls.is_empty()));
    }
}
