//! VFS-owned, coherent read-only file pages. MAP_SHARED never becomes a private
//! snapshot. The same inode/offset shares one physical page across all clients.
use super::*;
use alloc::vec::Vec;

pub(super) const PAGE_BYTES: usize = 4096;
const MAX_PAGES: usize = 1024;
const MAX_LEASES: usize = 4096;
const PAGE_BUCKETS: usize = 256;
const LEASE_BUCKETS: usize = 256;

#[cfg(test)]
static LAST_LEASE_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static PAGE_KEY_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static LEASE_KEY_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static UPDATE_PAGE_CHECKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn lease_matches(lease: &Lease, owner: Word, handle: Word, peer: Word, inode: u32, offset: usize) -> bool {
    #[cfg(test)]
    LEASE_KEY_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    lease.owner == owner && lease.handle == handle && lease.peer == peer
        && lease.inode == inode && lease.offset == offset
}

fn page_matches(page: &Page, inode: u32, offset: usize) -> bool {
    #[cfg(test)]
    PAGE_KEY_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    page.inode == inode && page.offset == offset
}

struct Page {
    inode: u32,
    lease_count: u16,
    next_in_bucket: u16,
    offset: usize,
    address: Word,
}

// The bucket link occupies the former padding beside the lease count.
const _: () = assert!(core::mem::size_of::<Page>()
    == core::mem::size_of::<(u32, u16, usize, Word)>());

struct Lease {
    owner: Word,
    handle: Word,
    peer: Word,
    inode: u32,
    previous_in_bucket: u16,
    next_in_bucket: u16,
    offset: usize,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<Lease>()
    == core::mem::size_of::<(Word, Word, Word, u32, usize)>());

pub(super) struct Pages {
    pages: Vec<Page>,
    leases: Vec<Lease>,
    heads: [u16; PAGE_BUCKETS],
    lease_heads: [u16; LEASE_BUCKETS],
}

