//! Real ext2 dirent parsing/writing and continuation logic; block I/O mocked.
#![allow(dead_code)]
extern crate alloc;
extern crate self as nanami_client;
extern crate self as nanami_runtime;
use std::{cmp::min, ptr};
use std::collections::BTreeMap;
type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError { InvalidArgument, Protocol }
pub mod block {
    use super::*;
    pub fn block_device_flush(_:Word)->Result<(),RequestError> {panic!("readdir must not flush");}
    pub fn block_device_write(_:Word,_:Word,_:Word,_:Word)->Result<Word,RequestError> {panic!("readdir must not write");}
}
#[path = "../../nanami/servers/core-services/ext2-server/src/block_cache.rs"]
mod block_cache;
const MAX_HANDLES: usize = 2;
const EXT2_NAME_LEN_MAX: usize = 255;
const EXT2_ROOT_INODE: u32 = 2;
const BLOCK_SIZE: usize = 128;
const EXT2_S_IFMT:u16=0xf000;
const EXT2_S_IFDIR:u16=0x4000;
const EXT2_S_IFLNK:u16=0xa000;
const EXT2_S_IFSOCK:u16=0xc000;
const EXT2_DENTRY_CACHE_ENTRIES:usize=128;
#[path = "../../nanami/servers/core-services/ext2-server/src/dentry_cache.rs"]
mod dentry_cache;
#[path = "../../nanami/servers/core-services/ext2-server/src/directory_lookup.rs"]
mod directory_lookup;
#[path = "../../nanami/servers/core-services/ext2-server/src/name_compare.rs"]
mod name_compare;
use name_compare::bytes_eq;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_walk.rs"]
mod path_walk;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_input.rs"]
mod path_input;
#[path = "../../nanami/servers/core-services/ext2-server/src/canonical_path.rs"]
mod canonical_path;
#[path = "../../nanami/servers/core-services/ext2-server/src/resolved_path.rs"]
mod resolved_path;
use directory_lookup::lookup_in_directory;
#[path = "../../nanami/servers/core-services/ext2-server/src/directory_parent.rs"]
mod directory_parent;
use directory_parent::read_parent_name;
use path_walk::{Error, Filesystem, Kind, Node};
#[path = "../../nanami/servers/core-services/ext2-server/src/namespace_disk.rs"]
mod namespace_disk;
#[path = "baseline-namespace-disk.rs"]
mod baseline_namespace_disk;
pub const OS_RESPONSE_INVALID_DESCRIPTOR: Word = 3;
pub mod vfs {
    pub use nanami_uapi::vfs::*;
}
#[derive(Clone, Copy)]
struct ClientSession {
    shm_local: Word,
    shm_size: Word,
}
#[derive(Clone, Copy)]
struct Ext2Inode {
    size: u32,
    mode: u16,
}
#[derive(Clone, Copy)]
struct Ext2DirectoryEntry {
    inode: u32,
    record_len: usize,
    name_len: usize,
    file_type: u8,
    name_ptr: *const u8,
}
struct Handle {
    inode: u32,
}
struct Ext2Runtime {
    metadata: BTreeMap<u32, Ext2Inode>,
    links: BTreeMap<u32, Vec<u8>>,
    fail_inode_number: Option<u32>,
    parent_inode: Ext2Inode,
    inode_reads:usize,
    dentry_cache:dentry_cache::Cache,
    dentry_stores:usize,
    fail_inode:bool,
    fail_map:Option<usize>,
    mapped: Vec<usize>,
    handles: [Handle; MAX_HANDLES],
    directory_cursors: directory::Cursors,
    root_path: canonical_path::RootCache,
    resolved_paths: resolved_path::Cache,
    block_size: usize,
    block_count: usize,
    block_shm_size: usize,
    block_cache: block_cache::BlockCache,
    block_shm: Word,
    scratch: Box<[u8; BLOCK_SIZE]>,
    disk: Vec<[u8; BLOCK_SIZE]>,
    reads: usize,
    fail_read: Option<usize>,
}
#[path = "baseline-directory.rs"]
mod baseline;
#[path = "../../nanami/servers/core-services/ext2-server/src/directory.rs"]
mod directory;
use directory::{parse_directory_entry, write_vfs_dirent};

#[test]
fn serializer_preserves_all_bytes_and_guards_with_only_name_tail_zeroing() {
    directory::take_name_zero_bytes();
    let capacity = vfs::VFS_DIRECTORY_ENTRY_NAME_BYTES;
    let record_bytes = vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES;
    let mut expected_zero = 0;
    for len in (0..=257).chain(core::iter::once(usize::MAX)) {
        let copied = len.min(capacity);
        let name: Vec<u8> = (0..capacity).map(|i| (i * 37 + 11) as u8).collect();
        for alignment in 0..8 {
            let start = 8 + alignment;
            let mut output = vec![0xa5; start + record_bytes + 8];
            let mut expected = output.clone();
            let record = &mut expected[start..start + record_bytes];
            record.fill(0);
            let inode = 0xfedc_ba98u32;
            for (offset, value) in [
                (vfs::VFS_DIRECTORY_ENTRY_INODE_OFFSET, inode as Word),
                (vfs::VFS_DIRECTORY_ENTRY_TYPE_OFFSET, vfs::VFS_FILE_TYPE_REGULAR),
                (vfs::VFS_DIRECTORY_ENTRY_NAME_LEN_OFFSET, len),
                (vfs::VFS_DIRECTORY_ENTRY_RECORD_LEN_OFFSET, record_bytes),
            ] {
                record[offset..offset + core::mem::size_of::<Word>()].copy_from_slice(&value.to_ne_bytes());
            }
            let offset = vfs::VFS_DIRECTORY_ENTRY_NAME_OFFSET;
            record[offset..offset + copied].copy_from_slice(&name[..copied]);
            unsafe {
                write_vfs_dirent(output.as_mut_ptr().add(start), Ext2DirectoryEntry {
                    inode, record_len: 0, name_len: len, file_type: 1, name_ptr: name.as_ptr(),
                });
            }
            assert_eq!(output, expected, "len={len} alignment={alignment}");
            expected_zero += capacity - copied;
        }
    }
    let zeroed = directory::take_name_zero_bytes();
    println!("EXT2-DIRENT-TAIL zeroed-name-bytes={zeroed} expected-tail-bytes={expected_zero}");
    assert_eq!(zeroed, expected_zero);
}
fn max_file_blocks(_: &Ext2Runtime) -> usize {
    65536
}
fn get_data_block(r: &mut Ext2Runtime, _: impl core::borrow::Borrow<Ext2Inode>, logical: usize) -> Result<u32, ()> {
    r.mapped.push(logical);
    if r.fail_map == Some(logical) {return Err(());}
    Ok(if logical + 1 < r.disk.len() {
        (logical + 1) as u32
    } else {
        0
    })
}
fn read_inode(r:&mut Ext2Runtime,number:u32)->Result<Ext2Inode,RequestError> {
    r.inode_reads+=1;
    if r.fail_inode || r.fail_inode_number == Some(number) {Err(RequestError::Protocol)}
    else {Ok(*r.metadata.get(&number).unwrap_or(&r.parent_inode))}
}
fn read_link(r: &mut Ext2Runtime, number: u32) -> Result<Vec<u8>, Error> {
    r.links.get(&number).cloned().ok_or(Error::Missing)
}
unsafe fn load_cached_dentry(r:&mut Ext2Runtime,parent:u32,name:*const u8,len:usize)->Option<Option<u32>> {
    if len==0 || len>EXT2_NAME_LEN_MAX {return None;}
    r.dentry_cache.load(parent,core::slice::from_raw_parts(name,len))
}
unsafe fn store_cached_dentry(r:&mut Ext2Runtime,parent:u32,name:*const u8,len:usize,inode:Option<u32>) {
    if len==0 || len>EXT2_NAME_LEN_MAX {return;}
    r.dentry_stores+=1;
    r.dentry_cache.store(parent,core::slice::from_raw_parts(name,len),inode);
}
fn store_cached_dentry_query(r: &mut Ext2Runtime, query: Option<&dentry_cache::Query<'_>>, inode: Option<u32>) {
    if let Some(query) = query {
        r.dentry_stores += 1;
        r.dentry_cache.store_query(query, inode);
    }
}

