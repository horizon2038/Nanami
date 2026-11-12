//! The synchronous, read-only filesystem adapter used by namespace resolution.
use super::*;

pub(super) fn resolve(
    runtime: &mut Ext2Runtime, root: u32, start: u32, path: &[u8],
    follow: bool, allow_missing: bool, retain_leaf: bool, cacheable: bool,
) -> Result<(path_walk::Resolved, Option<Ext2Inode>), Error> {
    if cacheable && !retain_leaf {
        if let Some(result) = runtime.resolved_paths.load(root, start, path, follow) {
            #[cfg(not(test))]
            if option_env!("NANAMI_RESOLVED_PATH_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static HITS: AtomicUsize = AtomicUsize::new(0);
                if HITS.fetch_add(1, Ordering::Relaxed) < 16 {
                    nanami_runtime::println!("[ext2.path-reuse] root={} start={} bytes={}", root, start, path.len());
                }
            }
            // Namespace operations that consume inode metadata still read it
            // now. Regular-file writes can change size/version without changing
            // topology or invalidating directory iteration.
            return Ok((result, None));
        }
        if let Some((mut parent, suffix, error_prefix)) = runtime.resolved_paths.parent_with_origin(root, start, path, follow) {
            #[cfg(not(test))]
            if suffix.contains(&b'/') && option_env!("NANAMI_CANONICAL_PREFIX_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static HITS: AtomicUsize = AtomicUsize::new(0);
                let count = HITS.fetch_add(1, Ordering::Relaxed) + 1;
                if count <= 8 || count.is_power_of_two() {
                    nanami_runtime::println!("[ext2.prefix-reuse] count={} root={} parent={} path={}",
                        count, root, parent, core::str::from_utf8(path).unwrap_or("<non-utf8>"));
                }
            }
            let mut disk = Disk::new(runtime, Node { inode: parent, kind: Kind::Directory });
            if error_prefix && option_env!("NANAMI_ERROR_PREFIX_TRACE") == Some("1") {
                disk.prefix_reuse = Some((parent, path.len() - suffix.len(), path.len()));
            }
            disk.retain_error_prefixes(root, follow);
            let mut consumed = path.len() - suffix.len();
            let mut last_directory = None;
            let mut names = suffix.split(|&byte| byte == b'/').peekable();
            let node = loop {
                let name = names.next().unwrap(); // Cache returns a nonempty canonical suffix.
                let name_end = consumed + name.len();
                consumed = name_end + 1;
                let final_component = names.peek().is_none();
                match disk.lookup(parent, name) {
                    // New links require the complete original ancestor and
                    // 40-link state, including links in intermediate suffixes.
                    Ok(node) if node.kind == Kind::Symlink && (follow || !final_component) => break None,
                    Ok(node) if final_component => break Some(Some(node)),
                    Ok(node) if node.kind == Kind::Directory => {
                        last_directory = Some((parent, node, name_end));
                        parent = node.inode;
                    }
                    Ok(_) => return Err(Error::NotDirectory),
                    Err(error) => {
                        if let Some((parent, node, end)) = last_directory {
                            disk.directory_resolved(parent, node, &path[..end]);
                        }
                        if error == Error::Missing && final_component && allow_missing {
                            break Some(None);
                        }
                        return Err(error);
                    }
                }
            };
            let inode = node.flatten().and_then(|node| disk.last_inode(node.inode));
            if node.is_none() {
                // A suffix link needs the complete ancestor/link-limit state;
                // this partial attempt is not a completed shortcut.
                disk.prefix_reuse = None;
            }
            if !matches!(node, Some(None)) {
                disk.discard_error_prefix();
            }
            drop(disk);
            if let Some(node) = node {
                let result = path_walk::Resolved {
                    node, parent, name: Vec::new(), path: path.to_vec(),
                };
                runtime.resolved_paths.store(root, start, path, follow, &result);
                #[cfg(not(test))]
                if option_env!("NANAMI_CANONICAL_PARENT_DIAG") == Some("1") {
                    use core::sync::atomic::{AtomicUsize, Ordering};
                    static HITS: AtomicUsize = AtomicUsize::new(0);
                    let count = HITS.fetch_add(1, Ordering::Relaxed) + 1;
                    if count <= 8 || count.is_power_of_two() {
                        nanami_runtime::println!("[ext2.parent-reuse] count={} root={} parent={} path={}",
                            count, root, parent, core::str::from_utf8(path).unwrap_or("<non-utf8>"));
                    }
                }
                return Ok((result, inode));
            }
        }
    }
    let (resolved, inode) = {
        let mut disk = Disk::new(runtime, Node { inode: root, kind: Kind::Directory });
        if cacheable && !retain_leaf {
            disk.retain_error_prefixes(root, follow);
        }
        let result = path_walk::resolve_with_leaf_name(
            &mut disk, root, start, path, follow, allow_missing, retain_leaf,
        );
        if result.as_ref().is_ok_and(|result| result.node.is_some()) {
            disk.discard_error_prefix();
        }
        let resolved = result?;
        let inode = resolved.node.and_then(|node| disk.last_inode(node.inode));
        (resolved, inode)
    };
    if cacheable && !retain_leaf { runtime.resolved_paths.store(root, start, path, follow, &resolved); }
    Ok((resolved, inode))
}

