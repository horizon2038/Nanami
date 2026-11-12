//! Namespace-rooted pathname walking. Resolve links before interpreting `..`.
use alloc::{borrow::Cow, vec::Vec};

pub const PATH_MAX: usize = 4096;
const SYMLINK_MAX: usize = 40;

// Request-local traversal history. Most paths fit inline; deeper paths retain
// the Vec behavior and the existing PATH_MAX checks, without a depth limit.
struct ComponentStack<T: Copy + Default + PartialEq> {
    inline: [T; 16],
    len: usize,
    spill: Option<Vec<T>>,
}

impl<T: Copy + Default + PartialEq> ComponentStack<T> {
    fn new() -> Self { Self { inline: [T::default(); 16], len: 0, spill: None } }
    fn len(&self) -> usize { self.spill.as_ref().map_or(self.len, Vec::len) }
    fn last(&self) -> Option<&T> { self.as_slice().last() }
    fn contains(&self, value: &T) -> bool { self.as_slice().contains(value) }
    fn as_slice(&self) -> &[T] { self.spill.as_deref().unwrap_or(&self.inline[..self.len]) }
    fn push(&mut self, value: T) {
        if let Some(spill) = self.spill.as_mut() { spill.push(value); }
        else if self.len < self.inline.len() {
            self.inline[self.len] = value;
            self.len += 1;
        } else {
            let mut spill = Vec::with_capacity(self.inline.len() * 2);
            spill.extend_from_slice(&self.inline);
            spill.push(value);
            self.spill = Some(spill);
        }
    }
    fn pop(&mut self) -> Option<T> {
        if let Some(spill) = self.spill.as_mut() { spill.pop() }
        else { self.len = self.len.checked_sub(1)?; Some(self.inline[self.len]) }
    }
    fn truncate(&mut self, len: usize) {
        if let Some(spill) = self.spill.as_mut() { spill.truncate(len); }
        else { self.len = self.len.min(len); }
    }
    fn clear(&mut self) { self.truncate(0); }
    fn reverse(&mut self) {
        if let Some(spill) = self.spill.as_mut() { spill.reverse(); }
        else { self.inline[..self.len].reverse(); }
    }
}

#[cfg(test)]
mod component_stack_tests {
    use super::*;
    #[test]
    fn inline_and_spilled_operations_match_vec_through_clear_and_reuse() {
        let mut stack = ComponentStack::<usize>::new();
        let mut reference = Vec::new();
        for round in 0..100 {
            for value in 0..(round % 35) {
                stack.push(value); reference.push(value);
                assert_eq!(stack.as_slice(), reference);
            }
            stack.reverse(); reference.reverse();
            assert_eq!(stack.as_slice(), reference);
            assert_eq!(stack.last(), reference.last());
            assert_eq!(stack.contains(&17), reference.contains(&17));
            assert_eq!(stack.pop(), reference.pop());
            stack.truncate(round % 20); reference.truncate(round % 20);
            assert_eq!(stack.as_slice(), reference);
            stack.clear(); reference.clear();
            assert_eq!(stack.pop(), None);
        }
        let mut inline = ComponentStack::<u32>::new();
        for value in 0..16 { inline.push(value); }
        assert!(inline.spill.is_none());
        inline.push(16);
        assert!(inline.spill.is_some());
    }
}

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
    Unsupported,
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
    /// A proven parent directory immediately before a failed lookup.
    /// Observers may retain its canonical name without caching the failure.
    fn directory_resolved(&mut self, _parent: u32, _directory: Node, _path: &[u8]) {}
}

pub struct Resolved {
    pub node: Option<Node>,
    pub parent: u32,
    pub name: Vec<u8>,
    pub path: Vec<u8>,
}

/// Resolve a reference-only request, which does not consume the final name.
pub fn resolve_reference(
    fs: &mut impl Filesystem, root: u32, start: u32, input: &[u8],
    follow_final: bool, allow_missing: bool,
) -> Result<Resolved, Error> {
    resolve_with_leaf_name(fs, root, start, input, follow_final, allow_missing, false)
}

pub fn resolve(
    fs: &mut impl Filesystem,
    root: u32,
    start: u32,
    input: &[u8],
    follow_final: bool,
    allow_missing: bool,
) -> Result<Resolved, Error> {
    resolve_with_leaf_name(fs, root, start, input, follow_final, allow_missing, true)
}

