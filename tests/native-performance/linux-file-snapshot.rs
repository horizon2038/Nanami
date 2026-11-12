//! Production split publisher; storage admission and kernel effects modeled.
extern crate self as nanami_runtime;
extern crate alloc;
use std::cell::RefCell;
type Word = usize;
const LINUX_PAGE_SIZE: Word = 4096;
const EINVAL: i32 = 22;
const EIO: i32 = 5;
#[derive(Debug)]
pub struct RequestError;
fn map_request_error(_: RequestError) -> i32 { EIO }
mod arch { pub const ELF_MACHINE: u16 = 62; }
#[path = "../../nanami/servers/apps/alter/shared/src/common/elf.rs"]
mod elf;
#[path = "../../nanami/servers/apps/alter/shared/src/common/image_cache.rs"]
pub(crate) mod image_cache;
mod common { pub(crate) use super::image_cache; }
struct Runtime { image_cache: image_cache::ImageCache, shared: bool, data: Vec<u8>, reads: usize, read_limit: usize }
impl Runtime {
    fn posix_image_read_buffer_size(&self) -> usize { 8*1024*1024 }
    fn pread_posix(&mut self, _: usize, _: usize, bytes: usize, offset: usize) -> Result<(usize,usize),RequestError> {
        self.reads += 1;
        let count=bytes.min(self.read_limit).min(self.data.len().saturating_sub(offset));
        Ok((count,self.data.as_ptr() as usize+offset))
    }
}
#[derive(Clone,Copy)]
struct LinuxFile { posix_fd: usize }
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_mapping.rs"]
mod file_mapping;
mod private_memory {
    pub(super) fn with_mapping_mutation<T>(_: &super::Runtime, _: usize, _: usize, _: usize, _: bool,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> { operation() }
    pub fn mark_readonly_shared(runtime: &mut super::Runtime, _: usize) { runtime.shared = true; }
}
#[derive(Debug, PartialEq)]
struct Call { kind: u8, base: Word, bytes: Word, source: Word, initialized: Word, prot: Word }
thread_local! {
    static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    static FAIL: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
fn record(call: Call) -> Result<(), RequestError> {
    CALLS.with(|calls| {
        let mut calls = calls.borrow_mut();
        let index = calls.len();
        calls.push(call);
        if FAIL.with(|fail| fail.get()) == Some(index) { Err(RequestError) } else { Ok(()) }
    })
}
pub fn request_process_map_private_snapshot(_: Word, base: Word, bytes: Word, source: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 0, base, bytes, source, initialized: bytes, prot })
}
pub fn request_process_map_initialized_protected_at(_: Word, base: Word, bytes: Word, source: Word, initialized: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 1, base, bytes, source, initialized, prot })
}
pub fn request_process_map_initialized_at(_: Word, base: Word, bytes: Word, source: Word, initialized: Word) -> Result<(), RequestError> {
    record(Call { kind: 2, base, bytes, source, initialized, prot: 7 })
}
pub fn request_process_mapping_protect(_: Word, base: Word, bytes: Word, prot: Word) -> Result<(), RequestError> {
    record(Call { kind: 3, base, bytes, source: 0, initialized: 0, prot })
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/file_snapshot.rs"]
mod file_snapshot;
fn fixture() -> Runtime {
    CALLS.with(|calls| calls.borrow_mut().clear()); FAIL.with(|fail| fail.set(None));
    EAGER.with(|value| value.set(0));
    let mut cache=image_cache::ImageCache::new();
    cache.insert(0,1<<32|7,0x1000000,3514368,3514368,None);
    Runtime { image_cache: cache, shared: false, data: Vec::new(), reads: 0, read_limit: usize::MAX }
}
#[test]
fn actual_large_mapping_aliases_file_pages_and_initializes_only_four_zero_pages() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 3530752,
        0x1000000, 3514168, 5, true), Ok(true));
    assert!(runtime.shared);
    CALLS.with(|calls| assert_eq!(*calls.borrow(), vec![
        Call { kind: 0, base: 0x2000000, bytes: 3514368, source: 0x1000000, initialized: 3514368, prot: 5 },
        Call { kind: 1, base: 0x2000000 + 3514368, bytes: 16384, source: 0, initialized: 0, prot: 5 },
    ]));
}
#[test]
fn no_zero_suffix_needs_only_the_alias_request() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 4096, 0x1001000, 4096, 1, true), Ok(true));
    CALLS.with(|calls| assert_eq!(calls.borrow().len(), 1));
}
#[test]
fn unsupported_rights_unaligned_sources_and_outside_ranges_decline_before_publication() {
    for (source, bytes, mapped, prot) in [
        (0x1000000, 4096, 4096, 3), (0x1000001, 4096, 4096, 1),
        (0x1000000, 3514369, 3530752, 1), (0x1000000, 8192, 4096, 1),
        (0x1000000, 0, 4096, 1),
    ] {
        let mut runtime = fixture();
        assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, mapped, source, bytes, prot, true), Ok(false));
        assert!(!runtime.shared);
        CALLS.with(|calls| assert!(calls.borrow().is_empty()));
    }
}
#[test]
fn zero_tail_keeps_the_legacy_constructor_and_protect_route_when_requested() {
    let mut runtime = fixture();
    assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 8192, 0x1000000, 4096, 1, false), Ok(true));
    CALLS.with(|calls| assert_eq!(calls.borrow().iter().map(|call| call.kind).collect::<Vec<_>>(), [0, 2, 3]));
}
#[test]
fn failures_never_retry_or_copy_after_a_possible_alias_prefix() {
    for failure in 0..3 {
        let mut runtime = fixture(); FAIL.with(|fail| fail.set(Some(failure)));
        assert_eq!(file_snapshot::map_readonly(&mut runtime, 2, 0x2000000, 8192, 0x1000000, 4096, 1, false), Err(EIO));
        CALLS.with(|calls| assert_eq!(calls.borrow().len(), failure + 1));
    }
}

