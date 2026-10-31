//! VFS-owned, coherent read-only file pages. MAP_SHARED never becomes a private
//! snapshot. The same inode/offset shares one physical page across all clients.
use super::*;
use alloc::vec::Vec;

pub(super) const PAGE_BYTES: usize = 4096;
const MAX_PAGES: usize = 1024;
const MAX_LEASES: usize = 4096;

struct Page {
    inode: u32,
    offset: usize,
    address: Word,
}

struct Lease {
    owner: Word,
    handle: Word,
    peer: Word,
    inode: u32,
    offset: usize,
}

pub(super) struct Pages {
    pages: Vec<Page>,
    leases: Vec<Lease>,
}

impl Pages {
    pub(super) const fn new() -> Self {
        Self {
            pages: Vec::new(),
            leases: Vec::new(),
        }
    }

    pub(super) fn pins(&self, inode: u32) -> bool {
        self.pages.iter().any(|page| page.inode == inode)
    }

    /// Called after each successful data write, including writes from native
    /// clients. Readers of existing aliases observe the new bytes directly.
    pub(super) unsafe fn update(&self, inode: u32, offset: usize, source: Word, bytes: usize) {
        let end = offset + bytes;
        for page in self.pages.iter().filter(|page| page.inode == inode) {
            let start = offset.max(page.offset);
            let stop = end.min(page.offset + PAGE_BYTES);
            if start < stop {
                ptr::copy_nonoverlapping(
                    (source + start - offset) as *const u8,
                    (page.address + start - page.offset) as *mut u8,
                    stop - start,
                );
            }
        }
    }
}

pub(super) fn handle(request: ServiceRequest, runtime: &mut Ext2Runtime) -> (Word, Word, Word) {
    match operate(request, runtime) {
        Ok(address) => (nanami_runtime::OS_RESPONSE_OK, address, 0),
        Err(error) => (map_request_error_to_status(error), 0, 0),
    }
}

fn operate(request: ServiceRequest, runtime: &mut Ext2Runtime) -> Result<Word, RequestError> {
    let file = runtime
        .handles
        .get(request.arg0)
        .copied()
        .filter(|file| {
            file.active
                && file.owner_pid == request.identifier
                && file.mode & EXT2_S_IFMT == EXT2_S_IFREG
        })
        .ok_or(RequestError::Status(
            nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR,
        ))?;
    let offset = request.arg1;
    if offset & (PAGE_BYTES - 1) != 0 || request.arg2 == 0 {
        return Err(RequestError::InvalidArgument);
    }
    if request.code == nanami_client::vfs::VFS_REQUEST_UNMAP_READ_PAGE {
        let index = runtime
            .mapped_pages
            .leases
            .iter()
            .position(|lease| {
                lease.owner == request.identifier
                    && lease.handle == request.arg0
                    && lease.peer == request.arg2
                    && lease.inode == file.inode
                    && lease.offset == offset
            })
            .ok_or(RequestError::InvalidArgument)?;
        let last = runtime
            .mapped_pages
            .leases
            .iter()
            .enumerate()
            .all(|(other, lease)| {
                other == index || lease.inode != file.inode || lease.offset != offset
            });
        if last {
            let page = runtime
                .mapped_pages
                .pages
                .iter()
                .position(|page| page.inode == file.inode && page.offset == offset)
                .ok_or(RequestError::Protocol)?;
            nanami_runtime::request_mapping_release(
                runtime.mapped_pages.pages[page].address,
                PAGE_BYTES,
            )?;
            runtime.mapped_pages.pages.swap_remove(page);
        }
        runtime.mapped_pages.leases.swap_remove(index);
        return Ok(0);
    }
    let inode = read_inode(runtime, file.inode)?;
    if offset >= inode.size as usize || runtime.frozen_by.is_some() {
        return Err(RequestError::InvalidArgument);
    }
    if runtime.mapped_pages.leases.len() >= MAX_LEASES {
        return Err(RequestError::Unsupported);
    }
    runtime
        .mapped_pages
        .leases
        .try_reserve(1)
        .map_err(|_| RequestError::Unsupported)?;
    let address = if let Some(page) = runtime
        .mapped_pages
        .pages
        .iter()
        .find(|page| page.inode == file.inode && page.offset == offset)
    {
        page.address
    } else {
        if runtime.mapped_pages.pages.len() >= MAX_PAGES {
            return Err(RequestError::Unsupported);
        }
        runtime
            .mapped_pages
            .pages
            .try_reserve(1)
            .map_err(|_| RequestError::Unsupported)?;
        let address = nanami_runtime::request_memory_create(PAGE_BYTES)?;
        let session = ClientSession {
            active: true,
            pid: 0,
            shm_local: address,
            shm_size: PAGE_BYTES,
        };
        if let Err(status) = read_file(runtime, session, inode, offset, PAGE_BYTES, 0) {
            let _ = nanami_runtime::request_mapping_release(address, PAGE_BYTES);
            return Err(RequestError::Status(status));
        }
        runtime.mapped_pages.pages.push(Page {
            inode: file.inode,
            offset,
            address,
        });
        address
    };
    let peer_address =
        match nanami_runtime::request_mapping_share(request.arg2, address, PAGE_BYTES) {
            Ok(peer) => peer,
            Err(error) => {
                if !runtime
                    .mapped_pages
                    .leases
                    .iter()
                    .any(|lease| lease.inode == file.inode && lease.offset == offset)
                {
                    if nanami_runtime::request_mapping_release(address, PAGE_BYTES).is_ok() {
                        runtime
                            .mapped_pages
                            .pages
                            .retain(|page| page.address != address);
                    }
                }
                return Err(error);
            }
        };
    runtime.mapped_pages.leases.push(Lease {
        owner: request.identifier,
        handle: request.arg0,
        peer: request.arg2,
        inode: file.inode,
        offset,
    });
    Ok(peer_address)
}
