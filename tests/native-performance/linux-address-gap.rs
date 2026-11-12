//! Real first-fit finder versus exhaustive candidate coverage and old scan work.
type Word = usize;
const LINUX_PAGE_SIZE: usize = 4096;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/address_gap.rs"]
mod production;
use production::first_fit;
use production::private_first_fit;
use std::cell::Cell;
const P: usize = LINUX_PAGE_SIZE;

#[test]
fn large_private_reservations_use_fresh_aligned_intervals_after_small_pages_and_retirement() {
    const LARGE: usize = 1 << 21;
    let base = 1usize << 32;
    for bytes in [LARGE, LARGE + P, 3 * LARGE / 2] {
        let mappings = [(base, P)];
        assert_eq!(first_fit(mappings.into_iter(), base, bytes, base + 8 * LARGE), Some(base + P));
        assert_eq!(private_first_fit(mappings.into_iter(), base, bytes, base + 8 * LARGE, base + P), Some(base + LARGE));
        // No live VMA remains, but an old small leaf table can still block it.
        assert_eq!(private_first_fit([].into_iter(), base, bytes, base + 8 * LARGE, base + LARGE + P), Some(base + 2 * LARGE));
        // A forged/stale history value is not a proof that a hole is free.
        let occupied = [(base + 2 * LARGE, LARGE)];
        assert_eq!(private_first_fit(occupied.into_iter(), base, bytes, base + 8 * LARGE, base + 2 * LARGE), Some(base + 3 * LARGE));
    }
}

#[test]
fn large_placement_falls_back_to_ordinary_holes_at_exhaustion_and_rounding_overflow() {
    const LARGE: usize = 1 << 21;
    assert_eq!(private_first_fit([].into_iter(), P, LARGE, P + LARGE, 0), Some(P));
    for cursor in [8 * LARGE, usize::MAX] {
        let mappings = [(P, P), (3 * LARGE, LARGE)];
        assert_eq!(private_first_fit(mappings.into_iter(), P, LARGE, 4 * LARGE, cursor), Some(2 * P));
    }
    assert_eq!(private_first_fit([(P, 4 * LARGE)].into_iter(), P, LARGE, 4 * LARGE, 0), None);
    for pages in [1, 2, 511] {
        assert_eq!(private_first_fit([(P, P)].into_iter(), P, pages * P, 8 * LARGE, 7 * LARGE), Some(2 * P));
    }
}

#[test]
fn preferred_holes_match_exhaustive_aligned_search_with_unordered_vmas() {
    const LARGE: usize = 1 << 21;
    let mut seed = 97u64;
    for count in 0..24 {
        for _ in 0..100 {
            let mut mappings = Vec::new();
            for _ in 0..count {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                mappings.push((P + ((seed >> 8) as usize % 40) * LARGE / 2,
                    ((seed >> 32) as usize % 5) * LARGE / 2));
            }
            let cursor = (seed as usize % 8) * LARGE + P;
            let bytes = LARGE + P;
            let limit = 24 * LARGE;
            let expected = (cursor.div_ceil(LARGE)..24).map(|page| page * LARGE)
                .find(|base| base + bytes <= limit && mappings.iter().all(|&(start, size)|
                    *base >= start + size || start >= *base + bytes))
                .or_else(|| first_fit(mappings.iter().copied(), P, bytes, limit));
            assert_eq!(private_first_fit(mappings.iter().copied(), P, bytes, limit, cursor), expected);
        }
    }
}

#[test]
fn unordered_overlaps_and_holes_match_exhaustive_first_fit() {
    let mut seed = 0x13579bdu64;
    for count in 0..32 {
        for _ in 0..400 {
            let mut mappings = Vec::new();
            for _ in 0..count {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let start = (seed as usize % 40) * P + ((seed >> 32) as usize % P);
                let size = ((seed >> 12) as usize % 5) * P;
                mappings.push((start, size));
            }
            for pages in [1, 2, 5] {
                let bytes = pages * P;
                let expected = (1..=48-pages).map(|page| page * P).find(|base| {
                    mappings.iter().all(|&(start, size)| start == 0
                        || *base >= start + size || start >= *base + bytes)
                });
                assert_eq!(first_fit(mappings.iter().copied(), P, bytes, 48 * P), expected,
                    "mappings={mappings:?} pages={pages}");
            }
        }
    }
}

#[test]
fn ordered_dense_reservations_consume_each_vma_once() {
    for count in [1, 16, 128, 512] {
        let mappings: Vec<_> = (1..=count).map(|page| (page * P, P)).collect();
        let visited = Cell::new(0);
        let input = mappings.iter().map(|&mapping| { visited.set(visited.get() + 1); mapping });
        assert_eq!(first_fit(input, P, P, (count + 2) * P), Some((count + 1) * P));
        assert_eq!(visited.get(), count);
        // Previous implementation scans all entries once per conflicting page,
        // and once again to confirm the final hole.
        let old_visits = count * (count + 1);
        println!("VMAs={count} old_visits={old_visits} new_visits={}", visited.get());
    }
}

#[test]
fn previously_seen_future_vma_is_rechecked_only_when_reached() {
    for mappings in [vec![(3*P,P),(P,2*P)], vec![(5*P,P),(3*P,2*P),(P,2*P)]] {
        let expected = mappings.iter().map(|(start,size)| start+size).max();
        assert_eq!(first_fit(mappings.iter().copied(), P, P, 8*P), expected);
    }
    let mappings = [(10*P,P),(P,P),(2*P,P)];
    assert_eq!(first_fit(mappings.into_iter(), P, P, 20*P), Some(3*P));
}

#[test]
fn exhaustion_and_overflow_never_return_an_overlapping_range() {
    assert_eq!(first_fit([(P,P)].into_iter(), P, P, 2*P), None);
    assert_eq!(first_fit([(0,usize::MAX)].into_iter(), P, P, 2*P), Some(P));
    assert_eq!(first_fit([(usize::MAX-1,4)].into_iter(), P, P, 4*P), None);
    assert_eq!(first_fit([].into_iter(), usize::MAX-1, 4, usize::MAX), None);
    assert_eq!(first_fit([(P,usize::MAX-P)].into_iter(), P, P, usize::MAX), None);
    assert_eq!(first_fit([(P,0)].into_iter(), P, P, 2*P), Some(P));
}