pub fn request_process_map_private_snapshot_prefix(_: Word, base: Word, bytes: Word, source: Word, prefix: Word) -> Result<(), RequestError> {
    record(Call { kind: 4, base, bytes, source, initialized: prefix, prot: 1 })
}
#[test]
fn dynamic_library_prefix_is_used_only_for_initial_readonly_mapping() {
    for prot in [1,5] {
        let mut r=fixture();let data=dso(3514368,3514368);
        r.image_cache.insert(0,1<<32|7,0x1000000,3514368,3514368,Some(elf::parse_elf64_image(&data).unwrap()));
        assert_eq!(file_snapshot::map_readonly(&mut r,2,0x2000000,3514368,0x1000000,3514368,prot,true),Ok(true));
        CALLS.with_borrow(|calls| {assert_eq!(calls.len(),1);assert_eq!(calls[0].kind,if prot==1 {4}else{0});
            assert_eq!(calls[0].initialized,if prot==1 {65536}else{3514368});});
    }
}

thread_local! { static PAYLOAD: RefCell<Vec<u8>> = const {RefCell::new(Vec::new())}; }
thread_local! { static EAGER: std::cell::Cell<usize> = const {std::cell::Cell::new(0)}; }
pub fn request_process_map_initialized_deferred_at(_: Word, base: Word, bytes: Word,
    source: Word, initialized: Word, eager: Word) -> Result<(),RequestError> {
    EAGER.with(|value| value.set(eager));
    record(Call {kind:7,base,bytes,source,initialized,prot:1})?;
    let mut payload=vec![0;bytes];
    payload[..initialized].copy_from_slice(unsafe {std::slice::from_raw_parts(source as *const u8,initialized)});
    PAYLOAD.with_borrow_mut(|v|*v=payload);Ok(())
}
fn dso(bytes: usize, loaded: usize) -> Vec<u8> {
    let mut data:Vec<u8>=(0..bytes).map(|i|(i%239)as u8).collect();data[..120].fill(0);
    data[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    for (offset,value,size) in [(16,3usize,2),(18,62,2),(20,1,4),(24,120,8),(32,64,8),
        (52,64,2),(54,56,2),(56,1,2),(64,1,4),(68,5,4),(96,loaded,8),(104,loaded,8),(112,4096,8)] {
        data[offset..offset+size].copy_from_slice(&value.to_le_bytes()[..size]);
    }
    data
}

#[test]
fn cached_and_uncached_dso_publish_metadata_prefix_in_the_existing_single_constructor_call() {
    let size = 2 * 1024 * 1024;
    let metadata_bytes = 1101656usize;
    let code = metadata_bytes.next_multiple_of(4096);
    let mut data = dso(size, size);
    data[56..58].copy_from_slice(&2u16.to_le_bytes());
    data[68..72].copy_from_slice(&4u32.to_le_bytes());
    data[96..104].copy_from_slice(&(metadata_bytes as u64).to_le_bytes());
    data[104..112].copy_from_slice(&(metadata_bytes as u64).to_le_bytes());
    data[120..176].fill(0);
    for (offset, value, bytes) in [(120,1usize,4),(124,5,4),(128,code,8),(136,code,8),
        (152,size-code,8),(160,size-code,8),(168,4096,8)] {
        data[offset..offset+bytes].copy_from_slice(&value.to_le_bytes()[..bytes]);
    }
    let metadata = elf::parse_elf64_image(&data).unwrap();
    let mut r = fixture();
    r.image_cache.insert(0, 1 << 32 | 7, 0x1000000, size, size, Some(metadata));
    assert_eq!(file_snapshot::map_readonly(&mut r, 2, 0x2000000, size, 0x1000000, size, 1, true), Ok(true));
    CALLS.with_borrow(|calls| {
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].kind, 4);
        assert_eq!(calls[0].initialized, code);
    });
    let mut r = fixture();
    r.data = data;
    assert_eq!(file_mapping::initialize(&mut r, 2, 0x20000000, size,
        LinuxFile { posix_fd: 7 }, 0, size, Some(1)), Ok(true));
    assert_eq!(EAGER.with(|value| value.get()), code);
    assert_eq!(r.reads, 1);
    CALLS.with_borrow(|calls| assert_eq!(calls.len(), 1));
    PAYLOAD.with_borrow(|bytes| assert_eq!(bytes.as_slice(), r.data.as_slice()));
}

