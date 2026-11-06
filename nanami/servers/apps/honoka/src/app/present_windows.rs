//! An opaque window handle also carries a coalescing-safe present hint.
//!
//! Live windows have disjoint low bits even when z-order changes. Higher bits
//! distinguish successive allocations, but are never decoded as event data:
//! A9N ORs notification identifiers, so a received value is not an object ID.
use nanami_runtime::Word;

pub const HINT_BITS: usize = 16;
pub const HINT_MASK: Word = (1 << HINT_BITS) - 1;
// Full-handle clients must not collide with PRESENT (bit 48) or INPUT (bit 49).
const MAX_GENERATION: Word = (1 << (48 - HINT_BITS)) - 1;

pub fn allocate(generation: Word, live_hints: Word) -> Option<(Word, Word)> {
    let available = HINT_MASK & !live_hints;
    if available == 0 || generation > MAX_GENERATION {
        return None;
    }
    let hint = 1 << available.trailing_zeros();
    Some(((generation << HINT_BITS) | hint, generation + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_hints_are_disjoint_and_survive_legacy_low_word_badges() {
        let mut live = 0;
        let mut generation = 0;
        for _ in 0..HINT_BITS {
            let (id, next) = allocate(generation, live).unwrap();
            let hint = id & HINT_MASK;
            assert_eq!(hint.count_ones(), 1);
            assert_eq!(live & hint, 0);
            assert_eq!((id & 0xffff_ffff) & HINT_MASK, hint);
            assert_eq!(id & !((1usize << 48) - 1), 0);
            live |= hint;
            generation = next;
        }
        assert!(allocate(generation, live).is_none());
    }

    #[test]
    fn reused_hint_gets_a_new_handle_and_generation_cannot_wrap_into_flags() {
        let (first, next) = allocate(0, 0).unwrap();
        let (reused, _) = allocate(next, 0).unwrap();
        assert_eq!(first & HINT_MASK, reused & HINT_MASK);
        assert_ne!(first, reused);
        let (last, exhausted) = allocate(MAX_GENERATION, 0).unwrap();
        assert!(last < 1usize << 48);
        assert!(allocate(exhausted, 0).is_none());
    }
}
