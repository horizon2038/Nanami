const EXT2_DENTRY_CACHE_ENTRIES: usize = 512;
const EXT2_NAME_LEN_MAX: usize = 255;
#[path = "../../nanami/servers/core-services/ext2-server/src/dentry_cache.rs"]
mod dentry_cache;
use dentry_cache::Cache;

#[test]
fn head_eviction_and_bulk_removal_do_not_rehash_stored_names() {
    let mut names = Vec::new();
    let mut seen = [false; 128];
    for seed in 0u32.. {
        let mut name = vec![b'x'; 255];
        name[247..].copy_from_slice(format!("{seed:08x}").as_bytes());
        let mut hash = 7u32.wrapping_mul(0x9e3779b1);
        for &byte in &name { hash = (hash ^ u32::from(byte)).wrapping_mul(16777619); }
        let bucket = (hash ^ (hash >> 16)) as usize % 128;
        if !seen[bucket] { seen[bucket] = true; names.push(name); }
        if names.len() == 128 { break; }
    }
    let mut cache = Cache::new();
    let mut model = Model::new();
    for (i, name) in names.iter().enumerate() {
        cache.store(7, name, Some(i as u32 + 1)); model.store(7, name, Some(i as u32 + 1));
    }
    cache.take_hash_bytes();
    cache.invalidate_inode(7); model.invalidate_inode(7);
    let bulk = cache.take_hash_bytes();
    for name in &names { assert_eq!(cache.load(7, name), model.load(7, name)); }
    for (i, name) in names.iter().enumerate() {
        cache.store(7, name, Some(i as u32 + 1)); model.store(7, name, Some(i as u32 + 1));
    }
    let fresh = [b'z'; 255];
    cache.take_hash_bytes();
    cache.store(9, &fresh, Some(999)); model.store(9, &fresh, Some(999));
    let eviction = cache.take_hash_bytes();
    for name in &names { assert_eq!(cache.load(7, name), model.load(7, name)); }
    assert_eq!(cache.load(9, &fresh), model.load(9, &fresh));
    println!("DENTRY-OWNER-BUCKET entries=128 name-bytes=255 bulk-hash-bytes={bulk} eviction-hash-bytes={eviction}");
    assert_eq!((bulk, eviction), (0, 255));
}

// Independent ordered-list LRU oracle, including updates and invalid holes.
struct Model {
    entries: Vec<Option<(u32, Vec<u8>, Option<u32>)>>,
    age: Vec<usize>,
}
impl Model {
    fn new() -> Self { Self { entries: vec![None; EXT2_DENTRY_CACHE_ENTRIES], age: Vec::new() } }
    fn touch(&mut self, index: usize) {
        self.age.retain(|&entry| entry != index);
        self.age.push(index);
    }
    fn load(&mut self, parent: u32, name: &[u8]) -> Option<Option<u32>> {
        let index = self.entries.iter().position(|entry| entry.as_ref()
            .is_some_and(|(p,n,_)| *p == parent && n == name))?;
        let result = self.entries[index].as_ref().unwrap().2;
        self.touch(index);
        Some(result)
    }
    fn store(&mut self, parent: u32, name: &[u8], inode: Option<u32>) {
        let inode = inode.filter(|number| *number != 0);
        let existing = self.entries.iter().position(|e| e.as_ref().is_some_and(|(p,n,_)| *p == parent && n == name));
        if existing.is_none() && name.len() > 32
            && self.entries.iter().flatten().filter(|(_,name,_)| name.len() > 32).count() == 128
        {
            let index = *self.age.iter().find(|&&index| self.entries[index].as_ref().unwrap().1.len() > 32).unwrap();
            self.entries[index] = None;
            self.age.retain(|&entry| entry != index);
        }
        let index = existing
            .unwrap_or_else(|| self.entries.iter().position(Option::is_none).unwrap_or_else(|| self.age[0]));
        self.entries[index] = Some((parent, name.to_vec(), inode));
        self.touch(index);
    }
    fn invalidate(&mut self, parent: u32, name: &[u8]) {
        for e in &mut self.entries {
            if e.as_ref().is_some_and(|(p,n,_)| *p == parent && n == name) { *e=None; }
        }
        self.age.retain(|&index| self.entries[index].is_some());
    }
    fn invalidate_inode(&mut self, inode: u32) {
        for e in &mut self.entries {
            if e.as_ref().is_some_and(|(p,_,i)| *p == inode || i.unwrap_or(0) == inode) { *e=None; }
        }
        self.age.retain(|&index| self.entries[index].is_some());
    }
}

