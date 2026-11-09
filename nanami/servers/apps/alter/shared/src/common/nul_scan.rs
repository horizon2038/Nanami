//! Bounded NUL search in owned, initialized bytes (never a live guest mapping).
pub fn find_nul(bytes: &[u8]) -> Option<usize> {
    const WIDTH: usize = core::mem::size_of::<usize>();
    const ONES: usize = usize::MAX / 255;
    const HIGHS: usize = ONES * 128;
    let mut offset = 0;
    while bytes.len() - offset >= WIDTH {
        let word = usize::from_le(unsafe {
            (bytes.as_ptr().add(offset) as *const usize).read_unaligned()
        });
        let candidates = word.wrapping_sub(ONES) & !word & HIGHS;
        if candidates != 0 {
            // Little-endian byte order puts the first zero in the least
            // significant flagged byte. Borrow propagation can flag later
            // bytes too, but cannot precede that first actual zero.
            return Some(offset + candidates.trailing_zeros() as usize / 8);
        }
        offset += WIDTH;
    }
    bytes[offset..].iter().position(|b| *b == 0).map(|index| offset + index)
}
