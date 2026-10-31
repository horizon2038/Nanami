//! Production VFS page leases; only Alpha and disk I/O are mocked.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::{cell::RefCell, collections::BTreeMap, ptr};
type Word = usize;
const EXT2_S_IFMT: u16 = 0xf000;
const EXT2_S_IFREG: u16 = 0x8000;
const OS_RESPONSE_OK: Word = 0;
const OS_RESPONSE_INVALID_ARGUMENT: Word = 1;
const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
const OS_RESPONSE_ILLEGAL_OPERATION: Word = 4;
const OS_RESPONSE_FATAL: Word = 5;
#[path = "../../nanami/servers/uapi/src/vfs.rs"]
mod vfs;
#[derive(Clone, Copy, Debug, PartialEq)]
enum RequestError {
    InvalidArgument,
    Unsupported,
    Protocol,
    Status(Word),
}
fn map_request_error_to_status(error: RequestError) -> Word {
    match error {
        RequestError::InvalidArgument => OS_RESPONSE_INVALID_ARGUMENT,
        RequestError::Unsupported => OS_RESPONSE_ILLEGAL_OPERATION,
        RequestError::Protocol => OS_RESPONSE_FATAL,
        RequestError::Status(status) => status,
    }
}
#[derive(Clone, Copy)]
struct ServiceRequest {
    identifier: Word,
    code: Word,
    arg0: Word,
    arg1: Word,
    arg2: Word,
}
#[derive(Clone, Copy)]
struct FileHandle {
    active: bool,
    owner_pid: Word,
    inode: u32,
    mode: u16,
}
#[derive(Clone, Copy)]
struct Ext2Inode {
    size: u32,
}
struct ClientSession {
    active: bool,
    pid: Word,
    shm_local: Word,
    shm_size: Word,
}
struct Ext2Runtime {
    handles: [FileHandle; 2],
    mapped_pages: mapped_pages::Pages,
    frozen_by: Option<Word>,
    data: Vec<u8>,
    fail_read: bool,
}
#[path = "../../nanami/servers/core-services/ext2-server/src/mapped_pages.rs"]
mod mapped_pages;
#[repr(align(4096))]
struct Page([u8; 4096]);
#[derive(Default)]
struct Alpha {
    pages: BTreeMap<Word, Box<Page>>,
    shares: Vec<(Word, Word)>,
    fail_share: bool,
    fail_release: bool,
}
thread_local! { static ALPHA: RefCell<Alpha> = RefCell::new(Alpha::default()); }
fn request_memory_create(bytes: Word) -> Result<Word, RequestError> {
    assert_eq!(bytes, 4096);
    let mut page = Box::new(Page([0; 4096]));
    let address = page.0.as_mut_ptr() as Word;
    ALPHA.with(|alpha| alpha.borrow_mut().pages.insert(address, page));
    Ok(address)
}
fn request_mapping_share(peer: Word, address: Word, bytes: Word) -> Result<Word, RequestError> {
    assert_eq!(bytes, 4096);
    ALPHA.with(|alpha| {
        let mut alpha = alpha.borrow_mut();
        if alpha.fail_share {
            return Err(RequestError::Protocol);
        }
        alpha.shares.push((peer, address));
        // Tests dereference the very same RAM; no private data snapshots.
        Ok(address)
    })
}
fn request_mapping_release(address: Word, bytes: Word) -> Result<(), RequestError> {
    assert_eq!(bytes, 4096);
    ALPHA.with(|alpha| {
        let mut alpha = alpha.borrow_mut();
        if alpha.fail_release {
            return Err(RequestError::Protocol);
        }
        assert!(alpha.pages.remove(&address).is_some());
        Ok(())
    })
}
fn read_inode(runtime: &mut Ext2Runtime, inode: u32) -> Result<Ext2Inode, RequestError> {
    assert_eq!(inode, 7);
    Ok(Ext2Inode {
        size: runtime.data.len() as u32,
    })
}
fn read_file(
    runtime: &mut Ext2Runtime,
    session: ClientSession,
    _: Ext2Inode,
    offset: usize,
    bytes: usize,
    output: usize,
) -> Result<usize, Word> {
    if runtime.fail_read {
        return Err(OS_RESPONSE_FATAL);
    }
    let count = bytes.min(runtime.data.len() - offset);
    unsafe {
        ptr::copy_nonoverlapping(
            runtime.data[offset..].as_ptr(),
            (session.shm_local + output) as *mut u8,
            count,
        );
    }
    Ok(count)
}
fn runtime() -> Ext2Runtime {
    ALPHA.with(|alpha| *alpha.borrow_mut() = Alpha::default());
    Ext2Runtime {
        handles: [FileHandle {
            active: true,
            owner_pid: 10,
            inode: 7,
            mode: EXT2_S_IFREG,
        }; 2],
        mapped_pages: mapped_pages::Pages::new(),
        frozen_by: None,
        data: vec![0x35; 5000],
        fail_read: false,
    }
}
fn request(handle: Word, peer: Word, offset: Word, unmap: bool) -> ServiceRequest {
    ServiceRequest {
        identifier: 10,
        code: if unmap {
            vfs::VFS_REQUEST_UNMAP_READ_PAGE
        } else {
            vfs::VFS_REQUEST_MAP_READ_PAGE
        },
        arg0: handle,
        arg1: offset,
        arg2: peer,
    }
}