#[test]
fn churn_collisions_negative_hits_updates_and_invalidations_match_lru_oracle() {
    let mut cache=Cache::new(); let mut model=Model::new();
    let names: Vec<_>=(0..310).map(|i| format!("library-{i:03}.so").into_bytes()).collect();
    let mut rng=0x19845abc_u64;
    for step in 0..20_000 {
        rng=rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let parent=((rng>>32)%9) as u32 + 1;
        let name=&names[(rng as usize)%names.len()];
        match (rng>>48)%8 {
            0=>{cache.invalidate(parent,name); model.invalidate(parent,name);},
            1=>{cache.invalidate_inode(parent); model.invalidate_inode(parent);},
            2..=5=>{
                let inode=if rng & 8 == 0 { None } else {Some(((rng>>16)%1000) as u32+1)};
                cache.store(parent,name,inode);model.store(parent,name,inode);
            },
            _=>assert_eq!(cache.load(parent,name),model.load(parent,name),"step {step}"),
        }
        if step%127==0 {
            for p in 1..=9 { for n in &names { assert_eq!(cache.load(p,n),model.load(p,n),"step {step}"); } }
        }
    }
}

#[test]
fn wrapped_slots_never_serve_stale_hints_or_confuse_parents_and_prefixes() {
    let mut cache=Cache::new();
    cache.store(7,b"lib",Some(99)); cache.store(8,b"lib",None);
    cache.store(7,b"library",Some(100));
    assert_eq!(cache.load(7,b"lib"),Some(Some(99)));
    assert_eq!(cache.load(8,b"lib"),Some(None));
    assert_eq!(cache.load(7,b"library"),Some(Some(100)));
    for i in 0..512_u32 { cache.store(1000+i,b"x",Some(i+1)); }
    assert_eq!(cache.load(7,b"lib"),None); assert_eq!(cache.load(8,b"lib"),None);
    cache.store(7,b"lib",Some(101)); cache.invalidate_inode(101);
    assert_eq!(cache.load(7,b"lib"),None);
    cache.store(7,b"lib",None); cache.invalidate(7,b"lib");
    assert_eq!(cache.load(7,b"lib"),None);
}

#[test]
fn invalid_names_do_not_replace_entries_and_maximum_name_is_exact() {
    let mut cache=Cache::new(); let name=[b'a';255];
    cache.store(1,&name,Some(2));
    cache.store(1,b"",Some(3));cache.store(1,&[b'a';256],Some(4));
    assert_eq!(cache.load(1,&name),Some(Some(2)));
    assert_eq!(cache.load(1,&name[..254]),None);
    assert_eq!(cache.load(1,b""),None);assert_eq!(cache.load(1,&[b'a';256]),None);
    cache.invalidate(1,&[b'a';256]);assert_eq!(cache.load(1,&name),Some(Some(2)));
    cache.invalidate_inode(1);assert_eq!(cache.load(1,&name),None);
}

#[test]
fn compact_cache_quadruples_short_capacity_with_bounded_metadata_growth() {
    let old = 34692; // Actual previous 128-entry production cache layout.
    let current = core::mem::size_of::<Cache>();
    println!("DENTRY-COMPACT entries=512 long-names=128 old-bytes={old} new-bytes={current}");
    assert!(current <= old + 24 * 1024);
    assert!(current < 2 * old);
}

