//! Frozen pre-optimization namespace adapter for differential tests.
use super::*;

pub(super) struct Disk<'a>(&'a mut Ext2Runtime);

fn kind(inode: Ext2Inode) -> Kind {
    match inode.mode & EXT2_S_IFMT {
        EXT2_S_IFDIR => Kind::Directory,
        EXT2_S_IFLNK => Kind::Symlink,
        EXT2_S_IFSOCK => Kind::Socket,
        _ => Kind::File,
    }
}

impl<'a> Disk<'a> {
    pub(super) fn new(runtime: &'a mut Ext2Runtime, _: Node) -> Self { Self(runtime) }
}

impl Filesystem for Disk<'_> {
    fn lookup(&mut self, directory: u32, name: &[u8]) -> Result<Node, Error> {
        let parent = read_inode(self.0, directory).map_err(|_| Error::Io)?;
        if kind(parent) != Kind::Directory {
            return Err(Error::NotDirectory);
        }
        if name.len() > EXT2_NAME_LEN_MAX {
            return Err(Error::TooLong);
        }
        let inode = unsafe { super::directory_lookup::lookup_in_directory_with_inode(
            self.0, directory, parent, name.as_ptr(), name.len()) }
            .ok_or(Error::Missing)?;
        Ok(Node {
            inode,
            kind: kind(read_inode(self.0, inode).map_err(|_| Error::Io)?),
        })
    }
    fn read_link(&mut self, inode: u32) -> Result<Vec<u8>, Error> {
        read_link(self.0, inode)
    }
    fn parent_name(&mut self, directory: u32) -> Result<(u32, Vec<u8>), Error> {
        let parent = self.lookup(directory, b"..")?.inode;
        let name = read_parent_name(self.0, parent, directory)?;
        Ok((parent, name))
    }
}