impl Pages {
    #[cfg(test)]
    pub fn take_update_page_checks(&self) -> usize {
        UPDATE_PAGE_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn take_lease_key_checks(&self) -> usize {
        LEASE_KEY_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn take_page_key_checks(&self) -> usize {
        PAGE_KEY_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn take_last_lease_checks(&self) -> usize {
        LAST_LEASE_CHECKS.swap(0, core::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub fn page_record_bytes(&self) -> usize { core::mem::size_of::<Page>() }

    #[cfg(test)]
    pub fn lease_record_bytes(&self) -> usize { core::mem::size_of::<Lease>() }

    #[cfg(test)]
    pub fn assert_consistent(&self) {
        let mut seen_leases = alloc::vec![false; self.leases.len()];
        for (bucket, &head) in self.lease_heads.iter().enumerate() {
            let mut previous = 0;
            let mut link = head;
            while link != 0 {
                let index = usize::from(link) - 1;
                assert!(!seen_leases[index]);
                seen_leases[index] = true;
                let lease = &self.leases[index];
                assert_eq!(lease.previous_in_bucket, previous);
                assert_eq!(Self::lease_bucket(lease.owner, lease.handle, lease.peer, lease.inode, lease.offset), bucket);
                previous = link;
                link = lease.next_in_bucket;
            }
        }
        assert!(seen_leases.into_iter().all(|present| present));
        let mut seen = alloc::vec![false; self.pages.len()];
        for (bucket, &head) in self.heads.iter().enumerate() {
            let mut link = head;
            while link != 0 {
                let index = usize::from(link) - 1;
                assert!(!seen[index]);
                seen[index] = true;
                let page = &self.pages[index];
                assert_eq!(Self::bucket(page.inode, page.offset), bucket);
                link = page.next_in_bucket;
            }
        }
        assert!(seen.into_iter().all(|present| present));
        for page in &self.pages {
            assert_eq!(usize::from(page.lease_count), self.leases.iter()
                .filter(|lease| lease.inode == page.inode && lease.offset == page.offset).count());
            assert_eq!(self.pages.iter().filter(|other|
                other.inode == page.inode && other.offset == page.offset).count(), 1);
        }
        for lease in &self.leases {
            assert!(self.pages.iter().any(|page| page.inode == lease.inode && page.offset == lease.offset));
        }
    }

    pub(super) const fn new() -> Self {
        assert!(MAX_LEASES <= u16::MAX as usize);
        assert!(MAX_PAGES <= u16::MAX as usize);
        Self {
            pages: Vec::new(),
            leases: Vec::new(),
            heads: [0; PAGE_BUCKETS],
            lease_heads: [0; LEASE_BUCKETS],
        }
    }

    fn lease_bucket(owner: Word, handle: Word, peer: Word, inode: u32, offset: usize) -> usize {
        let hash = owner.wrapping_mul(0x9e37_79b1) ^ handle.wrapping_mul(0x85eb_ca6b)
            ^ peer.wrapping_mul(0xc2b2_ae35) ^ inode as usize ^ (offset / PAGE_BYTES);
        (hash ^ (hash >> 8) ^ (hash >> 16)) % LEASE_BUCKETS
    }

    fn find_lease(&self, owner: Word, handle: Word, peer: Word, inode: u32, offset: usize) -> Option<usize> {
        let mut link = self.lease_heads[Self::lease_bucket(owner, handle, peer, inode, offset)];
        while link != 0 {
            let index = usize::from(link) - 1;
            let lease = &self.leases[index];
            if lease_matches(lease, owner, handle, peer, inode, offset) {
                return Some(index);
            }
            link = lease.next_in_bucket;
        }
        None
    }

    fn insert_lease(&mut self, mut lease: Lease) {
        let bucket = Self::lease_bucket(lease.owner, lease.handle, lease.peer, lease.inode, lease.offset);
        let link = (self.leases.len() + 1) as u16;
        lease.previous_in_bucket = 0;
        lease.next_in_bucket = self.lease_heads[bucket];
        if lease.next_in_bucket != 0 {
            self.leases[usize::from(lease.next_in_bucket) - 1].previous_in_bucket = link;
        }
        self.leases.push(lease);
        self.lease_heads[bucket] = link;
    }

    fn remove_lease(&mut self, index: usize) {
        let lease = &self.leases[index];
        let bucket = Self::lease_bucket(lease.owner, lease.handle, lease.peer, lease.inode, lease.offset);
        let (previous, next) = (lease.previous_in_bucket, lease.next_in_bucket);
        if previous == 0 {
            self.lease_heads[bucket] = next;
        } else {
            self.leases[usize::from(previous) - 1].next_in_bucket = next;
        }
        if next != 0 {
            self.leases[usize::from(next) - 1].previous_in_bucket = previous;
        }
        let last = self.leases.len() - 1;
        self.leases.swap_remove(index);
        if index != last {
            let moved = &self.leases[index];
            let bucket = Self::lease_bucket(moved.owner, moved.handle, moved.peer, moved.inode, moved.offset);
            let (previous, next) = (moved.previous_in_bucket, moved.next_in_bucket);
            let link = (index + 1) as u16;
            if previous == 0 {
                self.lease_heads[bucket] = link;
            } else {
                self.leases[usize::from(previous) - 1].next_in_bucket = link;
            }
            if next != 0 {
                self.leases[usize::from(next) - 1].previous_in_bucket = link;
            }
        }
    }

    fn bucket(inode: u32, offset: usize) -> usize {
        let page = offset / PAGE_BYTES;
        (page ^ (page >> 8) ^ (inode as usize).wrapping_mul(0x9e37_79b1)) % PAGE_BUCKETS
    }

    fn find_page(&self, inode: u32, offset: usize) -> Option<usize> {
        let mut link = self.heads[Self::bucket(inode, offset)];
        while link != 0 {
            let index = usize::from(link) - 1;
            let page = &self.pages[index];
            if page_matches(page, inode, offset) {
                return Some(index);
            }
            link = page.next_in_bucket;
        }
        None
    }

    fn replace_link(&mut self, bucket: usize, old: u16, new: u16) {
        if self.heads[bucket] == old {
            self.heads[bucket] = new;
            return;
        }
        let mut link = self.heads[bucket];
        while link != 0 {
            let page = &mut self.pages[usize::from(link) - 1];
            if page.next_in_bucket == old {
                page.next_in_bucket = new;
                return;
            }
            link = page.next_in_bucket;
        }
        panic!("mapped page missing from bucket");
    }

    fn remove_page(&mut self, index: usize) {
        let page = &self.pages[index];
        self.replace_link(Self::bucket(page.inode, page.offset), (index + 1) as u16, page.next_in_bucket);
        let last = self.pages.len() - 1;
        self.pages.swap_remove(index);
        if index != last {
            let moved = &self.pages[index];
            self.replace_link(Self::bucket(moved.inode, moved.offset), (last + 1) as u16, (index + 1) as u16);
        }
    }

    fn rebuild_buckets(&mut self) {
        self.heads.fill(0);
        for (index, page) in self.pages.iter_mut().enumerate() {
            let bucket = Self::bucket(page.inode, page.offset);
            page.next_in_bucket = self.heads[bucket];
            self.heads[bucket] = (index + 1) as u16;
        }
    }

    pub(super) fn pins(&self, inode: u32) -> bool {
        self.pages.iter().any(|page| page.inode == inode)
    }

    /// Called after each successful data write, including writes from native
    /// clients. Readers of existing aliases observe the new bytes directly.
    pub(super) unsafe fn update(&self, inode: u32, offset: usize, source: Word, bytes: usize) {
        let end = offset + bytes;
        if bytes == 0 {
            return;
        }
        // Keep large/sparse ranges on the original bounded page-table scan.
        // Candidate slots are request-local and sorted to preserve copy order,
        // even when one destination is part of a later copy's source range.
        const UPDATE_SLOTS: usize = 32;
        if end > offset {
            let first = offset / PAGE_BYTES;
            let count = (end - 1) / PAGE_BYTES - first + 1;
            if count <= UPDATE_SLOTS && count <= self.pages.len() {
                let mut slots = [0usize; UPDATE_SLOTS];
                let mut used = 0;
                for number in first..first + count {
                    if let Some(index) = self.find_page(inode, number * PAGE_BYTES) {
                        let mut position = used;
                        while position != 0 && slots[position - 1] > index {
                            slots[position] = slots[position - 1];
                            position -= 1;
                        }
                        slots[position] = index;
                        used += 1;
                    }
                }
                for &index in &slots[..used] {
                    Self::update_page(&self.pages[index], offset, end, source);
                }
                return;
            }
        }
        for page in self.pages.iter().filter(|page| {
            #[cfg(test)]
            UPDATE_PAGE_CHECKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            page.inode == inode
        }) {
            Self::update_page(page, offset, end, source);
        }
    }

    unsafe fn update_page(page: &Page, offset: usize, end: usize, source: Word) {
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
            .find_lease(request.identifier, request.arg0, request.arg2, file.inode, offset)
            .ok_or(RequestError::InvalidArgument)?;
        let page = runtime
            .mapped_pages
            .find_page(file.inode, offset)
            .ok_or(RequestError::Protocol)?;
        let count = runtime.mapped_pages.pages[page].lease_count;
        if count == 0 { return Err(RequestError::Protocol); }
        if count == 1 {
            nanami_runtime::request_mapping_release(
                runtime.mapped_pages.pages[page].address,
                PAGE_BYTES,
            )?;
            runtime.mapped_pages.remove_page(page);
        } else {
            runtime.mapped_pages.pages[page].lease_count -= 1;
        }
        runtime.mapped_pages.remove_lease(index);
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
    let page = if let Some(page) = runtime
        .mapped_pages
        .find_page(file.inode, offset)
    {
        page
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
        let bucket = Pages::bucket(file.inode, offset);
        let next_in_bucket = runtime.mapped_pages.heads[bucket];
        runtime.mapped_pages.pages.push(Page {
            inode: file.inode,
            lease_count: 0,
            next_in_bucket,
            offset,
            address,
        });
        runtime.mapped_pages.heads[bucket] = runtime.mapped_pages.pages.len() as u16;
        runtime.mapped_pages.pages.len() - 1
    };
    let address = runtime.mapped_pages.pages[page].address;
    let peer_address =
        match nanami_runtime::request_mapping_share(request.arg2, address, PAGE_BYTES) {
            Ok(peer) => peer,
            Err(error) => {
                if runtime.mapped_pages.pages[page].lease_count == 0 {
                    if nanami_runtime::request_mapping_release(address, PAGE_BYTES).is_ok() {
                        runtime
                            .mapped_pages
                            .pages
                            .retain(|page| page.address != address);
                        runtime.mapped_pages.rebuild_buckets();
                    }
                }
                return Err(error);
            }
        };
    runtime.mapped_pages.insert_lease(Lease {
        owner: request.identifier,
        handle: request.arg0,
        peer: request.arg2,
        inode: file.inode,
        previous_in_bucket: 0,
        next_in_bucket: 0,
        offset,
    });
    // Lease capacity was reserved before sharing. Publish the count only after
    // success; a failed final release leaves both the lease and count intact.
    runtime.mapped_pages.pages[page].lease_count += 1;
    Ok(peer_address)
}

#[cfg(test)]
#[test]
fn colliding_page_keys_keep_chains_valid_when_slots_move() {
    let mut pages = Pages::new();
    let mut keys = Vec::new();
    for inode in 1..=128 {
        let mut offset = 0;
        while Pages::bucket(inode, offset) != 7 {
            offset += PAGE_BYTES;
        }
        keys.push((inode, offset));
        pages.pages.push(Page { inode, offset, address: inode as Word, lease_count: 0, next_in_bucket: 0 });
    }
    pages.rebuild_buckets();
    for step in 0..128 {
        for &(inode, offset) in &keys {
            let expected = pages.pages.iter().position(|page| page.inode == inode && page.offset == offset);
            assert_eq!(pages.find_page(inode, offset), expected);
            assert_eq!(pages.find_page(inode, offset + PAGE_BYTES), None);
        }
        pages.assert_consistent();
        let (inode, offset) = keys[step * 17 % 128];
        let index = pages.find_page(inode, offset).unwrap();
        pages.remove_page(index);
    }
    pages.assert_consistent();
}

#[cfg(test)]
#[test]
fn colliding_lease_keys_keep_both_links_valid_when_slots_move() {
    let mut pages = Pages::new();
    let mut covered = [false; LEASE_BUCKETS];
    for peer in 1..=4096 {
        covered[Pages::lease_bucket(10, 0, peer, 7, 0)] = true;
    }
    assert!(covered.into_iter().all(|present| present));
    pages.pages.push(Page { inode: 7, offset: 0, address: 1, lease_count: 128, next_in_bucket: 0 });
    pages.rebuild_buckets();
    let mut keys = Vec::new();
    for index in 0..128 {
        let (owner, handle) = (1 + index % 7, index % 11);
        let mut peer = index * 65536 + 1;
        while Pages::lease_bucket(owner, handle, peer, 7, 0) != 3 {
            peer += 1;
            assert!(peer < (index + 1) * 65536, "collision fixture must terminate");
        }
        keys.push((owner, handle, peer));
        pages.insert_lease(Lease { owner, handle, peer, inode: 7, offset: 0, previous_in_bucket: 0, next_in_bucket: 0 });
    }
    for step in 0..128 {
        for &(owner, handle, peer) in &keys {
            for (owner, handle, peer, inode, offset) in [
                (owner, handle, peer, 7, 0),
                (owner + 1, handle, peer, 7, 0),
                (owner, handle + 1, peer, 7, 0),
                (owner, handle, peer + 1, 7, 0),
                (owner, handle, peer, 8, 0),
                (owner, handle, peer, 7, PAGE_BYTES),
            ] {
                let expected = pages.leases.iter().position(|lease| lease.owner == owner
                    && lease.handle == handle && lease.peer == peer && lease.inode == inode && lease.offset == offset);
                assert_eq!(pages.find_lease(owner, handle, peer, inode, offset), expected);
            }
        }
        pages.assert_consistent();
        let (owner, handle, peer) = keys[step * 17 % 128];
        let index = pages.find_lease(owner, handle, peer, 7, 0).unwrap();
        pages.remove_lease(index);
        pages.pages[0].lease_count -= 1;
    }
    pages.assert_consistent();
}

#[cfg(test)]
#[test]
fn indexed_updates_keep_original_slot_order_when_copies_feed_each_other() {
    let mut storage = alloc::vec![0x11u8; PAGE_BYTES * 3];
    storage[PAGE_BYTES..PAGE_BYTES * 2].fill(0x39);
    storage[PAGE_BYTES * 2..].fill(0x61);
    let source = storage.as_mut_ptr() as Word;
    let mut pages = Pages::new();
    for (offset, address) in [(PAGE_BYTES, source + PAGE_BYTES * 2), (0, source + PAGE_BYTES)] {
        pages.pages.push(Page { inode: 7, offset, address, lease_count: 0, next_in_bucket: 0 });
    }
    pages.rebuild_buckets();
    unsafe { pages.update(7, 0, source, PAGE_BYTES * 2); }
    assert!(storage[..PAGE_BYTES * 2].iter().all(|byte| *byte == 0x11));
    assert!(storage[PAGE_BYTES * 2..].iter().all(|byte| *byte == 0x39));
}
