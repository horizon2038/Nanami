extern crate alloc;
use std::collections::BTreeMap;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_walk.rs"]
mod path_walk;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_input.rs"]
mod path_input;
use path_walk::{resolve, Error, Filesystem, Kind, Node};

thread_local! {
    static ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static REALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static FAIL_RESERVATION_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
struct CountingAllocator;
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if FAIL_RESERVATION_BYTES.with(|bytes| {
            if bytes.get() != 0 && bytes.get() == layout.size() { bytes.set(0); true } else { false }
        }) { return core::ptr::null_mut(); }
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() {
                count.set(Some(value + 1));
            }
        });
        unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::GlobalAlloc::dealloc(&std::alloc::System, ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        let _ = REALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() { count.set(Some(value + 1)); }
        });
        let _ = ALLOCATIONS.try_with(|count| {
            if let Some(value) = count.get() {
                count.set(Some(value + 1));
            }
        });
        unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn main_path_input_snapshots_bytes_with_bounds_and_inline_size_boundary() {
    let mut data: Vec<u8> = (0..4120).map(|index| (index % 251) as u8).collect();
    for (offset, bytes) in [(0,0),(0,1),(1,255),(3,256),(7,257),(8,4096),
        (0,4097),(4120,0),(4121,0),(4119,2),(usize::MAX,1)] {
        let expected = if bytes > path_walk::PATH_MAX
            || offset.checked_add(bytes).is_none_or(|end| end > data.len()) {
            Err(Error::Invalid)
        } else { Ok(data[offset..offset+bytes].to_vec()) };
        let mut storage = [0xa5u8; 256];
        let actual = path_input::path(data.as_ptr() as usize, data.len(), offset, bytes, &mut storage);
        assert_eq!(actual.map(|value| value.into_owned()), expected);
    }
    let mut counts = Vec::new();
    for bytes in [0,1,255,256,257,4096] {
        let mut storage = [0xa5u8; 256];
        ALLOCATIONS.with(|count| count.set(Some(0)));
        let copied = path_input::path(data.as_ptr() as usize, data.len(), 0, bytes, &mut storage).unwrap();
        counts.push(ALLOCATIONS.with(|count| count.replace(None).unwrap()));
        assert_eq!(&*copied, &data[..bytes]);
    }
    let expected = data[7..24].to_vec();
    let mut storage = [0xa5u8; 256];
    let base = data.as_mut_ptr() as usize;
    let copied = path_input::path(base, data.len(), 7, 17, &mut storage).unwrap();
    data[7..24].fill(0xcc);
    assert_eq!(&*copied, expected.as_slice(), "SHM overwrites must not affect the snapshot");
    println!("main-path snapshot allocations 0/1/255/256/257/4096: {counts:?}");
}

