#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
use std::collections::VecDeque;
type Word = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestError {
    InvalidArgument,
    Protocol,
    Transport,
    Status(Word),
}
const TIMER_COUNTER_SOURCE_HPET64: Word = 1;

struct Fake {
    memory: usize,
    mapped_physical: Word,
    source: Result<(Word, Word), RequestError>,
    readings: VecDeque<Result<(Word, Word), RequestError>>,
    reads: usize,
    queries: usize,
    maps: usize,
    releases: Vec<Word>,
    map_error: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            memory: 0,
            mapped_physical: 0x1000,
            source: Ok((0x1000, 10_000_000)),
            readings: [
                (9000, 1_000_000_000),
                (11000, 1_000_000_000),
                (13000, 1_000_000_000),
                (14000, 1_000_000_000),
            ]
            .into_iter()
            .map(Ok)
            .collect(),
            reads: 0,
            queries: 0,
            maps: 0,
            releases: Vec::new(),
            map_error: false,
        }
    }
}
thread_local! { static FAKE: RefCell<Fake> = RefCell::new(Fake::default()); }
fn timer_service_monotonic_ticks(port: Word) -> Result<(Word, Word), RequestError> {
    assert_ne!(port, 0);
    FAKE.with_borrow_mut(|f| {
        f.reads += 1;
        f.readings.pop_front().expect("unexpected clock RPC")
    })
}
fn timer_service_counter_source(port: Word, kind: Word) -> Result<(Word, Word), RequestError> {
    assert_ne!(port, 0);
    assert_eq!(kind, TIMER_COUNTER_SOURCE_HPET64);
    FAKE.with_borrow_mut(|f| {
        f.queries += 1;
        f.source
    })
}
fn request_mmio(physical: Word, bytes: Word) -> Result<(Word, Word), RequestError> {
    assert_eq!(physical, 0x1000);
    assert_eq!(bytes, 4096);
    FAKE.with_borrow_mut(|f| {
        f.maps += 1;
        if f.map_error {
            return Err(RequestError::Transport);
        }
        Ok((f.mapped_physical, f.memory))
    })
}
fn request_mapping_release(base: Word, bytes: Word) -> Result<(), RequestError> {
    assert_eq!(bytes, 4096);
    FAKE.with_borrow_mut(|f| f.releases.push(base));
    Ok(())
}
#[path = "../../nanami/servers/libs/nanami-client/src/timer/counter.rs"]
mod counter;
use counter::MonotonicReader;

#[repr(align(4096))]
struct Page([u64; 512]);
fn fixture() -> Box<Page> {
    let mut page = Box::new(Page([0; 512]));
    page.0[0] = (10_000_000 << 32) | (1 << 13);
    page.0[0x10 / 8] = 1;
    page.0[0xf0 / 8] = 1000;
    FAKE.with_borrow_mut(|f| {
        *f = Fake::default();
        f.memory = page.0.as_mut_ptr() as usize;
    });
    page
}

#[test]
fn fresh_local_reads_advance_without_rpc_and_never_program_the_device() {
    let mut page = fixture();
    let mut reader = MonotonicReader::new();
    let snapshot = page.0;
    assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
    assert!(reader.uses_local_counter());
    assert_eq!(page.0, snapshot);
    page.0[0xf0 / 8] = 1200;
    assert_eq!(reader.sample(7), Ok((12000, 1_000_000_000)));
    page.0[0xf0 / 8] = 1250;
    assert_eq!(reader.sample(7), Ok((12500, 1_000_000_000)));
    FAKE.with_borrow(|f| {
        assert_eq!(f.reads, 2);
        assert_eq!(f.queries, 1);
        assert_eq!(f.maps, 1);
    });
    drop(reader);
    FAKE.with_borrow(|f| assert_eq!(f.releases, [page.0.as_ptr() as usize]));
}

#[test]
fn legacy_provider_declines_once_and_every_fallback_sample_is_fresh() {
    let _page = fixture();
    FAKE.with_borrow_mut(|f| f.source = Err(RequestError::Status(2)));
    let mut reader = MonotonicReader::new();
    assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
    assert_eq!(reader.sample(7), Ok((13000, 1_000_000_000)));
    assert!(!reader.uses_local_counter());
    FAKE.with_borrow(|f| {
        assert_eq!(f.queries, 1);
        assert_eq!(f.reads, 3);
        assert_eq!(f.maps, 0);
    });
}

#[test]
fn periodic_units_do_not_query_or_map_an_hpet_source() {
    let _page = fixture();
    FAKE.with_borrow_mut(|f| f.readings = [Ok((90, 100)), Ok((91, 100)), Ok((92, 100))].into());
    let mut reader = MonotonicReader::new();
    assert_eq!(reader.sample(7), Ok((91, 100)));
    assert_eq!(reader.sample(7), Ok((92, 100)));
    FAKE.with_borrow(|f| {
        assert_eq!(f.maps, 0);
        assert_eq!(f.queries, 0);
    });
}

