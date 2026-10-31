//! A deterministic ext2 revision-1 image builder. No host filesystem tools are
//! invoked. The image uses 1 KiB blocks, 128-byte inodes and classic block maps
//! (direct, single and double indirect), as supported by Nanami's ext2 server.
//! See <https://www.kernel.org/doc/html/latest/filesystems/ext4/overview.html>.

use std::collections::BTreeMap;
use std::io::{self, Write};

pub const BLOCK_SIZE: usize = 1024;
pub const MAX_FILE_BYTES: usize = (12 + 256 + 256 * 256) * BLOCK_SIZE;
const BLOCKS_PER_GROUP: usize = 8192;
const INODES_PER_GROUP: usize = 1024;
const INODE_SIZE: usize = 128;
const DIR: u16 = 0x4000;
const FILE: u16 = 0x8000;
const SYMLINK: u16 = 0xa000;

#[derive(Clone, Debug)]
enum Contents {
    Directory,
    File(Vec<u8>),
    Symlink(Vec<u8>),
}

#[derive(Clone, Debug)]
struct Node {
    inode: u32,
    parent: usize,
    mode: u16,
    links: u16,
    contents: Contents,
    entries: Vec<(String, usize)>,
}

/// Paths are guest paths using `/`, independent of the host's path separator.
/// Parents must exist. Existing entries, traversal and directory hard links are
/// rejected. Insertion order controls inode/data layout, making builds repeatable.
///
/// ```
/// let mut fs = ext2_image::ImageBuilder::new(8 * 1024 * 1024)?;
/// fs.add_directory("/bin", 0o755)?;
/// fs.add_file("/bin/hello", 0o755, b"payload".to_vec())?;
/// fs.add_hard_link("/bin/alias", "/bin/hello")?;
/// let bytes = fs.build()?;
/// assert_eq!(bytes.len(), 8 * 1024 * 1024);
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ImageBuilder {
    size: usize,
    nodes: Vec<Node>,
    paths: BTreeMap<String, usize>,
    label: [u8; 16],
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

impl ImageBuilder {
    pub fn new(size_bytes: u64) -> io::Result<Self> {
        let size = usize::try_from(size_bytes).map_err(|_| invalid("image is too large"))?;
        if size < 1024 * 1024 || size % BLOCK_SIZE != 0 || size / BLOCK_SIZE > u32::MAX as usize {
            return Err(invalid(
                "size must be at least 1 MiB and a multiple of 1024 bytes",
            ));
        }
        Ok(Self {
            size,
            nodes: vec![Node {
                inode: 2,
                parent: 0,
                mode: DIR | 0o755,
                links: 2,
                contents: Contents::Directory,
                entries: Vec::new(),
            }],
            paths: BTreeMap::from([("/".into(), 0)]),
            label: [0; 16],
        })
    }

    pub fn set_label(&mut self, label: &str) -> io::Result<()> {
        if label.len() > 16 || label.as_bytes().contains(&0) {
            return Err(invalid("ext2 label must be at most 16 bytes without NUL"));
        }
        self.label = [0; 16];
        self.label[..label.len()].copy_from_slice(label.as_bytes());
        Ok(())
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains_key(path)
    }
    pub fn is_directory(&self, path: &str) -> bool {
        self.paths
            .get(path)
            .is_some_and(|&i| matches!(self.nodes[i].contents, Contents::Directory))
    }

    pub fn add_directory(&mut self, path: &str, permissions: u16) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        let links = self.nodes[parent]
            .links
            .checked_add(1)
            .ok_or_else(|| invalid("too many subdirectories"))?;
        self.insert(
            path,
            parent,
            name,
            DIR | (permissions & 0o7777),
            Contents::Directory,
            2,
        )?;
        self.nodes[parent].links = links;
        Ok(())
    }

    pub fn add_file(&mut self, path: &str, permissions: u16, data: Vec<u8>) -> io::Result<()> {
        if data.len() > MAX_FILE_BYTES {
            return Err(invalid(format!(
                "file exceeds double-indirect limit: {path}"
            )));
        }
        let (parent, name) = self.parent(path)?;
        self.insert(
            path,
            parent,
            name,
            FILE | (permissions & 0o7777),
            Contents::File(data),
            1,
        )
    }

    pub fn add_hard_link(&mut self, path: &str, target: &str) -> io::Result<()> {
        let (parent, name) = self.parent(path)?;
        let &index = self
            .paths
            .get(target)
            .ok_or_else(|| invalid(format!("missing link target: {target}")))?;
        if !matches!(self.nodes[index].contents, Contents::File(_)) {
            return Err(invalid("only regular files may be hard linked"));
        }
        self.nodes[index].links = self.nodes[index]
            .links
            .checked_add(1)
            .ok_or_else(|| invalid("too many hard links"))?;
        self.nodes[parent].entries.push((name, index));
        self.paths.insert(path.into(), index);
        Ok(())
    }

    /// Store the guest's link text verbatim; never resolve it on the build host.
    pub fn add_symlink(&mut self, path: &str, target: &str) -> io::Result<()> {
        if target.is_empty() || target.as_bytes().contains(&0) || target.len() > 4095 {
            return Err(invalid("invalid symlink target"));
        }
        let (parent, name) = self.parent(path)?;
        self.insert(
            path,
            parent,
            name,
            SYMLINK | 0o777,
            Contents::Symlink(target.as_bytes().to_vec()),
            1,
        )
    }

    fn parent(&self, path: &str) -> io::Result<(usize, String)> {
        if !path.starts_with('/') || path == "/" || path.ends_with('/') || self.contains(path) {
            return Err(invalid(format!("invalid or duplicate guest path: {path}")));
        }
        for name in path[1..].split('/') {
            if name.is_empty()
                || name == "."
                || name == ".."
                || name.len() > 255
                || name.as_bytes().contains(&0)
            {
                return Err(invalid(format!("invalid guest filename: {path}")));
            }
        }
        let (parent, name) = path.rsplit_once('/').unwrap();
        let parent = if parent.is_empty() { "/" } else { parent };
        let &index = self
            .paths
            .get(parent)
            .ok_or_else(|| invalid(format!("parent does not exist: {parent}")))?;
        if !matches!(self.nodes[index].contents, Contents::Directory) {
            return Err(invalid("parent is not a directory"));
        }
        Ok((index, name.into()))
    }

    fn insert(
        &mut self,
        path: &str,
        parent: usize,
        name: String,
        mode: u16,
        contents: Contents,
        links: u16,
    ) -> io::Result<()> {
        let inode = u32::try_from(self.nodes.len() + 10).map_err(|_| invalid("too many inodes"))?;
        let index = self.nodes.len();
        self.nodes.push(Node {
            inode,
            parent,
            mode,
            links,
            contents,
            entries: Vec::new(),
        });
        self.nodes[parent].entries.push((name, index));
        self.paths.insert(path.into(), index);
        Ok(())
    }

    pub fn build(self) -> io::Result<Vec<u8>> {
        let mut disk = Disk::new(self.size)?;
        for node in &self.nodes {
            disk.use_inode(node.inode, matches!(node.contents, Contents::Directory))?;
            let data = match &node.contents {
                Contents::File(data) | Contents::Symlink(data) => data.clone(),
                Contents::Directory => directory_data(node, &self.nodes),
            };
            disk.write_node(node, &data)?;
        }
        disk.finish(self.label);
        Ok(disk.bytes)
    }

    pub fn write_to<W: Write>(self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.build()?)
    }
}

