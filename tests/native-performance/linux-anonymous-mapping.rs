//! Real allocation transaction; model Alpha transport and VMA publication.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
const ENOMEM: i32 = 12;
const LINUX_PAGE_SIZE: Word = 4096;
const BASE: Word = 0x100000;
#[derive(Debug, PartialEq)]
enum Event { AutoFinal(Word, Word, Word), Final(Word, Word, Word, Word, Word, Word), Dynamic(Word, Word), Fixed(Word, Word, Word), Add(Word, Word, Word), Protect(Word, Word, Word), Release(Word, Word), Unmap(Word, Word) }
#[derive(Default)]
struct Effects { events: Vec<Event>, construct_error: bool, release_error: bool, protect_error: bool, response: Option<(Word, Word)> }
thread_local! { static EFFECTS: RefCell<Effects> = RefCell::new(Effects::default()); }
fn event(e: Event) { EFFECTS.with_borrow_mut(|s| s.events.push(e)); }
fn map_request_error(e: i32) -> i32 { e }
mod private_memory {
    pub(super) fn with_mapping_mutation<T>(_: &super::Runtime, _: usize, _: usize, _: usize, _: bool,
        operation: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> { operation() }
}
pub fn request_process_map_initialized_protected_at(pid: Word, base: Word, bytes: Word, source: Word, copied: Word, prot: Word) -> Result<(), i32> {
    event(Event::Final(pid, base, bytes, source, copied, prot));
    if base == 0 || bytes == 0 || (base | bytes) & 4095 != 0 { return Err(22); }
    EFFECTS.with_borrow(|s| if s.construct_error { Err(5) } else { Ok(()) })
}
pub fn request_process_map_anonymous(pid: Word, bytes: Word) -> Result<(Word, Word), i32> {
    event(Event::Dynamic(pid, bytes)); answer(bytes)
}
pub fn request_process_map_anonymous_protected(pid: Word, bytes: Word, prot: Word) -> Result<(Word, Word), i32> {
    event(Event::AutoFinal(pid, bytes, prot)); answer(bytes)
}
pub fn request_process_map_anonymous_at(pid: Word, base: Word, bytes: Word) -> Result<(Word, Word), i32> {
    event(Event::Fixed(pid, base, bytes)); answer(bytes)
}
fn answer(bytes: Word) -> Result<(Word, Word), i32> { EFFECTS.with_borrow(|s| if s.construct_error { Err(5) } else { Ok(s.response.unwrap_or((BASE, (bytes+4095)&!4095))) }) }
pub fn request_process_mapping_release(_: Word, base: Word, bytes: Word) -> Result<(), i32> {
    event(Event::Release(base, bytes)); EFFECTS.with_borrow(|s| if s.release_error { Err(9) } else { Ok(()) })
}
pub fn request_process_mapping_protect(_: Word, base: Word, bytes: Word, prot: Word) -> Result<(), i32> {
    event(Event::Protect(base, bytes, prot)); EFFECTS.with_borrow(|s| if s.protect_error { Err(13) } else { Ok(()) })
}
struct Runtime { reject: bool }
impl Runtime { fn add_mapping(&mut self, _: Word, base: Word, bytes: Word, prot: Word) -> bool { event(Event::Add(base, bytes, prot)); !self.reject } }
fn sys_munmap(_: &mut Runtime, _: Word, base: Word, bytes: Word) -> Result<Word, i32> { event(Event::Unmap(base, bytes)); Ok(0) }
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/anonymous_mapping.rs"]
mod production;
fn reset() -> Runtime { EFFECTS.with_borrow_mut(|s| *s=Effects::default()); Runtime { reject: false } }
fn events() -> Vec<Event> { EFFECTS.with_borrow_mut(|s| std::mem::take(&mut s.events)) }

#[test]
fn fixed_zero_backing_uses_one_request_and_publishes_final_rights() {
    for prot in 1..=7 { for len in [1, 4096, 4097, 64*4096] {
        let mut r=reset();let bytes=(len+4095)&!4095;
        assert_eq!(production::map(&mut r,7,Some(BASE),len,prot),Ok((BASE,bytes)));
        assert_eq!(events(),vec![Event::Final(7,BASE,bytes,0,0,prot),Event::Add(BASE,bytes,prot)]);
    }}
}
#[test]
fn none_and_invalid_rights_keep_original_constructor_and_protection() {
    for requested in [None,Some(BASE)] { for prot in 0..=8 {
        if (1..=7).contains(&prot) {continue}
        let mut r=reset();assert_eq!(production::map(&mut r,7,requested,4097,prot),Ok((BASE,8192)));
        let first=if requested.is_some() {Event::Fixed(7,BASE,4097)} else {Event::Dynamic(7,4097)};
        assert_eq!(events(),vec![first,Event::Add(BASE,8192,prot),Event::Protect(BASE,8192,prot)]);
    }}
}
#[test]
fn constructor_failure_never_publishes_or_retries() {
    for requested in [None,Some(BASE)] {
        let mut r=reset();EFFECTS.with_borrow_mut(|s|s.construct_error=true);
        assert_eq!(production::map(&mut r,7,requested,4096,3),Err(5));assert_eq!(events().len(),1);
    }
}
#[test]
fn rejected_publication_releases_new_backing_and_preserves_release_error() {
    for release_error in [false,true] {
        let mut r=reset();r.reject=true;EFFECTS.with_borrow_mut(|s|s.release_error=release_error);
        assert_eq!(production::map(&mut r,7,Some(BASE),4097,3),Err(if release_error {9} else {ENOMEM}));
        assert_eq!(events(),vec![Event::Final(7,BASE,8192,0,0,3),Event::Add(BASE,8192,3),Event::Release(BASE,8192)]);
    }
}
#[test]
fn legacy_bad_response_and_protection_failure_cleanup_are_preserved() {
    let mut r=reset();EFFECTS.with_borrow_mut(|s|s.response=Some((BASE,4096)));
    assert_eq!(production::map(&mut r,7,None,8192,3),Err(ENOMEM));
    assert_eq!(events(),vec![Event::AutoFinal(7,8192,3),Event::Release(BASE,4096)]);
    let mut r=reset();EFFECTS.with_borrow_mut(|s|s.protect_error=true);
    assert_eq!(production::map(&mut r,7,None,4096,0),Err(13));
    assert_eq!(events(),vec![Event::Dynamic(7,4096),Event::Add(BASE,4096,0),Event::Protect(BASE,4096,0),Event::Unmap(BASE,4096)]);
}
#[test]
fn automatic_placement_publishes_final_rights_without_protect() {
    for prot in 1..=7 { for len in [1,4096,4097,256*4096] {
        let mut r=reset(); let mapped=(len+4095)&!4095;
        assert_eq!(production::map(&mut r,7,None,len,prot),Ok((BASE,mapped)));
        assert_eq!(events(),vec![Event::AutoFinal(7,len,prot),Event::Add(BASE,mapped,prot)]);
    }}
}
#[test]
fn automatic_publication_failure_releases_without_retry_or_protect() {
    for release_error in [false,true] {
        let mut r=reset();r.reject=true;EFFECTS.with_borrow_mut(|s|s.release_error=release_error);
        assert_eq!(production::map(&mut r,7,None,4097,3),Err(if release_error {9} else {ENOMEM}));
        assert_eq!(events(),vec![Event::AutoFinal(7,4097,3),Event::Add(BASE,8192,3),Event::Release(BASE,8192)]);
    }
}
#[test]
fn fixed_rounding_overflow_and_invalid_address_do_not_publish() {
    let mut r=reset();assert_eq!(production::map(&mut r,7,Some(BASE),usize::MAX,3),Err(ENOMEM));assert!(events().is_empty());
    for base in [0,BASE+1] {let mut r=reset();assert_eq!(production::map(&mut r,7,Some(base),4096,3),Err(22));assert_eq!(events().len(),1);}
}
