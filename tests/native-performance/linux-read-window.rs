//! Production cache with explicit pin/map/release effects and byte assertions.
use std::{cell::RefCell, collections::BTreeMap, mem::MaybeUninit, rc::Rc};
type Word = usize;
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/page_window.rs"]
mod production;
use production::*;

type Page = Rc<RefCell<[u8; PAGE]>>;
#[derive(Default)]
struct Machine {
    pages: BTreeMap<(Word, Word), Page>,
    maps: BTreeMap<Word, Page>,
    grants: usize,
    releases: usize,
    uncertain: bool,
    fail_release: bool,
}
impl Machine {
    fn page(&mut self, pid: Word, address: Word, value: u8) {
        self.pages
            .insert((pid, address), Rc::new(RefCell::new([value; PAGE])));
    }
}
impl LeaseEffects for Machine {
    fn grant(&mut self, pid: Word, page: Word, local: Word) -> Result<(), GrantError> {
        self.grants += 1;
        let Some(backing) = self.pages.get(&(pid, page)).cloned() else {
            return Err(GrantError::NotInstalled);
        };
        assert!(
            self.maps.insert(local, backing).is_none(),
            "overwrote a live lease"
        );
        if self.uncertain {
            return Err(GrantError::Uncertain);
        }
        Ok(())
    }
    fn release(&mut self, local: Word) -> Result<(), ()> {
        self.releases += 1;
        if self.fail_release {
            return Err(());
        }
        assert!(self.maps.remove(&local).is_some());
        Ok(())
    }
}
impl Effects for Machine {
    fn read(&mut self, local: Word, offset: Word, destination: &mut [MaybeUninit<u8>]) {
        let page = self.maps[&local].borrow();
        let bytes = destination.len();
        for (output, value) in destination.iter_mut().zip(&page[offset..offset + bytes]) {
            output.write(*value);
        }
    }
}
impl WriteEffects for Machine {
    fn write(&mut self, local: Word, offset: Word, source: &[u8]) {
        self.maps[&local].borrow_mut()[offset..offset + source.len()].copy_from_slice(source);
    }
}

#[test]
fn range_invalidation_releases_touched_aliases_and_preserves_exact_disjoint_owners() {
    let reader = ReadWindow::new();
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for owner in [7, 8] {
        for page in [PAGE, 2 * PAGE, 3 * PAGE] {
            machine.page(owner, page, owner as u8);
            assert!(reader.copy_into(owner, 1, owner, page, &mut [MaybeUninit::uninit()], &mut machine));
            assert!(writer.copy_from(owner, 1, owner, page, &[owner as u8], &mut machine));
        }
    }
    let grants = machine.grants;
    reader.clear_range(7, 2 * PAGE + 7, 1, &mut machine);
    writer.clear_range(7, 2 * PAGE + 7, 1, &mut machine);
    assert_eq!(machine.releases, 2);
    assert_eq!(reader.preserve_disjoint(7, 1, 2, 2 * PAGE + 7, 1), 2);
    assert_eq!(writer.preserve_disjoint(7, 1, 2, 2 * PAGE + 7, 1), 2);
    machine.page(7, 2 * PAGE, 99);
    assert!(!writer.copy_from_cached(7, 2, 7, 2 * PAGE, &[17], &mut machine));
    for page in [PAGE, 3 * PAGE] {
        assert!(writer.copy_from_cached(7, 2, 7, page, &[19], &mut machine));
        assert!(reader.copy_into(7, 2, 7, page, &mut [MaybeUninit::uninit()], &mut machine));
    }
    assert!(writer.copy_from_cached(8, 1, 8, 2 * PAGE, &[21], &mut machine));
    assert_eq!(machine.grants, grants);
    for owner in [7, 8] {
        reader.clear_owner(owner, &mut machine);
        writer.clear_owner(owner, &mut machine);
    }
    assert!(machine.maps.is_empty());
}