#[test]
fn uncached_complete_dso_read_initializes_private_ram_in_one_request_without_growing_cache() {
    let mut r=fixture();r.data=dso(3514168,3514168);
    r.image_cache=image_cache::ImageCache::new();
    for i in 0..8 {r.image_cache.insert(i,1<<32|(i+1),0x1000000+i*0x200000,0x200000,0x200000,None);}
    assert_eq!(file_mapping::initialize(&mut r,2,0x20000000,3530752,LinuxFile{posix_fd:7},0,3514168,Some(1)),Ok(true));
    assert_eq!(r.reads,1);assert!(!r.shared);assert_eq!(r.image_cache.retained_bytes(),16*1024*1024);
    assert_eq!(EAGER.with(|value| value.get()), 65536);
    CALLS.with_borrow(|calls| {assert_eq!(calls.iter().map(|c|c.kind).collect::<Vec<_>>(),[7]);
        assert_eq!(calls[0].initialized,3514168);assert_eq!(calls[0].source,r.data.as_ptr()as usize);assert_eq!(calls[0].bytes,3530752);});
    PAYLOAD.with_borrow(|v|{assert_eq!(&v[..3514168],r.data.as_slice());assert!(v[3514168..].iter().all(|b|*b==0));});
}

#[test]
fn deferred_constructor_failure_never_retries_or_publishes_another_destination() {
    let mut r=fixture();r.data=dso(256*1024,256*1024);FAIL.with(|f|f.set(Some(0)));
    assert_eq!(file_mapping::initialize(&mut r,2,0x20000000,256*1024,LinuxFile{posix_fd:7},0,256*1024,Some(1)),Err(EIO));
    assert!(!r.shared);
    CALLS.with_borrow(|calls|assert_eq!(calls.iter().map(|c|c.kind).collect::<Vec<_>>(),vec![7]));
}

#[test]
fn short_read_offset_non_dso_and_writable_mappings_keep_the_streaming_constructor() {
    for mode in 0..5 {
        let mut r=fixture();r.data=dso(512*1024,256*1024);
        let mut offset=0;let mut rights=Some(1);
        match mode {0=>r.read_limit=65536,1=>offset=4096,2=>r.data[0]=0,3=>rights=Some(3),_=>rights=None}
        assert_eq!(file_mapping::initialize(&mut r,2,0x20000000,256*1024,LinuxFile{posix_fd:7},offset,256*1024,rights),Ok(false));
        CALLS.with_borrow(|calls|assert!(calls.iter().all(|c|matches!(c.kind,1|2))));
    }
}