#[test]
fn missing_clock_is_retryable_and_never_uses_capability_zero() {
    let _page = fixture();
    let mut reader = MonotonicReader::new();
    assert_eq!(reader.sample(0), Err(RequestError::InvalidArgument));
    FAKE.with_borrow_mut(|f| f.readings.push_front(Err(RequestError::Transport)));
    assert_eq!(reader.sample(7), Err(RequestError::Transport));
    assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
    FAKE.with_borrow(|f| assert_eq!(f.queries, 1));
}

#[test]
fn invalid_metadata_declines_before_any_mapping() {
    for source in [
        (0, 10),
        (0x1001, 10),
        (0x1ff8, 10),
        (0x1000, 0),
        (0x1000, 100_000_001),
    ] {
        let _page = fixture();
        FAKE.with_borrow_mut(|f| f.source = Ok(source));
        let mut reader = MonotonicReader::new();
        assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
        FAKE.with_borrow(|f| assert_eq!(f.maps, 0));
    }
}

#[test]
fn narrow_disabled_or_mismatched_period_releases_the_mapping_and_uses_rpc() {
    for condition in 0..3 {
        let mut page = fixture();
        match condition {
            0 => page.0[0] &= !(1 << 13),
            1 => page.0[0x10 / 8] = 0,
            _ => page.0[0] ^= 1 << 32,
        }
        let mut reader = MonotonicReader::new();
        assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
        FAKE.with_borrow(|f| assert_eq!(f.releases.len(), 1));
    }
}

#[test]
fn epoch_or_unit_mismatch_is_not_accepted_as_a_local_clock() {
    for after in [(9999, 1_000_000_000), (11000, 100)] {
        let _page = fixture();
        FAKE.with_borrow_mut(|f| {
            f.readings = [
                Ok((9000, 1_000_000_000)),
                Ok(after),
                Ok((13000, 1_000_000_000)),
            ]
            .into()
        });
        let mut reader = MonotonicReader::new();
        assert_eq!(reader.sample(7), Ok((13000, 1_000_000_000)));
        FAKE.with_borrow(|f| assert_eq!(f.releases.len(), 1));
    }
    let mut page = fixture();
    page.0[0xf0 / 8] = 1;
    let mut reader = MonotonicReader::new();
    assert_eq!(reader.sample(7), Ok((13000, 1_000_000_000)));
}

#[test]
fn invalid_mapping_reply_or_mapping_failure_falls_back_without_reading_it() {
    for condition in 0..4 {
        let _page = fixture();
        FAKE.with_borrow_mut(|f| match condition {
            0 => f.map_error = true,
            1 => f.memory = 0,
            2 => f.memory = 1,
            _ => f.mapped_physical = 0x2000,
        });
        let mut reader = MonotonicReader::new();
        assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
        FAKE.with_borrow(|f| assert_eq!(f.releases.len(), usize::from(condition == 3)));
    }
}

#[test]
fn reset_or_disabled_counter_stops_local_reads_instead_of_freezing_time() {
    for disabled in [false, true] {
        let mut page = fixture();
        let mut reader = MonotonicReader::new();
        assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
        if disabled {
            page.0[0x10 / 8] = 0;
        } else {
            page.0[0xf0 / 8] = 1;
        }
        assert_eq!(reader.sample(7), Ok((13000, 1_000_000_000)));
        assert_eq!(reader.sample(7), Ok((14000, 1_000_000_000)));
        FAKE.with_borrow(|f| {
            assert_eq!(f.queries, 1);
            assert_eq!(f.releases.len(), 1);
        });
    }
}

#[test]
fn provider_replacement_drops_the_old_alias_and_revalidates() {
    let _page = fixture();
    let mut reader = MonotonicReader::new();
    assert_eq!(reader.sample(7), Ok((11000, 1_000_000_000)));
    FAKE.with_borrow_mut(|f| f.source = Err(RequestError::Status(2)));
    assert_eq!(reader.sample(8), Ok((14000, 1_000_000_000)));
    FAKE.with_borrow(|f| {
        assert_eq!(f.queries, 2);
        assert_eq!(f.releases.len(), 1);
    });
}

#[test]
fn incoherent_hardware_reads_are_bounded_and_rollover_samples_are_coherent() {
    let mut calls = 0;
    assert_eq!(
        counter::coherent_counter(|_| {
            calls += 1;
            calls
        }),
        None
    );
    assert_eq!(calls, 24);
    let mut values = [1, u32::MAX, 2, 2, 3, 2].into_iter();
    assert_eq!(
        counter::coherent_counter(|_| values.next().unwrap()),
        Some((2 << 32) | 3)
    );
}

#[test]
fn zero_frequency_is_a_protocol_error_not_a_division_or_cached_time() {
    let _page = fixture();
    FAKE.with_borrow_mut(|f| f.readings = [Ok((9000, 0))].into());
    assert_eq!(
        MonotonicReader::new().sample(7),
        Err(RequestError::Protocol)
    );
    FAKE.with_borrow(|f| {
        assert_eq!(f.maps, 0);
        assert_eq!(f.queries, 0);
    });
}