#[test]
fn aliases_share_one_inode_page_and_native_writes_are_immediately_visible() {
    let mut runtime = runtime();
    let (_, first, _) = mapped_pages::handle(request(0, 20, 0, false), &mut runtime);
    let (_, other, _) = mapped_pages::handle(request(1, 21, 0, false), &mut runtime);
    assert_eq!(first, other);
    let changed = b"visible";
    unsafe {
        runtime
            .mapped_pages
            .update(7, 11, changed.as_ptr() as Word, changed.len());
        assert_eq!(
            std::slice::from_raw_parts((other + 11) as *const u8, changed.len()),
            changed
        );
    }
    assert!(runtime.mapped_pages.pins(7));
    assert_eq!(
        mapped_pages::handle(request(0, 20, 0, true), &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert!(runtime.mapped_pages.pins(7));
    assert_eq!(
        mapped_pages::handle(request(1, 21, 0, true), &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert!(!runtime.mapped_pages.pins(7));
    ALPHA.with(|alpha| assert!(alpha.borrow().pages.is_empty()));
}

#[test]
fn partial_eof_is_zero_and_cross_page_updates_only_touch_intersections() {
    let mut runtime = runtime();
    let (_, first, _) = mapped_pages::handle(request(0, 20, 0, false), &mut runtime);
    let (_, last, _) = mapped_pages::handle(request(0, 20, 4096, false), &mut runtime);
    unsafe {
        assert!(
            std::slice::from_raw_parts((last + 904) as *const u8, 4096 - 904)
                .iter()
                .all(|b| *b == 0)
        );
    }
    let changed = [0x71u8; 16];
    unsafe {
        runtime
            .mapped_pages
            .update(7, 4090, changed.as_ptr() as Word, changed.len());
        assert_eq!(
            std::slice::from_raw_parts((first + 4090) as *const u8, 6),
            &[0x71; 6]
        );
        assert_eq!(
            std::slice::from_raw_parts(last as *const u8, 10),
            &[0x71; 10]
        );
        assert_eq!(*(last as *const u8).add(10), 0x35);
    }
}

#[test]
fn leases_validate_owner_handle_peer_and_offset_before_allocation_or_release() {
    let mut runtime = runtime();
    for bad in [
        request(2, 20, 0, false),
        request(0, 0, 0, false),
        request(0, 20, 1, false),
        request(0, 20, 8192, false),
    ] {
        assert_ne!(mapped_pages::handle(bad, &mut runtime).0, OS_RESPONSE_OK);
    }
    let mut bad = request(0, 20, 0, false);
    bad.identifier = 99;
    assert_eq!(
        mapped_pages::handle(bad, &mut runtime).0,
        OS_RESPONSE_INVALID_DESCRIPTOR
    );
    assert!(!runtime.mapped_pages.pins(7));
    assert_eq!(
        mapped_pages::handle(request(0, 20, 0, false), &mut runtime).0,
        OS_RESPONSE_OK
    );
    for bad in [
        request(1, 20, 0, true),
        request(0, 21, 0, true),
        request(0, 20, 4096, true),
    ] {
        assert_ne!(mapped_pages::handle(bad, &mut runtime).0, OS_RESPONSE_OK);
    }
    assert!(runtime.mapped_pages.pins(7));
}

#[test]
fn failures_do_not_publish_leases_or_discard_live_storage_and_release_is_retryable() {
    let mut runtime = runtime();
    runtime.fail_read = true;
    assert_ne!(
        mapped_pages::handle(request(0, 20, 0, false), &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert!(!runtime.mapped_pages.pins(7));
    runtime.fail_read = false;
    ALPHA.with(|alpha| alpha.borrow_mut().fail_share = true);
    assert_ne!(
        mapped_pages::handle(request(0, 20, 0, false), &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert!(!runtime.mapped_pages.pins(7));
    ALPHA.with(|alpha| {
        let mut alpha = alpha.borrow_mut();
        assert!(alpha.pages.is_empty());
        alpha.fail_share = false;
    });
    assert_eq!(
        mapped_pages::handle(request(0, 20, 0, false), &mut runtime).0,
        OS_RESPONSE_OK
    );
    ALPHA.with(|alpha| alpha.borrow_mut().fail_release = true);
    assert_ne!(
        mapped_pages::handle(request(0, 20, 0, true), &mut runtime).0,
        OS_RESPONSE_OK
    );
    assert!(runtime.mapped_pages.pins(7));
    ALPHA.with(|alpha| alpha.borrow_mut().fail_release = false);
    assert_eq!(
        mapped_pages::handle(request(0, 20, 0, true), &mut runtime).0,
        OS_RESPONSE_OK
    );
    ALPHA.with(|alpha| assert!(alpha.borrow().pages.is_empty()));
}
