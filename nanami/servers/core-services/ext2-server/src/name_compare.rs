//! Exact comparison of validated directory names and owned request names.
/// Both pointers must be readable for len bytes. Zero length needs no pointer.
pub(super) unsafe fn bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    let mut i = 0usize;
    while len - i >= 8 {
        // Names have no alignment guarantee. Read only complete words inside
        // the validated extent, including when its end is a page boundary.
        if core::ptr::read_unaligned(a.add(i).cast::<u64>())
            != core::ptr::read_unaligned(b.add(i).cast::<u64>())
        {
            return false;
        }
        i += 8;
    }
    while i < len {
        if *a.add(i) != *b.add(i) { return false; }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaligned_names_match_slice_equality_without_using_different_guard_bytes() {
        let mut cases = 0;
        for len in 0..=255 {
            for left in 0..8 {
                for right in 0..8 {
                    let begin_a = 16 + left;
                    let begin_b = 16 + right;
                    let mut a = vec![0xa5; begin_a + len + 16];
                    let mut b = vec![0x5a; begin_b + len + 16];
                    for i in 0..len {
                        let byte = (i * 71 + len * 13) as u8;
                        a[begin_a + i] = byte; b[begin_b + i] = byte;
                    }
                    let original_a = a.clone(); let original_b = b.clone();
                    assert!(unsafe { bytes_eq(a.as_ptr().add(begin_a), b.as_ptr().add(begin_b), len) });
                    cases += 1;
                    if len != 0 {
                        for mismatch in [0, len / 2, len - 1] {
                            b[begin_b + mismatch] ^= 0x80;
                            let expected = a[begin_a..begin_a + len] == b[begin_b..begin_b + len];
                            assert_eq!(unsafe { bytes_eq(a.as_ptr().add(begin_a), b.as_ptr().add(begin_b), len) }, expected);
                            b[begin_b + mismatch] ^= 0x80;
                            cases += 1;
                        }
                    }
                    assert_eq!(a, original_a); assert_eq!(b, original_b);
                }
            }
        }
        assert!(unsafe { bytes_eq(core::ptr::null(), core::ptr::null(), 0) });
        println!("exact unaligned name comparison cases: {cases}");
    }
}