#[test]
fn dentry_query_hash_is_shared_by_name_probe_and_result_store() {
    let (mut r, _) = fixture(96);
    let name = vec![b'z'; 255];
    let mut hashes = Vec::new();
    r.dentry_cache.take_hash_bytes();
    for (name, expected) in [(name.as_slice(), None), (name.as_slice(), None), (b"f0000000", Some(1))] {
        assert_eq!(unsafe { directory_lookup::lookup_in_directory(&mut r, 7, name.as_ptr(), name.len()) }, expected);
        hashes.push(r.dentry_cache.take_hash_bytes());
    }
    let (mut failed, _) = fixture(96); failed.fail_inode = true;
    assert_eq!(unsafe { directory_lookup::lookup_in_directory(&mut failed, 7, name.as_ptr(), name.len()) }, None);
    hashes.push(failed.dentry_cache.take_hash_bytes());
    for name in [b"".as_slice(), &[b'z'; 256]] {
        assert_eq!(unsafe { directory_lookup::lookup_in_directory(&mut r, 7, name.as_ptr(), name.len()) }, None);
        hashes.push(r.dentry_cache.take_hash_bytes());
    }
    println!("dentry hashed bytes miss/hit/positive/inode-error/empty/overlong: {hashes:?}");
    assert_eq!(hashes, [255, 255, 8, 255, 0, 0]);
}
fn validate_run(r:&Ext2Runtime, block:usize, count:usize)->Result<(),RequestError> {
    if count == 0 || block.checked_add(count).is_none_or(|end|end>r.block_count)
        || count.checked_mul(r.block_size).is_none_or(|bytes|bytes>r.block_shm_size) {
        return Err(RequestError::InvalidArgument);
    }
    Ok(())
}
fn read_block(r: &mut Ext2Runtime, block: usize) -> Result<(), RequestError> {
    validate_run(r, block, 1)?;
    r.reads += 1;
    if r.fail_read == Some(block) {
        return Err(RequestError::Protocol);
    }
    *r.scratch = r.disk[block];
    Ok(())
}
fn r16(p: usize) -> u16 {
    unsafe { ptr::read_unaligned(p as *const u16) }
}
fn r32(p: usize) -> u32 {
    unsafe { ptr::read_unaligned(p as *const u32) }
}
unsafe fn write_word(p: *mut u8, value: Word) {
    ptr::write_unaligned(p as *mut Word, value);
}
fn ext2_dirent_file_type_to_vfs(kind: u8) -> Word {
    match kind {
        1 => vfs::VFS_FILE_TYPE_REGULAR,
        2 => vfs::VFS_FILE_TYPE_DIRECTORY,
        _ => vfs::VFS_FILE_TYPE_UNKNOWN,
    }
}
fn fixture(entries: usize) -> (Ext2Runtime, Ext2Inode) {
    let mut disk = vec![[0; BLOCK_SIZE]];
    for block in 0..entries.div_ceil(8) {
        let mut data = [0; BLOCK_SIZE];
        for index in 0..8 {
            let number = block * 8 + index;
            let base = index * 16;
            let live = number < entries && number % 7 != 3;
            data[base..base + 4]
                .copy_from_slice(&if live { number as u32 + 1 } else { 0 }.to_le_bytes());
            data[base + 4..base + 6].copy_from_slice(&16u16.to_le_bytes());
            data[base + 6] = 8;
            data[base + 7] = if number % 5 == 0 { 2 } else { 1 };
            data[base + 8..base + 16].copy_from_slice(format!("f{number:07}").as_bytes());
        }
        disk.push(data);
    }
    let inode = Ext2Inode {
        size: ((disk.len() - 1) * BLOCK_SIZE) as u32,
        mode:EXT2_S_IFDIR,
    };
    let mut scratch = Box::new([0; BLOCK_SIZE]);
    (
        Ext2Runtime {
            metadata: BTreeMap::new(), links: BTreeMap::new(), fail_inode_number: None,
            parent_inode:inode,inode_reads:0,dentry_cache:dentry_cache::Cache::new(),
            dentry_stores:0,fail_inode:false,fail_map:None,mapped:Vec::new(),
            handles: [Handle { inode: 7 }, Handle { inode: 7 }],
            directory_cursors: directory::Cursors::new(),
            root_path: canonical_path::RootCache::new(),
            resolved_paths: resolved_path::Cache::new(),
            block_size: BLOCK_SIZE,
            block_count: disk.len(), block_shm_size: BLOCK_SIZE,
            block_cache:block_cache::BlockCache::new(BLOCK_SIZE,32*BLOCK_SIZE,BLOCK_SIZE),
            block_shm: scratch.as_mut_ptr() as Word,
            scratch,
            disk,
            reads: 0,
            fail_read: None,
        },
        inode,
    )
}

fn namespace_chain(warm: bool) -> (Ext2Runtime, Vec<u8>, u32) {
    let (mut r, _) = fixture(8);
    let mut path = Vec::new();
    let mut parent = 2;
    for (index, number) in [0, 1, 2, 4, 5, 6].into_iter().enumerate() {
        let name = if warm { format!("d{index}") } else { format!("f{number:07}") };
        let child = if warm { index as u32 + 10 } else { number + 1 };
        if warm { r.dentry_cache.store(parent, name.as_bytes(), Some(child)); }
        path.push(b'/');
        path.extend_from_slice(name.as_bytes());
        parent = child;
    }
    (r, path, parent)
}

#[test]
fn canonical_prefix_avoids_ancestor_reads_for_96_distinct_missing_descendants() {
    let mut counts = [0; 2];
    for (optimized, count) in [false, true].into_iter().zip(&mut counts) {
        let (mut r, prefix, parent) = namespace_chain(true);
        for index in 0..96 { r.dentry_cache.store(parent, format!("absent{index:03}").as_bytes(), None); }
        namespace_disk::resolve(&mut r, 2, 2, &prefix, true, false, false, true).unwrap();
        r.inode_reads = 0;
        for index in 0..96 {
            let path = [prefix.as_slice(), format!("/absent{index:03}/lib.so").as_bytes()].concat();
            // Neither an exact positive path nor a direct child of the proven
            // directory: the previous final-component shortcut could not help.
            assert!(r.resolved_paths.load(2, 2, &path, true).is_none());
            assert!(matches!(namespace_disk::resolve(&mut r, 2, 2, &path,
                true, false, false, optimized), Err(Error::Missing)));
            assert!(matches!(namespace_disk::resolve(&mut r, 2, 2, &path,
                true, true, false, optimized), Err(Error::Missing)));
        }
        *count = r.inode_reads;
    }
    println!("CANONICAL-PREFIX missing-descendants=96 modes=2 ancestor-inode-reads={counts:?}");
    assert_eq!(counts, [1152, 0]);
}

fn prefix_subtree() -> Ext2Runtime {
    let mut r = namespace_tree(true);
    for (parent, name, inode, mode) in [
        (11,b"sub".as_slice(),50,EXT2_S_IFDIR), (50,b"deep",51,EXT2_S_IFDIR),
        (51,b"value",52,0x8000), (50,b"link",53,EXT2_S_IFLNK),
        (50,b"cycle",54,EXT2_S_IFLNK), (50,b"empty",55,EXT2_S_IFLNK),
    ] {
        r.dentry_cache.store(parent,name,Some(inode));
        r.metadata.insert(inode,Ext2Inode { mode,size:73 });
    }
    r.links.insert(53,b"/a/b".to_vec());
    r.links.insert(54,b"cycle".to_vec());
    r.links.insert(55,Vec::new());
    r
}

#[test]
fn failed_walks_keep_the_proven_parent_without_caching_any_missing_name() {
    let mut counts = [0; 2];
    for (optimized, count) in [false, true].into_iter().zip(&mut counts) {
        let (mut r, prefix, parent) = namespace_chain(true);
        for index in 0..96 {
            r.dentry_cache.store(parent, format!("absent{index:03}").as_bytes(), None);
        }
        r.inode_reads = 0;
        for index in 0..96 {
            let path = [prefix.as_slice(), format!("/absent{index:03}/lib.so").as_bytes()].concat();
            for allow_missing in [false, true] {
                assert!(matches!(namespace_disk::resolve(&mut r, 2, 2, &path,
                    true, allow_missing, false, optimized), Err(Error::Missing)));
                assert!(r.resolved_paths.load(2, 2, &path, true).is_none());
            }
        }
        if optimized {
            assert_eq!(r.resolved_paths.parent_with_origin(2, 2,
                &[prefix.as_slice(), b"/next"].concat(), true), Some((parent, b"next".as_slice(), true)));
        }
        *count = r.inode_reads;
    }
    assert_eq!(counts, [1152, 6]);
    println!("FAILED-PATH-PREFIX ancestor-inode-reads={counts:?}");
}

#[test]
fn failed_prefixes_keep_fresh_leaf_metadata_and_are_invalidated_before_creation_and_replacement() {
    let mut r = prefix_subtree();
    assert!(matches!(namespace_disk::resolve(&mut r, 2, 2,
        b"/a/b/sub/deep/missing", true, false, false, true), Err(Error::Missing)));
    r.inode_reads = 0;
    r.metadata.get_mut(&52).unwrap().size = 8197;
    let (_, inode) = namespace_disk::resolve(&mut r, 2, 2,
        b"/a/b/sub/deep/value", true, false, false, true).unwrap();
    assert_eq!(inode.unwrap().size, 8197);
    assert_eq!(r.inode_reads, 1);
    r.dentry_cache.store(51, b"error", Some(56));
    r.fail_inode_number = Some(56);
    assert!(matches!(namespace_disk::resolve(&mut r, 2, 2,
        b"/a/b/sub/deep/error", true, false, false, true), Err(Error::Io)));
    r.fail_inode_number = None;
    directory::invalidate(&mut r);
    assert!(r.resolved_paths.parent(2, 2, b"/a/b/sub/deep/missing", true).is_none());
    r.dentry_cache.store(51, b"missing", Some(56));
    r.metadata.insert(56, Ext2Inode { mode: 0x8000, size: 73 });
    assert_eq!(namespace_disk::resolve(&mut r, 2, 2,
        b"/a/b/sub/deep/missing", true, false, false, true).unwrap().0.node.unwrap().inode, 56);
    directory::invalidate(&mut r);
    r.dentry_cache.store(50, b"deep", Some(57));
    r.metadata.insert(57, Ext2Inode { mode: 0x8000, size: 73 });
    assert!(matches!(namespace_disk::resolve(&mut r, 2, 2,
        b"/a/b/sub/deep/missing", true, false, false, true), Err(Error::NotDirectory)));
}

