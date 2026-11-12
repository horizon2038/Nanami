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
fn known_lru_victim_does_not_repeat_the_collision_chain_search() {
    let keys: Vec<u32> = (0u32..).filter(|n| {
        let hash = n.wrapping_mul(0x9e37_79b1);
        (hash ^ (hash >> 16)) & 255 == 255
    }).take(72).collect();
    let mut cache = inode_cache::Cache::new();
    let mut model = Model::default();
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

// A simple ordered list oracle, independent of the production hash/age links.
#[derive(Default)]
struct Model { entries: Vec<(u32, Ext2Inode)> }
impl Model {
    fn load(&mut self, n: u32) -> Option<Ext2Inode> {
        let index = self.entries.iter().position(|&(number, _)| number == n)?;
        let entry = self.entries.remove(index);
        self.entries.push(entry);
        Some(entry.1)
    }
    fn store(&mut self, n: u32, inode: Ext2Inode) {
        if let Some(index) = self.entries.iter().position(|&(number, _)| number == n) {
            self.entries.remove(index);
        } else if self.entries.len() == EXT2_INODE_CACHE_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((n, inode));
    }
    fn invalidate(&mut self, n: u32) {
        self.entries.retain(|&(number, _)| number != n);
    }
}

#[test]
fn indexed_lru_preserves_updates_invalidation_and_all_metadata_through_churn() {
    let mut cache = inode_cache::Cache::new();
    let mut model = Model::default();
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
    let mut model = Model::default();
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
    let previous = 64 * core::mem::size_of::<Original>() + core::mem::size_of::<usize>() + 256;
    let current = core::mem::size_of::<inode_cache::Cache>();
    assert!(current <= previous + 128);
    println!("INODE-CACHE-METADATA before={previous} after={current}");
}

#[test]
fn full_collision_chains_survive_head_middle_tail_deletion_updates_and_lru_replacement() {
    let keys: Vec<u32> = (0u32..).filter(|n| {
        let hash = n.wrapping_mul(0x9e37_79b1);
        (hash ^ (hash >> 16)) & 255 == 255
    }).take(128).collect();
    let mut cache = inode_cache::Cache::new();
    let mut model = Model::default();
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

#[test]
fn repeated_directory_inodes_survive_a_stream_of_cold_file_inodes() {
    let mut cache = inode_cache::Cache::new();
    let mut fifo = [None; EXT2_INODE_CACHE_ENTRIES];
    let mut hand = 0;
    let mut fifo_misses = 0;
    let mut lru_misses = 0;
    for child in 100..1124u32 {
        for number in (1..=8).chain(core::iter::once(child)) {
            let hot = number <= 8;
            if cache.load(number).is_none() {
                if hot { lru_misses += 1; }
                cache.store(number, metadata(number));
            }
            if !fifo.contains(&Some(number)) {
                if hot { fifo_misses += 1; }
                fifo[hand] = Some(number);
                hand = (hand + 1) % EXT2_INODE_CACHE_ENTRIES;
            }
        }
    }
    assert_eq!(lru_misses, 8); // Initial compulsory reads only.
    assert!(fifo_misses >= 128);
    println!("INODE-HOT-READS fifo={fifo_misses} lru={lru_misses} cold-files=1024 capacity=64");
}

#[test]
fn misses_do_not_change_recency_updates_do_and_invalidated_slots_are_reused() {
    let mut cache = inode_cache::Cache::new();
    for number in 0..64 { cache.store(number, metadata(number)); }
    assert_eq!(cache.load(0), Some(metadata(0)));
    cache.store(1, metadata(900));
    assert_eq!(cache.load(u32::MAX), None);
    cache.store(64, metadata(64));
    assert_eq!(cache.load(2), None);
    assert_eq!(cache.load(0), Some(metadata(0)));
    assert_eq!(cache.load(1), Some(metadata(900)));
    cache.invalidate(1);
    assert_eq!(cache.load(1), None);
    cache.store(65, metadata(65));
    for number in 3..=65 {
        assert_eq!(cache.load(number), Some(metadata(number)));
    }
    for number in 0..=65 { cache.invalidate(number); }
    cache.store(u32::MAX, metadata(42));
    assert_eq!(cache.load(u32::MAX), Some(metadata(42)));
    cache.invalidate(u32::MAX);
    assert_eq!(cache.load(u32::MAX), None);
}
