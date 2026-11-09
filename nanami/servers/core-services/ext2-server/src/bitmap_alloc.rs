//! Skip occupied bitmap words; allocation still selects the first free bit.
use super::*;

pub(super) fn alloc_bitmap_bit(
    runtime: &mut Ext2Runtime,
    bitmap_block: usize,
    start_bit: usize,
    limit_bits: usize,
) -> Result<Option<usize>, RequestError> {
    // Keep the read even for an empty interval, including its failure order.
    read_block(runtime, bitmap_block)?;
    let bit = unsafe { claim(runtime.block_shm as usize, runtime.block_size, start_bit, limit_bits) };
    if bit.is_some() { write_block(runtime, bitmap_block)?; }
    Ok(bit)
}

unsafe fn claim(base: usize, block_size: usize, start: usize, limit: usize) -> Option<usize> {
    let mut bit = start;
    if bit >= limit { return None; }
    if bit % 8 != 0 {
        let byte = ptr::read((base + bit / 8) as *const u8);
        let free = !byte & (u8::MAX << (bit % 8));
        if free != 0 {
            let candidate = (bit & !7) + free.trailing_zeros() as usize;
            if candidate >= limit { return None; }
            ptr::write((base + bit / 8) as *mut u8, byte | (1 << (candidate % 8)));
            return Some(candidate);
        }
        bit = bit.checked_add(8 - bit % 8)?;
        if bit >= limit { return None; }
    }
    // Invalid filesystem geometry can request bits beyond the loaded block.
    // Keep the old byte-at-a-time access footprint there instead of reading
    // ahead across the known scratch block boundary.
    let word_limit = limit.min(block_size.saturating_mul(8));
    while word_limit.saturating_sub(bit) >= 64 {
        // Scratch belongs exclusively to this request. Read only whole words
        // within the requested bit range; the scratch address may be unaligned.
        let word = u64::from_le(ptr::read_unaligned((base + bit / 8) as *const u64));
        if word != u64::MAX {
            let offset = (!word).trailing_zeros() as usize;
            let candidate = bit + offset;
            let byte = (word >> ((offset / 8) * 8)) as u8;
            ptr::write((base + candidate / 8) as *mut u8, byte | (1 << (candidate % 8)));
            return Some(candidate);
        }
        bit += 64;
    }
    while bit < limit {
        let byte = ptr::read((base + bit / 8) as *const u8);
        if byte != u8::MAX {
            let offset = (!byte).trailing_zeros() as usize;
            if offset >= limit - bit { return None; }
            let candidate = bit + offset;
            ptr::write((base + bit / 8) as *mut u8, byte | (1 << offset));
            return Some(candidate);
        }
        if limit - bit < 8 { return None; }
        bit += 8;
    }
    None
}
