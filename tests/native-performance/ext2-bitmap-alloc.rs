use std::ptr;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestError { Read, Write }
struct Ext2Runtime {
    block_shm: usize, block_size: usize, disk: Vec<u8>, buffer: Vec<u8>,
    fail_read: bool, fail_write: bool, calls: Vec<&'static str>,
}
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    assert_eq!(block, 17); r.calls.push("read");
    if r.fail_read { return Err(RequestError::Read); }
    unsafe { ptr::copy_nonoverlapping(r.disk.as_ptr(), r.block_shm as *mut u8, r.disk.len()); }
    Ok(())
}
fn write_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    assert_eq!(block, 17); r.calls.push("write");
    if r.fail_write { return Err(RequestError::Write); }
    unsafe { ptr::copy_nonoverlapping(r.block_shm as *const u8, r.disk.as_mut_ptr(), r.disk.len()); }
    Ok(())
}
#[path = "../../nanami/servers/core-services/ext2-server/src/bitmap_alloc.rs"]
mod bitmap_alloc;
fn runtime(bytes: &[u8], offset: usize) -> Ext2Runtime {
    let mut buffer = vec![0xa5; bytes.len() + 24];
    let block_shm = unsafe { buffer.as_mut_ptr().add(offset) } as usize;
    Ext2Runtime { block_shm, block_size: bytes.len(), disk: bytes.to_vec(), buffer, fail_read: false, fail_write: false, calls: vec![] }
}
fn check(bytes: &[u8], offset: usize, start: usize, limit: usize) {
    let mut expected = bytes.to_vec();
    let bit = (start..limit).find(|&bit| expected[bit / 8] & (1 << (bit % 8)) == 0);
    if let Some(bit) = bit { expected[bit / 8] |= 1 << (bit % 8); }
    let mut r = runtime(bytes, offset);
    assert_eq!(bitmap_alloc::alloc_bitmap_bit(&mut r, 17, start, limit), Ok(bit));
    assert_eq!(r.disk, expected);
    assert_eq!(&r.buffer[offset..offset + bytes.len()], &expected);
    assert!(r.buffer[..offset].iter().chain(&r.buffer[offset + bytes.len()..]).all(|&b| b == 0xa5));
    assert_eq!(r.calls, if bit.is_some() { vec!["read", "write"] } else { vec!["read"] });
}
#[test]
fn every_byte_value_and_partial_bit_interval_matches_original_first_fit() {
    for byte in 0..=255u8 {
        for start in 0..=8 {
            for limit in start..=8 { check(&[byte], 3, start, limit); }
        }
    }
}
#[test]
fn occupied_words_unaligned_bases_and_partial_edges_never_change_other_bits() {
    for bytes in [8, 9, 15, 16, 17, 64, 127, 128, 129, 513, 1024, 4096] {
        let bits = bytes * 8;
        for offset in 0..8 {
            for free in [0, 1, 7, 8, 31, 32, 63, bits / 2, bits - 1] {
                let mut data = vec![0xff; bytes]; data[free / 8] &= !(1 << (free % 8));
                for start in [0, 1, 7, 8, 11, 63, bits - 1, bits] {
                    for limit in [free, free + 1, bits] {
                        if start <= limit { check(&data, offset, start, limit); }
                    }
                }
            }
            let mut seed = 0x7e39_a15du32;
            for _ in 0..32 {
                let data: Vec<_> = (0..bytes).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed >> 24) as u8 }).collect();
                let start = seed as usize % (bits + 1);
                check(&data, offset, start, bits);
            }
            check(&vec![0xff; bytes], offset, 0, bits);
        }
    }
}
#[test]
fn read_and_write_failures_keep_original_scratch_mutation_and_error_order() {
    for (start, limit) in [(0, 8), (8, 8), (9, 8), (usize::MAX, usize::MAX)] {
        let mut r = runtime(&[0xfe], 3); r.fail_read = true;
        assert_eq!(bitmap_alloc::alloc_bitmap_bit(&mut r, 17, start, limit), Err(RequestError::Read));
        assert_eq!(r.calls, ["read"]); assert!(r.buffer.iter().all(|&b| b == 0xa5));
    }
    let mut r = runtime(&[0xfe], 3); r.fail_write = true;
    assert_eq!(bitmap_alloc::alloc_bitmap_bit(&mut r, 17, 0, 8), Err(RequestError::Write));
    assert_eq!(r.calls, ["read", "write"]); assert_eq!(r.disk, [0xfe]); assert_eq!(r.buffer[3], 0xff);
    for (start, limit) in [(8, 8), (9, 8), (usize::MAX, usize::MAX)] { check(&[0xfe], 3, start, limit); }
}

#[test]
fn oversized_geometry_keeps_scalar_selection_beyond_the_loaded_block() {
    // Model accessible scratch beyond the loaded block. Malformed limits keep
    // their original byte-by-byte semantics, without word lookahead there.
    for size in [1, 7, 8, 9, 15] {
        for free in 0..24 * 8 {
            let mut data = vec![0xff; 24]; data[free / 8] &= !(1 << (free % 8));
            let mut r = runtime(&data, 3); r.block_size = size;
            assert_eq!(bitmap_alloc::alloc_bitmap_bit(&mut r, 17, 0, 24 * 8), Ok(Some(free)));
            assert!(r.disk.iter().all(|&b| b == 0xff));
        }
    }
}