#[test]
fn colliding_names_preserve_lru_after_head_middle_tail_and_bulk_inode_removal() {
    let names: Vec<_> = (0u32..).map(|i| format!("collision-{i:05}").into_bytes()).filter(|name| {
        let mut hash = 7u32.wrapping_mul(0x9e3779b1);
        for &byte in name { hash = (hash ^ u32::from(byte)).wrapping_mul(16777619); }
        (hash ^ (hash >> 16)) % 128 == 127
    }).take(260).collect();
    let mut cache = Cache::new(); let mut model = Model::new();
    for (i, name) in names[..128].iter().enumerate() {
        let inode = Some(i as u32 + 1); cache.store(7, name, inode); model.store(7, name, inode);
    }
    for index in [127, 64, 0, 63, 65, 126, 1, 127] {
        cache.invalidate(7, &names[index]); model.invalidate(7, &names[index]);
        for name in &names { assert_eq!(cache.load(7, name), model.load(7, name)); }
    }
    for step in 0..1040 {
        let name = &names[(step * 37) % names.len()];
        let inode = if step % 3 == 0 { None } else { Some((step % 11) as u32) };
        cache.store(7, name, inode); model.store(7, name, inode);
        if step % 13 == 0 {
            let inode = (step % 11) as u32;
            cache.invalidate_inode(inode); model.invalidate_inode(inode);
        }
        for name in &names { assert_eq!(cache.load(7, name), model.load(7, name), "step={step}"); }
    }
    cache.invalidate_inode(7); model.invalidate_inode(7);
    for name in &names { assert_eq!(cache.load(7, name), model.load(7, name)); }
}

#[test]
fn zero_inode_invalidation_preserves_original_negative_entry_semantics() {
    let mut cache = Cache::new();
    cache.store(1, b"missing", None);
    cache.store(2, b"zero", Some(0));
    cache.store(0, b"parent-zero", Some(12));
    cache.store(3, b"present", Some(13));
    cache.invalidate_inode(0);
    for (parent, name) in [(1, b"missing".as_slice()), (2, b"zero"), (0, b"parent-zero")] {
        assert_eq!(cache.load(parent, name), None);
    }
    assert_eq!(cache.load(3, b"present"), Some(Some(13)));
}

#[test]
fn hot_positive_and_negative_names_survive_cold_directory_churn() {
    let mut cache = Cache::new();
    let mut fifo: Vec<Option<(u32, Vec<u8>, Option<u32>)>> = vec![None; 128];
    let mut next = 0;
    let mut hot_misses = [0; 2];
    // Both policies receive the same successful reference stream, inserting
    // after each miss, including hypothetical FIFO misses on actual LRU hits.
    for cold in 0..1024u32 {
        let cold_name = format!("cold-{cold:04}").into_bytes();
        let mut names: Vec<_> = (0..8u32).map(|index|
            (2 + index, b"reused".to_vec(), (index % 2 == 0).then_some(500 + index))).collect();
        names.push((99, cold_name, Some(2000 + cold)));
        for (index, (parent, name, inode)) in names.iter().enumerate() {
            if let Some(result) = cache.load(*parent, name) { assert_eq!(result, *inode); }
            else {
                if index < 8 { hot_misses[1] += 1; }
                cache.store(*parent, name, *inode);
            }
            if let Some(entry) = fifo.iter().flatten().find(|(p,n,_)| p == parent && n == name) {
                assert_eq!(entry.2, *inode);
            } else {
                if index < 8 { hot_misses[0] += 1; }
                fifo[next] = Some((*parent, name.clone(), *inode));
                next = (next + 1) % fifo.len();
            }
        }
    }
    println!("DENTRY-LRU hot-name directory searches FIFO={} LRU={}", hot_misses[0], hot_misses[1]);
    assert_eq!(hot_misses[1], 8);
    assert!(hot_misses[0] > hot_misses[1]);
}