#[test]
fn reference_resolution_preserves_walk_results_errors_and_link_reads() {
    let paths: &[&[u8]] = &[b"/", b".", b"..", b"/dir/value", b"/dir/nested/../value",
        b"/link/../value", b"/absolute", b"/escape", b"/dir/new", b"/dir/value/",
        b"/dir/value/child", b"/link/../../dir/value", b"/dir/./value", b"/missing/x", b"", b"/dir\0/value"];
    let mut cases = 0;
    for root in [1, 2] {
        for start in [root, 3, 4, 9] {
            for follow in [false, true] {
                for missing in [false, true] {
                    for path in paths {
                        let mut old = Tree::new();
                        let mut new = Tree::new();
                        let expected = resolve(&mut old, root, start, path, follow, missing);
                        let actual = path_walk::resolve_reference(&mut new, root, start, path, follow, missing);
                        match (expected, actual) {
                            (Ok(a), Ok(b)) => {
                                assert_eq!(a.node, b.node); assert_eq!(a.parent, b.parent);
                                assert_eq!(a.path, b.path); assert!(b.name.is_empty());
                            }
                            (Err(a), Err(b)) => assert_eq!(a, b),
                            _ => panic!("walk result mismatch"),
                        }
                        assert_eq!(old.reads, new.reads);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("reference resolver equivalent walk cases: {cases}");
}

#[test]
fn reference_resolution_skips_unused_leaf_allocation() {
    let mut counts = Vec::new();
    for depth in [0, 1, 4, 15] {
        let mut input = b"/".to_vec();
        for _ in 0..depth { input.extend_from_slice(b"segment/"); }
        input.extend_from_slice(b"value");
        let mut fs = Chain { depth };
        ALLOCATIONS.with(|c| c.set(Some(0)));
        let found = path_walk::resolve_reference(&mut fs, 1, 1, &input, true, false).unwrap();
        counts.push(ALLOCATIONS.with(|c| c.replace(None).unwrap()));
        assert_eq!(found.path, input); assert!(found.name.is_empty());
        assert_eq!(found.node.unwrap().inode, depth + 2);
    }
    println!("reference path allocations depth0/1/4/15: {counts:?}");
    assert_eq!(counts, [1, 1, 1, 1]);
}

#[test]
fn scalar_path_inputs_match_owned_reference_bounds_bytes_and_endianness() {
    fn owned(data: &[u8], offset: usize, bytes: usize) -> Result<Vec<u8>, Error> {
        if bytes > path_walk::PATH_MAX || offset.checked_add(bytes).is_none_or(|end| end > data.len()) {
            return Err(Error::Invalid);
        }
        Ok(data[offset..offset + bytes].to_vec())
    }
    let mut data = vec![0xa5u8; 4120];
    data[0] = b'/'; data[7] = b'/'; data[4095] = b'/';
    data[15..23].copy_from_slice(&0xfedcba9876543210usize.to_le_bytes());
    let before = data.clone();
    let mut cases = 0;
    for offset in [0, 1, 7, 15, 4095, 4096, 4112, 4120, 4121, usize::MAX] {
        for bytes in [0, 1, 7, 8, 9, 24, 4095, 4096, 4097, usize::MAX] {
            let root = owned(&data, offset, bytes).and_then(|v| {
                if v.first() == Some(&b'/') { Ok(()) } else { Err(Error::Invalid) }
            });
            let word = owned(&data, offset, bytes).and_then(|v| {
                Ok(usize::from_le_bytes(v.as_slice().try_into().map_err(|_| Error::Invalid)?))
            });
            assert_eq!(path_input::root(data.as_ptr() as usize, data.len(), offset, bytes), root);
            assert_eq!(path_input::word(data.as_ptr() as usize, data.len(), offset, bytes), word);
            cases += 2;
        }
    }
    assert_eq!(path_input::word(data.as_ptr() as usize, data.len(), 15, 8), Ok(0xfedcba9876543210));
    assert_eq!(data, before);
    // Invalid bounds must fail before constructing or dereferencing a pointer.
    assert_eq!(path_input::root(0, 0, 0, 1), Err(Error::Invalid));
    assert_eq!(path_input::word(0, 0, 0, 8), Err(Error::Invalid));
    println!("scalar path input reference cases: {cases}");
}

#[test]
fn scalar_path_inputs_do_not_allocate_owned_request_fields() {
    let mut data = [0x5au8; 32]; data[0] = b'/';
    ALLOCATIONS.with(|c| c.set(Some(0)));
    let root = path_input::root(data.as_ptr() as usize, data.len(), 0, data.len());
    let word = path_input::word(data.as_ptr() as usize, data.len(), 1, 8);
    let allocations = ALLOCATIONS.with(|c| c.replace(None).unwrap());
    assert_eq!(root, Ok(())); assert_eq!(word, Ok(usize::from_le_bytes([0x5a; 8])));
    println!("root+word request allocations: {allocations}");
    assert_eq!(allocations, 0);
}

struct Chain {
    depth: u32,
}

#[test]
fn output_reservation_failure_keeps_resolution_and_symlink_growth_beyond_hint() {
    let input=b"/segment/segment/segment/segment/value";
    let mut fs=Chain { depth:4 };
    FAIL_RESERVATION_BYTES.with(|bytes|bytes.set(input.len()));
    let result=resolve(&mut fs,1,1,input,true,false).unwrap();
    assert_eq!(FAIL_RESERVATION_BYTES.with(|bytes|bytes.get()),0);
    assert_eq!(result.path,input); assert_eq!(result.name,b"value");
    // The relative path hint includes four parent names and the ./ input.
    FAIL_RESERVATION_BYTES.with(|bytes|bytes.set(40));
    let relative=resolve(&mut fs,1,5,b"./value",true,false).unwrap();
    assert_eq!(FAIL_RESERVATION_BYTES.with(|bytes|bytes.get()),0);
    assert_eq!(relative.path,input); assert_eq!(relative.name,b"value");
    let mut tree=Tree::new();
    tree.add(2,b"l",30,Kind::Symlink);
    tree.links.insert(30,b"/dir/nested/../value".to_vec());
    let linked=resolve(&mut tree,2,2,b"/l",true,false).unwrap();
    assert_eq!(linked.path,b"/dir/value");
    assert_eq!(linked.node.unwrap().inode,5);
    let normalized=resolve(&mut tree,2,2,b"////dir/./nested/../value",true,false).unwrap();
    assert_eq!(normalized.path,linked.path);
}

#[test]
fn component_stack_depth_boundaries_preserve_rooted_and_relative_resolution() {
    for depth in [0,1,4,15,16,17,31,128] {
        let mut fs=Chain { depth };
        let mut input=b"/".to_vec();
        for _ in 0..depth { input.extend_from_slice(b"segment/"); }
        input.extend_from_slice(b"value");
        ALLOCATIONS.with(|count|count.set(Some(0)));
        let result=resolve(&mut fs,1,1,&input,true,false).unwrap();
        let allocations=ALLOCATIONS.with(|count|count.replace(None).unwrap());
        assert_eq!(result.path,input); assert_eq!(result.name,b"value");
        assert_eq!(result.node.unwrap().inode,depth+2);
        println!("PATH-COMPONENT-STACK depth={depth} allocations={allocations}");
        REALLOCATIONS.with(|count|count.set(Some(0)));
        let relative=resolve(&mut fs,1,depth+1,b"./value",true,false).unwrap();
        let reallocations=REALLOCATIONS.with(|count|count.replace(None).unwrap());
        assert_eq!(relative.path,input); assert_eq!(relative.name,b"value");
        println!("RELATIVE-PATH depth={depth} reallocations={reallocations}");
        if depth!=0 {
            let parent=resolve(&mut fs,1,depth+1,b"..",true,false).unwrap();
            assert_eq!(parent.node.unwrap().inode,depth);
        }
    }
}
impl Filesystem for Chain {
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error> {
        if name == b"segment" && directory <= self.depth {
            return Ok(Node {
                inode: directory + 1,
                kind: Kind::Directory,
            });
        }
        if name == b"value" && directory == self.depth + 1 {
            return Ok(Node {
                inode: directory + 1,
                kind: Kind::File,
            });
        }
        Err(Error::Missing)
    }
    fn read_link(&mut self, _: u32) -> Result<Vec<u8>, Error> {
        Err(Error::Invalid)
    }
    fn parent_name(&mut self, directory: u32) -> Result<(u32, Vec<u8>), Error> {
        if directory > 1 && directory <= self.depth + 1 {
            return Ok((directory - 1, b"segment".to_vec()));
        }
        Err(Error::Invalid)
    }
}

struct Tree {
    children: BTreeMap<(u32, Vec<u8>), Node>,
    parents: BTreeMap<u32, (u32, Vec<u8>)>,
    links: BTreeMap<u32, Vec<u8>>,
    reads: usize,
}
impl Tree {
    fn new() -> Self {
        let mut tree = Self {
            children: BTreeMap::new(),
            parents: BTreeMap::new(),
            links: BTreeMap::new(),
            reads: 0,
        };
        tree.parents.insert(1, (1, Vec::new()));
        tree.add(1, b"root", 2, Kind::Directory);
        tree.add(1, b"outside", 9, Kind::Directory);
        tree.add(9, b"token", 10, Kind::File);
        tree.add(2, b"dir", 3, Kind::Directory);
        tree.add(3, b"nested", 4, Kind::Directory);
        tree.add(3, b"value", 5, Kind::File);
        tree.add(2, b"link", 6, Kind::Symlink);
        tree.links.insert(6, b"dir/nested".to_vec());
        tree.add(2, b"absolute", 7, Kind::Symlink);
        tree.links.insert(7, b"/dir/value".to_vec());
        tree.add(2, b"escape", 8, Kind::Symlink);
        tree.links.insert(8, b"/outside/token".to_vec());
        tree
    }
    fn add(&mut self, parent: u32, name: &[u8], inode: u32, kind: Kind) {
        self.children
            .insert((parent, name.to_vec()), Node { inode, kind });
        self.parents.insert(inode, (parent, name.to_vec()));
    }
}
impl Filesystem for Tree {
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error> {
        self.children
            .get(&(directory, name.to_vec()))
            .copied()
            .ok_or(Error::Missing)
    }
    fn read_link(&mut self, inode: u32) -> Result<Vec<u8>, Error> {
        self.reads += 1;
        self.links.get(&inode).cloned().ok_or(Error::Invalid)
    }
    fn parent_name(&mut self, directory: u32) -> Result<(u32, Vec<u8>), Error> {
        self.parents.get(&directory).cloned().ok_or(Error::Missing)
    }
}

#[test]
fn symlink_is_expanded_before_dot_dot_and_absolute_target_stays_in_root() {
    let mut tree = Tree::new();
    let found = resolve(&mut tree, 2, 2, b"/link/../value", true, false).unwrap();
    assert_eq!(found.node.unwrap().inode, 5);
    assert_eq!(found.path, b"/dir/value");
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/absolute", true, false)
            .unwrap()
            .node
            .unwrap()
            .inode,
        5
    );
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/escape", true, false).err(),
        Some(Error::Missing)
    );
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/../../outside/token", true, false).err(),
        Some(Error::Missing)
    );
}

#[test]
fn directory_fd_uses_current_inode_parent_after_rename_and_cannot_escape_root() {
    let mut tree = Tree::new();
    tree.children.remove(&(2, b"dir".to_vec()));
    tree.add(2, b"renamed", 3, Kind::Directory);
    let found = resolve(&mut tree, 2, 3, b"value", true, false).unwrap();
    assert_eq!(found.path, b"/renamed/value");
    assert_eq!(
        resolve(&mut tree, 2, 9, b"token", true, false).err(),
        Some(Error::Invalid)
    );
    // Absolute paths ignore even an unrelated/invalid dirfd.
    assert_eq!(
        resolve(&mut tree, 2, 999, b"/renamed/value", true, false)
            .unwrap()
            .node
            .unwrap()
            .inode,
        5
    );
}

#[test]
fn final_nofollow_missing_creation_and_trailing_slash_are_distinct() {
    let mut tree = Tree::new();
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/link", false, false)
            .unwrap()
            .node
            .unwrap()
            .kind,
        Kind::Symlink
    );
    let absent = resolve(&mut tree, 2, 2, b"/dir/new", false, true).unwrap();
    assert!(absent.node.is_none());
    assert_eq!((absent.parent, absent.name.as_slice()), (3, &b"new"[..]));
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/absolute/", false, false).err(),
        Some(Error::NotDirectory)
    );
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/dir/value/..", true, false).err(),
        Some(Error::NotDirectory)
    );
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/dir/missing/sub", true, true).err(),
        Some(Error::Missing)
    );
}

