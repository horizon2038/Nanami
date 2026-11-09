//! Production zeroing body; window lookup/failure is modeled with owned RAM.
const PAGE_SIZE: usize = 4096;
#[derive(Debug, PartialEq, Eq)]
enum CapabilityError { Map, InvalidArgument }
struct Alpha {
    backing: Vec<u8>, first: usize, calls: Vec<usize>, fail: Option<usize>,
    ready: Vec<bool>, runs: Vec<usize>,
}
impl Alpha {
    fn new(pages: usize) -> Self {
        Self { backing: vec![0xa5; (pages + 2) * PAGE_SIZE], first: 100,
            calls: Vec::new(), fail: None, ready: vec![false; pages], runs: Vec::new() }
    }
    fn map_initialization_run(&mut self, page: usize, maximum: usize) -> Result<(usize, usize), CapabilityError> {
        let address = self.map_initialization_page(page)?;
        let mut count = 1;
        while count < maximum && self.ready[page - self.first + count] {
            count += 1;
        }
        self.runs.push(count);
        Ok((address, count))
    }
    fn map_initialization_page(&mut self, page: usize) -> Result<usize, CapabilityError> {
        self.calls.push(page);
        if self.fail == Some(page) { return Err(CapabilityError::Map); }
        assert!(page >= self.first);
        let offset = (page - self.first + 1) * PAGE_SIZE;
        assert!(offset + PAGE_SIZE <= self.backing.len() - PAGE_SIZE);
        self.ready[page - self.first] = true;
        Ok(unsafe { self.backing.as_mut_ptr().add(offset) } as usize)
    }
}
#[path = "../../nanami/src/nanami_core/alpha/memory_zero.rs"]
mod production;

#[test]
fn reused_owned_pages_are_cleared_every_time_without_touching_neighbor_bytes() {
    for pages in [1, 3, 513, 858] {
        let mut alpha = Alpha::new(pages);
        for generation in 0..3 {
            alpha.backing.fill(0xa5 + generation);
            alpha.calls.clear();
            alpha.zero_physical_pages(100, pages).unwrap();
            if generation == 0 {
                assert_eq!(alpha.calls, (100..100 + pages).collect::<Vec<_>>());
            } else {
                assert_eq!(alpha.calls, vec![100]);
                assert_eq!(alpha.runs.last(), Some(&pages));
            }
            assert!(alpha.backing[..PAGE_SIZE].iter().all(|&b| b == 0xa5 + generation));
            assert!(alpha.backing[PAGE_SIZE..(pages + 1) * PAGE_SIZE].iter().all(|&b| b == 0));
            assert!(alpha.backing[(pages + 1) * PAGE_SIZE..].iter().all(|&b| b == 0xa5 + generation));
        }
    }
}

#[test]
fn failed_window_mapping_clears_only_the_confirmed_prefix_and_stops() {
    for offset in [0, 1, 512] {
        let mut alpha = Alpha::new(513); alpha.fail = Some(100 + offset);
        assert_eq!(alpha.zero_physical_pages(100, 513), Err(CapabilityError::Map));
        assert_eq!(alpha.calls, (100..=100 + offset).collect::<Vec<_>>());
        assert!(alpha.backing[PAGE_SIZE..(offset + 1) * PAGE_SIZE].iter().all(|&b| b == 0));
        assert!(alpha.backing[(offset + 1) * PAGE_SIZE..].iter().all(|&b| b == 0xa5));
    }
}

#[test]
fn physical_range_overflow_is_rejected_before_any_mapping() {
    let mut alpha = Alpha::new(1);
    assert_eq!(alpha.zero_physical_pages(usize::MAX, 2), Err(CapabilityError::InvalidArgument));
    assert!(alpha.calls.is_empty());
    assert!(alpha.backing.iter().all(|&b| b == 0xa5));
}

#[test]
fn ready_prefix_is_zeroed_before_the_next_cold_mapping_failure() {
    for prefix in [1, 17, 512] {
        let mut alpha = Alpha::new(513);
        alpha.ready[..prefix].fill(true);
        alpha.fail = Some(100 + prefix);
        assert_eq!(alpha.zero_physical_pages(100, 513), Err(CapabilityError::Map));
        assert_eq!(alpha.calls, vec![100, 100 + prefix]);
        assert_eq!(alpha.runs, vec![prefix]);
        assert!(alpha.backing[..PAGE_SIZE].iter().all(|&b| b == 0xa5));
        assert!(alpha.backing[PAGE_SIZE..(prefix + 1) * PAGE_SIZE].iter().all(|&b| b == 0));
        assert!(alpha.backing[(prefix + 1) * PAGE_SIZE..].iter().all(|&b| b == 0xa5));
    }
}