fn directory_data(node: &Node, nodes: &[Node]) -> Vec<u8> {
    let mut records = vec![(".", node.inode, 2u8), ("..", nodes[node.parent].inode, 2)];
    records.extend(node.entries.iter().map(|(name, i)| {
        (
            name.as_str(),
            nodes[*i].inode,
            match nodes[*i].mode & 0xf000 {
                DIR => 2,
                SYMLINK => 7,
                _ => 1,
            },
        )
    }));
    let mut result = vec![0; BLOCK_SIZE];
    let mut cursor = 0;
    let mut last = 0;
    for (name, inode, kind) in records {
        let len = (8 + name.len() + 3) & !3;
        let in_block = cursor % BLOCK_SIZE;
        if in_block + len > BLOCK_SIZE {
            put16(
                &mut result,
                last + 4,
                (BLOCK_SIZE - last % BLOCK_SIZE) as u16,
            );
            cursor = result.len();
            result.resize(result.len() + BLOCK_SIZE, 0);
        }
        put32(&mut result, cursor, inode);
        put16(&mut result, cursor + 4, len as u16);
        result[cursor + 6] = name.len() as u8;
        result[cursor + 7] = kind;
        result[cursor + 8..cursor + 8 + name.len()].copy_from_slice(name.as_bytes());
        last = cursor;
        cursor += len;
        // A full block requires a fresh block for the next entry.
        if cursor == result.len() {
            result.resize(result.len() + BLOCK_SIZE, 0);
        }
    }
    if cursor % BLOCK_SIZE == 0 {
        result.truncate(cursor);
    } else {
        put16(
            &mut result,
            last + 4,
            (BLOCK_SIZE - last % BLOCK_SIZE) as u16,
        );
    }
    result
}

