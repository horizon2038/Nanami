//! Fault injection for the production Alpha MMIO handler's stage diagnostics.
#![allow(dead_code)]
use std::cell::RefCell;
type Word = usize;
const PAGE_SIZE: usize = 4096;
const PROCESS_FRAME_TOTAL_PAGES: usize = 4096;
const PROCESS_FRAME_CHUNK_PAGES: usize = 512;
#[derive(Clone, Copy)]
struct OsRequestEvent {
    identifier: Word,
    arg0: Word,
    arg1: Word,
}
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    PermissionDenied,
    InvalidArgument,
    Fatal,
}
#[derive(Default)]
struct Fake {
    fail: Option<&'static str>,
    skip: usize,
    calls: Vec<&'static str>,
    logs: Vec<String>,
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
macro_rules! info { ($($args:tt)*) => {{ let _ = format_args!($($args)*); }} }
macro_rules! error { ($($args:tt)*) => { FAKE.with(|fake| fake.borrow_mut().logs.push(format!($($args)*))) } }
fn step(stage: &'static str) -> Result<(), CapabilityError> {
    FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        let seen = fake.calls.iter().filter(|&&name| name == stage).count();
        fake.calls.push(stage);
        if fake.fail == Some(stage) && seen == fake.skip {
            Err(CapabilityError::Fatal)
        } else {
            Ok(())
        }
    })
}
struct Processes {
    vm: (),
}
impl Processes {
    fn reserve_process_heap(
        &mut self,
        _: Word,
        _: usize,
        _: usize,
        _: usize,
    ) -> Result<(Word, Word, Word, usize), CapabilityError> {
        step("reserve-virtual")?;
        Ok((1, 2, 0x10000000, 0))
    }
    fn vm_space_mut(&mut self, _: Word) -> Option<&mut ()> {
        Some(&mut self.vm)
    }
}
struct Memory;
impl Memory {
    fn allocate_physical_at(
        &mut self,
        address: usize,
        _: usize,
        allow_device: bool,
    ) -> Result<usize, CapabilityError> {
        assert!(allow_device);
        step("allocate-physical")?;
        Ok(address / PAGE_SIZE)
    }
    fn ensure_alpha_frames_for_range_from_initial_generic(
        &mut self,
        address: usize,
        bytes: usize,
        device: bool,
    ) -> Result<(usize, usize, usize), CapabilityError> {
        assert!(device);
        step("convert-frames")?;
        Ok((address / PAGE_SIZE, 0, bytes / PAGE_SIZE))
    }
    fn physical_frame_descriptor_from_index(&self, index: usize) -> Option<Word> {
        Some(index + 1)
    }
    fn map_frame(&mut self, _: Word, _: Word, _: Word, _: &mut ()) -> Result<(), CapabilityError> {
        step("map-frames")
    }
}
struct Alpha {
    memory: Memory,
    processes: Processes,
}
impl Alpha {
    fn ensure_process_frame_chunks(
        &mut self,
        _: Word,
        _: Word,
        _: usize,
        _: usize,
    ) -> Result<(), CapabilityError> {
        step("frame-slots")
    }
}
mod arch {
    pub mod node {
        pub fn copy(_: usize, _: usize, _: usize) -> Result<(), super::super::CapabilityError> {
            super::super::step("copy-frames")
        }
    }
}
fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
fn process_frame_descriptor(_: usize, slot: usize) -> Word {
    slot + 1
}
fn process_frame_chunk_descriptor(_: usize, chunk: usize) -> Word {
    chunk + 1
}
#[path = "../../nanami/src/nanami_core/alpha/mmio.rs"]
mod mmio;

fn run(
    fail: Option<&'static str>,
    skip: usize,
    request: OsRequestEvent,
) -> Result<(usize, usize), CapabilityError> {
    FAKE.with(|fake| {
        *fake.borrow_mut() = Fake {
            fail,
            skip,
            ..Fake::default()
        }
    });
    let mut alpha = Alpha {
        memory: Memory,
        processes: Processes { vm: () },
    };
    alpha.handle_mmio_request(request)
}
fn request() -> OsRequestEvent {
    OsRequestEvent {
        identifier: 1,
        arg0: 0x8c7c8000,
        arg1: 2 * PAGE_SIZE,
    }
}

#[test]
fn all_mmio_failure_stages_report_original_request_and_preserve_error() {
    for stage in [
        "reserve-virtual",
        "allocate-physical",
        "convert-frames",
        "frame-slots",
        "copy-frames",
        "map-frames",
    ] {
        assert_eq!(run(Some(stage), 0, request()), Err(CapabilityError::Fatal));
        FAKE.with(|fake| {
            let fake = fake.borrow();
            assert_eq!(fake.logs.len(), 1);
            assert!(fake.logs[0].contains(&format!("stage={stage} page=0")));
            assert!(fake.logs[0].contains("pid=1 paddr=0x8c7c8000 bytes=0x2000"));
        });
    }
}

#[test]
fn later_page_failures_identify_page_index() {
    for stage in ["copy-frames", "map-frames"] {
        assert_eq!(run(Some(stage), 1, request()), Err(CapabilityError::Fatal));
        FAKE.with(|fake| assert!(fake.borrow().logs[0].contains(&format!("stage={stage} page=1"))));
    }
}

#[test]
fn validation_failure_never_touches_allocators() {
    let mut request = request();
    request.arg0 += 1;
    assert_eq!(run(None, 0, request), Err(CapabilityError::InvalidArgument));
    FAKE.with(|fake| {
        assert!(fake.borrow().calls.is_empty());
        assert!(fake.borrow().logs[0].contains("stage=validate"));
    });
}

#[test]
fn successful_mapping_keeps_response_and_emits_no_error() {
    assert_eq!(run(None, 0, request()), Ok((0x8c7c8000, 0x10000000)));
    FAKE.with(|fake| {
        let fake = fake.borrow();
        assert!(fake.logs.is_empty());
        assert_eq!(
            fake.calls,
            [
                "reserve-virtual",
                "allocate-physical",
                "convert-frames",
                "frame-slots",
                "copy-frames",
                "copy-frames",
                "map-frames",
                "map-frames"
            ]
        );
    });
}