pub(super) struct Disk<'a> {
    runtime: &'a mut Ext2Runtime,
    // An owned value from the current walk, valid until it returns to dispatch.
    previous: Option<(Node, Option<Ext2Inode>)>,
    prefix_context: Option<(u32, bool)>,
    prefix: Option<DirectoryPrefix>,
    prefix_reuse: Option<(u32, usize, usize)>,
}

struct DirectoryPrefix {
    parent: u32,
    directory: Node,
    path: [u8; super::resolved_path::INPUT_BYTES],
    len: usize,
}

impl Drop for Disk<'_> {
    fn drop(&mut self) {
        #[cfg(not(test))]
        if option_env!("NANAMI_ERROR_PREFIX_TRACE") == Some("1") {
            if let Some((directory, prefix, bytes)) = self.prefix_reuse {
                nanami_runtime::print!("[ext2.error-prefix-hit] directory={} prefix-bytes={} path-bytes={}\n",
                    directory, prefix, bytes);
            }
        }
        if let (Some((root, follow)), Some(prefix)) = (self.prefix_context, self.prefix.as_ref()) {
            self.runtime.resolved_paths.store_directory(root, follow, prefix.parent,
                prefix.directory, &prefix.path[..prefix.len]);
        }
    }
}

pub(super) fn kind(inode: Ext2Inode) -> Kind {
    match inode.mode & EXT2_S_IFMT {
        EXT2_S_IFDIR => Kind::Directory,
        EXT2_S_IFLNK => Kind::Symlink,
        EXT2_S_IFSOCK => Kind::Socket,
        _ => Kind::File,
    }
}

impl<'a> Disk<'a> {
    /// Seed only from an inode already validated in this synchronous request.
    pub(super) fn new(runtime: &'a mut Ext2Runtime, known: Node) -> Self {
        Self { runtime, previous: Some((known, None)), prefix_context: None, prefix: None, prefix_reuse: None }
    }

    fn retain_error_prefixes(&mut self, root: u32, follow: bool) {
        self.prefix_context = Some((root, follow));
    }

    fn discard_error_prefix(&mut self) {
        self.prefix_context = None;
    }

    pub(super) fn last_inode(&self, number: u32) -> Option<Ext2Inode> {
        self.previous
            .filter(|(node, _)| node.inode == number)
            .and_then(|(_, inode)| inode)
    }

    fn directory_inode(runtime: &mut Ext2Runtime, number: u32) -> Result<Ext2Inode, Error> {
        let inode = read_inode(runtime, number).map_err(|_| Error::Io)?;
        if kind(inode) != Kind::Directory { return Err(Error::NotDirectory); }
        Ok(inode)
    }
}

impl Filesystem for Disk<'_> {
    fn directory_resolved(&mut self, parent: u32, directory: Node, path: &[u8]) {
        if self.prefix_context.is_none() || directory.kind != Kind::Directory
            || path.len() > super::resolved_path::INPUT_BYTES
            || self.prefix.as_ref().is_some_and(|prefix| prefix.len >= path.len())
        { return; }
        let mut prefix = DirectoryPrefix { parent, directory,
            path: [0; super::resolved_path::INPUT_BYTES], len: path.len() };
        prefix.path[..path.len()].copy_from_slice(path);
        self.prefix = Some(prefix);
    }
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error> {
        let parent = if let Some((known, inode)) = self.previous.take().filter(|(node, _)| node.inode == directory) {
            if known.kind != Kind::Directory { return Err(Error::NotDirectory); }
            inode
        } else { Some(Self::directory_inode(self.runtime, directory)?) };
        if name.len() > EXT2_NAME_LEN_MAX {
            return Err(Error::TooLong);
        }
        let inode = unsafe {
            if let Some(parent) = parent {
                super::directory_lookup::lookup_in_directory_with_inode(
                    self.runtime, directory, parent, name.as_ptr(), name.len())
            } else {
                super::directory_lookup::lookup_in_directory_with_loader(
                    self.runtime, directory, name.as_ptr(), name.len(),
                    |runtime| Self::directory_inode(runtime, directory))?
            }
        }.ok_or(Error::Missing)?;
        let record = read_inode(self.runtime, inode).map_err(|_| Error::Io)?;
        let node = Node { inode, kind: kind(record) };
        self.previous = Some((node, Some(record)));
        Ok(node)
    }
    fn read_link(&mut self, inode: u32) -> Result<Vec<u8>, Error> {
        self.previous = None;
        read_link(self.runtime, inode)
    }
    fn parent_name(&mut self, directory: u32) -> Result<(u32, Vec<u8>), Error> {
        let parent = self.lookup(directory, b"..")?.inode;
        let name = read_parent_name(self.runtime, parent, directory)?;
        Ok((parent, name))
    }
}