#[test]
fn invalid_ranges_clear_the_owner_and_failed_range_releases_remain_inert_and_owned() {
    for (base, bytes) in [(0, PAGE), (PAGE, 0), (Word::MAX - 1, PAGE)] {
        let writer = WriteWindow::new();
        let mut machine = Machine::default();
        for owner in [7, 8] {
            machine.page(owner, PAGE, 11);
            assert!(writer.copy_from(owner, 1, owner, PAGE, &[12], &mut machine));
        }
        writer.clear_range(7, base, bytes, &mut machine);
        assert!(!writer.has_owner(7));
        assert!(writer.copy_from_cached(8, 1, 8, PAGE, &[13], &mut machine));
        writer.clear_owner(8, &mut machine);
        assert!(machine.maps.is_empty());
    }
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert!(writer.copy_from(7, 1, 7, PAGE, &[12], &mut machine));
    machine.fail_release = true;
    writer.clear_range(7, PAGE, PAGE, &mut machine);
    assert!(writer.has_owner(7));
    assert_eq!(machine.maps.len(), 1);
    assert_eq!(writer.preserve_disjoint(7, 1, 2, 8 * PAGE, PAGE), 0);
    assert!(!writer.copy_from_cached(7, 2, 7, PAGE, &[13], &mut machine));
    machine.fail_release = false;
    writer.clear_owner(7, &mut machine);
    assert!(!writer.has_owner(7));
    assert!(machine.maps.is_empty());
}

#[test]
fn adjacent_vm_owners_keep_same_stack_pages_without_repeated_grants_or_releases() {
    for page in [PAGE, 0x12345 * PAGE, 0x7fff_ffff_e000] {
        let reader = ReadWindow::new();
        let writer = WriteWindow::new();
        let mut machine = Machine::default();
        for owner in 32..40 { machine.page(owner, page, owner as u8); }
        for round in 0..64 {
            for owner in 32..40 {
                let value = (owner + round) as u8;
                machine.pages[&(owner,page)].borrow_mut()[..32].fill(value);
                assert_eq!(read(&reader, &mut machine, owner, 1, owner, page), Some([value;32]));
                assert!(writer.copy_from(owner, 1, owner, page + 8, &[value.wrapping_add(1);16], &mut machine));
                assert_eq!(&machine.pages[&(owner,page)].borrow()[8..24], &[value.wrapping_add(1);16]);
            }
        }
        let expected = if option_env!("NANAMI_LEASE_OWNER_HASH") == Some("0") { (1024,1016) } else { (16,0) };
        assert_eq!((machine.grants,machine.releases),expected);
        for owner in 32..40 {
            reader.clear_owner(owner,&mut machine);
            writer.clear_owner(owner,&mut machine);
        }
        assert!(machine.maps.is_empty());
    }
}

#[test]
fn successful_disjoint_mmaps_keep_exact_read_and_write_leases_without_more_effects() {
    for preserve in [false, true] {
        let reader = ReadWindow::new();
        let writer = WriteWindow::new();
        let mut machine = Machine::default();
        machine.page(7, PAGE, 11);
        let mut generation = 1;
        for index in 0..129 {
            if index != 0 {
                let next = (generation ^ (1usize << (usize::BITS - 7))) + 1;
                if preserve {
                    assert_eq!(reader.preserve_disjoint(7, generation, next, 8 * PAGE, PAGE), 1);
                    assert_eq!(writer.preserve_disjoint(7, generation, next, 8 * PAGE, PAGE), 1);
                }
                generation = next;
            }
            assert!(writer.copy_from(7, generation, 7, PAGE + 9, &[index as u8; 3], &mut machine));
            let mut output = [MaybeUninit::uninit(); 3];
            assert!(reader.copy_into(7, generation, 7, PAGE + 9, &mut output, &mut machine));
            assert!(output.iter().all(|byte| unsafe { byte.assume_init() } == index as u8));
        }
        assert_eq!(machine.grants, if preserve { 2 } else { 258 });
        assert_eq!(machine.releases, if preserve { 0 } else { 250 });
    }
}

