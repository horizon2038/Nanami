const EXT2_INODE_CACHE_ENTRIES: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Ext2Inode { mode: u16, size: u32, links: u16, sectors: u32, block: [u32; 15] }
#[derive(Clone, Copy)]
struct CachedInode { valid: bool, previous_in_bucket: u8, next_in_bucket: u8, inode_no: u32, inode: Ext2Inode }
impl CachedInode {
    const EMPTY: Self = Self { valid: false, previous_in_bucket: 0, next_in_bucket: 0, inode_no: 0,
        inode: Ext2Inode { mode: 0, size: 0, links: 0, sectors: 0, block: [0; 15] } };
}
#[path = "../../nanami/servers/core-services/ext2-server/src/inode_cache.rs"]
mod inode_cache;

fn metadata(seed: u32) -> Ext2Inode {
    Ext2Inode { mode: seed as u16, size: seed.wrapping_mul(17), links: (seed >> 16) as u16,
        sectors: seed ^ 0x19c3_785f, block: core::array::from_fn(|i| seed.wrapping_add(i as u32)) }
}

#[test]
fn known_fifo_victim_does_not_repeat_the_collision_chain_search() {
    let keys: Vec<u32> = (0u32..).filter(|n| {
        let hash = n.wrapping_mul(0x9e37_79b1);
        (hash ^ (hash >> 16)) & 255 == 255
    }).take(72).collect();
    let mut cache = inode_cache::Cache::new();
    let mut model = Model { entries: [CachedInode::EMPTY; 64], next: 0 };
    for &key in &keys[..64] { cache.store(key, metadata(key)); model.store(key, metadata(key)); }
    let mut total = 0;
    for &key in &keys[64..] {
        cache.take_chain_comparisons();
        cache.store(key, metadata(key));
        let comparisons = cache.take_chain_comparisons();
        total += comparisons;
        model.store(key, metadata(key));
        for &probe in &keys { assert_eq!(cache.load(probe), model.load(probe)); }
    }
    println!("INODE-EVICTION-LINK collision-chain=64 replacements=8 comparisons={total}");
    assert_eq!(total, 512);
}

// The original linear FIFO, with updates retaining their slot and invalid
// holes left for the original replacement cursor rather than reused early.
struct Model { entries: [CachedInode; 64], next: usize }
impl Model {
    fn load(&self, n: u32) -> Option<Ext2Inode> {
        self.entries.iter().find(|e| e.valid && e.inode_no == n).map(|e| e.inode)
    }
    fn store(&mut self, n: u32, inode: Ext2Inode) {
        let slot = self.entries.iter().position(|e| e.valid && e.inode_no == n)
            .unwrap_or_else(|| { let slot = self.next; self.next = (slot + 1) % 64; slot });
        self.entries[slot] = CachedInode { valid: true, previous_in_bucket: 0, next_in_bucket: 0, inode_no: n, inode };
    }
    fn invalidate(&mut self, n: u32) {
        for entry in &mut self.entries { if entry.valid && entry.inode_no == n { entry.valid = false; } }
    }
}

#[test]
fn hints_preserve_fifo_updates_invalidation_and_all_metadata_through_churn() {
    let mut cache = inode_cache::Cache::new();
    let mut model = Model { entries: [CachedInode::EMPTY; 64], next: 0 };
    let mut rng = 0x139d_78b5u32;
    for step in 0..20_000u32 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let n = (rng >> 16) % 193;
        if rng & 7 == 0 { cache.invalidate(n); model.invalidate(n); }
        else { let inode = metadata(step ^ rng); cache.store(n, inode); model.store(n, inode); }
        for number in 0..193 { assert_eq!(cache.load(number), model.load(number), "step={step} inode={number}"); }
    }
}

#[test]
fn zero_maximum_keys_stale_slots_and_collisions_never_return_another_inode() {
    let mut cache = inode_cache::Cache::new();
    let mut model = Model { entries: [CachedInode::EMPTY; 64], next: 0 };
    let keys: Vec<_> = (0..700u32).map(|n| n.wrapping_mul(0x9e37_79b9)).chain([0, u32::MAX]).collect();
    for (step, &key) in keys.iter().enumerate() {
        let inode = metadata(step as u32); cache.store(key, inode); model.store(key, inode);
        if step % 3 == 0 { cache.invalidate(key); model.invalidate(key); }
        for &probe in &keys { assert_eq!(cache.load(probe), model.load(probe)); }
    }
}

#[test]
fn bucket_links_fit_original_entry_padding() {
    #[allow(dead_code)]
    struct Original { valid: bool, inode_no: u32, inode: Ext2Inode }
    assert_eq!(core::mem::size_of::<CachedInode>(), core::mem::size_of::<Original>());
    assert_eq!(core::mem::size_of::<inode_cache::Cache>(),
        64 * core::mem::size_of::<Original>() + core::mem::size_of::<usize>() + 256);
}

#[test]
fn full_collision_chains_survive_head_middle_tail_deletion_updates_and_fifo_wrap() {
    let keys: Vec<u32> = (0u32..).filter(|n| {
        let hash = n.wrapping_mul(0x9e37_79b1);
        (hash ^ (hash >> 16)) & 255 == 255
    }).take(128).collect();
    let mut cache = inode_cache::Cache::new();
    let mut model = Model { entries: [CachedInode::EMPTY; 64], next: 0 };
    for &key in &keys[..64] { cache.store(key, metadata(key)); model.store(key, metadata(key)); }
    // These include the head, middle, tail and adjacent chain nodes.
    for index in [63, 32, 0, 31, 33, 62, 1, 63] {
        cache.invalidate(keys[index]); model.invalidate(keys[index]);
        for &key in &keys { assert_eq!(cache.load(key), model.load(key)); }
    }
    for step in 0..512 {
        let key = keys[(step * 37) % keys.len()];
        cache.store(key, metadata(step as u32)); model.store(key, metadata(step as u32));
        if step % 5 == 0 {
            let key = keys[(step * 29) % keys.len()];
            cache.invalidate(key); model.invalidate(key);
        }
        for &key in &keys { assert_eq!(cache.load(key), model.load(key)); }
    }
}
