#![allow(dead_code)]
use std::collections::BTreeMap;

type CapabilityDescriptor = usize;
const PAGE_SIZE: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    InvalidArgument,
    IllegalOperation,
}

struct Alpha {
    copy_window: memory_copy::CopyWindow,
    mappings: BTreeMap<usize, usize>,
    maps: usize,
    unmaps: usize,
    fail_map: bool,
    fail_unmap: bool,
}
impl Alpha {
    fn new() -> Self {
        Self {
            copy_window: memory_copy::CopyWindow::new(),
            mappings: BTreeMap::new(),
            maps: 0,
            unmaps: 0,
            fail_map: false,
            fail_unmap: false,
        }
    }
    fn map_alpha_temporary_frame(
        &mut self,
        frame: usize,
        va: usize,
    ) -> Result<(), CapabilityError> {
        assert!(self.mappings.insert(va, frame).is_none());
        self.maps += 1;
        // Fail after the PTE was installed, not only before MAP.
        if self.fail_map {
            Err(CapabilityError::IllegalOperation)
        } else {
            Ok(())
        }
    }
    fn unmap_alpha_frame(&mut self, frame: usize, va: usize) -> Result<(), CapabilityError> {
        if self.fail_unmap {
            return Err(CapabilityError::IllegalOperation);
        }
        assert_eq!(self.mappings.remove(&va), Some(frame));
        self.unmaps += 1;
        Ok(())
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_copy.rs"]
mod memory_copy;

#[test]
fn warm_clock_and_poll_transfers_do_not_map_or_unmap_in_either_direction() {
    let mut a = Alpha::new();
    let guest = a.map_process_copy_frame(1, 100, None).unwrap();
    let scratch = a.map_process_copy_frame(2, 200, Some(guest)).unwrap();
    for _ in 0..10_000 {
        assert_eq!(a.map_process_copy_frame(2, 200, None).unwrap(), scratch);
        assert_eq!(
            a.map_process_copy_frame(1, 100, Some(scratch)).unwrap(),
            guest
        );
        assert_eq!(a.map_process_copy_frame(1, 100, None).unwrap(), guest);
        assert_eq!(
            a.map_process_copy_frame(2, 200, Some(guest)).unwrap(),
            scratch
        );
    }
    assert_eq!((a.maps, a.unmaps), (2, 0));
}

#[test]
fn eviction_never_overwrites_the_pinned_source() {
    let mut a = Alpha::new();
    let source = a.map_process_copy_frame(1, 100, None).unwrap();
    for frame in 200..1200 {
        let destination = a.map_process_copy_frame(2, frame, Some(source)).unwrap();
        assert_ne!(destination, source);
        assert_eq!(a.mappings[&source], 100);
        assert!(a.mappings.len() <= 16);
    }
    a.invalidate_process_copy_mappings(2).unwrap();
    assert_eq!(a.mappings.len(), 1);
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
    assert_eq!(a.maps, a.unmaps);
}

#[test]
fn unmap_exec_reap_invalidation_prevents_same_descriptor_reuse_hits() {
    let mut a = Alpha::new();
    for _ in 0..20 {
        a.map_process_copy_frame(1, 100, None).unwrap();
        a.map_process_copy_frame(2, 200, None).unwrap();
        let before = a.maps;
        a.invalidate_process_copy_mappings(1).unwrap();
        a.map_process_copy_frame(1, 100, None).unwrap();
        a.map_process_copy_frame(2, 200, None).unwrap();
        assert_eq!(a.maps, before + 1);
    }
}

#[test]
fn failed_map_retains_cleanup_metadata_but_is_never_a_hit() {
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_process_copy_frame(1, 100, None).is_err());
    a.fail_map = false;
    a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!((a.maps, a.unmaps), (2, 1));
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
}

#[test]
fn failed_unmap_does_not_forget_or_reuse_a_possibly_live_mapping() {
    let mut a = Alpha::new();
    a.map_process_copy_frame(1, 100, None).unwrap();
    a.fail_unmap = true;
    assert!(a.invalidate_process_copy_mappings(1).is_err());
    assert_eq!(a.mappings.len(), 1);
    a.fail_unmap = false;
    a.invalidate_process_copy_mappings(1).unwrap();
    assert!(a.mappings.is_empty());
    a.map_process_copy_frame(1, 100, None).unwrap();
    assert_eq!(a.maps, 2);
}