#[test]
fn preservation_excludes_touched_pages_stale_foreign_and_uncertain_leases() {
    let reader = ReadWindow::new();
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for page in [PAGE, 2 * PAGE, 3 * PAGE] { machine.page(7, page, 11); }
    for page in [PAGE, 2 * PAGE, 3 * PAGE] {
        assert!(writer.copy_from(7, 1, 7, page, &[12], &mut machine));
        assert!(reader.copy_into(7, 1, 7, page, &mut [MaybeUninit::uninit()], &mut machine));
    }
    for (owner, before, after, base, bytes) in [(0,1,2,8*PAGE,PAGE), (8,1,2,8*PAGE,PAGE),
        (7,0,2,8*PAGE,PAGE), (7,1,0,8*PAGE,PAGE), (7,1,1,8*PAGE,PAGE),
        (7,9,2,8*PAGE,PAGE), (7,1,2,usize::MAX,2), (7,1,2,8*PAGE,0)] {
        assert_eq!(writer.preserve_disjoint(owner,before,after,base,bytes),0);
        assert_eq!(reader.preserve_disjoint(owner,before,after,base,bytes),0);
    }
    // Both neighboring pages merely touch this range. Even one touched byte
    // excludes the whole affected lease; replacing backing must not expose it.
    assert_eq!(writer.preserve_disjoint(7,1,2,2*PAGE+1,1),2);
    assert_eq!(reader.preserve_disjoint(7,1,2,2*PAGE+1,1),2);
    machine.page(7,2*PAGE,99);
    let effects = (machine.grants, machine.releases);
    assert!(!writer.copy_from_cached(7,2,7,2*PAGE,&[44],&mut machine));
    assert_eq!((machine.grants,machine.releases),effects);
    assert!(writer.copy_from_cached(7,2,7,PAGE,&[33],&mut machine));
    let mut fresh=[MaybeUninit::uninit()];
    assert!(reader.copy_into(7,2,7,2*PAGE,&mut fresh,&mut machine));
    assert_eq!(unsafe {fresh[0].assume_init()},99);
    assert_eq!(writer.preserve_disjoint(7,2,3,8*PAGE,PAGE),2);
    assert!(!writer.copy_from_cached(7,3,7,2*PAGE,&[44],&mut machine));
    // A failed grant and an unacknowledged release must never become Ready.
    let mut machine=Machine::default();machine.page(7,3*PAGE,11);
    let uncertain=WriteWindow::new();machine.uncertain=true;
    assert!(!uncertain.copy_from(7,1,7,3*PAGE,&[88],&mut machine));
    assert_eq!(uncertain.preserve_disjoint(7,1,2,8*PAGE,PAGE),0);
    assert!(!uncertain.copy_from_cached(7,2,7,3*PAGE,&[88],&mut machine));
    machine.fail_release=true;uncertain.clear_owner(7,&mut machine);
    assert_eq!(uncertain.preserve_disjoint(7,1,3,8*PAGE,PAGE),0);
}

#[test]
fn cached_vectors_check_every_field_before_stores_and_keep_overlap_order() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for page in [PAGE, PAGE * 17, PAGE * 33, PAGE * 49, PAGE * 65] {
        machine.page(7, page, 11);
    }
    assert!(writer.copy_from(7, 1, 7, PAGE, &[11], &mut machine));
    let buffers = [(PAGE + 3, &[1u8; 4][..]), (PAGE * 17, &[2u8; 4][..])];
    assert!(!writer.copy_from_many_cached(7, 1, 7, &buffers, &mut machine));
    assert_eq!(*machine.pages[&(7, PAGE)].borrow(), [11; PAGE]);
    assert_eq!((machine.grants, machine.releases), (1, 0));
    assert!(writer.copy_from(7, 1, 7, PAGE * 17, &[11], &mut machine));
    let overlap = [(PAGE + 3, &[1u8; 4][..]), (PAGE + 5, &[2u8; 4][..]),
        (PAGE * 17, &[3u8; 4][..])];
    for _ in 0..2048 { assert!(writer.copy_from_many_cached(7, 1, 7, &overlap, &mut machine)); }
    assert_eq!((machine.grants, machine.releases), (2, 0));
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[3..9], &[1, 1, 2, 2, 2, 2]);
    for page in [PAGE * 33, PAGE * 49, PAGE * 65] {
        assert!(writer.copy_from(7, 1, 7, page, &[11], &mut machine));
    }
    let effects = (machine.grants, machine.releases);
    assert!(!writer.copy_from_many_cached(7, 1, 7, &buffers, &mut machine));
    for (owner, generation) in [(8, 1), (7, 2), (7, 0)] {
        assert!(!writer.copy_from_many_cached(owner, generation, 7, &overlap, &mut machine));
    }
    assert_eq!((machine.grants, machine.releases), effects);
}

#[test]
fn cached_write_preflights_all_pages_without_grant_or_release() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for page in [PAGE, PAGE * 2] { machine.page(7, page, 11); }
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE, &[99], &mut machine));
    assert_eq!((machine.grants, machine.releases), (0, 0));
    assert!(writer.copy_from(7, 1, 7, PAGE, &[12], &mut machine));
    let before = *machine.pages[&(7, PAGE)].borrow();
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE * 2 - 8, &[99; 16], &mut machine));
    assert_eq!(*machine.pages[&(7, PAGE)].borrow(), before);
    assert_eq!((machine.grants, machine.releases), (1, 0));
    for (owner, generation) in [(8, 1), (7, 2), (7, 0)] {
        assert!(!writer.copy_from_cached(owner, generation, 7, PAGE, &[99], &mut machine));
    }
    assert!(writer.copy_from(7, 1, 7, PAGE * 2, &[12], &mut machine));
    for _ in 0..2048 {
        assert!(writer.copy_from_cached(7, 1, 7, PAGE * 2 - 8, &[37; 16], &mut machine));
    }
    assert_eq!((machine.grants, machine.releases), (2, 0));
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[PAGE-8..], &[37; 8]);
    assert_eq!(&machine.pages[&(7, PAGE * 2)].borrow()[..8], &[37; 8]);
    machine.fail_release = true;
    writer.clear_owner(7, &mut machine);
    let effects = (machine.grants, machine.releases);
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE, &[99], &mut machine));
    assert_eq!((machine.grants, machine.releases), effects);
}

