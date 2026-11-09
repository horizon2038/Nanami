const EXT2_DENTRY_CACHE_ENTRIES: usize = 128;
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

// Independent model of the original FIFO, including updates and invalid holes.
struct Model {
    entries: Vec<Option<(u32, Vec<u8>, Option<u32>)>>,
    next: usize,
}
impl Model {
    fn new() -> Self { Self { entries: vec![None; 128], next: 0 } }
    fn load(&self, parent: u32, name: &[u8]) -> Option<Option<u32>> {
        self.entries.iter().flatten().find(|(p,n,_)| *p == parent && n == name).map(|e| e.2)
    }
    fn store(&mut self, parent: u32, name: &[u8], inode: Option<u32>) {
        let inode = inode.filter(|number| *number != 0);
        let index = self.entries.iter().position(|e| e.as_ref().is_some_and(|(p,n,_)| *p == parent && n == name))
            .unwrap_or_else(|| { let i=self.next; self.next=(i+1)%128; i });
        self.entries[index] = Some((parent, name.to_vec(), inode));
    }
    fn invalidate(&mut self, parent: u32, name: &[u8]) {
        for e in &mut self.entries {
            if e.as_ref().is_some_and(|(p,n,_)| *p == parent && n == name) { *e=None; }
        }
    }
    fn invalidate_inode(&mut self, inode: u32) {
        for e in &mut self.entries {
            if e.as_ref().is_some_and(|(p,_,i)| *p == inode || i.unwrap_or(0) == inode) { *e=None; }
        }
    }
}

#[test]
fn churn_collisions_negative_hits_updates_and_invalidations_match_original_fifo() {
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
fn bucket_links_fit_original_name_cache_padding() {
    #[allow(dead_code)]
    struct Original { valid: bool, parent: u32, inode: u32, len: u16, name: [u8; 255] }
    assert_eq!(core::mem::size_of::<Cache>(),
        128 * core::mem::size_of::<Original>() + core::mem::size_of::<usize>() + 128);
}

#[test]
fn colliding_names_preserve_fifo_after_head_middle_tail_and_bulk_inode_removal() {
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