#[test]
fn loops_and_invalid_targets_are_bounded_without_lexical_shortcuts() {
    let mut tree = Tree::new();
    tree.links.insert(6, b"link".to_vec());
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/link", true, false).err(),
        Some(Error::Loop)
    );
    assert_eq!(tree.reads, 40);
    tree.links.insert(6, b"".to_vec());
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/link", true, false).err(),
        Some(Error::Missing)
    );
    tree.links.insert(6, b"dir\0/value".to_vec());
    assert_eq!(
        resolve(&mut tree, 2, 2, b"/link", true, false).err(),
        Some(Error::Invalid)
    );
}

#[test]
fn deep_paths_keep_the_final_leaf_and_parent() {
    let mut tree = Chain { depth: 64 };
    let path = format!("{}/value", "/segment".repeat(tree.depth as usize));
    let result = resolve(&mut tree, 1, 1, path.as_bytes(), true, false);
    let found = result.unwrap();
    assert_eq!(found.path, path.as_bytes());
    assert_eq!(found.name, b"value");
    assert_eq!(found.parent, 65);
    assert_eq!(
        found.node,
        Some(Node {
            inode: 66,
            kind: Kind::File
        })
    );
}

#[test]
fn deep_absolute_paths_do_not_allocate_for_each_component() {
    let mut tree = Chain { depth: 128 };
    let path = format!("{}/value", "/segment".repeat(tree.depth as usize));
    ALLOCATIONS.with(|count| count.set(Some(0)));
    let result = resolve(&mut tree, 1, 1, path.as_bytes(), true, false);
    let allocations = ALLOCATIONS.with(|count| count.replace(None).unwrap());
    eprintln!("128-directory resolver allocations: {allocations}");
    let found = result.unwrap();
    assert_eq!(found.path, path.as_bytes());
    assert_eq!(found.name, b"value");
    // The filesystem stub allocates nothing. Geometric buffer growth is fine;
    // one heap allocation per component would exceed this budget by far.
    assert!(allocations <= 32, "resolver allocations: {allocations}");
}

