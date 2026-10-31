#![allow(dead_code)]
use std::collections::BTreeMap;
type CapabilityDescriptor = usize;
const PAGE_SIZE: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError {
    InvalidArgument,
    IllegalOperation,
}
struct Memory;
impl Memory {
    fn physical_frame_descriptor_from_index(&self, page: usize) -> Option<usize> {
        (page != 0).then_some(page + 100_000)
    }
}
struct Alpha {
    memory: Memory,
    initialization_window: memory_init::InitializationWindow,
    mappings: BTreeMap<usize, usize>,
    maps: usize,
    unmaps: usize,
    fail_map: bool,
    fail_unmap: bool,
}
impl Alpha {
    fn new() -> Self {
        Self {
            memory: Memory,
            initialization_window: memory_init::InitializationWindow::new(),
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
        if self.fail_map {
            return Err(CapabilityError::IllegalOperation);
        }
        Ok(())
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
#[path = "../../nanami/src/nanami_core/alpha/memory_init.rs"]
mod memory_init;

#[test]
fn repeated_process_initialization_uses_pool_capabilities_without_mapping_churn() {
    let mut a = Alpha::new();
    for _ in 0..50 {
        for page in 1..=512 {
            let va = a.map_initialization_page(page).unwrap();
            assert_eq!(a.mappings[&va], page + 100_000);
        }
    }
    assert_eq!((a.maps, a.unmaps), (512, 0));
}
#[test]
fn collisions_evict_before_mapping_and_have_a_fixed_space_bound() {
    let mut a = Alpha::new();
    for page in 1..5000 {
        a.map_initialization_page(page).unwrap();
    }
    assert_eq!(a.mappings.len(), 1024);
    assert_eq!(a.unmaps, a.maps - 1024);
}
#[test]
fn failed_map_after_installation_is_never_a_hit() {
    let mut a = Alpha::new();
    a.fail_map = true;
    assert!(a.map_initialization_page(1).is_err());
    a.fail_map = false;
    a.map_initialization_page(1).unwrap();
    assert_eq!((a.maps, a.unmaps), (2, 1));
}
#[test]
fn failed_unmap_keeps_the_original_mapping_tracked_and_prevents_replacement() {
    let mut a = Alpha::new();
    let va = a.map_initialization_page(1).unwrap();
    a.fail_unmap = true;
    assert!(a.map_initialization_page(1025).is_err());
    assert_eq!(a.mappings[&va], 100_001);
    assert_eq!(a.maps, 1);
    a.fail_unmap = false;
    assert_eq!(a.map_initialization_page(1025).unwrap(), va);
    assert_eq!(a.mappings[&va], 101_025);
}
#[test]
fn unowned_physical_page_is_rejected_without_mapping() {
    let mut a = Alpha::new();
    assert!(a.map_initialization_page(0).is_err());
    assert_eq!(a.maps, 0);
}
