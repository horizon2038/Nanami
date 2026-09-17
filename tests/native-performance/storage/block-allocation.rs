//! Exercise the production direct/single/double-indirect allocator, including
//! initialization ordering. Only free-block selection and block IPC are mocked.
use super::{
    flush_blocks, read_block, runtime, write_block, write_blocks, zero_block, Ext2Runtime,
    RequestError, BLOCK_SIZE, FAKE,
};

const EXT2_MAX_DIRECT_BLOCKS: usize = 12;
const EXT2_SINGLE_INDIRECT_INDEX: usize = 12;
const EXT2_DOUBLE_INDIRECT_INDEX: usize = 13;
#[derive(Default)]
struct Ext2Inode {
    block: [u32; 15],
}
fn indirect_entries_per_block(runtime: &Ext2Runtime) -> usize {
    runtime.block_size / 4
}
fn r32(address: usize) -> u32 {
    unsafe { (address as *const u32).read_unaligned() }
}
fn w32_mem(address: usize, value: u32) {
    unsafe { (address as *mut u32).write_unaligned(value) }
}
fn alloc_block(runtime: &mut Ext2Runtime) -> Result<usize, RequestError> {
    // Bitmap and free-count I/O clobber the shared scratch buffer.
    runtime.scratch.fill(0xbb);
    Ok(FAKE.with(|fake| {
        let mut fake = fake.borrow_mut();
        let block = fake.next_block;
        fake.next_block += 1;
        block
    }))
}
#[path = "../../../nanami/servers/core-services/ext2-server/src/block_allocation.rs"]
mod production;
use production::*;

fn mapped(inode: &Ext2Inode, logical: usize) -> u32 {
    let entry = |block: u32, index: usize| {
        if block == 0 {
            return 0;
        }
        FAKE.with(|fake| {
            let fake = fake.borrow();
            let start = block as usize * BLOCK_SIZE + index * 4;
            u32::from_le_bytes(fake.disk[start..start + 4].try_into().unwrap())
        })
    };
    let entries = BLOCK_SIZE / 4;
    if logical < 12 {
        inode.block[logical]
    } else if logical < 12 + entries {
        entry(inode.block[12], logical - 12)
    } else {
        let index = logical - 12 - entries;
        entry(entry(inode.block[13], index / entries), index % entries)
    }
}

#[test]
fn initializes_once_before_mapping_and_does_not_touch_existing_data() {
    for logical in [0, 11, 12, 267, 268, 523, 524, 65803] {
        let mut runtime = runtime(vec![]);
        let mut inode = Ext2Inode::default();
        let block = ensure_data_block_with(&mut runtime, &mut inode, logical, |runtime, block| {
            runtime.scratch[..BLOCK_SIZE].fill(0x42);
            write_blocks(runtime, block, 1)
        })
        .unwrap();
        flush_blocks(&mut runtime).unwrap();
        assert_eq!(mapped(&inode, logical), block);
        assert_eq!(
            ensure_data_block_with(&mut runtime, &mut inode, logical, |_, _| {
                panic!("existing blocks must not be initialized again")
            }),
            Ok(block)
        );
        FAKE.with(|fake| {
            let fake = fake.borrow();
            let data_write = fake
                .writes
                .iter()
                .position(|&(b, _)| b == block as usize)
                .unwrap();
            assert_eq!(
                fake.writes
                    .iter()
                    .filter(|&&(b, _)| b == block as usize)
                    .count(),
                1
            );
            assert!(
                fake.disk[block as usize * BLOCK_SIZE..(block as usize + 1) * BLOCK_SIZE]
                    .iter()
                    .all(|byte| *byte == 0x42)
            );
            if logical >= 12 {
                assert_eq!(data_write, 0, "file data must precede metadata");
            }
        });
    }
}

#[test]
fn failed_initialization_never_publishes_a_data_pointer() {
    for logical in [0, 12, 268] {
        let mut runtime = runtime(vec![]);
        let mut inode = Ext2Inode::default();
        let result = ensure_data_block_with(&mut runtime, &mut inode, logical, |_, _| {
            Err(RequestError::Transport)
        });
        assert_eq!(result, Err(RequestError::Transport));
        flush_blocks(&mut runtime).unwrap();
        assert_eq!(mapped(&inode, logical), 0);
    }
}

#[test]
fn directory_allocation_still_zero_initializes_blocks() {
    let mut runtime = runtime(vec![]);
    let mut inode = Ext2Inode::default();
    for logical in [0, 12, 268] {
        let block = ensure_data_block(&mut runtime, &mut inode, logical).unwrap();
        flush_blocks(&mut runtime).unwrap();
        FAKE.with(|fake| {
            assert!(fake.borrow().disk
                [block as usize * BLOCK_SIZE..(block as usize + 1) * BLOCK_SIZE]
                .iter()
                .all(|byte| *byte == 0))
        });
    }
}

#[test]
fn failed_pointer_table_initialization_is_not_linked_into_inode() {
    for logical in [12, 268] {
        let mut runtime = runtime(vec![]);
        let mut inode = Ext2Inode::default();
        FAKE.with(|fake| fake.borrow_mut().write_result = Some(Err(RequestError::Transport)));
        zero_block(&mut runtime, 100).unwrap();
        assert_eq!(flush_blocks(&mut runtime), Err(RequestError::Transport));
        assert_eq!(
            ensure_data_block(&mut runtime, &mut inode, logical),
            Err(RequestError::Transport)
        );
        assert_eq!(inode.block, [0; 15]);
    }
}
