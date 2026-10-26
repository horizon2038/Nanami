//! Namespace-rooted pathname walking. Resolve links before interpreting `..`.
use alloc::vec::Vec;

pub const PATH_MAX: usize = 4096;
const SYMLINK_MAX: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Missing,
    NotDirectory,
    Loop,
    TooLong,
    Exists,
    Io,
    NoSpace,
    ReadOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Socket,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub inode: u32,
    pub kind: Kind,
}

pub trait Filesystem {
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error>;
    fn read_link(&mut self, inode: u32) -> Result<Vec<u8>, Error>;
    /// Resolve a directory's actual parent/name, including after rename.
    fn parent_name(&mut self, directory: u32) -> Result<(u32, Vec<u8>), Error>;
}

pub struct Resolved {
    pub node: Option<Node>,
    pub parent: u32,
    pub name: Vec<u8>,
    pub path: Vec<u8>,
}

pub fn resolve(
    fs: &mut impl Filesystem,
    root: u32,
    start: u32,
    input: &[u8],
    follow_final: bool,
    allow_missing: bool,
) -> Result<Resolved, Error> {
    if input.is_empty() || input.len() > PATH_MAX || input.contains(&0) {
        return Err(Error::Invalid);
    }
    let mut names = Vec::new();
    let mut ancestors = Vec::new();
    let mut current = if input[0] == b'/' { root } else { start };
    while current != root {
        if ancestors.len() == PATH_MAX || ancestors.contains(&current) {
            return Err(Error::Invalid);
        }
        ancestors.push(current);
        let (parent, name) = fs.parent_name(current)?;
        if parent == current {
            return Err(Error::Invalid);
        } // A dirfd outside the namespace.
        names.push(name);
        current = parent;
    }
    ancestors.push(root);
    ancestors.reverse();
    names.reverse();
    let mut remaining = input.to_vec();
    let mut cursor = 0;
    let mut links = 0;
    let mut node = Some(Node {
        inode: *ancestors.last().ok_or(Error::Invalid)?,
        kind: Kind::Directory,
    });
    let mut parent = node.unwrap().inode;
    let mut last_name = Vec::new();
    loop {
        while cursor < remaining.len() && remaining[cursor] == b'/' {
            cursor += 1;
        }
        if cursor == remaining.len() {
            break;
        }
        if node.is_none_or(|node| node.kind != Kind::Directory) {
            return Err(Error::NotDirectory);
        }
        let start = cursor;
        while cursor < remaining.len() && remaining[cursor] != b'/' {
            cursor += 1;
        }
        let name = &remaining[start..cursor];
        let final_component = remaining[cursor..].iter().all(|byte| *byte == b'/');
        let trailing_slash = cursor < remaining.len();
        if name == b"." {
            continue;
        }
        if name == b".." {
            if ancestors.len() > 1 {
                ancestors.pop();
                names.pop();
            }
            let directory = *ancestors.last().ok_or(Error::Invalid)?;
            node = Some(Node {
                inode: directory,
                kind: Kind::Directory,
            });
            parent = directory;
            last_name.clear();
            continue;
        }
        parent = node.unwrap().inode;
        node = match fs.lookup(parent, name) {
            Ok(found) => Some(found),
            Err(Error::Missing) if final_component && !trailing_slash && allow_missing => None,
            Err(error) => return Err(error),
        };
        if node.is_some_and(|node| node.kind == Kind::Symlink)
            && (follow_final || !final_component || trailing_slash)
        {
            links += 1;
            if links > SYMLINK_MAX {
                return Err(Error::Loop);
            }
            let mut target = fs.read_link(node.unwrap().inode)?;
            if target.is_empty() {
                return Err(Error::Missing);
            }
            if target.contains(&0) {
                return Err(Error::Invalid);
            }
            if target.len() + remaining.len() - cursor > PATH_MAX {
                return Err(Error::TooLong);
            }
            if target[0] == b'/' {
                ancestors.truncate(1);
                names.clear();
            }
            target.extend_from_slice(&remaining[cursor..]);
            remaining = target;
            cursor = 0;
            node = Some(Node {
                inode: *ancestors.last().ok_or(Error::Invalid)?,
                kind: Kind::Directory,
            });
            continue;
        }
        last_name = name.to_vec();
        names.push(last_name.clone());
        if let Some(found) = node {
            if found.kind == Kind::Directory {
                ancestors.push(found.inode);
            }
            if final_component && trailing_slash && found.kind != Kind::Directory {
                return Err(Error::NotDirectory);
            }
        }
    }
    let mut path = Vec::new();
    path.push(b'/');
    for (index, name) in names.iter().enumerate() {
        if index != 0 {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        if path.len() > PATH_MAX {
            return Err(Error::TooLong);
        }
    }
    Ok(Resolved {
        node,
        parent,
        name: last_name,
        path,
    })
}