#[test]
fn failed_prefix_reuse_matches_full_walks_across_links_dotdot_roots_and_missing_modes() {
    let paths: &[&[u8]] = &[b"/a/b/sub/deep/missing", b"/a/b/sub/deep/value",
        b"/a/b/sub/link/missing", b"/a/b/sub/link/../value", b"/a/b/sub/cycle/missing",
        b"/a/b/sub/empty/missing", b"/a/b/sub//deep/missing", b"/a/b/sub/deep/../missing",
        b"/a/b/sub/deep/value/child", b"/a/b/sub/deep/value/", b"/a/b/sub/deep/missing\0"];
    let pack = |result: Result<(path_walk::Resolved, Option<Ext2Inode>), Error>|
        result.map(|(r, _)| (r.node, r.parent, r.name, r.path));
    for follow in [false, true] {
        for missing in [false, true] {
            let mut actual = prefix_subtree();
            let mut expected = prefix_subtree();
            for _ in 0..3 {
                for path in paths {
                    assert_eq!(pack(namespace_disk::resolve(&mut actual, 2, 2, path, follow, missing, false, true)),
                        pack(namespace_disk::resolve(&mut expected, 2, 2, path, follow, missing, false, false)), "{path:?}");
                }
            }
            assert!(actual.resolved_paths.parent(3, 3, b"/a/b/sub/deep/other", follow).is_none());
        }
    }
    for (retain, cacheable) in [(true, true), (false, false)] {
        let mut r = prefix_subtree();
        assert!(namespace_disk::resolve(&mut r, 2, 2, b"/a/b/sub/deep/missing",
            true, false, retain, cacheable).is_err());
        assert!(r.resolved_paths.parent(2, 2, b"/a/b/sub/deep/other", true).is_none());
    }
}

#[test]
fn canonical_prefix_descendants_match_full_walk_and_keep_link_and_error_precedence() {
    let paths: &[&[u8]] = &[b"/a/b/sub/deep/value",b"/a/b/sub/deep/missing",b"/a/b/sub/missing/value",
        b"/a/b/sub/deep/value/child",b"/a/b/sub/deep/value/",b"/a/b/sub/link/value",
        b"/a/b/sub/link/../value",b"/a/b/sub/link",b"/a/b/sub/cycle/value",b"/a/b/sub/empty/value",
        b"/a/b/sub/./deep/value",b"/a/b/sub//deep/value",b"/a/b/sub/deep/../deep/value",
        b"/a/b/sub/deep/value\0",b"/a/b/sub/deep/.."];
    for follow in [false,true] {
        for missing in [false,true] {
            for retain in [false,true] {
                for path in paths {
                    let mut actual=prefix_subtree();let mut expected=prefix_subtree();
                    namespace_disk::resolve(&mut actual,2,2,b"/a/b",follow,false,false,true).unwrap();
                    let pack=|result: Result<(path_walk::Resolved,Option<Ext2Inode>),Error>|
                        result.map(|(r,_)|(r.node,r.parent,r.name,r.path));
                    assert_eq!(pack(namespace_disk::resolve(&mut actual,2,2,path,follow,missing,retain,true)),
                        pack(namespace_disk::resolve(&mut expected,2,2,path,follow,missing,retain,false)),
                        "path={path:?} follow={follow} missing={missing} retain={retain}");
                }
            }
        }
    }
    // Overlong later components must not supersede an earlier missing name.
    let path=[b"/a/b/sub/missing/".as_slice(),&[b'z';256]].concat();
    let mut r=prefix_subtree();
    namespace_disk::resolve(&mut r,2,2,b"/a/b",true,false,false,true).unwrap();
    assert!(matches!(namespace_disk::resolve(&mut r,2,2,&path,true,false,false,true),Err(Error::Missing)));
}

#[test]
fn canonical_prefix_chooses_longest_exact_directory_and_keeps_key_and_shape_limits() {
    let mut cache=resolved_path::Cache::new();
    for (path,inode,parent) in [(b"/a".as_slice(),10,2),(b"/a/b",11,10),(b"/a/b/sub",50,11)] {
        cache.store(2,2,path,true,&path_walk::Resolved { node:Some(Node { inode,kind:Kind::Directory }),
            parent,name:Vec::new(),path:path.to_vec() });
    }
    assert_eq!(cache.parent(2,2,b"/a/b/sub/deep/value",true),Some((50,b"deep/value".as_slice())));
    assert_eq!(cache.parent(2,2,b"/a/b/submarine/deep/value",true),Some((11,b"submarine/deep/value".as_slice())));
    for (root,start,follow) in [(3,2,true),(2,3,true),(2,2,false)] {
        assert!(cache.parent(root,start,b"/a/b/sub/deep/value",follow).is_none());
    }
    for path in [b"/a/b/sub//value".as_slice(),b"/a/b/sub/./value",b"/a/b/sub/../value",b"/a/b/sub/value/",
        b"//a/b/sub/value",b"a/b/sub/value",b"/a/b/sub/val\0ue"] {
        assert!(cache.parent(2,2,path,true).is_none(),"{path:?}");
    }
    cache.invalidate();assert!(cache.parent(2,2,b"/a/b/sub/deep/value",true).is_none());
}

#[test]
fn canonical_prefix_reads_new_component_metadata_and_invalidates_before_namespace_changes() {
    let mut r=prefix_subtree();
    namespace_disk::resolve(&mut r,2,2,b"/a/b",true,false,false,true).unwrap();
    r.inode_reads=0;r.metadata.get_mut(&52).unwrap().size=8197;
    let (result,inode)=namespace_disk::resolve(&mut r,2,2,b"/a/b/sub/deep/value",true,false,false,true).unwrap();
    assert_eq!(result.node.unwrap().inode,52);assert_eq!(inode.unwrap().size,8197);
    assert_eq!(r.inode_reads,3);
    // A previously unknown suffix component's I/O failure is still observed.
    r.dentry_cache.store(51,b"error",Some(56));r.fail_inode_number=Some(56);
    assert!(matches!(namespace_disk::resolve(&mut r,2,2,b"/a/b/sub/deep/error",true,false,false,true),Err(Error::Io)));
    r.fail_inode_number=None;
    directory::invalidate(&mut r);
    r.dentry_cache.store(10,b"b",Some(60));r.metadata.insert(60,Ext2Inode { mode:EXT2_S_IFDIR,size:73 });
    r.dentry_cache.store(60,b"sub",Some(50));
    let result=namespace_disk::resolve(&mut r,2,2,b"/a/b/sub/deep/value",true,false,false,true).unwrap().0;
    assert_eq!(result.node.unwrap().inode,52);
    // Repeat after another namespace change to a nondirectory: no old prefix.
    directory::invalidate(&mut r);
    r.metadata.get_mut(&60).unwrap().mode=0x8000;
    assert!(matches!(namespace_disk::resolve(&mut r,2,2,b"/a/b/sub/deep/value",true,false,false,true),Err(Error::NotDirectory)));
}

#[test]
fn canonical_parent_reuse_skips_shared_components_for_distinct_sibling_files() {
    let mut counts = [0; 2];
    for (optimized, count) in [false, true].into_iter().zip(&mut counts) {
        let (mut runtime, mut prefix, _) = namespace_chain(true);
        prefix.truncate(prefix.iter().rposition(|&byte| byte == b'/').unwrap() + 1);
        for index in 0..96 {
            let name = format!("leaf{index:03}");
            runtime.dentry_cache.store(14, name.as_bytes(), Some(100 + index));
            runtime.metadata.insert(100 + index, Ext2Inode { size: index, mode: 0x8000 });
        }
        for index in 0..96 {
            let mut path = prefix.clone(); path.extend_from_slice(format!("leaf{index:03}").as_bytes());
            runtime.metadata.get_mut(&(100 + index)).unwrap().size = 4096 + index;
            let (resolved, inode) = namespace_disk::resolve(&mut runtime, 2, 2,
                &path, true, false, false, optimized).unwrap();
            assert_eq!(resolved.node, Some(Node { inode: 100 + index, kind: Kind::File }));
            assert_eq!(resolved.parent, 14);
            assert_eq!(resolved.path, path);
            assert!(resolved.name.is_empty());
            assert_eq!(inode.unwrap().size, 4096 + index);
        }
        *count = runtime.inode_reads;
    }
    println!("CANONICAL-PARENT distinct-files=96 components=6 inode_reads_before={} inode_reads_after={}", counts[0], counts[1]);
    assert_eq!(counts, [576, 101]);
}

#[test]
fn parent_reuse_matches_full_walk_across_siblings_links_dotdot_and_missing_modes() {
    let paths: &[&[u8]] = &[b"/a/b/value", b"/a/b/a", b"/a/b/b", b"/a/b/missing",
        b"/a/b/absolute", b"/a/b/link/value", b"/a/b/../value", b"/a/b/./value",
        b"/a/b/value/child", b"/a/b/value/", b"/a/b/loop", b"/a/b/empty",
        b"//value", b"/", b"/a\0/value", b"/a/b/.."];
    for follow in [false, true] {
        for missing in [false, true] {
            let mut actual = namespace_tree(true); let mut expected = namespace_tree(true);
            for round in 0..8 {
                for path in paths {
                    let retain = round % 3 == 2;
                    let pack = |result: Result<(path_walk::Resolved, Option<Ext2Inode>), Error>|
                        result.map(|(r, _)| (r.node, r.parent, r.name, r.path));
                    assert_eq!(pack(namespace_disk::resolve(&mut actual, 2, 2, path, follow, missing, retain, true)),
                        pack(namespace_disk::resolve(&mut expected, 2, 2, path, follow, missing, retain, false)),
                        "path={path:?} follow={follow} missing={missing} retain={retain}");
                }
            }
        }
    }
}

