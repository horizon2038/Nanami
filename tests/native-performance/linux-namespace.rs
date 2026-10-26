extern crate alloc;
use std::collections::BTreeMap;
#[path = "../../nanami/servers/core-services/ext2-server/src/path_walk.rs"]
mod path_walk;
use path_walk::{resolve, Error, Filesystem, Kind, Node};

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