#[test]
fn owner_presence_includes_stale_and_uncertain_leases_until_release() {
    let reader = ReadWindow::new();
    let writer = WriteWindow::new();
    let mut machine = Machine::default(); machine.page(7, PAGE, 11);
    assert!(!reader.has_owner(7)); assert!(!writer.has_owner(7));
    assert!(reader.copy_into(7, 1, 7, PAGE, &mut [MaybeUninit::uninit(); 1], &mut machine));
    assert!(reader.has_owner(7)); assert!(!reader.has_owner(8));
    machine.fail_release = true;
    reader.clear_owner(7, &mut machine);
    assert!(reader.has_owner(7));
    // A spare way can hold the new generation while the stale lease remains.
    assert!(reader.copy_into(7, 2, 7, PAGE, &mut [MaybeUninit::uninit(); 1], &mut machine));
    assert!(reader.has_owner(7));
    machine.fail_release = false; reader.clear_owner(7, &mut machine);
    assert!(!reader.has_owner(7));
    machine.uncertain = true; machine.fail_release = true;
    assert!(!writer.copy_from(7, 1, 7, PAGE, &[99], &mut machine));
    assert!(writer.has_owner(7)); assert!(!writer.has_owner(8));
    writer.clear_owner(7, &mut machine); assert!(writer.has_owner(7));
    machine.fail_release = false; writer.clear_owner(7, &mut machine);
    assert!(!writer.has_owner(7));
}

#[test]
fn contiguous_large_writes_prepare_all_pages_and_reuse_every_lease() {
    for offset in [0, 1, 7, PAGE - 1] {
        for len in [PAGE + 1, 2 * PAGE, MAX_CONTIGUOUS_WRITE_BYTES - 1, MAX_CONTIGUOUS_WRITE_BYTES] {
            let writer = WriteWindow::new();
            let mut machine = Machine::default();
            let pages = (offset + len).div_ceil(PAGE);
            for i in 0..pages { machine.page(7, PAGE * (i + 1), 0x33); }
            let mut expected = vec![0x33; pages * PAGE];
            if len > 4 * PAGE {
                assert!(!writer.copy_from(7, 1, 7, PAGE + offset, &vec![0xa5; len], &mut machine));
                assert_eq!((machine.grants, machine.releases), (0, 0));
            }
            for epoch in 0..3 {
                let bytes: Vec<_> = (0..len).map(|i| (i * 37 + epoch * 11) as u8).collect();
                assert!(writer.copy_from(7, 1, 7, PAGE + offset, &bytes, &mut machine));
                expected[offset..offset + len].copy_from_slice(&bytes);
                for i in 0..pages {
                    assert_eq!(&machine.pages[&(7, PAGE * (i + 1))].borrow()[..], &expected[i * PAGE..(i + 1) * PAGE]);
                }
                assert_eq!(machine.grants, pages);
            }
            writer.clear_owner(7, &mut machine);
            assert!(machine.maps.is_empty());
        }
    }
}

#[test]
fn large_write_missing_any_page_never_stores_a_prefix() {
    let pages = MAX_CONTIGUOUS_WRITE_BYTES / PAGE + 1;
    for missing in 0..pages {
        let writer = WriteWindow::new();
        let mut machine = Machine::default();
        for i in 0..pages {
            if i != missing { machine.page(7, PAGE * (i + 1), 0x55); }
        }
        assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &vec![0x77; MAX_CONTIGUOUS_WRITE_BYTES], &mut machine));
        assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &vec![0x77; MAX_CONTIGUOUS_WRITE_BYTES], &mut machine));
        for page in machine.pages.values() { assert_eq!(&page.borrow()[..], &[0x55; PAGE]); }
        writer.clear_owner(7, &mut machine);
        assert!(machine.maps.is_empty());
    }
}

