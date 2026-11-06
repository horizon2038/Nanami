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
    assert!(!writer.copy_from(7, 2, 7, PAGE, &[24; PAGE + 1], &mut machine));
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
