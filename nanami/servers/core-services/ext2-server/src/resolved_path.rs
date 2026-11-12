//! Positive namespace topology only; inode metadata is never retained here.
use super::path_walk::{Node, Resolved};
use alloc::vec::Vec;

const ENTRIES: usize = 8;
pub(super) const INPUT_BYTES: usize = 128;
const OUTPUT_BYTES: usize = 256;

#[derive(Clone, Copy)]
struct Entry {
    root: u32,
    start: u32,
    follow: bool,
    error_prefix: bool,
    node: Option<Node>,
    parent: u32,
    input_len: usize,
    output_len: usize,
    input: [u8; INPUT_BYTES],
    output: [u8; OUTPUT_BYTES],
}

impl Entry {
    const EMPTY: Self = Self {
        root: 0, start: 0, follow: false, error_prefix: false, node: None, parent: 0,
        input_len: 0, output_len: 0, input: [0; INPUT_BYTES], output: [0; OUTPUT_BYTES],
    };
}

pub(super) struct Cache {
    entries: [Entry; ENTRIES],
    next: usize,
}

const _: () = assert!(core::mem::size_of::<Cache>() <= 4096);

impl Cache {
    pub(super) const fn new() -> Self {
        Self { entries: [Entry::EMPTY; ENTRIES], next: 0 }
    }

    pub(super) fn invalidate(&mut self) {
        for entry in &mut self.entries { entry.node = None; }
        self.next = 0;
    }

    pub(super) fn load(&self, root: u32, start: u32, input: &[u8], follow: bool) -> Option<Resolved> {
        let entry = self.entries.iter().find(|entry| entry.node.is_some()
            && entry.root == root && entry.start == start && entry.follow == follow
            && entry.input_len == input.len() && entry.input[..entry.input_len] == *input)?;
        Some(Resolved { node: entry.node, parent: entry.parent, name: Vec::new(),
            path: entry.output[..entry.output_len].to_vec() })
    }

    /// Reuse the longest proven directory prefix of a canonical absolute path.
    /// Every remaining component still needs a fresh lookup and inode read.
    /// Dot components, repeated/trailing slashes and links in the new suffix
    /// retain the full walk's ancestor and symlink-limit state.
    pub(super) fn parent<'a>(&self, root: u32, start: u32, input: &'a [u8], follow: bool)
        -> Option<(u32, &'a [u8])> {
        self.parent_with_origin(root, start, input, follow)
            .map(|(parent, suffix, _)| (parent, suffix))
    }

    pub(super) fn parent_with_origin<'a>(&self, root: u32, start: u32, input: &'a [u8], follow: bool)
        -> Option<(u32, &'a [u8], bool)> {
        if input.first() != Some(&b'/') || input.len() > super::path_walk::PATH_MAX
            || input.contains(&0)
            || input[1..].split(|&byte| byte == b'/').any(|name|
                name.is_empty() || name == b"." || name == b".." || name.len() > 255)
        { return None; }
        let mut best = None;
        let mut consumed = 0;
        for entry in &self.entries {
            let Some(node) = entry.node else { continue; };
            if entry.root != root || entry.start != start || entry.follow != follow
                || entry.input_len != entry.output_len
                || entry.input[..entry.input_len] != entry.output[..entry.output_len]
                || entry.input.first() != Some(&b'/') { continue; }
            let old = &entry.input[..entry.input_len];
            let parent_end = old.iter().rposition(|&byte| byte == b'/').unwrap() + 1;
            if parent_end > consumed && input.starts_with(&old[..parent_end]) {
                best = Some((entry.parent, &input[parent_end..], entry.error_prefix));
                consumed = parent_end;
            }
            let directory_end = if old == b"/" { 1 } else { old.len() + 1 };
            if node.kind == super::path_walk::Kind::Directory && directory_end > consumed
                && input.starts_with(old) && (old == b"/" || input.get(old.len()) == Some(&b'/'))
                && directory_end < input.len()
            {
                best = Some((node.inode, &input[directory_end..], entry.error_prefix));
                consumed = directory_end;
            }
        }
        best
    }

    /// This exact canonical directory was reached before a later lookup failed.
    /// Retain only positive topology; neither the failed name nor metadata is saved.
    pub(super) fn store_directory(&mut self, root: u32, follow: bool,
        parent: u32, node: Node, path: &[u8]) -> bool {
        if node.kind != super::path_walk::Kind::Directory || path.len() <= 1
            || path.len() > INPUT_BYTES || path.first() != Some(&b'/')
        { return false; }
        if self.entries.iter().any(|entry| entry.node == Some(node)
            && entry.root == root && entry.start == root && entry.follow == follow
            && entry.parent == parent && entry.input_len == path.len()
            && entry.output_len == path.len() && entry.input[..path.len()] == *path
            && entry.output[..path.len()] == *path)
        { return false; }
        let entry = &mut self.entries[self.next];
        *entry = Entry { root, start: root, follow, error_prefix: true, node: Some(node), parent,
            input_len: path.len(), output_len: path.len(), ..Entry::EMPTY };
        entry.input[..path.len()].copy_from_slice(path);
        entry.output[..path.len()].copy_from_slice(path);
        self.next = (self.next + 1) % ENTRIES;
        true
    }

    /// Only a successful reference walk may populate this cache. Mutations
    /// invalidate it before their first directory/inode metadata write attempt.
    /// Missing results, long paths and callers consuming a leaf name bypass it.
    pub(super) fn store(&mut self, root: u32, start: u32, input: &[u8], follow: bool, result: &Resolved) {
        if result.node.is_none() || !result.name.is_empty() || input.is_empty()
            || input.len() > INPUT_BYTES || result.path.len() > OUTPUT_BYTES
        { return; }
        let entry = &mut self.entries[self.next];
        entry.root = root;
        entry.start = start;
        entry.follow = follow;
        entry.error_prefix = false;
        entry.node = result.node;
        entry.parent = result.parent;
        entry.input_len = input.len();
        entry.output_len = result.path.len();
        entry.input[..input.len()].copy_from_slice(input);
        entry.output[..result.path.len()].copy_from_slice(&result.path);
        self.next = (self.next + 1) % ENTRIES;
    }
}