#[test]
fn repeated_owned_writes_reuse_pins_and_do_not_overlap_read_aliases() {
    let reader = ReadWindow::new();
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert_eq!(
        read(&reader, &mut machine, 7, 1, 7, PAGE + 3),
        Some([11; 32])
    );
    for value in 12..40 {
        assert!(writer.copy_from(7, 1, 7, PAGE + 3, &[value; 32], &mut machine));
        assert_eq!(
            read(&reader, &mut machine, 7, 1, 7, PAGE + 3),
            Some([value; 32])
        );
    }
    assert_eq!(machine.grants, 2);
    assert_eq!(machine.maps.len(), 2);
    let backing = machine.pages[&(7, PAGE)].borrow();
    assert_eq!(&backing[..3], &[11; 3]);
    assert_eq!(&backing[35..], &[11; PAGE - 35]);
    drop(backing);
    reader.clear_owner(7, &mut machine);
    writer.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}

#[test]
fn wrapped_large_preflight_rejects_an_earlier_hit_evicted_by_its_last_page() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for page in (1..=17).chain([33, 49, 65]) { machine.page(7, PAGE * page, 0x55); }
    // Fill all four ways of the first set. Its next victim is the first page,
    // but hitting that page does not advance the replacement cursor.
    for page in [1, 33, 49, 65] {
        assert!(writer.copy_from(7, 1, 7, PAGE * page, &[0x55], &mut machine));
    }
    let bytes = vec![0x77; 64 * 1024];
    assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &bytes, &mut machine));
    assert_eq!(machine.releases, 0); // Cold admission keeps the checked copy.
    assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &bytes, &mut machine));
    assert_eq!(machine.releases, 1);
    assert!(machine.pages.values().all(|page| page.borrow().iter().all(|&b| b == 0x55)));
    let calls = (machine.grants, machine.releases);
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE + 1, &bytes, &mut machine));
    assert_eq!((machine.grants, machine.releases), calls);
    // The normal fallback has not been given a partially written prefix.
    // A later successful preparation can replace another old way and recover.
    assert!(writer.copy_from(7, 1, 7, PAGE + 1, &bytes, &mut machine));
    let calls = (machine.grants, machine.releases);
    for epoch in 0..128 {
        let bytes = vec![epoch as u8; 64 * 1024];
        assert!(writer.copy_from_cached(7, 1, 7, PAGE + 1, &bytes, &mut machine));
        for page in 1..=17 {
            let backing = machine.pages[&(7, page * PAGE)].borrow();
            let begin = if page == 1 { 1 } else { 0 };
            let end = if page == 17 { 1 } else { PAGE };
            assert!(backing[begin..end].iter().all(|&b| b == epoch as u8));
        }
    }
    assert_eq!((machine.grants, machine.releases), calls);
    assert_eq!(machine.pages[&(7, PAGE)].borrow()[0], 0x55);
    assert!(machine.pages[&(7, 17 * PAGE)].borrow()[1..].iter().all(|&b| b == 0x55));
    for page in [33, 49, 65] { assert!(machine.pages[&(7, page * PAGE)].borrow().iter().all(|&b| b == 0x55)); }
    writer.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}

#[test]
fn uncertain_last_page_of_a_64k_copy_never_stores_or_reuses_an_unconfirmed_alias() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    for page in 1..=17 { machine.page(7, PAGE * page, 0x55); }
    for page in 1..=16 {
        assert!(writer.copy_from(7, 1, 7, PAGE * page, &[0x55], &mut machine));
    }
    machine.uncertain = true;
    assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &vec![0x77; 64 * 1024], &mut machine));
    assert_eq!(machine.grants, 16); // Admission has not requested the last page.
    assert!(!writer.copy_from(7, 1, 7, PAGE + 1, &vec![0x77; 64 * 1024], &mut machine));
    assert_eq!(machine.grants, 17);
    let calls = (machine.grants, machine.releases);
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE + 1, &vec![0x77; 64 * 1024], &mut machine));
    assert_eq!((machine.grants, machine.releases), calls);
    assert!(machine.pages.values().all(|page| page.borrow().iter().all(|&b| b == 0x55)));
    machine.fail_release = true;
    writer.clear_owner(7, &mut machine);
    assert!(writer.has_owner(7));
    let calls = (machine.grants, machine.releases);
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE + 1, &vec![0x77; 64 * 1024], &mut machine));
    assert_eq!((machine.grants, machine.releases), calls);
    machine.fail_release = false;
    writer.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}