#[test]
fn leaf_names_survive_symlinks_dots_and_missing_targets() {
    let mut tree = Tree::new();
    tree.add(2, b"long-directory-name", 11, Kind::Directory);
    tree.add(11, b"x", 12, Kind::Directory);
    tree.add(12, b"short", 13, Kind::Symlink);
    tree.links.insert(13, b"../../dir/value".to_vec());
    for path in [
        &b"/long-directory-name/x/short"[..],
        &b"/dir/./nested/../value"[..],
        &b"/absolute"[..],
    ] {
        let found = resolve(&mut tree, 2, 2, path, true, false).unwrap();
        assert_eq!(
            found.node,
            Some(Node {
                inode: 5,
                kind: Kind::File
            })
        );
        assert_eq!(found.path, b"/dir/value");
        assert_eq!(found.name, b"value");
        assert_eq!(found.parent, 3);
    }
    let missing = resolve(
        &mut tree,
        2,
        2,
        b"/long-directory-name/x/../../dir/z",
        false,
        true,
    )
    .unwrap();
    assert_eq!(missing.name, b"z");
    assert_eq!(missing.path, b"/dir/z");
    assert_eq!(missing.parent, 3);
    assert_eq!(missing.node, None);
    let root = resolve(&mut tree, 2, 2, b"/dir/..", true, false).unwrap();
    assert_eq!(root.path, b"/");
    assert!(root.name.is_empty());
}