pub fn resolve_with_leaf_name(
    fs: &mut impl Filesystem, root: u32, start: u32, input: &[u8],
    follow_final: bool, allow_missing: bool, retain_leaf: bool,
) -> Result<Resolved, Error> {
    if input.is_empty() || input.len() > PATH_MAX || input.contains(&0) {
        return Err(Error::Invalid);
    }
    let mut names = Vec::new();
    let mut prefix_bytes = 0usize;
    let mut ancestors = ComponentStack::new();
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
        prefix_bytes = prefix_bytes.saturating_add(name.len()).saturating_add(1);
        names.push(name);
        current = parent;
    }
    ancestors.push(root);
    ancestors.reverse();
    names.reverse();
    // Keep canonical components in the result buffer itself. Offsets allow `..`
    // to remove a component without allocating a separate name for every lookup.
    let mut path = Vec::new();
    // Parent names are already known for relative paths. Include that prefix
    // in the one-time hint as well; links can still grow the result normally.
    // Reservation failure restores the original incremental growth path.
    let capacity = prefix_bytes.saturating_add(input.len())
        .saturating_add(usize::from(input[0] != b'/')).min(PATH_MAX);
    let _ = path.try_reserve_exact(capacity);
    path.push(b'/');
    let mut component_starts = ComponentStack::new();
    for name in names {
        component_starts.push(path.len());
        if component_starts.len() > 1 {
            path.push(b'/');
        }
        path.extend_from_slice(&name);
    }
    let mut remaining = Cow::Borrowed(input);
    let mut cursor = 0;
    let mut links = 0;
    let mut node = Some(Node {
        inode: *ancestors.last().ok_or(Error::Invalid)?,
        kind: Kind::Directory,
    });
    let mut parent = node.unwrap().inode;
    let mut last_name = Vec::new();
    // Keep the latest ordinary component in the current input until return or
    // a symlink replaces that input. Dot keeps it; dot-dot clears it.
    let mut pending_name: Option<(usize, usize)> = None;
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
                path.truncate(component_starts.pop().ok_or(Error::Invalid)?);
            }
            let directory = *ancestors.last().ok_or(Error::Invalid)?;
            node = Some(Node {
                inode: directory,
                kind: Kind::Directory,
            });
            parent = directory;
            last_name.clear();
            pending_name = None;
            continue;
        }
        parent = node.unwrap().inode;
        node = match fs.lookup(parent, name) {
            Ok(found) => Some(found),
            Err(error) => {
                let ancestors = ancestors.as_slice();
                let grandparent = ancestors.get(ancestors.len().saturating_sub(2)).copied().unwrap_or(root);
                fs.directory_resolved(grandparent, Node { inode: parent, kind: Kind::Directory }, &path);
                if error == Error::Missing && final_component && !trailing_slash && allow_missing {
                    None
                } else {
                    return Err(error);
                }
            }
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
                component_starts.clear();
                path.truncate(1);
            }
            target.extend_from_slice(&remaining[cursor..]);
            if let Some((start, end)) = pending_name.take() {
                last_name.clear();
                last_name.extend_from_slice(&remaining[start..end]);
            }
            remaining = Cow::Owned(target);
            cursor = 0;
            node = Some(Node {
                inode: *ancestors.last().ok_or(Error::Invalid)?,
                kind: Kind::Directory,
            });
            continue;
        }
        component_starts.push(path.len());
        if component_starts.len() > 1 {
            path.push(b'/');
        }
        path.extend_from_slice(name);
        // Reference operations consume only node/path. Do not allocate a
        // second owned copy of the final component unless the caller uses it.
        if retain_leaf {
            pending_name = Some((start, cursor));
        }
        if let Some(found) = node {
            if found.kind == Kind::Directory {
                ancestors.push(found.inode);
            }
            if final_component && trailing_slash && found.kind != Kind::Directory {
                return Err(Error::NotDirectory);
            }
        }
    }
    if path.len() > PATH_MAX {
        return Err(Error::TooLong);
    }
    if let Some((start, end)) = pending_name {
        last_name.clear();
        last_name.extend_from_slice(&remaining[start..end]);
    }
    Ok(Resolved {
        node,
        parent,
        name: last_name,
        path,
    })
}