#[test]
fn one_shot_large_copies_and_cached_misses_never_prime_authority_or_issue_grants() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    let bytes = vec![0x77; 64 * 1024];
    assert!(!writer.copy_from_cached(7, 1, 7, PAGE + 1, &bytes, &mut machine));
    for (owner, generation, destination) in [(7,1,PAGE+1), (7,1,2*PAGE+1),
        (8,1,2*PAGE+1), (8,2,2*PAGE+1)] {
        assert!(!writer.copy_from(owner, generation, 7, destination, &bytes, &mut machine));
        assert_eq!((machine.grants, machine.releases), (0, 0));
    }
    writer.clear_owner(8, &mut machine);
    assert!(!writer.copy_from(8, 2, 7, 2 * PAGE + 1, &bytes, &mut machine));
    assert_eq!((machine.grants, machine.releases), (0, 0));
    assert!(!writer.has_owner(8));
}

#[test]
fn write_prepares_both_pages_before_any_external_store() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert!(!writer.copy_from(7, 1, 7, PAGE * 2 - 7, &[99; 32], &mut machine));
    assert_eq!(*machine.pages[&(7, PAGE)].borrow(), [11; PAGE]);
    machine.page(7, PAGE * 2, 22);
    assert!(writer.copy_from(7, 1, 7, PAGE * 2 - 7, &[99; 32], &mut machine));
    let first = machine.pages[&(7, PAGE)].borrow();
    let second = machine.pages[&(7, PAGE * 2)].borrow();
    assert_eq!(&first[..PAGE - 7], &[11; PAGE - 7]);
    assert_eq!(&first[PAGE - 7..], &[99; 7]);
    assert_eq!(&second[..25], &[99; 25]);
    assert_eq!(&second[25..], &[22; PAGE - 25]);
}

#[test]
fn write_generation_change_cannot_modify_replaced_backing() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert!(writer.copy_from(7, 1, 7, PAGE, &[12; 32], &mut machine));
    let old = machine.pages[&(7, PAGE)].clone();
    machine.page(7, PAGE, 22);
    assert!(writer.copy_from(7, 2, 7, PAGE, &[23; 32], &mut machine));
    assert_eq!(&old.borrow()[..32], &[12; 32]);
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[..32], &[23; 32]);
    assert!(!writer.copy_from(7, 0, 7, PAGE, &[24; 32], &mut machine));
    assert!(!writer.copy_from(7, 2, 7, usize::MAX - 10, &[24; 32], &mut machine));
    assert!(!writer.copy_from(7, 2, 7, PAGE, &[24; MAX_CONTIGUOUS_WRITE_BYTES + 1], &mut machine));
}

#[test]
fn write_uncertainty_and_failed_release_never_publish_a_hit_or_store() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    machine.uncertain = true;
    machine.fail_release = true;
    for _ in 0..20 {
        assert!(!writer.copy_from(7, 1, 7, PAGE, &[99; 32], &mut machine));
    }
    assert_eq!(*machine.pages[&(7, PAGE)].borrow(), [11; PAGE]);
    assert_eq!(machine.maps.len(), 4);
    writer.clear_owner(7, &mut machine);
    assert_eq!(machine.maps.len(), 4);
    machine.fail_release = false;
    machine.uncertain = false;
    writer.clear_owner(7, &mut machine);
    assert!(writer.copy_from(7, 1, 7, PAGE, &[99; 32], &mut machine));
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[..32], &[99; 32]);
}

#[test]
fn write_vm_peers_share_only_the_current_owner_and_generation() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert!(writer.copy_from(7, 1, 7, PAGE, &[12; 32], &mut machine));
    assert!(writer.copy_from(7, 1, 8, PAGE, &[13; 32], &mut machine));
    assert_eq!(machine.grants, 1);
    machine.page(8, PAGE, 22);
    assert!(writer.copy_from(8, 1, 8, PAGE, &[23; 32], &mut machine));
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[..32], &[13; 32]);
    assert_eq!(&machine.pages[&(8, PAGE)].borrow()[..32], &[23; 32]);
}

#[test]
fn owned_write_vectors_keep_order_on_overlapping_ranges_and_reuse_leases() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    let buffers = [(PAGE + 3, &[1u8, 2, 3, 4][..]), (PAGE + 5, &[8u8, 9][..])];
    for _ in 0..10 {
        assert!(writer.copy_from_many(7, 1, 7, &buffers, &mut machine));
    }
    assert_eq!(machine.grants, 1);
    let page = machine.pages[&(7, PAGE)].borrow();
    assert_eq!(&page[..3], &[11; 3]);
    assert_eq!(&page[3..7], &[1, 2, 8, 9]);
    assert_eq!(&page[7..], &[11; PAGE - 7]);
}