#[test]
fn canonical_parent_keys_reject_alias_paths_and_root_slash_ambiguity() {
    let mut cache = resolved_path::Cache::new();
    let mut result = path_walk::Resolved { node: Some(Node { inode: 11, kind: Kind::Directory }),
        parent: 10, name: Vec::new(), path: b"/a/b".to_vec() };
    cache.store(2, 2, b"/a/b", true, &result);
    assert_eq!(cache.parent(2, 2, b"/a/b/value", true), Some((11, b"value".as_slice())));
    assert_eq!(cache.parent(2, 2, b"/a/other", true), Some((10, b"other".as_slice())));
    for path in [b"/a/b/".as_slice(), b"/a/b/.", b"/a/b/..", b"/a/b//value", b"/a/./other", b"a/b/value", b"/a/b/a\0"] {
        assert!(cache.parent(2, 2, path, true).is_none(), "{path:?}");
    }
    assert!(cache.parent(3, 2, b"/a/b/value", true).is_none());
    assert!(cache.parent(2, 3, b"/a/b/value", true).is_none());
    assert!(cache.parent(2, 2, b"/a/b/value", false).is_none());
    cache.invalidate();
    cache.store(2, 2, b"/link", true, &result);
    assert!(cache.parent(2, 2, b"/other", true).is_none());
    assert!(cache.parent(2, 2, b"/link/value", true).is_none());
    result.path = b"/".to_vec(); result.node = Some(Node { inode: 2, kind: Kind::Directory }); result.parent = 2;
    cache.store(2, 2, b"/", true, &result);
    assert!(cache.parent(2, 2, b"//value", true).is_none());
    assert_eq!(cache.parent(2, 2, b"/value", true), Some((2, b"value".as_slice())));
}

#[test]
fn parent_reuse_reads_current_leaf_and_discards_old_parent_after_namespace_mutation() {
    let mut r = namespace_tree(true);
    namespace_disk::resolve(&mut r, 2, 2, b"/a/b/a", true, false, false, true).unwrap();
    r.metadata.get_mut(&12).unwrap().size = 8197;
    let (_, inode) = namespace_disk::resolve(&mut r, 2, 2, b"/a/b/value", true, false, false, true).unwrap();
    assert_eq!(inode.unwrap().size, 8197);
    directory::invalidate(&mut r);
    r.dentry_cache.store(2, b"a", Some(20));
    r.dentry_cache.store(20, b"b", Some(21));
    r.dentry_cache.store(21, b"value", Some(22));
    r.metadata.insert(22, Ext2Inode { size: 37, mode: 0x8000 });
    let (resolved, _) = namespace_disk::resolve(&mut r, 2, 2, b"/a/b/value", true, false, false, true).unwrap();
    assert_eq!(resolved.node.unwrap().inode, 22);
    // Another leaf in the newly proven parent still performs its inode I/O.
    r.dentry_cache.store(21, b"error", Some(23));
    r.fail_inode_number = Some(23);
    assert!(matches!(namespace_disk::resolve(&mut r, 2, 2, b"/a/b/error", true, false, false, true), Err(Error::Io)));
}

#[test]
fn resolved_path_reuse_skips_component_walks_but_reads_current_leaf_metadata() {
    let mut counts = [0; 2];
    for (optimized, count) in [false, true].into_iter().zip(&mut counts) {
        let (mut runtime, path, leaf) = namespace_chain(true);
        runtime.metadata.insert(leaf, Ext2Inode { size: 0, mode: 0x8000 });
        for size in 0..128 {
            runtime.metadata.get_mut(&leaf).unwrap().size = size;
            let (resolved, known) = namespace_disk::resolve(
                &mut runtime, 2, 2, &path, true, false, false, optimized).unwrap();
            assert_eq!(resolved.path, path);
            assert_eq!(resolved.node, Some(Node { inode: leaf, kind: Kind::File }));
            assert!(resolved.name.is_empty());
            let inode = known.unwrap_or_else(|| read_inode(&mut runtime, leaf).unwrap());
            assert_eq!(inode.size, size);
        }
        *count = runtime.inode_reads;
        if optimized {
            runtime.fail_inode_number = Some(leaf);
            let (_, known) = namespace_disk::resolve(
                &mut runtime, 2, 2, &path, true, false, false, true).unwrap();
            assert!(known.is_none());
            assert!(read_inode(&mut runtime, leaf).is_err());
        }
    }
    println!("RESOLVED-PATH calls=128 components=6 inode_reads_before={} inode_reads_after={}", counts[0], counts[1]);
    assert_eq!(counts, [768, 133]);
}

#[test]
fn resolved_path_cache_keys_own_bytes_and_bound_fifo_storage_without_truncation() {
    let mut cache = resolved_path::Cache::new();
    let mut input = b"/a".to_vec();
    let mut result = path_walk::Resolved { node: Some(Node { inode: 19, kind: Kind::File }),
        parent: 17, name: Vec::new(), path: b"/canonical/a".to_vec() };
    cache.store(2, 7, &input, true, &result);
    input[1] = b'b'; result.path.fill(b'x');
    let found = cache.load(2, 7, b"/a", true).unwrap();
    assert_eq!(found.path, b"/canonical/a"); assert_eq!(found.parent, 17);
    for (root, start, path, follow) in [(3,7,b"/a".as_slice(),true),(2,8,b"/a",true),
        (2,7,b"/b",true),(2,7,b"/a/",true),(2,7,b"/a",false)] {
        assert!(cache.load(root,start,path,follow).is_none());
    }
    cache.invalidate();
    for i in 0..9 {
        let input = format!("/f{i}"); result.path = input.as_bytes().to_vec();
        cache.store(2,2,input.as_bytes(),true,&result);
    }
    assert!(cache.load(2,2,b"/f0",true).is_none());
    for i in 1..9 { assert!(cache.load(2,2,format!("/f{i}").as_bytes(),true).is_some()); }
    for (input_bytes, output_bytes, present, leaf_name) in [(129,2,true,false),(2,257,true,false),
        (2,2,false,false),(2,2,true,true),(0,2,true,false)] {
        let input = vec![b'z';input_bytes];
        let result = path_walk::Resolved {
            node: present.then_some(Node { inode: 33, kind: Kind::Directory }), parent: 2,
            name: if leaf_name { b"z".to_vec() } else { Vec::new() }, path: vec![b'z';output_bytes],
        };
        cache.store(2,2,&input,true,&result);
        assert!(cache.load(2,2,&input,true).is_none());
        assert!(cache.load(2,2,b"/f1",true).is_some());
    }
    cache.invalidate(); assert!(cache.load(2,2,b"/f8",true).is_none());
}

#[test]
fn resolved_path_reuse_matches_walks_for_links_dotdot_missing_and_error_precedence() {
    let paths: &[&[u8]] = &[b"/a/b/value",b"/link/value",b"/absolute",b"/absolute/..",
        b"/a/../value",b"/a/b/../../a",b"/value/child",b"/value/",b"/missing",b"/loop",b"/empty",b"/",b"",b"/a\0/b"];
    for follow in [false,true] {
        for allow_missing in [false,true] {
            for path in paths {
                let mut actual=namespace_tree(true);let mut expected=namespace_tree(true);
                for _ in 0..3 {
                    let pack = |result: Result<(path_walk::Resolved,Option<Ext2Inode>),Error>|
                        result.map(|(r,_)| (r.node,r.parent,r.name,r.path));
                    assert_eq!(pack(namespace_disk::resolve(&mut actual,2,2,path,follow,allow_missing,false,true)),
                        pack(namespace_disk::resolve(&mut expected,2,2,path,follow,allow_missing,false,false)),
                        "path={path:?} follow={follow} missing={allow_missing}");
                }
            }
        }
    }
}

#[test]
fn directory_invalidation_discards_resolved_links_and_leaf_names_bypass_reuse() {
    let mut r=namespace_tree(true);
    let first=namespace_disk::resolve(&mut r,2,2,b"/absolute",true,false,false,true).unwrap().0;
    assert_eq!(first.node.unwrap().inode,12);
    let raw=namespace_disk::resolve(&mut r,2,2,b"/absolute",false,false,false,true).unwrap().0;
    assert_eq!(raw.node.unwrap().inode,15);
    directory::invalidate(&mut r);
    r.links.insert(15,b"/a/b".to_vec());
    let changed=namespace_disk::resolve(&mut r,2,2,b"/absolute",true,false,false,true).unwrap().0;
    assert_eq!(changed.node,Some(Node { inode:11,kind:Kind::Directory }));
    let name=namespace_disk::resolve(&mut r,2,2,b"/absolute",true,false,true,true).unwrap().0;
    assert!(!name.name.is_empty());
    assert_eq!(name.path,changed.path);
    let missing=namespace_disk::resolve(&mut r,2,2,b"/missing",true,true,false,true).unwrap().0;
    assert!(missing.node.is_none());
    assert!(r.resolved_paths.load(2,2,b"/missing",true).is_none());
    r.dentry_cache.store(2,b"missing",Some(99));
    assert_eq!(namespace_disk::resolve(&mut r,2,2,b"/missing",true,true,false,true).unwrap().0.node.unwrap().inode,99);
    directory::invalidate(&mut r);
    r.fail_inode_number=Some(11);
    assert!(matches!(namespace_disk::resolve(&mut r,2,2,b"/absolute",true,false,false,true),Err(Error::Io)));
}

