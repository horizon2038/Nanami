//! Actual inode publication, including sector preservation and failed writes.
const EXT2_INODE_BLOCK_POINTERS: usize = 15;
const EXT2_S_IFMT: u16 = 0xf000;
const EXT2_S_IFREG: u16 = 0x8000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ext2Inode { mode: u16, size: u32, sectors: u32, links_count: u16, block: [u32; 15] }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestError { InvalidArgument, Io }
struct Versions { invalidations: Vec<u32> }
impl Versions { fn invalidate(&mut self, inode: u32) { self.invalidations.push(inode); } }
struct Superblock { inodes_per_group: u32, inodes_count: u32, inode_size: usize }
struct Group { inode_table_block: u32 }
struct Ext2Runtime {
    file_versions: Versions, superblock: Superblock, group_count: usize, groups: Vec<Group>,
    block_size: usize, block_shm: usize, scratch: Box<[u8]>, saved: Vec<u8>,
    recounts: usize, reads: Vec<usize>, writes: Vec<usize>,
    fail_count: bool, fail_read: bool, fail_write: bool,
    directory_invalidations: usize,
    cache_invalidations: Vec<u32>, cached: Vec<(u32, Ext2Inode)>, handles: Vec<(u32, Ext2Inode)>,
}
fn data_sectors(r: &mut Ext2Runtime, _: Ext2Inode) -> Result<u32, RequestError> {
    r.recounts += 1;
    if r.fail_count { return Err(RequestError::Io); }
    Ok(78)
}
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    r.reads.push(block);
    if r.fail_read { return Err(RequestError::Io); }
    r.scratch.fill(0xaa); Ok(())
}
fn write_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    r.directory_invalidations += 1;
    write_inode_table_block(r, block)
}
fn write_inode_table_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    r.writes.push(block);
    if r.fail_write { return Err(RequestError::Io); }
    r.saved = r.scratch.to_vec(); Ok(())
}
fn w16_mem(address: usize, value: u16) {
    unsafe { std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), address as *mut u8, 2); }
}
fn w32_mem(address: usize, value: u32) {
    unsafe { std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), address as *mut u8, 4); }
}
fn invalidate_cached_inode(r: &mut Ext2Runtime, inode: u32) { r.cache_invalidations.push(inode); }
fn store_cached_inode(r: &mut Ext2Runtime, number: u32, inode: Ext2Inode) { r.cached.push((number, inode)); }
fn update_cached_handle_metadata(r: &mut Ext2Runtime, number: u32, inode: Ext2Inode) { r.handles.push((number, inode)); }
#[path = "../../nanami/servers/core-services/ext2-server/src/inode_write.rs"]
mod inode_write;
fn fixture() -> (Ext2Runtime, Ext2Inode) {
    let mut scratch = vec![0; 1024].into_boxed_slice();
    let r = Ext2Runtime { file_versions: Versions { invalidations: Vec::new() },
        superblock: Superblock { inodes_per_group: 16, inodes_count: 32, inode_size: 128 },
        group_count: 2, groups: vec![Group { inode_table_block: 100 }, Group { inode_table_block: 200 }],
        block_size: 1024, block_shm: scratch.as_mut_ptr() as usize, scratch, saved: Vec::new(),
        recounts: 0, reads: Vec::new(), writes: Vec::new(), fail_count: false, fail_read: false,
        fail_write: false, directory_invalidations: 0, cache_invalidations: Vec::new(), cached: Vec::new(), handles: Vec::new() };
    (r, Ext2Inode { mode: 0x81a4, size: 65539, sectors: 76, links_count: 2,
        block: std::array::from_fn(|i| 0xff00_0000 + i as u32) })
}
#[test]
fn existing_allocation_preserves_sectors_and_publishes_every_inode_field() {
    let (mut r, inode) = fixture();
    r.fail_count = true; // A count would fail; this path must never call it.
    inode_write::write_inode_existing_blocks(&mut r, 26, inode).unwrap();
    assert_eq!(r.recounts, 0);
    assert_eq!(r.reads, [201]); assert_eq!(r.writes, [201]);
    let start = 128;
    let mut expected = vec![0xaa; 1024];
    expected[start..start+2].copy_from_slice(&inode.mode.to_le_bytes());
    expected[start+4..start+8].copy_from_slice(&inode.size.to_le_bytes());
    expected[start+26..start+28].copy_from_slice(&inode.links_count.to_le_bytes());
    expected[start+28..start+32].copy_from_slice(&inode.sectors.to_le_bytes());
    for (i, block) in inode.block.iter().enumerate() {
        expected[start+40+i*4..start+44+i*4].copy_from_slice(&block.to_le_bytes());
    }
    assert_eq!(r.saved, expected);
    assert_eq!(r.file_versions.invalidations, [26]);
    assert_eq!(r.cache_invalidations, [26]);
    assert_eq!(r.cached, [(26, inode)]); assert_eq!(r.handles, r.cached);
    assert_eq!(r.directory_invalidations, 0);
}