#[test]
fn misses_leave_age_unchanged_updates_touch_and_invalid_slots_are_reused() {
    let mut cache = Cache::new();
    for parent in 1..=512 { cache.store(parent, b"key", Some(parent + 200)); }
    assert_eq!(cache.load(999, b"missing"), None);
    cache.store(513, b"key", Some(713));
    assert_eq!(cache.load(1, b"key"), None); // Miss did not protect oldest.
    cache.store(2, b"key", None); // Updating to a negative answer makes it newest.
    cache.store(514, b"key", Some(714));
    assert_eq!(cache.load(3, b"key"), None);
    assert_eq!(cache.load(2, b"key"), Some(None));
    cache.invalidate(80, b"key");
    cache.store(515, b"key", Some(715));
    assert_eq!(cache.load(4, b"key"), Some(Some(204))); // Empty slot precedes eviction.
    cache.invalidate_inode(204);
    cache.store(516, b"key", Some(716));
    assert_eq!(cache.load(5, b"key"), Some(Some(205)));
    cache.invalidate_inode(0); // Unlink the negative entry from both indexes.
    cache.store(517, b"key", Some(717));
    assert_eq!(cache.load(6, b"key"), Some(Some(206)));
}

#[test]
fn all_long_slots_survive_short_churn_and_reused_suffixes_never_alias() {
    let mut cache = Cache::new();
    let mut names = Vec::new();
    for i in 0u32..129 {
        let mut name = vec![0xfe; 255];
        name[251..].copy_from_slice(&i.to_le_bytes());
        names.push(name);
    }
    for (i,name) in names[..128].iter().enumerate() { cache.store(7,name,Some(i as u32 + 1)); }
    for cold in 0..1024 {
        for (i,name) in names[..128].iter().enumerate() { assert_eq!(cache.load(7,name),Some(Some(i as u32 + 1))); }
        cache.store(3,format!("short-{cold}").as_bytes(),None);
    }
    cache.store(7,&names[128],Some(129));
    assert_eq!(cache.load(7,&names[0]),None);
    for (i,name) in names[1..].iter().enumerate() { assert_eq!(cache.load(7,name),Some(Some(i as u32 + 2))); }
    cache.invalidate_inode(7);
    for (i,name) in names[..128].iter().enumerate() { cache.store(9,name,Some(i as u32 + 500)); }
    for (i,name) in names[..128].iter().enumerate() { assert_eq!(cache.load(9,name),Some(Some(i as u32 + 500))); }
    assert_eq!(cache.load(9,&names[128]),None);
}

#[test]
fn mixed_inline_boundaries_full_cache_and_long_pool_churn_match_full_byte_oracle() {
    let mut cache = Cache::new();let mut model = Model::new();
    let names: Vec<_> = (0usize..768).map(|index| {
        let len = [1,31,32,33,63,254,255][index % 7];
        let mut name = vec![0xa5;len];
        let bytes = index.to_le_bytes();
        let count = len.min(bytes.len());
        name[len-count..].copy_from_slice(&bytes[..count]);name
    }).collect();
    let mut rng = 0x347e091ca_u64;
    for step in 0..6000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let parent = 1 + ((rng >> 32) % 3) as u32;
        let name = &names[(rng as usize) % names.len()];
        match (rng >> 48) % 10 {
            0 => { cache.invalidate(parent,name);model.invalidate(parent,name); }
            1 => { cache.invalidate_inode(parent);model.invalidate_inode(parent); }
            2..=7 => {
                let inode = (rng & 8 != 0).then_some(10 + ((rng >> 16) % 31) as u32);
                cache.store(parent,name,inode);model.store(parent,name,inode);
            }
            _ => assert_eq!(cache.load(parent,name),model.load(parent,name)),
        }
        if step % 149 == 0 {
            for parent in 1..=3 { for name in &names {
                assert_eq!(cache.load(parent,name),model.load(parent,name),"step={step} len={}",name.len());
            } }
        }
    }
}