#[test]
fn namespace_parent_validation_uses_the_previous_node_kind_on_warm_components() {
    for warm in [false, true] {
        let (mut r, path, last) = namespace_chain(warm);
        assert_eq!(namespace_disk::kind(read_inode(&mut r, 2).unwrap()), Kind::Directory);
        r.inode_reads = 0;
        let resolved = path_walk::resolve_reference(
            &mut namespace_disk::Disk::new(&mut r, Node { inode: 2, kind: Kind::Directory }),
            2, 2, &path, true, false,
        ).unwrap();
        assert_eq!(resolved.node, Some(Node { inode: last, kind: Kind::Directory }));
        assert_eq!(resolved.path, path);
        assert_eq!(r.inode_reads, if warm { 6 } else { 7 });
        assert_eq!(r.reads, if warm { 0 } else { 6 });
        println!("NAMESPACE-PARENT-KIND warm={warm} components=6 inode_reads={}", r.inode_reads);
    }
}

#[test]
fn repeated_container_roots_reuse_the_walk_until_metadata_changes() {
    fn root(runtime: &mut Ext2Runtime, path: &[u8]) -> Result<u32, Error> {
        let session = ClientSession { shm_local: path.as_ptr() as usize, shm_size: path.len() };
        canonical_path::resolve_root(runtime, session, 0, path.len())
    }

    let (mut runtime, _) = fixture(8);
    let components = [b"alter".as_slice(), b"linux", b"tinycore", b"rootfs"];
    let path = b"/alter/linux/tinycore/rootfs";
    for (index, name) in components.iter().enumerate() {
        runtime.dentry_cache.store(2 + index as u32, name, Some(3 + index as u32));
    }
    const CALLS: usize = 128;
    for _ in 0..CALLS {
        directory::invalidate(&mut runtime);
        assert_eq!(root(&mut runtime, path), Ok(6));
    }
    let uncached = runtime.inode_reads;
    assert_eq!(uncached, CALLS * components.len());
    runtime.inode_reads = 0;
    directory::invalidate(&mut runtime);
    for _ in 0..CALLS {
        assert_eq!(root(&mut runtime, path), Ok(6));
    }
    assert_eq!(runtime.inode_reads, components.len());
    println!("ROOT-PATH-LOOKUP calls={CALLS} inode_reads_before={uncached} inode_reads_after={}", runtime.inode_reads);

    let long = [b"/".repeat(128), path.to_vec()].concat();
    assert_eq!(root(&mut runtime, &long), Ok(6));
    assert_eq!(root(&mut runtime, b"/"), Ok(2));
    runtime.inode_reads = 0;
    assert_eq!(root(&mut runtime, path), Ok(6));
    assert_eq!(runtime.inode_reads, 0);

    directory::invalidate(&mut runtime);
    runtime.dentry_cache.store(5, b"rootfs", Some(7));
    runtime.fail_inode_number = Some(7);
    assert_eq!(root(&mut runtime, path), Err(Error::Io));
    runtime.fail_inode_number = None;
    assert_eq!(root(&mut runtime, path), Ok(7));
    assert_eq!(root(&mut runtime, b"alter/linux/tinycore/rootfs"), Err(Error::Invalid));

    directory::invalidate(&mut runtime);
    runtime.metadata.insert(4, Ext2Inode { mode: EXT2_S_IFLNK, size: 4 });
    assert_eq!(root(&mut runtime, path), Err(Error::Missing));
    assert_eq!(root(&mut runtime, b"/alter/linux"), Err(Error::NotDirectory));
}

#[test]
fn rooted_leaf_metadata_is_owned_and_scoped_to_the_exact_inode_and_walk() {
    let mut runtime = namespace_tree(true);
    let known = Node { inode: 2, kind: Kind::Directory };
    {
        let mut disk = namespace_disk::Disk::new(&mut runtime, known);
        let result = path_walk::resolve_reference(&mut disk, 2, 2, b"/a/b/value", true, false).unwrap();
        let leaf = result.node.unwrap();
        assert_eq!(leaf.inode, 12);
        assert_eq!(disk.last_inode(leaf.inode).unwrap().size, 37);
        assert!(disk.last_inode(2).is_none());
    }
    assert_eq!(runtime.inode_reads, 3);
    runtime.metadata.get_mut(&12).unwrap().size = 93;
    let mut disk = namespace_disk::Disk::new(&mut runtime, known);
    assert!(disk.last_inode(12).is_none());
    let result = path_walk::resolve_reference(&mut disk, 2, 2, b"/absolute", true, false).unwrap();
    let leaf = result.node.unwrap();
    assert_eq!(disk.last_inode(leaf.inode).unwrap().size, 93);
    assert!(disk.last_inode(15).is_none());
}

fn namespace_tree(warm: bool) -> Ext2Runtime {
    let (mut r, _) = fixture(8);
    let entries: &[(&[u8], u32)] = &[(b"a",10),(b"b",11),(b"value",12),(b"link",14),
        (b"absolute",15),(b"loop",16),(b"empty",17),(b"..",2)];
    r.disk[1].fill(0);
    for (index, &(name, inode)) in entries.iter().enumerate() {
        let record = &mut r.disk[1][index * 16..index * 16 + 16];
        record[..4].copy_from_slice(&inode.to_le_bytes());
        record[4..6].copy_from_slice(&16u16.to_le_bytes());
        record[6] = name.len() as u8;
        record[8..8 + name.len()].copy_from_slice(name);
        if warm {
            for parent in [2,10,11] { r.dentry_cache.store(parent, name, Some(inode)); }
        }
    }
    if warm {
        for parent in [2,10,11] { r.dentry_cache.store(parent, b"missing", None); }
    }
    r.metadata.insert(12, Ext2Inode { size: 37, mode: 0x8000 });
    for inode in 14..=17 { r.metadata.insert(inode, Ext2Inode { size: 0, mode: EXT2_S_IFLNK }); }
    r.links.insert(14, b"a/b".to_vec());
    r.links.insert(15, b"/a/b/value".to_vec());
    r.links.insert(16, b"/loop".to_vec());
    r.links.insert(17, Vec::new());
    r
}