#[derive(Clone)]
struct Group {
    start: usize,
    count: usize,
    block_bitmap: usize,
    inode_bitmap: usize,
    inode_table: usize,
    next: usize,
    used_blocks: usize,
    used_inodes: usize,
    directories: u16,
}

struct Disk {
    bytes: Vec<u8>,
    groups: Vec<Group>,
    gdt_blocks: usize,
    alloc_group: usize,
}

impl Disk {
    fn new(size: usize) -> io::Result<Self> {
        let blocks = size / BLOCK_SIZE;
        let group_count = (blocks - 1).div_ceil(BLOCKS_PER_GROUP);
        if group_count > u16::MAX as usize {
            return Err(invalid("too many block groups"));
        }
        let gdt_blocks = (group_count * 32).div_ceil(BLOCK_SIZE);
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|e| invalid(format!("cannot allocate image: {e}")))?;
        bytes.resize(size, 0);
        let mut disk = Self {
            bytes,
            groups: Vec::new(),
            gdt_blocks,
            alloc_group: 0,
        };
        for group in 0..group_count {
            // s_first_data_block = 1 is part of the bitmap addressing contract.
            let start = 1 + group * BLOCKS_PER_GROUP;
            let count = (blocks - start).min(BLOCKS_PER_GROUP);
            let block_bitmap = start + 1 + gdt_blocks;
            let inode_bitmap = block_bitmap + 1;
            let inode_table = inode_bitmap + 1;
            let next = inode_table + INODES_PER_GROUP * INODE_SIZE / BLOCK_SIZE;
            if next > start + count {
                return Err(invalid("last block group is too small for metadata"));
            }
            let used_blocks = next - start;
            for bit in 0..BLOCKS_PER_GROUP {
                if bit < used_blocks || bit >= count {
                    set_bit(&mut disk.bytes, block_bitmap * BLOCK_SIZE, bit);
                }
            }
            // Inode bitmap padding is allocated too; otherwise fsck reports it.
            for bit in INODES_PER_GROUP..BLOCKS_PER_GROUP {
                set_bit(&mut disk.bytes, inode_bitmap * BLOCK_SIZE, bit);
            }
            let used_inodes = if group == 0 { 10 } else { 0 };
            for bit in 0..used_inodes {
                set_bit(&mut disk.bytes, inode_bitmap * BLOCK_SIZE, bit);
            }
            disk.groups.push(Group {
                start,
                count,
                block_bitmap,
                inode_bitmap,
                inode_table,
                next,
                used_blocks,
                used_inodes,
                directories: 0,
            });
        }
        Ok(disk)
    }

    fn use_inode(&mut self, inode: u32, directory: bool) -> io::Result<()> {
        let index = inode as usize - 1;
        let group = self
            .groups
            .get_mut(index / INODES_PER_GROUP)
            .ok_or_else(|| invalid("not enough inodes in image"))?;
        if inode != 2 {
            set_bit(
                &mut self.bytes,
                group.inode_bitmap * BLOCK_SIZE,
                index % INODES_PER_GROUP,
            );
            group.used_inodes += 1;
        }
        if directory {
            group.directories += 1;
        }
        Ok(())
    }

    fn alloc(&mut self) -> io::Result<u32> {
        while let Some(group) = self.groups.get_mut(self.alloc_group) {
            if group.next < group.start + group.count {
                let block = group.next;
                group.next += 1;
                group.used_blocks += 1;
                set_bit(
                    &mut self.bytes,
                    group.block_bitmap * BLOCK_SIZE,
                    block - group.start,
                );
                return Ok(block as u32);
            }
            self.alloc_group += 1;
        }
        Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "image is too small for filesystem contents",
        ))
    }

    fn write_node(&mut self, node: &Node, data: &[u8]) -> io::Result<()> {
        if data.len() > MAX_FILE_BYTES {
            return Err(invalid("inode exceeds double-indirect block mapping"));
        }
        // ext2 fast symlinks occupy i_block directly, without allocated blocks.
        let inline_link = matches!(node.contents, Contents::Symlink(_)) && data.len() < 60;
        let block_data = if inline_link { &[][..] } else { data };
        let mut blocks = Vec::new();
        for chunk in block_data.chunks(BLOCK_SIZE) {
            let block = self.alloc()?;
            let offset = block as usize * BLOCK_SIZE;
            self.bytes[offset..offset + chunk.len()].copy_from_slice(chunk);
            blocks.push(block);
        }
        let mut pointers = [0; 15];
        let direct = blocks.len().min(12);
        pointers[..direct].copy_from_slice(&blocks[..direct]);
        let mut metadata = 0;
        if blocks.len() > 12 {
            pointers[12] = self.pointer_block(&blocks[12..blocks.len().min(268)])?;
            metadata += 1;
        }
        if blocks.len() > 268 {
            let mut second = Vec::new();
            for chunk in blocks[268..].chunks(256) {
                second.push(self.pointer_block(chunk)?);
                metadata += 1;
            }
            pointers[13] = self.pointer_block(&second)?;
            metadata += 1;
        }
        let index = node.inode as usize - 1;
        let group = &self.groups[index / INODES_PER_GROUP];
        let offset = group.inode_table * BLOCK_SIZE + index % INODES_PER_GROUP * INODE_SIZE;
        put16(&mut self.bytes, offset, node.mode);
        put32(&mut self.bytes, offset + 4, data.len() as u32);
        put16(&mut self.bytes, offset + 26, node.links);
        put32(
            &mut self.bytes,
            offset + 28,
            ((blocks.len() + metadata) * 2) as u32,
        );
        if inline_link {
            self.bytes[offset + 40..offset + 40 + data.len()].copy_from_slice(data);
        } else {
            for (i, block) in pointers.into_iter().enumerate() {
                put32(&mut self.bytes, offset + 40 + i * 4, block);
            }
        }
        Ok(())
    }

    fn pointer_block(&mut self, blocks: &[u32]) -> io::Result<u32> {
        let block = self.alloc()?;
        for (i, pointer) in blocks.iter().enumerate() {
            put32(
                &mut self.bytes,
                block as usize * BLOCK_SIZE + i * 4,
                *pointer,
            );
        }
        Ok(block)
    }

    fn finish(&mut self, label: [u8; 16]) {
        let blocks = self.bytes.len() / BLOCK_SIZE;
        let inodes = self.groups.len() * INODES_PER_GROUP;
        let free_blocks = self
            .groups
            .iter()
            .map(|g| g.count - g.used_blocks)
            .sum::<usize>();
        let free_inodes = self
            .groups
            .iter()
            .map(|g| INODES_PER_GROUP - g.used_inodes)
            .sum::<usize>();
        let mut superblock = vec![0; BLOCK_SIZE];
        for (offset, value) in [
            (0, inodes),
            (4, blocks),
            (12, free_blocks),
            (16, free_inodes),
            (20, 1),
            (32, BLOCKS_PER_GROUP),
            (36, BLOCKS_PER_GROUP),
            (40, INODES_PER_GROUP),
            (76, 1),
            (84, 11),
            (96, 2),
        ] {
            put32(&mut superblock, offset, value as u32);
        }
        for (offset, value) in [
            (54, 0xffff),
            (56, 0xef53),
            (58, 1),
            (60, 1),
            (88, INODE_SIZE as u16),
        ] {
            put16(&mut superblock, offset, value);
        }
        superblock[104..120].copy_from_slice(b"RustExt2Image001");
        superblock[120..136].copy_from_slice(&label);
        let mut descriptors = vec![0; self.gdt_blocks * BLOCK_SIZE];
        for (i, g) in self.groups.iter().enumerate() {
            let offset = i * 32;
            put32(&mut descriptors, offset, g.block_bitmap as u32);
            put32(&mut descriptors, offset + 4, g.inode_bitmap as u32);
            put32(&mut descriptors, offset + 8, g.inode_table as u32);
            put16(
                &mut descriptors,
                offset + 12,
                (g.count - g.used_blocks) as u16,
            );
            put16(
                &mut descriptors,
                offset + 14,
                (INODES_PER_GROUP - g.used_inodes) as u16,
            );
            put16(&mut descriptors, offset + 16, g.directories);
        }
        // No sparse_super feature: every group contains both backup structures.
        for (i, g) in self.groups.iter().enumerate() {
            put16(&mut superblock, 90, i as u16);
            let offset = g.start * BLOCK_SIZE;
            self.bytes[offset..offset + BLOCK_SIZE].copy_from_slice(&superblock);
            self.bytes[offset + BLOCK_SIZE..offset + BLOCK_SIZE + descriptors.len()]
                .copy_from_slice(&descriptors);
        }
    }
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn set_bit(bytes: &mut [u8], offset: usize, bit: usize) {
    bytes[offset + bit / 8] |= 1 << (bit % 8);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn u32_at(bytes: &[u8], off: usize) -> usize {
        u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize
    }
    fn inode(bytes: &[u8], number: usize) -> &[u8] {
        let group = (number - 1) / INODES_PER_GROUP;
        let table = u32_at(bytes, 2 * BLOCK_SIZE + group * 32 + 8);
        let off = table * BLOCK_SIZE + (number - 1) % INODES_PER_GROUP * INODE_SIZE;
        &bytes[off..off + INODE_SIZE]
    }
    fn read_file(bytes: &[u8], number: usize) -> Vec<u8> {
        let node = inode(bytes, number);
        let size = u32_at(node, 4);
        let mut result = Vec::new();
        for index in 0..size.div_ceil(BLOCK_SIZE) {
            let block = if index < 12 {
                u32_at(node, 40 + index * 4)
            } else if index < 268 {
                u32_at(bytes, u32_at(node, 88) * BLOCK_SIZE + (index - 12) * 4)
            } else {
                let offset = index - 268;
                let second = u32_at(bytes, u32_at(node, 92) * BLOCK_SIZE + offset / 256 * 4);
                u32_at(bytes, second * BLOCK_SIZE + offset % 256 * 4)
            };
            result.extend_from_slice(&bytes[block * BLOCK_SIZE..(block + 1) * BLOCK_SIZE]);
        }
        result.truncate(size);
        result
    }
    #[test]
    fn files_round_trip_across_direct_indirect_and_groups() {
        let mut builder = ImageBuilder::new(16 * 1024 * 1024).unwrap();
        let data: Vec<_> = (0..9 * 1024 * 1024 + 23).map(|i| (i % 251) as u8).collect();
        builder.add_file("/large", 0o755, data.clone()).unwrap();
        let bytes = builder.build().unwrap();
        assert_eq!(read_file(&bytes, 11), data);
        assert_eq!(u32_at(&bytes, 1024 + 20), 1);
        assert_eq!(u32_at(&bytes, 8193 * 1024 + 4), 16384);
    }
    #[test]
    fn hard_links_share_inodes_and_empty_files_have_no_blocks() {
        let mut builder = ImageBuilder::new(1024 * 1024).unwrap();
        builder.add_directory("/bin", 0o755).unwrap();
        builder.add_file("/bin/app", 0o755, Vec::new()).unwrap();
        builder.add_hard_link("/app", "/bin/app").unwrap();
        let bytes = builder.build().unwrap();
        assert_eq!(&inode(&bytes, 12)[26..28], &2u16.to_le_bytes());
        assert_eq!(u32_at(inode(&bytes, 12), 28), 0);
    }
    #[test]
    fn symlinks_preserve_guest_targets_inline_and_in_blocks() {
        let mut builder = ImageBuilder::new(2 * 1024 * 1024).unwrap();
        builder.add_symlink("/sh", "/bin/busybox").unwrap();
        let long = format!("../{}", "x".repeat(80));
        builder.add_symlink("/long", &long).unwrap();
        // The 60-byte boundary is block-backed, matching ext2-server's reader.
        builder.add_symlink("/boundary", &"x".repeat(60)).unwrap();
        assert!(builder.add_symlink("/empty", "").is_err());
        assert!(builder.add_symlink("/nul", "a\0b").is_err());
        assert!(builder.add_file("/sh/child", 0o644, vec![]).is_err());
        let image = builder.build().unwrap();
        let short = inode(&image, 11);
        assert_eq!(
            u16::from_le_bytes(short[..2].try_into().unwrap()),
            SYMLINK | 0o777
        );
        assert_eq!(u32_at(short, 28), 0);
        assert_eq!(&short[40..52], b"/bin/busybox");
        assert_eq!(read_file(&image, 12), long.as_bytes());
        assert_eq!(read_file(&image, 13), vec![b'x'; 60]);
        let root = read_file(&image, 2);
        let mut offset = 0;
        let mut links = 0;
        while offset < root.len() {
            links += usize::from(root[offset + 7] == 7);
            offset += u16::from_le_bytes(root[offset + 4..offset + 6].try_into().unwrap()) as usize;
        }
        assert_eq!(links, 3);
    }
    #[test]
    fn directories_may_span_multiple_blocks() {
        let mut builder = ImageBuilder::new(2 * 1024 * 1024).unwrap();
        for i in 0..200 {
            builder
                .add_file(&format!("/file-{i:03}"), 0o644, vec![])
                .unwrap();
        }
        let bytes = builder.build().unwrap();
        let directory = read_file(&bytes, 2);
        assert!(directory.len() > BLOCK_SIZE);
        let mut cursor = 0;
        let mut entries = 0;
        while cursor < directory.len() {
            let len =
                u16::from_le_bytes(directory[cursor + 4..cursor + 6].try_into().unwrap()) as usize;
            assert!(len >= 8 && cursor % BLOCK_SIZE + len <= BLOCK_SIZE);
            entries += 1;
            cursor += len;
        }
        assert_eq!(entries, 202);
    }
    #[test]
    fn invalid_paths_duplicates_and_capacity_fail() {
        let mut builder = ImageBuilder::new(1024 * 1024).unwrap();
        assert!(builder.add_file("/../escape", 0o644, vec![]).is_err());
        assert!(builder.add_file("/missing/file", 0o644, vec![]).is_err());
        builder
            .add_file("/data", 0o644, vec![0; 1024 * 1024])
            .unwrap();
        assert!(builder.add_directory("/data", 0o755).is_err());
        assert!(builder.build().is_err());
    }
    #[test]
    fn repeatable_and_bitmap_counts_match() {
        fn build() -> Vec<u8> {
            let mut b = ImageBuilder::new(16 * 1024 * 1024).unwrap();
            b.add_directory("/tmp", 0o777).unwrap();
            b.add_file("/tmp/text", 0o644, b"hello".to_vec()).unwrap();
            b.build().unwrap()
        }
        let bytes = build();
        assert_eq!(bytes, build());
        let mut free = 0;
        for group in 0..2 {
            let bitmap = u32_at(&bytes, 2048 + group * 32);
            let count = (bytes.len() / 1024 - (1 + group * 8192)).min(8192);
            for bit in 0..count {
                free += usize::from(bytes[bitmap * 1024 + bit / 8] & (1 << (bit % 8)) == 0);
            }
        }
        assert_eq!(free, u32_at(&bytes, 1024 + 12));
    }
}