#[test]
fn deferred_leaf_name_survives_replaced_inputs_that_end_without_a_new_name() {
    let mut tree = Tree::new();
    for (parent, name, inode, target) in [
        (3, &b"root-link"[..], 20, &b"/"[..]),
        (3, &b"dot-link"[..], 21, &b"."[..]),
        (3, &b"nested-link"[..], 22, &b"root-link"[..]),
        (2, &b"again"[..], 23, &b"/./"[..]),
    ] {
        tree.add(parent, name, inode, Kind::Symlink);
        tree.links.insert(inode, target.to_vec());
    }
    // Preserve the existing parent/name result even when an absolute link
    // resets canonical path without supplying another ordinary component.
    for (path, canonical, name, parent, inode) in [
        (&b"/dir/root-link"[..], &b"/"[..], &b"dir"[..], 3, 2),
        (&b"/dir/nested-link"[..], &b"/"[..], &b"dir"[..], 3, 2),
        (&b"/dir/dot-link"[..], &b"/dir"[..], &b"dir"[..], 3, 3),
        (&b"/dir/root-link/again"[..], &b"/"[..], &b"dir"[..], 2, 2),
        (&b"/dir/dot-link/.."[..], &b"/"[..], &b""[..], 2, 2),
        (&b"/dir/dot-link/nested/."[..], &b"/dir/nested"[..], &b"nested"[..], 3, 4),
    ] {
        let found = resolve(&mut tree, 2, 2, path, true, false).unwrap();
        assert_eq!(found.path, canonical);
        assert_eq!(found.name, name);
        assert_eq!(found.parent, parent);
        assert_eq!(found.node.unwrap().inode, inode);
    }
    let mut found = resolve(&mut tree, 2, 2, b"/dir/new", true, true).unwrap();
    assert_eq!(found.name, b"new");
    found.path.fill(b'x');
    tree.links.clear();
    assert_eq!(found.name, b"new");
    let nofollow = resolve(&mut tree, 2, 2, b"/dir/root-link", false, false).unwrap();
    assert_eq!(nofollow.name, b"root-link");
    assert_eq!(nofollow.node.unwrap().kind, Kind::Symlink);
}

#[test]
fn reconstructed_directory_path_accepts_the_limit_and_rejects_longer_results() {
    let mut tree = Chain { depth: 512 };
    let found = resolve(&mut tree, 1, 512, b"segment", true, false).unwrap();
    assert_eq!(found.path.len(), path_walk::PATH_MAX);
    assert_eq!(found.name, b"segment");
    assert_eq!(
        resolve(&mut tree, 1, 513, b"value", true, false).err(),
        Some(Error::TooLong)
    );
}