#[test]
fn vector_preparation_failure_never_stores_an_earlier_field_or_page_prefix() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    let buffers = [(PAGE + 3, &[1u8; 4][..]), (PAGE * 2 - 2, &[2u8; 4][..])];
    assert!(!writer.copy_from_many(7, 1, 7, &buffers, &mut machine));
    assert_eq!(*machine.pages[&(7, PAGE)].borrow(), [11; PAGE]);
    machine.page(7, PAGE * 2, 22);
    assert!(writer.copy_from_many(7, 1, 7, &buffers, &mut machine));
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[3..7], &[1; 4]);
    assert_eq!(&machine.pages[&(7, PAGE)].borrow()[PAGE - 2..], &[2; 2]);
    assert_eq!(&machine.pages[&(7, PAGE * 2)].borrow()[..2], &[2; 2]);
}

#[test]
fn a_later_conflicting_grant_cannot_redirect_an_earlier_vector_store() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    // All five pages hash to the same four-way set. Its next victim is the
    // first page, even when that page was a hit in this operation's first field.
    for index in 0..5 {
        let page = (1 + index * 16) * PAGE;
        machine.page(7, page, 11);
        if index < 4 {
            assert!(writer.copy_from(7, 1, 7, page, &[11], &mut machine));
        }
    }
    let buffers = [(PAGE, &[33u8][..]), (65 * PAGE, &[44u8][..])];
    assert!(!writer.copy_from_many(7, 1, 7, &buffers, &mut machine));
    for page in machine.pages.values() {
        assert_eq!(*page.borrow(), [11; PAGE]);
    }
    assert_eq!(machine.maps.len(), 4);
}

#[test]
fn vector_limits_and_generation_reject_before_grants_or_stores() {
    let writer = WriteWindow::new();
    let mut machine = Machine::default();
    let small = [(PAGE, &[1u8][..]); MAX_WRITE_BUFFERS + 1];
    let large = [(PAGE, &[1u8; PAGE][..]), (PAGE, &[2u8][..])];
    assert!(!writer.copy_from_many(7, 1, 7, &small, &mut machine));
    assert!(!writer.copy_from_many(7, 1, 7, &large, &mut machine));
    assert!(!writer.copy_from_many(7, 0, 7, &[(PAGE, &[1])], &mut machine));
    assert!(!writer.copy_from_many(7, 1, 7, &[(usize::MAX, &[1])], &mut machine));
    assert!(writer.copy_from_many(7, 1, 7, &[], &mut machine));
    assert_eq!(machine.grants, 0);
    assert!(machine.maps.is_empty());
}
fn read(
    window: &ReadWindow,
    machine: &mut Machine,
    owner: Word,
    generation: Word,
    pid: Word,
    address: Word,
) -> Option<[u8; 32]> {
    let mut bytes = [MaybeUninit::uninit(); 32];
    if !window.copy_into(owner, generation, pid, address, &mut bytes, machine) {
        return None;
    }
    Some(bytes.map(|byte| unsafe { byte.assume_init() }))
}

#[test]
fn large_read_views_sample_current_bytes_and_reuse_all_five_pages() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    for i in 0..5 { machine.page(7, PAGE * (i + 1), 0); }
    for epoch in 0..5 {
        for i in 0..5 { machine.pages[&(7, PAGE * (i + 1))].borrow_mut().fill((epoch * 11 + i) as u8); }
        let mut bytes = vec![MaybeUninit::new(0xab); MAX_CONTIGUOUS_READ_BYTES];
        assert!(window.copy_into(7, 1, 7, PAGE + 19, &mut bytes, &mut machine));
        for (i, byte) in bytes.iter().enumerate() {
            assert_eq!(unsafe { byte.assume_init() }, (epoch * 11 + (i + 19) / PAGE) as u8);
        }
        assert_eq!(machine.grants, 5);
    }
    assert!(!window.copy_into(7, 1, 7, PAGE, &mut vec![MaybeUninit::uninit(); MAX_CONTIGUOUS_READ_BYTES + 1], &mut machine));
    window.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}

#[test]
fn large_read_missing_later_page_returns_false_with_only_local_prefix_sampled() {
    for missing in 0..5 {
        let window = ReadWindow::new();
        let mut machine = Machine::default();
        for i in 0..5 { if i != missing { machine.page(7, PAGE * (i + 1), 0x55); } }
        let mut bytes = vec![MaybeUninit::new(0xab); MAX_CONTIGUOUS_READ_BYTES];
        assert!(!window.copy_into(7, 1, 7, PAGE + 19, &mut bytes, &mut machine));
        let sampled = (missing * PAGE).saturating_sub(19);
        for (i, byte) in bytes.iter().enumerate() {
            assert_eq!(unsafe { byte.assume_init() }, if i < sampled { 0x55 } else { 0xab });
        }
        window.clear_owner(7, &mut machine);
        assert!(machine.maps.is_empty());
    }
}