#[test]
fn namespace_kind_hint_matches_frozen_adapter_for_links_dotdot_and_error_precedence() {
    let paths: &[&[u8]] = &[b"/",b".",b"..",b"/a/b/value",b"/a/../value",b"/link/value",
        b"/absolute",b"/empty",b"/loop",b"/value/child",b"/value/",b"/missing",b"/a/missing",
        b"/a/b/../../a",b"",b"/a\0/value"];
    let mut cases = 0;
    for warm in [false,true] {
        for start in [2,10,11] {
            for follow in [false,true] {
                for missing in [false,true] {
                    for path in paths {
                        let mut old = namespace_tree(warm);
                        let mut new = namespace_tree(warm);
                        let known = Node { inode: 2, kind: Kind::Directory };
                        let pack = |result: Result<path_walk::Resolved, Error>| result.map(|r| (r.node,r.parent,r.name,r.path));
                        let expected = pack(path_walk::resolve_reference(&mut baseline_namespace_disk::Disk::new(&mut old,known),2,start,path,follow,missing));
                        let actual = pack(path_walk::resolve_reference(&mut namespace_disk::Disk::new(&mut new,known),2,start,path,follow,missing));
                        assert_eq!(actual, expected, "warm={warm} start={start} path={path:?} follow={follow} missing={missing}");
                        assert_eq!((new.reads,new.mapped,new.dentry_stores), (old.reads,old.mapped,old.dentry_stores));
                        assert!(new.inode_reads <= old.inode_reads);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("NAMESPACE-KIND-ORACLE cases={cases}");
    assert_eq!(cases, 384);
}

#[test]
fn namespace_parent_loader_preserves_errors_and_cached_negative_results() {
    let mut r = namespace_tree(false);
    let name = b"new";
    let result = unsafe { directory_lookup::lookup_in_directory_with_loader(&mut r, 2,
        name.as_ptr(), name.len(), |_| Err::<Ext2Inode,_>(Error::Io)) };
    assert_eq!(result, Err(Error::Io));
    assert_eq!(r.reads, 0);
    r.dentry_cache.store(2, name, None);
    let result = unsafe { directory_lookup::lookup_in_directory_with_loader(&mut r, 2,
        name.as_ptr(), name.len(), |_| -> Result<Ext2Inode,Error> { panic!("negative hit must not load parent") }) };
    assert_eq!(result, Ok(None));
    r.dentry_cache.store(2, name, Some(12));
    let result = unsafe { directory_lookup::lookup_in_directory_with_loader(&mut r, 2,
        name.as_ptr(), name.len(), |_| -> Result<Ext2Inode,Error> { panic!("positive hit must not load parent") }) };
    assert_eq!(result, Ok(Some(12)));
}

#[test]
fn namespace_kind_hint_rechecks_child_errors_and_does_not_survive_requests() {
    let mut r = namespace_tree(true);
    r.fail_inode_number = Some(10);
    let known = Node { inode: 2, kind: Kind::Directory };
    assert_eq!(namespace_disk::Disk::new(&mut r,known).lookup(2,b"a"), Err(Error::Io));
    r.fail_inode_number = None;
    assert_eq!(namespace_disk::Disk::new(&mut r,known).lookup(2,b"a").unwrap().kind, Kind::Directory);
    r.metadata.insert(10, Ext2Inode { size: 37, mode: 0x8000 });
    let mut disk = namespace_disk::Disk::new(&mut r,known);
    assert_eq!(disk.lookup(2,b"a").unwrap().kind, Kind::File);
    assert_eq!(disk.lookup(10,&[b'x';256]), Err(Error::NotDirectory));
}
fn compare(
    actual: &mut Ext2Runtime,
    expected: &mut Ext2Runtime,
    inode: Ext2Inode,
    handle: usize,
    start: usize,
    limit: usize,
) -> (usize, usize) {
    let mut result = vec![0xa5u8; 8 + 16 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES + 8];
    let mut reference = result.clone();
    let session = ClientSession {
        shm_local: result.as_mut_ptr() as Word + 8,
        shm_size: result.len() - 16,
    };
    let baseline_session = ClientSession {
        shm_local: reference.as_mut_ptr() as Word + 8,
        ..session
    };
    let a = directory::read_directory(actual, session, inode, start, limit, 0, handle);
    let b = baseline::read_directory(expected, baseline_session, inode, start, limit, 0);
    assert_eq!(a, b, "start={start} limit={limit}");
    assert_eq!(result, reference);
    assert!(result[..8]
        .iter()
        .chain(&result[result.len() - 8..])
        .all(|&byte| byte == 0xa5));
    a.unwrap()
}

fn warm_blocks(runtime: &mut Ext2Runtime) {
    runtime.block_cache = block_cache::BlockCache::new(BLOCK_SIZE,
        (runtime.disk.len() + 1) * BLOCK_SIZE, BLOCK_SIZE);
    for (block, bytes) in runtime.disk.iter().enumerate().skip(1) {
        runtime.block_cache.store(block, bytes, None);
    }
    runtime.scratch.fill(0xcc);
}

#[test]
fn missing_directory_names_respect_inode_eof() {
    for blocks in [0, 1, 12, 13, 32] {
        let (mut runtime, _) = fixture(blocks * 8);
        let name = b"absent!!";
        assert_eq!(unsafe {
            directory_lookup::lookup_in_directory(&mut runtime, 7, name.as_ptr(), name.len())
        }, None);
        println!("DIRECTORY-EOF blocks={blocks} mapped={}", runtime.mapped.len());
        assert_eq!(runtime.mapped.len(), blocks);
        assert_eq!(runtime.reads, blocks);
        assert_eq!(runtime.dentry_stores, 1);
        assert!(runtime.mapped.iter().enumerate().all(|(index, block)| index == *block));
    }
    // An unused block pointer may exist past EOF, and failures there must not
    // trigger I/O or prevent caching a complete negative directory search.
    for bytes in [0, BLOCK_SIZE, 12 * BLOCK_SIZE, BLOCK_SIZE + 16] {
        let (mut runtime, _) = fixture(128);
        runtime.parent_inode.size = bytes as u32;
        runtime.fail_map = Some(bytes.div_ceil(BLOCK_SIZE));
        let name = b"absent!!";
        for _ in 0..2 {
            assert_eq!(unsafe {
                directory_lookup::lookup_in_directory(&mut runtime, 7, name.as_ptr(), name.len())
            }, None);
            assert_eq!(runtime.mapped.len(), bytes.div_ceil(BLOCK_SIZE));
            assert_eq!(runtime.dentry_stores, 1);
        }
    }
}

fn compare_lookup(actual:&mut Ext2Runtime, expected:&mut Ext2Runtime, name:&[u8])->Option<u32> {
    let result=unsafe {directory_lookup::lookup_in_directory(actual,7,name.as_ptr(),name.len())};
    let control=unsafe {baseline::lookup_in_directory(expected,7,name.as_ptr(),name.len())};
    assert_eq!(result,control);
    assert_eq!(actual.dentry_stores,expected.dentry_stores);
    result
}

#[test]
fn query_hash_is_lazy_and_reused_across_all_resident_directory_blocks() {
    let (mut actual, _) = fixture(1024);
    let (mut expected, _) = fixture(1024);
    warm_blocks(&mut actual);
    assert_eq!(compare_lookup(&mut actual, &mut expected, b"prime-miss"), None);
    let name = [b'Z'; 255];
    block_cache::BlockCache::take_name_hash_bytes();
    assert_eq!(compare_lookup(&mut actual, &mut expected, &name), None);
    let warm = block_cache::BlockCache::take_name_hash_bytes();
    assert_eq!(actual.reads, 0);
    assert!(actual.scratch.iter().all(|byte| *byte == 0xcc));
    assert_eq!(compare_lookup(&mut actual, &mut expected, &name), None);
    let dentry_hit = block_cache::BlockCache::take_name_hash_bytes();
    let (mut cold, _) = fixture(1024);
    let (mut reference, _) = fixture(1024);
    assert_eq!(compare_lookup(&mut cold, &mut reference, &name), None);
    let cold_bytes = block_cache::BlockCache::take_name_hash_bytes();
    let (mut positive, _) = fixture(1024);
    let (mut reference, _) = fixture(1024);
    warm_blocks(&mut positive);
    assert_eq!(compare_lookup(&mut positive, &mut reference, b"f0000000"), Some(1));
    let no_summary = block_cache::BlockCache::take_name_hash_bytes();
    println!("DIRECTORY-QUERY-HASH blocks=128 name-bytes=255 warm={warm} dentry-hit={dentry_hit} cold={cold_bytes} positive-without-summary={no_summary}");
    assert_eq!((warm, dentry_hit, cold_bytes, no_summary), (255, 0, 0, 0));
}

#[test]
fn known_parent_lookup_matches_legacy_results_cache_and_faults() {
    let mut cases = 0;
    for warm in [false, true] {
        for scenario in 0..6 {
            for name in [b"f0000000".as_slice(), b"f0000003", b"f0000048", b"absent!!"] {
                let (mut actual, _) = fixture(96);
                let (mut expected, _) = fixture(96);
                match scenario {
                    1 => { actual.fail_map = Some(0); expected.fail_map = Some(0); }
                    2 => { actual.fail_read = Some(1); expected.fail_read = Some(1); }
                    3 => { actual.block_count = 1; expected.block_count = 1; }
                    4 => { actual.disk[1][4..6].fill(0); expected.disk[1][4..6].fill(0); }
                    5 => { actual.parent_inode.size = 1; expected.parent_inode.size = 1; }
                    _ => {}
                }
                if warm { warm_blocks(&mut actual); warm_blocks(&mut expected); }
                for _ in 0..2 {
                    let parent = read_inode(&mut actual, 7).unwrap();
                    let _ = read_inode(&mut expected, 7).unwrap();
                    let got = unsafe { directory_lookup::lookup_in_directory_with_inode(
                        &mut actual, 7, parent, name.as_ptr(), name.len()) };
                    let old = unsafe { directory_lookup::lookup_in_directory(
                        &mut expected, 7, name.as_ptr(), name.len()) };
                    assert_eq!(got, old);
                    assert_eq!(actual.reads, expected.reads);
                    assert_eq!(actual.dentry_stores, expected.dentry_stores);
                    assert_eq!(actual.scratch.as_slice(), expected.scratch.as_slice());
                    actual.block_cache.assert_same_cache_for_test(&expected.block_cache);
                    cases += 1;
                }
            }
        }
    }
    println!("known parent lookup equivalent cases: {cases}");
}

#[test]
fn known_parent_lookup_avoids_second_inode_read_on_name_cache_miss() {
    let (mut r, _) = fixture(96);
    let mut counts = [0; 2];
    for count in &mut counts {
        r.inode_reads = 0;
        let parent = read_inode(&mut r, 7).unwrap();
        assert_eq!(unsafe { directory_lookup::lookup_in_directory_with_inode(
            &mut r, 7, parent, b"f0000000".as_ptr(), 8) }, Some(1));
        *count = r.inode_reads;
    }
    println!("known parent cold/cached name inode reads: {counts:?}");
    assert_eq!(counts, [1, 1]);
}

fn compare_parent(actual:&mut Ext2Runtime,expected:&mut Ext2Runtime,child:u32)
    ->Result<Vec<u8>,path_walk::Error> {
    let result=directory_parent::read_parent_name(actual,7,child);
    assert_eq!(result,baseline::parent_name(expected,7,child));
    assert_eq!(actual.inode_reads,expected.inode_reads);
    result
}

#[test]
fn warm_parent_names_match_old_scans_without_scratch_staging_or_inode_hints() {
    let (mut actual,_)=fixture(96);let (mut expected,_)=fixture(96);
    warm_blocks(&mut actual);
    for child in [1,2,8,49,96,4,999,0] {let _=compare_parent(&mut actual,&mut expected,child);}
    // Dot entries referencing the same inode must not replace its actual name.
    let mut changed=actual.disk[1];
    for (offset,name) in [(0,b".".as_slice()),(16,b"..".as_slice())] {
        changed[offset..offset+4].copy_from_slice(&49_u32.to_le_bytes());
        changed[offset+6]=name.len() as u8;
        changed[offset+8..offset+8+name.len()].copy_from_slice(name);
    }
    actual.block_cache.store(1,&changed,Some(block_cache::BlockKind::Metadata));
    expected.disk[1]=changed;
    assert_eq!(compare_parent(&mut actual,&mut expected,49),Ok(b"f0000048".to_vec()));
    assert_eq!(actual.inode_reads,9);
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn parent_name_is_owned_and_next_request_observes_current_dirty_rename() {
    let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
    warm_blocks(&mut actual);
    let old=compare_parent(&mut actual,&mut expected,2).unwrap();
    let mut changed=actual.disk[1];changed[24..32].copy_from_slice(b"renamed!");
    actual.block_cache.store(1,&changed,Some(block_cache::BlockKind::Metadata));
    expected.disk[1]=changed;
    assert_eq!(compare_parent(&mut actual,&mut expected,2),Ok(b"renamed!".to_vec()));
    assert_eq!(old,b"f0000001");
    assert_ne!(actual.disk[1],changed);
    assert_eq!(actual.inode_reads,2);
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn warm_parent_scan_preserves_full_final_block_malformed_and_cached_bounds_behavior() {
    let (mut actual,_)=fixture(16);let (mut expected,_)=fixture(16);
    warm_blocks(&mut actual);
    actual.parent_inode.size=1;expected.parent_inode.size=1;
    assert_eq!(compare_parent(&mut actual,&mut expected,8),Ok(b"f0000007".to_vec()));
    assert_eq!(compare_parent(&mut actual,&mut expected,16),Err(path_walk::Error::Missing));
    let mut malformed=actual.disk[1];malformed[68..70].fill(0);
    actual.block_cache.store(1,&malformed,None);expected.disk[1]=malformed;
    assert_eq!(compare_parent(&mut actual,&mut expected,6),Err(path_walk::Error::Io));
    assert_eq!(compare_parent(&mut actual,&mut expected,1),Ok(b"f0000000".to_vec()));
    actual.block_cache.store(1,&actual.disk[1],None);expected.disk[1]=actual.disk[1];
    for transfer in [false,true] {
        let count=actual.block_count;
        if transfer {actual.block_shm_size=0;expected.block_shm_size=0;}
        else {actual.block_count=1;expected.block_count=1;}
        assert_eq!(compare_parent(&mut actual,&mut expected,1),Err(path_walk::Error::Io));
        actual.block_count=count;expected.block_count=count;
        actual.block_shm_size=BLOCK_SIZE;expected.block_shm_size=BLOCK_SIZE;
    }
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn cold_parent_lookup_preserves_inode_map_io_hole_and_empty_parent_errors() {
    for failure in 0..3 {
        let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
        for r in [&mut actual,&mut expected] {
            match failure {0=>r.fail_inode=true,1=>r.fail_map=Some(1),_=>r.fail_read=Some(2)}
        }
        assert_eq!(compare_parent(&mut actual,&mut expected,9),Err(path_walk::Error::Io));
        for r in [&mut actual,&mut expected] {r.fail_inode=false;r.fail_map=None;r.fail_read=None;}
        assert_eq!(compare_parent(&mut actual,&mut expected,9),Ok(b"f0000008".to_vec()));
    }
    let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
    actual.disk.truncate(2);expected.disk.truncate(2);
    assert_eq!(compare_parent(&mut actual,&mut expected,9),Err(path_walk::Error::Io));
    actual.parent_inode.size=0;expected.parent_inode.size=0;
    assert_eq!(compare_parent(&mut actual,&mut expected,1),Err(path_walk::Error::Missing));
}

#[test]
fn warm_name_lookup_matches_old_scans_and_keeps_positive_negative_hit_precedence() {
    let (mut actual,_)=fixture(96);let (mut expected,_)=fixture(96);
    warm_blocks(&mut actual);
    for (name,result) in [(b"f0000000".as_slice(),Some(1)),(b"f0000048",Some(49)),
        (b"f0000095",Some(96)),(b"f0000003",None),(b"absent__",None),
        (b"f",None),(b"f00000000",None),(b"F0000000",None)] {
        assert_eq!(compare_lookup(&mut actual,&mut expected,name),result);
    }
    let inode_reads=actual.inode_reads;
    actual.fail_inode=true;expected.fail_inode=true;
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000000"),Some(1));
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"absent__"),None);
    assert_eq!(actual.inode_reads,inode_reads);
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn name_lookup_follows_current_dirty_directory_after_dentry_invalidation() {
    let (mut actual,_)=fixture(48);let (mut expected,_)=fixture(48);
    warm_blocks(&mut actual);
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"new-name"),None);
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000001"),Some(2));
    let mut changed=actual.disk[1];
    changed[16..20].copy_from_slice(&99_u32.to_le_bytes());
    changed[24..32].copy_from_slice(b"new-name");
    actual.block_cache.store(1,&changed,Some(block_cache::BlockKind::Metadata));
    expected.disk[1]=changed;
    actual.dentry_cache.invalidate_inode(7);expected.dentry_cache.invalidate_inode(7);
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"new-name"),Some(99));
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000001"),None);
    assert_ne!(actual.disk[1],changed);
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn distinct_directory_misses_skip_rescans_and_survive_size_changes_and_slot_reuse() {
    let (mut actual, _) = fixture(96);
    let (mut expected, _) = fixture(96);
    warm_blocks(&mut actual);
    for index in 0..1000 {
        let name = format!("absent-{index:08}");
        assert_eq!(compare_lookup(&mut actual, &mut expected, name.as_bytes()), None);
    }
    // The old search parses all twelve blocks for each distinct miss (12000
    // full scans). The first miss builds summaries; hash collisions still
    // take the authoritative parser, but almost all later scans disappear.
    assert!(actual.block_cache.directory_scans < 100,
        "{} scans", actual.block_cache.directory_scans);
    for index in 0..96 {
        let name = format!("f{index:07}");
        assert_eq!(compare_lookup(&mut actual, &mut expected, name.as_bytes()),
            (index % 7 != 3).then_some(index as u32 + 1));
    }
    // A summary of a shortened final block cannot reject names after growth.
    actual.parent_inode.size = 16;
    expected.parent_inode.size = 16;
    actual.dentry_cache.invalidate_inode(7);
    expected.dentry_cache.invalidate_inode(7);
    assert_eq!(compare_lookup(&mut actual, &mut expected, b"short-miss"), None);
    actual.parent_inode.size = 96 * 16;
    expected.parent_inode.size = 96 * 16;
    assert_eq!(compare_lookup(&mut actual, &mut expected, b"f0000001"), Some(2));
    // Force a resident slot's summary to be retired, then refill that slot
    // with different bytes under the same physical block number.
    actual.block_cache = block_cache::BlockCache::new(BLOCK_SIZE, BLOCK_SIZE, BLOCK_SIZE);
    actual.block_cache.store(1, &actual.disk[1], None);
    actual.dentry_cache.invalidate_inode(7);
    expected.dentry_cache.invalidate_inode(7);
    assert_eq!(compare_lookup(&mut actual, &mut expected, b"new-name"), None);
    actual.block_cache.store(2, &actual.disk[2], None);
    let mut changed = actual.disk[1];
    changed[24..32].copy_from_slice(b"new-name");
    actual.block_cache.store(1, &changed, Some(block_cache::BlockKind::Metadata));
    expected.disk[1] = changed;
    actual.dentry_cache.invalidate_inode(7);
    expected.dentry_cache.invalidate_inode(7);
    assert_eq!(compare_lookup(&mut actual, &mut expected, b"new-name"), Some(2));
}

#[test]
fn malformed_warm_lookup_never_publishes_a_negative_and_cached_bounds_remain_checked() {
    let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
    warm_blocks(&mut actual);
    let mut malformed=actual.disk[1];malformed[68..70].fill(0);
    actual.block_cache.store(1,&malformed,None);expected.disk[1]=malformed;
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000005"),None);
    assert_eq!(actual.dentry_stores,0);
    assert_eq!(actual.dentry_cache.load(7,b"f0000005"),None);
    // A match preceding the malformed record retains the old early success.
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000000"),Some(1));
    actual.block_cache.store(1,&actual.disk[1],None);expected.disk[1]=actual.disk[1];
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000005"),Some(6));
    for transfer in [false,true] {
        actual.dentry_cache.invalidate_inode(7);expected.dentry_cache.invalidate_inode(7);
        let stores=actual.dentry_stores;let count=actual.block_count;
        if transfer {actual.block_shm_size=0;expected.block_shm_size=0;}
        else {actual.block_count=1;expected.block_count=1;}
        assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000005"),None);
        assert_eq!(actual.dentry_stores,stores);
        actual.block_count=count;expected.block_count=count;
        actual.block_shm_size=BLOCK_SIZE;expected.block_shm_size=BLOCK_SIZE;
    }
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn cold_lookup_metadata_mapping_io_and_eof_results_keep_old_cache_publication() {
    for failure in 0..4 {
        let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
        for r in [&mut actual,&mut expected] {
            match failure {0=>r.fail_inode=true,1=>r.parent_inode.mode=0x8000,
                2=>r.fail_read=Some(2),_=>r.fail_map=Some(1)}
        }
        assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000008"),None);
        assert_eq!(actual.dentry_stores,0);
        for r in [&mut actual,&mut expected] {
            r.fail_inode=false;r.parent_inode.mode=EXT2_S_IFDIR;r.fail_read=None;r.fail_map=None;
        }
        assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000008"),Some(9));
    }
    let (mut actual,_)=fixture(32);let (mut expected,_)=fixture(32);
    warm_blocks(&mut actual);actual.parent_inode.size=0;expected.parent_inode.size=0;
    assert_eq!(compare_lookup(&mut actual,&mut expected,b"f0000000"),None);
    assert_eq!(actual.dentry_cache.load(7,b"f0000000"),Some(None));
    assert_eq!(actual.reads,0);assert!(actual.scratch.iter().all(|b|*b==0xcc));
}

#[test]
fn cached_directory_batches_match_rescans_without_scratch_copies() {
    for limit in [1, 7, 16] {
        let (mut actual, inode) = fixture(192);
        let (mut expected, _) = fixture(192);
        warm_blocks(&mut actual);
        let mut start = 0;
        loop {
            let (entries, next) = compare(&mut actual, &mut expected, inode, 0, start, limit);
            assert_eq!(actual.reads, 0);
            assert!(actual.scratch.iter().all(|byte| *byte == 0xcc));
            if entries == 0 { break; }
            start = next;
        }
    }
}

#[test]
fn cached_directory_reads_current_dirty_names_and_deletions_after_generation_change() {
    let (mut actual, inode) = fixture(96);
    let (mut expected, _) = fixture(96);
    warm_blocks(&mut actual);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 4);
    compare(&mut actual, &mut expected, inode, 1, 0, 7);
    let mut changed = actual.disk[1];
    changed[..4].fill(0);
    changed[24..32].copy_from_slice(b"changed!");
    actual.block_cache.store(1, &changed, Some(block_cache::BlockKind::Metadata));
    expected.disk[1] = changed;
    directory::invalidate(&mut actual);
    for (handle, start, limit) in [(0, next, 8), (1, 7, 8), (0, 0, 8), (1, 2, 3)] {
        compare(&mut actual, &mut expected, inode, handle, start, limit);
    }
    actual.handles[0].inode = 9;
    compare(&mut actual, &mut expected, inode, 0, 10, 8);
    assert_ne!(actual.disk[1], changed); // Dirty cache, not disk, supplied bytes.
    assert_eq!(actual.reads, 0);
    assert!(actual.scratch.iter().all(|byte| *byte == 0xcc));
}

#[test]
fn malformed_cached_directory_keeps_output_prefix_retry_and_cached_bounds_checks() {
    let (mut actual, inode) = fixture(32);
    let (mut expected, _) = fixture(32);
    warm_blocks(&mut actual);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 2);
    let mut malformed = actual.disk[1];
    malformed[68..70].fill(0); // Valid output prefix, then a zero-length record.
    actual.block_cache.store(1, &malformed, None);
    expected.disk[1] = malformed;
    let mut output = vec![0xa5u8; 8 + 16 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES + 8];
    let mut reference = output.clone();
    let session = ClientSession {shm_local:output.as_mut_ptr() as Word + 8, shm_size:output.len()-16};
    let control = ClientSession {shm_local:reference.as_mut_ptr() as Word + 8, ..session};
    assert_eq!(directory::read_directory(&mut actual, session, inode, next, 16, 0, 0),
        baseline::read_directory(&mut expected, control, inode, next, 16, 0));
    assert_eq!(output, reference);
    assert!(output[8..8 + vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES].iter().any(|v| *v != 0xa5));
    assert!(output[8 + vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES..].iter().all(|v| *v == 0xa5));
    actual.block_cache.store(1, &actual.disk[1], None);
    expected.disk[1] = actual.disk[1];
    compare(&mut actual, &mut expected, inode, 0, next, 8);
    for transfer_bounds in [false, true] {
        let count = actual.block_count;
        if transfer_bounds {actual.block_shm_size = 0;} else {actual.block_count = 1;}
        output.fill(0xa5);
        assert_eq!(directory::read_directory(&mut actual, session, inode, 0, 16, 0, 0),
            Err(OS_RESPONSE_INVALID_DESCRIPTOR));
        assert!(output.iter().all(|v| *v == 0xa5));
        actual.block_count = count;actual.block_shm_size = BLOCK_SIZE;
    }
    assert_eq!(actual.reads, 0);
    assert!(actual.scratch.iter().all(|byte| *byte == 0xcc));
}

#[test]
fn sequential_batches_are_linear_and_match_full_rescans_with_deleted_entries() {
    let (mut actual, inode) = fixture(1024);
    let (mut expected, _) = fixture(1024);
    let mut start = 0;
    loop {
        let (entries, next) = compare(&mut actual, &mut expected, inode, 0, start, 1);
        if entries == 0 {
            break;
        }
        assert_eq!(next, start + 1);
        start = next;
    }
    assert!(actual.reads <= 1024);
    assert!(expected.reads > actual.reads * 32);
}

#[test]
fn rewind_forward_backward_seek_and_interleaved_handles_preserve_index_cookies() {
    let (mut actual, inode) = fixture(96);
    let (mut expected, _) = fixture(96);
    for (handle, start, limit) in [
        (0, 0, 3),
        (1, 0, 7),
        (0, 3, 4),
        (1, 7, 8),
        (0, 7, 4),
        (0, 2, 1),
        (0, 70, 3),
        (0, 0, 2),
        (0, 200, 5),
        (0, 1, 16),
    ] {
        compare(&mut actual, &mut expected, inode, handle, start, limit);
    }
    actual.handles[0].inode = 9;
    compare(&mut actual, &mut expected, inode, 0, 17, 2);
}

#[test]
fn metadata_mutation_invalidates_cached_byte_position_even_when_size_is_unchanged() {
    let (mut actual, inode) = fixture(48);
    let (mut expected, _) = fixture(48);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 8);
    actual.disk[1][..4].fill(0);
    expected.disk[1][..4].fill(0);
    directory::invalidate(&mut actual);
    compare(&mut actual, &mut expected, inode, 0, next, 8);
}

#[test]
fn failed_io_and_malformed_entries_do_not_commit_a_bad_cursor() {
    let (mut actual, inode) = fixture(48);
    let (mut expected, _) = fixture(48);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 6);
    actual.fail_read = Some(2);
    expected.fail_read = Some(2);
    let mut bytes = vec![0u8; 16 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES];
    let session = ClientSession {
        shm_local: bytes.as_mut_ptr() as Word,
        shm_size: bytes.len(),
    };
    assert!(directory::read_directory(&mut actual, session, inode, next, 16, 0, 0).is_err());
    actual.fail_read = None;
    expected.fail_read = None;
    compare(&mut actual, &mut expected, inode, 0, next, 16);
    actual.disk[1][4..6].fill(0);
    directory::invalidate(&mut actual);
    assert!(directory::read_directory(&mut actual, session, inode, 0, 16, 0, 0).is_err());
}

#[test]
fn zero_capacity_and_invalid_output_range_do_not_advance_the_cursor() {
    let (mut actual, inode) = fixture(16);
    let (mut expected, _) = fixture(16);
    let (_, next) = compare(&mut actual, &mut expected, inode, 0, 0, 3);
    let session = ClientSession {
        shm_local: 0,
        shm_size: 0,
    };
    assert_eq!(
        directory::read_directory(&mut actual, session, inode, next, 1, 1, 0),
        Ok((0, 0))
    );
    assert_eq!(
        directory::read_directory(&mut actual, session, inode, next, 0, 0, 0),
        Ok((0, 0))
    );
    compare(&mut actual, &mut expected, inode, 0, next, 3);
}

fn live_before(raw_entry: usize) -> usize {
    (0..raw_entry).filter(|number| number % 7 != 3).count()
}

#[test]
fn backward_seeks_revalidate_after_mutation_and_handle_reuse() {
    let (mut actual, inode) = fixture(1024);
    let (mut expected, _) = fixture(1024);
    let first = live_before(64 * 8);
    compare(&mut actual, &mut expected, inode, 0, first, 5);
    compare(&mut actual, &mut expected, inode, 0, 4, 3);
    compare(&mut actual, &mut expected, inode, 0, first, 5);
    actual.disk[1][..4].fill(0);
    expected.disk[1][..4].fill(0);
    directory::invalidate(&mut actual);
    let reads = actual.reads;
    compare(&mut actual, &mut expected, inode, 0, first + 2, 2);
    assert!(
        actual.reads - reads > 1,
        "a generation change invalidates the prefix count"
    );
    actual.handles[0].inode = 9;
    let reads = actual.reads;
    compare(&mut actual, &mut expected, inode, 0, first + 3, 2);
    assert!(
        actual.reads - reads > 1,
        "a reused handle must not reuse another inode's checkpoint"
    );
}

#[test]
fn failed_backward_seek_read_preserves_the_stream() {
    let (mut actual, inode) = fixture(1024);
    let (mut expected, _) = fixture(1024);
    let first = live_before(64 * 8);
    compare(&mut actual, &mut expected, inode, 0, first, 5);
    let mut bytes = [0xa5u8; 2 * vfs::VFS_DIRECTORY_ENTRY_RECORD_BYTES];
    let session = ClientSession {
        shm_local: bytes.as_mut_ptr() as Word,
        shm_size: bytes.len(),
    };
    actual.fail_read = Some(65);
    assert_eq!(
        directory::read_directory(&mut actual, session, inode, first + 2, 2, 0, 0),
        Err(OS_RESPONSE_INVALID_DESCRIPTOR)
    );
    assert!(bytes.iter().all(|&byte| byte == 0xa5));
    actual.fail_read = None;
    compare(&mut actual, &mut expected, inode, 0, first + 3, 2);
}

#[test]
fn mixed_seeks_rollbacks_deleted_entries_and_generations_match_the_full_scan_oracle() {
    let (mut actual, inode) = fixture(192);
    let (mut expected, _) = fixture(192);
    let mut seed = 0x12345678u32;
    for step in 0..2048 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let handle = step % MAX_HANDLES;
        let start = if step % 97 == 0 {
            usize::MAX
        } else {
            (seed as usize >> 8) % 200
        };
        let limit = seed as usize % 17;
        if step % 113 == 0 {
            let inode = if step % 226 == 0 { 0u32 } else { 1u32 };
            actual.disk[1][..4].copy_from_slice(&inode.to_le_bytes());
            expected.disk[1][..4].copy_from_slice(&inode.to_le_bytes());
            directory::invalidate(&mut actual);
            actual.handles[handle].inode = 7 + step as u32;
        }
        compare(&mut actual, &mut expected, inode, handle, start, limit);
    }
}
