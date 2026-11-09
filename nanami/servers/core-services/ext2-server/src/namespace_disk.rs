//! The synchronous, read-only filesystem adapter used by namespace resolution.
use super::*;

pub(super) struct Disk<'a> {
    runtime: &'a mut Ext2Runtime,
    // Only the previous successful lookup's kind, never a resident cache slot.
    // This adapter performs no inode mutation or reentrant service dispatch.
    previous: Option<Node>,
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
        Self { runtime, previous: Some(known) }
    }

    fn directory_inode(runtime: &mut Ext2Runtime, number: u32) -> Result<Ext2Inode, Error> {
        let inode = read_inode(runtime, number).map_err(|_| Error::Io)?;
        if kind(inode) != Kind::Directory { return Err(Error::NotDirectory); }
        Ok(inode)
    }
}

impl Filesystem for Disk<'_> {
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error> {
        let parent = if let Some(known) = self.previous.take().filter(|node| node.inode == directory) {
            if known.kind != Kind::Directory { return Err(Error::NotDirectory); }
            None
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
        let node = Node {
            inode,
            kind: kind(read_inode(self.runtime, inode).map_err(|_| Error::Io)?),
        };
        self.previous = Some(node);
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