#[test]
fn repeated_reads_reuse_a_lease_but_never_cache_contents() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    for value in 11..40 {
        machine.pages[&(7, PAGE)].borrow_mut().fill(value);
        assert_eq!(
            read(&window, &mut machine, 7, 1, 7, PAGE + 3),
            Some([value; 32])
        );
    }
    assert_eq!(machine.grants, 1);
    window.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}

#[test]
fn backing_replacement_and_new_generations_cannot_hit_old_bytes() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), Some([11; 32]));
    let old = machine.pages[&(7, PAGE)].clone();
    machine.page(7, PAGE, 22);
    assert_eq!(read(&window, &mut machine, 7, 2, 7, PAGE), Some([22; 32]));
    assert_eq!(old.borrow()[0], 11);
    assert_eq!(machine.grants, 2);
    assert_eq!(read(&window, &mut machine, 7, 0, 7, PAGE), None);
}

#[test]
fn vm_peers_share_hits_and_unrelated_owners_do_not() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), Some([11; 32]));
    assert_eq!(read(&window, &mut machine, 7, 1, 8, PAGE), Some([11; 32]));
    assert_eq!(machine.grants, 1);
    machine.page(8, PAGE, 22);
    assert_eq!(read(&window, &mut machine, 8, 1, 8, PAGE), Some([22; 32]));
    window.clear_owner(7, &mut machine);
    assert_eq!(read(&window, &mut machine, 8, 1, 8, PAGE), Some([22; 32]));
}

#[test]
fn page_boundary_reads_preserve_order_and_exact_lengths() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    machine.page(7, PAGE * 2, 22);
    let bytes = read(&window, &mut machine, 7, 1, 7, PAGE * 2 - 7).unwrap();
    assert_eq!(&bytes[..7], &[11; 7]);
    assert_eq!(&bytes[7..], &[22; 25]);
    assert_eq!(machine.grants, 2);
    assert_eq!(read(&window, &mut machine, 7, 1, 7, usize::MAX - 10), None);
}

#[test]
fn second_page_failure_never_reports_a_partly_initialized_buffer_as_complete() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    let mut bytes = [MaybeUninit::new(99); 32];
    assert!(!window.copy_into(7, 1, 7, PAGE * 2 - 7, &mut bytes, &mut machine));
    let initialized = bytes.map(|byte| unsafe { byte.assume_init() });
    assert_eq!(&initialized[..7], &[11; 7]);
    assert_eq!(&initialized[7..], &[99; 25]);
    assert_eq!(machine.maps.len(), 1);
    machine.page(7, PAGE * 2, 22);
    assert!(window.copy_into(7, 1, 7, PAGE * 2 - 7, &mut bytes, &mut machine));
    let initialized = bytes.map(|byte| unsafe { byte.assume_init() });
    assert_eq!(&initialized[..7], &[11; 7]);
    assert_eq!(&initialized[7..], &[22; 25]);
}

#[test]
fn denied_creation_is_not_a_valid_hit_and_can_be_retried() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), None);
    machine.page(7, PAGE, 11);
    assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), Some([11; 32]));
    assert_eq!(machine.releases, 0);
}

#[test]
fn ambiguous_creation_and_failed_cleanup_never_overwrite_or_return_a_hit() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    machine.page(7, PAGE, 11);
    machine.uncertain = true;
    machine.fail_release = true;
    for _ in 0..20 {
        assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), None);
    }
    assert_eq!(machine.maps.len(), 4);
    window.clear_owner(7, &mut machine);
    assert_eq!(machine.maps.len(), 4);
    machine.fail_release = false;
    machine.uncertain = false;
    window.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
    assert_eq!(read(&window, &mut machine, 7, 1, 7, PAGE), Some([11; 32]));
}

#[test]
fn successful_eviction_is_bounded_and_drops_old_pins() {
    let window = ReadWindow::new();
    let mut machine = Machine::default();
    for index in 1..1000 {
        let address = index * PAGE;
        machine.page(7, address, index as u8);
        assert_eq!(
            read(&window, &mut machine, 7, 1, 7, address),
            Some([index as u8; 32])
        );
        assert!(machine.maps.len() <= 64);
    }
    assert!(machine.releases > 900);
    window.clear_owner(7, &mut machine);
    assert!(machine.maps.is_empty());
}
