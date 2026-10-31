//! Production block bitmap allocation/freeing for both ext2 group origins.
#[derive(Debug, PartialEq, Eq)]
enum RequestError {
    InvalidArgument,
    Unsupported,
}
struct Superblock {
    first_data_block: u32,
    blocks_per_group: u32,
}
struct Group {
    block_count: u32,
    block_bitmap_block: u32,
}
struct Ext2Runtime {
    superblock: Superblock,
    groups: Vec<Group>,
    group_count: usize,
    block_count: usize,
    used: Vec<Vec<bool>>,
    free: Vec<isize>,
}
#[path = "../../nanami/servers/core-services/ext2-server/src/block_bitmap.rs"]
mod production;
fn alloc_bitmap_bit(
    r: &mut Ext2Runtime,
    bitmap: usize,
    start: usize,
    limit: usize,
) -> Result<Option<usize>, RequestError> {
    for bit in start..limit {
        if !r.used[bitmap][bit] {
            r.used[bitmap][bit] = true;
            return Ok(Some(bit));
        }
    }
    Ok(None)
}
fn clear_bitmap_bit(r: &mut Ext2Runtime, bitmap: usize, bit: usize) -> Result<(), RequestError> {
    assert!(r.used[bitmap][bit], "free must address the allocated bit");
    r.used[bitmap][bit] = false;
    Ok(())
}
fn adjust_free_count(
    r: &mut Ext2Runtime,
    group: usize,
    inode: bool,
    delta: i32,
) -> Result<(), RequestError> {
    assert!(!inode);
    r.free[group] += delta as isize;
    Ok(())
}
fn fixture(first: u32) -> Ext2Runtime {
    Ext2Runtime {
        superblock: Superblock {
            first_data_block: first,
            blocks_per_group: 8,
        },
        groups: vec![
            Group {
                block_count: 8,
                block_bitmap_block: 0,
            },
            Group {
                block_count: 3,
                block_bitmap_block: 1,
            },
        ],
        group_count: 2,
        block_count: first as usize + 11,
        // Metadata/existing files pin the first six blocks of group zero.
        used: vec![
            vec![true, true, true, true, true, true, false, false],
            vec![false; 3],
        ],
        free: vec![2, 3],
    }
}
#[test]
fn allocation_and_free_are_inverses_at_both_group_boundaries() {
    for first in [0, 1] {
        let mut r = fixture(first);
        let expected: Vec<_> = (first as usize + 6..first as usize + 11).collect();
        let allocated: Vec<_> = (0..5)
            .map(|_| production::alloc_block(&mut r).unwrap())
            .collect();
        assert_eq!(allocated, expected);
        assert_eq!(r.free, [0, 0]);
        assert_eq!(
            production::alloc_block(&mut r),
            Err(RequestError::Unsupported)
        );
        for block in allocated {
            production::free_block(&mut r, block).unwrap();
        }
        assert_eq!(r.free, [2, 3]);
        assert!(r.used[0][..6].iter().all(|used| *used));
        assert_eq!(production::alloc_block(&mut r).unwrap(), first as usize + 6);
        assert_eq!(
            production::free_block(&mut r, first as usize + 11),
            Err(RequestError::InvalidArgument)
        );
        if first == 1 {
            assert_eq!(
                production::free_block(&mut r, 0),
                Err(RequestError::InvalidArgument)
            );
        }
    }
}