#[test]
fn only_regular_inode_records_preserve_directory_positions_even_on_write_error() {
    for mode in [0, 0x4000, 0xa000, 0xc000, 0x8000, 0x81ff] {
        for fail in [false, true] {
            let (mut r, mut inode) = fixture(); inode.mode = mode; r.fail_write = fail;
            let result = inode_write::write_inode(&mut r, 1, inode);
            assert_eq!(result, if fail { Err(RequestError::Io) } else { Ok(()) });
            assert_eq!(r.directory_invalidations, usize::from(mode & EXT2_S_IFMT != EXT2_S_IFREG));
            assert_eq!(r.writes, [100]);
            assert_eq!(r.file_versions.invalidations, [1]);
            assert_eq!(r.cached.is_empty(), fail);
        }
    }
}
#[test]
fn ordinary_inode_updates_still_recount_and_commit_the_normalized_count() {
    let (mut r, mut inode) = fixture();
    inode_write::write_inode(&mut r, 1, inode).unwrap();
    assert_eq!(r.recounts, 1); inode.sectors = 78;
    assert_eq!(r.cached, [(1, inode)]); assert_eq!(r.handles, r.cached);
    assert_eq!(&r.saved[28..32], &78u32.to_le_bytes());
}
#[test]
fn count_read_or_write_failure_invalidates_versions_without_committing_caches() {
    for stage in 0..3 {
        let (mut r, inode) = fixture();
        r.fail_count = stage == 0; r.fail_read = stage == 1; r.fail_write = stage == 2;
        assert_eq!(inode_write::write_inode(&mut r, 1, inode), Err(RequestError::Io));
        assert_eq!(r.file_versions.invalidations, [1]);
        assert!(r.cached.is_empty() && r.handles.is_empty() && r.cache_invalidations.is_empty());
        assert!(r.saved.is_empty());
    }
    for fail_write in [false, true] {
        let (mut r, inode) = fixture(); r.fail_read = !fail_write; r.fail_write = fail_write;
        assert_eq!(inode_write::write_inode_existing_blocks(&mut r, 1, inode), Err(RequestError::Io));
        assert_eq!(r.recounts, 0); assert_eq!(r.file_versions.invalidations, [1]);
        assert!(r.cached.is_empty() && r.handles.is_empty());
    }
}
#[test]
fn existing_allocation_path_keeps_inode_and_group_bounds_checks() {
    for case in 0..4 {
        let (mut r, inode) = fixture();
        let number = match case { 0 => 0, 1 => 33, _ => 17 };
        if case == 2 { r.superblock.inodes_per_group = 0; }
        if case == 3 { r.group_count = 1; }
        assert_eq!(inode_write::write_inode_existing_blocks(&mut r, number, inode), Err(RequestError::InvalidArgument));
        assert_eq!(r.file_versions.invalidations, [number]);
        assert!(r.reads.is_empty() && r.writes.is_empty() && r.cached.is_empty());
    }
}
