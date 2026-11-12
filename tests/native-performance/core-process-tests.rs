#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;

thread_local! {
    static PROTECT_ALLOCATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
struct CountedAllocator;
fn count_allocation() {
    let _=PROTECT_ALLOCATIONS.try_with(|count| {
        if let Some(n)=count.get() {count.set(Some(n+1));}
    });
}
unsafe impl std::alloc::GlobalAlloc for CountedAllocator {
    unsafe fn alloc(&self,layout:std::alloc::Layout)->*mut u8 {
        let result=std::alloc::GlobalAlloc::alloc(&std::alloc::System,layout);
        if !result.is_null() {count_allocation();} result
    }
    unsafe fn realloc(&self,pointer:*mut u8,layout:std::alloc::Layout,bytes:usize)->*mut u8 {
        let result=std::alloc::GlobalAlloc::realloc(&std::alloc::System,pointer,layout,bytes);
        if !result.is_null() {count_allocation();} result
    }
    unsafe fn dealloc(&self,pointer:*mut u8,layout:std::alloc::Layout) {
        std::alloc::GlobalAlloc::dealloc(&std::alloc::System,pointer,layout);
    }
}
#[global_allocator]
static ALLOCATOR:CountedAllocator=CountedAllocator;

pub type Word = usize;
pub type CapabilityDescriptor = usize;
pub mod capability_call {
    pub use a9n_abi::capability_call::process_control_block;
    pub mod address_space {
        pub use a9n_abi::capability_call::address_space::Attribute;
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityError {
    IllegalOperation,
    InvalidArgument,
    PermissionDenied,
    Fatal,
}
#[macro_export]
macro_rules! info {
    ($($args:tt)*) => {};
}
#[macro_export]
macro_rules! error {
    ($($args:tt)*) => {};
}

#[path = "../../nanami/src/nanami_utils/avl.rs"]
pub mod avl;
#[path = "../../nanami/src/nanami_utils/static_avl.rs"]
pub mod static_avl;
pub mod nanami_utils {
    pub use crate::{avl, static_avl};
}
#[path = "../../nanami/src/nanami_core/vm_space.rs"]
pub mod vm_space;
pub mod nanami_core {
    pub use crate::vm_space;
    pub mod elf_loader {
        #[derive(Clone, Copy)]
        pub struct ElfImage;
    }
    pub mod process {
        include!(env!("NANAMI_PROCESS_SOURCE"));
    }
}
use nanami_core::process::ProcessManager;
use vm_space::VmTracker;

#[path = "shared-memory-reservations.rs"]
mod shared_memory_reservations;

#[path = "allocation-index.rs"]
mod allocation_index;

#[path = "alpha-mapping-release.rs"]
mod mapping_release_tests;
#[path = "alpha-mapping-protection.rs"]
mod mapping_protection_tests;

#[path = "alpha-checked-memory.rs"]
mod checked_memory_tests;
#[path = "alpha-memory-clone.rs"]
mod memory_clone_tests;
#[path = "alpha-memory-private.rs"]
mod memory_private_tests;
#[path = "alpha-memory-staging.rs"]
mod memory_staging_tests;
#[path = "alpha-capability-sealing.rs"]
mod capability_sealing_tests;
#[path = "alpha-memory-retirement.rs"]
mod memory_retirement_tests;
#[path = "alpha-exec-retirement.rs"]
mod exec_retirement_tests;
#[path = "alpha-exec-space.rs"]
mod exec_space_tests;
#[path = "shared-memory-lifecycle.rs"]
mod shared_memory_lifecycle;

fn manager() -> ProcessManager {
    ProcessManager::new_alpha(1, 2, 99, 3, 4096, &[], 4).unwrap()
}

#[test]
fn preferred_heap_alignment_keeps_exact_slots_shared_cursor_and_space_fallback() {
    let mut m = manager();
    install(&mut m, 1, 201);
    install(&mut m, 2, 202);
    m.share_address_space(2, 1).unwrap();
    let (_, _, small, first) = m.reserve_process_heap(1, 1, 4096, 4096).unwrap();
    assert_eq!(small, 0x200000);
    let (_, _, base, slot) = m.reserve_process_heap_preferred_alignment(2, 511, 4096, 4096, 1 << 21).unwrap();
    // The aligned location would exceed this fixture's four-MiB heap limit.
    assert_eq!(base, small + 4096);
    assert_eq!(slot, first + 1);
    let owner = m.find_entry_by_pid(1).unwrap();
    assert_eq!(owner.user_heap_next_va, base + 511 * 4096);
    assert_eq!(owner.next_frame_slot, first + 512);
    let before = (owner.user_heap_next_va, owner.next_frame_slot);
    for alignment in [0, 3] {
        assert!(m.reserve_process_heap_preferred_alignment(2, 1, 4096, 4096, alignment).is_err());
    }
    assert!(m.reserve_process_heap_preferred_alignment(2, usize::MAX, 1, usize::MAX, 1 << 21).is_err());
    let after = m.find_entry_by_pid(1).unwrap();
    assert_eq!((after.user_heap_next_va, after.next_frame_slot), before);
}

#[test]
fn preferred_heap_alignment_uses_no_slots_for_padding_and_falls_back_on_rounding_overflow() {
    let mut m = manager();
    install(&mut m, 1, 201);
    m.reserve_process_heap(1, 1, 4096, 4096).unwrap();
    let (_, _, base, slot) = m.reserve_process_heap_preferred_alignment(1, 1, 4096, 4096, 1 << 20).unwrap();
    assert_eq!(base, 0x300000);
    assert_eq!(slot, 1);
    let next = m.find_entry_by_pid(1).unwrap();
    assert_eq!(next.user_heap_next_va, base + 4096);
    assert_eq!(next.next_frame_slot, 2);
    m.install_process(3, 0, 203, 204, 205, 206, 207, 3, 0, 0,
        usize::MAX - 4095, usize::MAX).unwrap();
    let (_, _, base, slot) = m.reserve_process_heap_preferred_alignment(3, 1, 1, 4096, 1 << 21).unwrap();
    assert_eq!(base, usize::MAX - 4095);
    assert_eq!(slot, 0);
}

#[test]
fn batch_pool_registration_matches_individual_references_through_partial_retirement() {
    let base = 0x400000;
    let pages = 512;
    let first = 8192;
    let mut batch = manager();
    let mut scalar = manager();
    for m in [&mut batch, &mut scalar] {
        install(m, 1, 201);
        install(m, 2, 202);
        install(m, 3, 203);
        m.share_address_space(2, 1).unwrap();
        m.register_pool_alias(1, base + 7 * 4096, 800, 900).unwrap();
        m.register_pool_alias(3, 0x800000, 0, first + 8).unwrap();
    }
    for (pid, address, slot, physical, count) in [
        (0, base, 32, first, pages),
        (2, base + 1, 32, first, pages),
        (2, base, usize::MAX, first, pages),
        (2, base, 32, usize::MAX, pages),
        (2, base, 32, first, usize::MAX),
        (2, usize::MAX - 4095, 32, first, 1),
        (2, base, 32, first, 0),
    ] {
        assert!(batch.register_pool_page_run(pid, address, slot, physical, count).is_err());
    }
    assert!(batch.physical_pages_in_range(2, base, base + pages * 4096)
        == scalar.physical_pages_in_range(2, base, base + pages * 4096));
    batch.register_pool_page_run(2, base, 32, first, pages).unwrap();
    for offset in 0..pages {
        scalar.register_pool_alias(2, base + offset * 4096, 32 + offset, first + offset).unwrap();
    }
    for offset in 0..pages {
        let va = base + offset * 4096;
        let actual = batch.find_active_physical_allocation_reference(1, va, 1).unwrap();
        let expected = scalar.find_active_physical_allocation_reference(1, va, 1).unwrap();
        assert!(actual == expected);
        assert_eq!(batch.physical_reference_count(actual), 1 + usize::from(offset == 8));
    }
    for offset in [0, 8, 511] {
        let va = base + offset * 4096;
        for m in [&mut batch, &mut scalar] {
            m.defer_active_physical_allocation(2, va, 1).unwrap();
        }
    }
    assert!(batch.physical_pages_in_range(2, base, base + pages * 4096)
        == scalar.physical_pages_in_range(2, base, base + pages * 4096));
    assert!(batch.deferred_physical_allocations_for_pid(1)
        .eq(scalar.deferred_physical_allocations_for_pid(1)));
    assert!(batch.releasable_deferred_physical_allocations_for_pid(1)
        == scalar.releasable_deferred_physical_allocations_for_pid(1));
}

#[test]
fn retirement_page_order_matches_stable_snapshot_and_avoids_sort_scratch() {
    fn fields(pages: &[nanami_core::process::ProcessPhysicalAllocation]) -> Vec<(usize,usize,usize,bool)> {
        pages.iter().map(|p|(p.base_va,p.start_slot,p.base_page,p.owns_frame_cap)).collect()
    }
    fn old(m: &ProcessManager, pid: usize, start: usize, end: usize)
        -> Vec<nanami_core::process::ProcessPhysicalAllocation> {
        let mut pages: Vec<_> = m.private_page_reference_walk(pid,start,end)
            .map(|r|r.allocation).filter(|a| a.page_count==1 && a.base_va>=start && a.base_va<end
                && (a.base_va-start)%4096==0).collect();
        pages.sort_by_key(|a|a.base_va); pages.dedup_by_key(|a|a.base_va); pages
    }
    let mut m=manager(); install(&mut m,1,201); install(&mut m,2,202);
    m.share_address_space(2,1).unwrap();
    for page in 0..4096 { m.register_physical_allocation(1,(page+1)*4096,page,10000+page,1).unwrap(); }
    PROTECT_ALLOCATIONS.with(|count|count.set(Some(0)));
    let expected=old(&m,2,4096,4097*4096);
    let before=PROTECT_ALLOCATIONS.with(|count|count.replace(None).unwrap());
    PROTECT_ALLOCATIONS.with(|count|count.set(Some(0)));
    let actual=m.physical_pages_in_range(2,4096,4097*4096);
    let after=PROTECT_ALLOCATIONS.with(|count|count.replace(None).unwrap());
    assert_eq!(fields(&actual),fields(&expected));
    assert!(after<before,"old={before} new={after}");
    println!("RETIREMENT-PAGE-ORDER pages=4096 old-allocations={before} new-allocations={after}");
    // Disorder, duplicate VAs, pool/non-pool references, extents, alignment
    // relative to the requested start and current one-hop owner all keep the
    // previous stable newest-reference selection.
    for (index,va) in [4096,900*4096,100*4096,4097,usize::MAX,2*4096,900*4096].into_iter().enumerate() {
        m.register_pool_alias(1,va,5000+index,30000+index).unwrap();
    }
    m.register_physical_allocation(1,6000*4096,8000,40000,3).unwrap();
    for pid in [1,2,99] {
        for (start,end) in [(0,usize::MAX),(4096,4097*4096),(4097,10*4096),(4096,4096),(9000,1000),
            (899*4096,901*4096),(usize::MAX-4096,usize::MAX)] {
            assert_eq!(fields(&m.physical_pages_in_range(pid,start,end)),fields(&old(&m,pid,start,end)));
        }
    }
    let newest=m.physical_pages_in_range(1,900*4096,901*4096);
    assert_eq!(newest.len(),1); assert_eq!(newest[0].base_page,30006);
}

#[test]
fn fresh_stack_slots_are_single_use_owner_shared_and_available_at_cursor_limit() {
    let mut m=manager();install(&mut m,1,201);install(&mut m,2,202);
    m.seal_capabilities(1).unwrap();
    m.reset_runtime_memory_for_exec(1,4096,0x200000,0x400000).unwrap();
    m.register_fresh_stack_mapping(1,202,0x4000000,64,100).unwrap();
    m.share_address_space(2,1).unwrap();
    let mut claim_probes = 0;
    for page in 0..64 {
        let va=0x4000000+page*4096;
        ProcessManager::take_lazy_mapping_probes();
        let token=m.take_fresh_stack_frame_slot(2,va,4096).unwrap();
        claim_probes += ProcessManager::take_lazy_mapping_probes();
        assert_eq!(token.into_parts(1,va),Some((202,204,100+page)));
        assert!(m.take_fresh_stack_frame_slot(1,va,4096).is_none());
        assert!(m.take_fresh_stack_frame_slot(2,va,4096).is_none());
    }
    println!("fresh stack 64 successful claim mapping probes: {claim_probes}");
    assert_eq!(claim_probes, 64);
    assert_eq!(m.find_entry_by_pid(1).unwrap().next_frame_slot,4096);
}

#[test]
fn fresh_stack_claim_does_not_reappear_when_backing_records_are_deferred_or_reused() {
    let mut m=manager();install(&mut m,1,201);
    m.seal_capabilities(1).unwrap();
    m.reset_runtime_memory_for_exec(1,128,0x200000,0x400000).unwrap();
    m.register_fresh_stack_mapping(1,202,0x4000000,64,32).unwrap();
    let va=0x4003000;
    assert_eq!(m.take_fresh_stack_frame_slot(1,va,4096).unwrap().into_parts(1,va),Some((202,204,35)));
    m.register_physical_allocation(1,va,35,700,1).unwrap();
    m.defer_active_physical_allocation(1,va,1).unwrap();
    m.take_deferred_physical_allocation(1,va,1).unwrap();
    m.register_physical_allocation(1,va,127,700,1).unwrap();
    assert!(m.take_fresh_stack_frame_slot(1,va,4096).is_none());
    // Legacy zero plans convey no fresh-root/unused-cap claim.
    m.register_lazy_mapping(1,0x5000000,1,97,nanami_core::process::ProcessLazyMappingKind::Zero).unwrap();
    assert!(m.take_fresh_stack_frame_slot(1,0x5000000,4096).is_none());
    assert!(m.take_fresh_stack_frame_slot(1,0x4001001,4096).is_none());
    assert!(m.take_fresh_stack_frame_slot(1,0x4001000,16).is_none());
    assert!(m.take_fresh_stack_frame_slot(1,0x4001000,4096).is_some());
    assert!(m.register_fresh_stack_mapping(1,202,0x6000000,65,200).is_err());
    install(&mut m,1,501);
    m.seal_capabilities(1).unwrap();
    assert!(m.take_fresh_stack_frame_slot(1,0x4004000,4096).is_none());
}

#[test]
fn sealing_later_does_not_turn_untrusted_old_stack_slots_into_virgin_claims() {
    let mut m=manager();install(&mut m,1,201);
    m.reset_runtime_memory_for_exec(1,128,0x200000,0x400000).unwrap();
    m.register_fresh_stack_mapping(1,202,0x4000000,64,32).unwrap();
    assert!(m.take_fresh_stack_frame_slot(1,0x4000000,4096).is_none());
    m.seal_capabilities(1).unwrap();
    assert!(m.take_fresh_stack_frame_slot(1,0x4000000,4096).is_none());
}

#[test]
fn fixed_ranges_covering_auto_cursor_advance_it_within_existing_arena_bounds() {
    let mut m=manager();install(&mut m,1,201);install(&mut m,2,202);
    for (base,pages,wanted) in [(0x100000,1,0x200000),(0x300000,1,0x200000),
        (0x1ff000,2,0x201000),(0x201000,3,0x204000)] {
        m.reserve_process_heap_at(1,base,pages,4096,4096).unwrap();
        assert_eq!(m.find_entry_by_pid(1).unwrap().user_heap_next_va,wanted);
    }
    let (_,_,base,_)=m.reserve_process_heap(1,1,4096,4096).unwrap();
    assert_eq!(base,0x204000);
    m.share_address_space(2,1).unwrap();
    m.reserve_process_heap_at(2,0x204000,3,4096,4096).unwrap();
    assert_eq!(m.find_entry_by_pid(1).unwrap().user_heap_next_va,0x207000);
    let before=m.find_entry_by_pid(1).unwrap();
    assert!(m.reserve_process_heap_at(1,0x207000,4096,4096,8192).is_err());
    let after=m.find_entry_by_pid(1).unwrap();
    assert_eq!(before.user_heap_next_va,after.user_heap_next_va);
    assert_eq!(before.next_frame_slot,after.next_frame_slot);
    // Disjoint fixed mappings retain their existing address-space freedom.
    m.reserve_process_heap_at(1,0x500000,1,4096,8192).unwrap();
    assert_eq!(m.find_entry_by_pid(1).unwrap().user_heap_next_va,0x207000);
}

#[test]
fn lazy_mapping_plan_keeps_first_match_owner_and_malformed_cutoff_without_copying_kind() {
    use nanami_core::process::ProcessLazyMappingKind::{Image, Zero};
    let mut m = manager();
    for pid in [1, 2, 3] { install(&mut m, pid, 200 + pid); }
    m.share_address_space(3, 1).unwrap();
    m.register_lazy_mapping(2, 0x1000, 4, 70, Zero).unwrap();
    m.register_lazy_mapping(1, 0x2000, 3, 10,
        Image { image: b"metadata", elf: nanami_core::elf_loader::ElfImage }).unwrap();
    m.register_lazy_mapping(1, 0x3000, 1, 99, Zero).unwrap();
    m.register_lazy_mapping(1, 0x10000, usize::MAX, 100, Zero).unwrap();
    m.register_lazy_mapping(1, 0x20000, 1, 200, Zero).unwrap();
    for (pid, va, expected) in [(1, 0x3000, Some((1, 0x2000, 10))),
        (3, 0x4000, Some((1, 0x2000, 10))), (2, 0x3000, Some((0, 0x1000, 70))),
        (1, 0x5000, None), (1, 0x20000, None), (99, 0x3000, None)]
    {
        let plan = m.find_lazy_mapping_plan(pid, va);
        assert_eq!(plan, expected);
        if let Some((index, _, _)) = plan {
            // Physical accounting changes do not change lazy entry positions.
            m.register_physical_allocation(1, va, 500, 700, 1).unwrap();
            match (index, m.lazy_mapping_kind(index).unwrap()) {
                (1, Image { image, .. }) => assert_eq!(*image, b"metadata"),
                (0, Zero) => {},
                _ => panic!("wrong lazy kind"),
            }
        }
    }
}

#[test]
fn image_loop_plan_probes_preserve_physical_registration_and_owner_aliases() {
    use nanami_core::process::ProcessLazyMappingKind::Zero;
    let mut m = manager();
    for pid in [1, 2] { install(&mut m, pid, 200 + pid); }
    m.share_address_space(2, 1).unwrap();
    m.register_lazy_mapping(1, 0x10000, 64, 0, Zero).unwrap();
    ProcessManager::take_lazy_mapping_probes();
    let plan = m.uniform_lazy_mapping_plan(2, 0x10000, 0x50000).unwrap();
    let plans = [plan; 64];
    let probes = ProcessManager::take_lazy_mapping_probes();
    eprintln!("64 image pages plan probes={probes}");
    assert_eq!(probes, 1);
    assert!(plans.iter().all(|plan| *plan == plans[0]));
    for page in 0..64 {
        m.register_pool_allocation(1, 0x10000 + page * 4096, page, 700 + page, 1).unwrap();
        assert_eq!(m.find_lazy_mapping_plan(2, 0x10000 + page * 4096), Some(plans[page]));
    }
}

#[test]
fn uniform_lazy_plan_matches_independent_point_oracle_with_first_match_and_overflow() {
    use nanami_core::process::ProcessLazyMappingKind::Zero;
    let mut m = manager();
    for pid in [1, 2, 3] { install(&mut m, pid, 200 + pid); }
    m.share_address_space(3, 1).unwrap();
    for index in 0..47 {
        m.register_lazy_mapping(1 + index % 2, (1 + (index * 13) % 97) * 4096 + index % 3,
            1 + index % 7, index, Zero).unwrap();
    }
    m.register_lazy_mapping(1, 0x100000, usize::MAX, 0, Zero).unwrap();
    m.register_lazy_mapping(1, 0x200000, 64, 0, Zero).unwrap();
    let mut seed = 17u64;
    for _ in 0..3000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let start = (seed >> 19) as usize % (600 * 4096);
        let end = start + (seed >> 39) as usize % (48 * 4096);
        for pid in [1, 2, 3, 99] {
            let first = m.linear_lazy_mapping_plan_for_test(pid, start);
            let expected = if start < end && first.is_some()
                && (start..end).step_by(4096).all(|va| m.linear_lazy_mapping_plan_for_test(pid, va) == first)
            { first } else { None };
            assert_eq!(m.uniform_lazy_mapping_plan(pid, start, end), expected, "{pid}/{start:#x}/{end:#x}");
        }
    }
    assert_eq!(m.uniform_lazy_mapping_plan(1, 8192, 4096), None);
    m.register_lazy_mapping(2, usize::MAX - 8191, 1, 9, Zero).unwrap();
    assert!(m.uniform_lazy_mapping_plan(2, usize::MAX - 8191, usize::MAX - 4095).is_some());
    assert_eq!(m.uniform_lazy_mapping_plan(2, usize::MAX - 8191, usize::MAX), None);
}

#[test]
fn preinstall_plan_declines_existing_frames_installed_owners_and_nonuniform_ranges() {
    use nanami_core::process::ProcessLazyMappingKind::Zero;
    let mut m = manager();
    m.ensure_vm_space_for_pid(77).unwrap();
    m.register_lazy_mapping(77, 0x10000, 64, 0, Zero).unwrap();
    let plan = m.preinstall_lazy_mapping_plan(77, 0x10000, 0x50000).unwrap();
    for rights in [7, 0, 1] {
        let vm = m.vm_space_mut(77).unwrap();
        vm.record_frame(0x4f000, 900).unwrap();
        vm.set_permissions(0x4f000, rights);
        if rights == 1 { vm.mark_mapping_uncertain(0x4f000); }
        assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10000, 0x50000), None);
        m.vm_space_mut(77).unwrap().forget_frame(0x4f000);
    }
    m.vm_space_mut(77).unwrap().record_frame(0x10001, 901).unwrap();
    assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10000, 0x50000), Some(plan));
    assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10000, 0x51000), None);
    assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10001, 0x50000), None);
    assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10000, 0x10000), None);
    assert_eq!(m.preinstall_lazy_mapping_plan(99, 0x10000, 0x50000), None);
    install(&mut m, 77, 277);
    assert_eq!(m.preinstall_lazy_mapping_plan(77, 0x10000, 0x50000), None);
    let mut overlap = manager();
    overlap.ensure_vm_space_for_pid(77).unwrap();
    overlap.register_lazy_mapping(77, 0x11000, 1, 100, Zero).unwrap();
    overlap.register_lazy_mapping(77, 0x10000, 64, 0, Zero).unwrap();
    assert_eq!(overlap.preinstall_lazy_mapping_plan(77, 0x10000, 0x50000), None);
}

#[test]
fn lazy_mapping_lookup_limits_probes_to_pid_bucket() {
    use nanami_core::process::{ProcessManager, ProcessLazyMappingKind::Zero};
    let mut m = manager();
    for round in 0..4 {
        for pid in 1..=128 {
            m.register_lazy_mapping(pid, (round + 1) * 0x10000, 2, round * 10, Zero).unwrap();
        }
    }
    ProcessManager::take_lazy_mapping_probes();
    assert_eq!(m.find_lazy_mapping_plan(127, 0x41000), Some((510, 0x40000, 30)));
    let probes = ProcessManager::take_lazy_mapping_probes();
    println!("lazy mapping 128 PID x 4 maps probes: {probes}");
    assert_eq!(probes, 8);
}

#[test]
fn repeated_frame_chunk_checks_reuse_verified_position() {
    let mut m = manager();
    for pid in 1..=128 { m.register_frame_chunk(pid, 0).unwrap(); }
    ProcessManager::take_frame_chunk_probes();
    for _ in 0..64 { assert!(m.has_frame_chunk(128, 0)); }
    let probes = ProcessManager::take_frame_chunk_probes();
    println!("64 repeated frame chunk checks among128 records probes: {probes}");
    assert_eq!(probes, 64);
}

#[test]
fn frame_chunk_hints_match_records_through_collisions_removal_exec_and_reuse() {
    let mut m = manager();
    for pid in [1, 2, 65, 129] { install(&mut m, pid, 400 + pid); }
    m.share_address_space(2, 1).unwrap();
    let pids = [0, 1, 2, 64, 65, 128, 129, 193];
    let chunks = [0, 1, 2, 64, 65, usize::MAX];
    let mut checks = 0;
    for step in 0..96 {
        let pid = pids[step % pids.len()];
        let chunk = chunks[(step / pids.len()) % chunks.len()];
        m.register_frame_chunk(pid, chunk).unwrap();
        m.register_frame_chunk(pid, chunk).unwrap();
        for pid in pids {
            let owner = m.memory_owner_pid(pid);
            for chunk in chunks {
                let expected = m.frame_chunk_indices(owner).any(|value| value == chunk);
                assert_eq!(m.has_frame_chunk(pid, chunk), expected);
                let copies = m.frame_chunk_indices(owner).filter(|value| *value == chunk).count();
                assert!(copies <= 1);
                checks += 1;
            }
        }
        if step % 7 == 6 { m.discard_process_artifacts(64, 0); }
        if step % 11 == 10 { m.reset_runtime_memory_for_exec(65, 0, 0, 0).unwrap(); }
        if step % 17 == 16 {
            m.mark_exited(129, 1, 0).unwrap(); m.reap_process(129, true).unwrap();
            install(&mut m, 129, 529);
        }
        if step % 19 == 18 { m.share_address_space(2, if step % 2 == 0 { 65 } else { 1 }).unwrap(); }
    }
    println!("frame chunk hint/record equivalence checks: {checks}");
}

#[test]
fn lazy_mapping_bucket_churn_matches_linear_first_match_and_cutoff() {
    use nanami_core::process::ProcessLazyMappingKind::{Image, Zero};
    let mut m = manager();
    for pid in [1, 2, 65, 66, 129] { install(&mut m, pid, 400 + pid); }
    m.share_address_space(2, 1).unwrap();
    let mut checks = 0;
    for round in 0..40 {
        for pid in [1, 65, 66, 129] {
            m.register_lazy_mapping(pid, 0x1000 + (round % 7) * 0x1000,
                if round % 17 == 16 { usize::MAX } else { 2 }, round,
                if round % 3 == 0 { Image { image: b"test", elf: nanami_core::elf_loader::ElfImage } } else { Zero }).unwrap();
        }
        for pid in [1, 2, 65, 66, 129, 193, 99] {
            for va in [0, 0x1000, 0x2000, 0x3500, 0x7000, 0x9000, usize::MAX] {
                assert_eq!(m.find_lazy_mapping_plan(pid, va), m.linear_lazy_mapping_plan_for_test(pid, va));
                checks += 1;
            }
        }
        if round % 4 == 3 { m.drop_image_lazy_mappings_for_pid(65); }
        if round % 7 == 6 { m.discard_process_artifacts(129, 0); }
        if round % 9 == 8 { m.reset_runtime_memory_for_exec(66, 0, 0, 0).unwrap(); }
    }
    // Query once more after final structural changes, then append/reuse.
    m.drop_image_lazy_mappings_for_pid(1);
    m.register_lazy_mapping(65, 0x50000, 1, 900, Zero).unwrap();
    for pid in [1, 2, 65, 66, 129, 193] {
        for va in [0x1000, 0x50000, usize::MAX] {
            assert_eq!(m.find_lazy_mapping_plan(pid, va), m.linear_lazy_mapping_plan_for_test(pid, va));
            checks += 1;
        }
    }
    m.mark_exited(65, 1, 0).unwrap();
    m.reap_process(65, true).unwrap();
    install(&mut m, 65, 965);
    m.register_lazy_mapping(65, 0x60000, 1, 950, Zero).unwrap();
    m.share_address_space(2, 66).unwrap();
    for pid in [1, 2, 65, 66, 129, 193] {
        for va in [0x1000, 0x50000, 0x60000, usize::MAX] {
            assert_eq!(m.find_lazy_mapping_plan(pid, va), m.linear_lazy_mapping_plan_for_test(pid, va));
            checks += 1;
        }
    }
    println!("lazy mapping ordered bucket/linear churn checks: {checks}");
}

#[test]
fn lazy_page_range_matches_point_queries_with_overlaps_unaligned_keys_and_memory_peers() {
    use nanami_core::process::ProcessLazyMappingKind::Zero;
    let mut m = manager();
    for pid in [1, 2, 3] { install(&mut m, pid, 200 + pid); }
    m.share_address_space(3, 1).unwrap();
    for i in 0..47 {
        m.register_lazy_mapping(1 + i % 2, (1 + (i * 13) % 97) * 4096 + i % 3,
            1 + i % 7, i, Zero).unwrap();
    }
    let mut seed = 17u64;
    for _ in 0..3000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let start = ((seed >> 19) as usize % (128 * 4096)) & !1;
        let end = start + ((seed >> 39) as usize % (48 * 4096));
        for pid in [1, 2, 3, 99] {
            let expected = (start..end).step_by(4096)
                .any(|va| m.find_lazy_mapping(pid, va).is_some());
            assert_eq!(m.has_lazy_mapping_at_pages(pid, start, end), expected,
                "pid={pid} start={start:#x} end={end:#x}");
        }
    }
    assert!(!m.has_lazy_mapping_at_pages(1, 4096, 4096));
    assert!(!m.has_lazy_mapping_at_pages(1, 8192, 4096));
    assert!(!m.has_lazy_mapping_at_pages(99, 0, usize::MAX));
    assert!(m.has_lazy_mapping_at_pages(3, 0, usize::MAX));
}

#[test]
fn lazy_page_range_preserves_malformed_entry_cutoff_and_high_address_boundaries() {
    use nanami_core::process::ProcessLazyMappingKind::Zero;
    let mut m = manager();
    for pid in [1, 2] { install(&mut m, pid, 200 + pid); }
    m.register_lazy_mapping(2, usize::MAX, 1, 0, Zero).unwrap();
    m.register_lazy_mapping(1, 0x10000, 1, 0, Zero).unwrap();
    m.register_lazy_mapping(1, 0x20000, usize::MAX, 1, Zero).unwrap();
    m.register_lazy_mapping(1, 0x40000, 1, 2, Zero).unwrap();
    for (start, end) in [(0, 0x50000), (0x11000, 0x50000), (0x40000, 0x41000),
        (0x10001, 0x11001), (usize::MAX - 8192, usize::MAX)] {
        let expected = (start..end).step_by(4096)
            .any(|va| m.find_lazy_mapping(1, va).is_some());
        assert_eq!(m.has_lazy_mapping_at_pages(1, start, end), expected);
    }
    let mut m = manager();
    install(&mut m, 1, 201);
    m.register_lazy_mapping(1, usize::MAX - 8192, 1, 0, Zero).unwrap();
    for start in [usize::MAX - 12287, usize::MAX - 8192, usize::MAX - 4096] {
        assert_eq!(m.has_lazy_mapping_at_pages(1, start, usize::MAX),
            (start..usize::MAX).step_by(4096).any(|va| m.find_lazy_mapping(1, va).is_some()));
    }
    // This mapping overflows its end, so point lookup stops before later rows.
    m.register_lazy_mapping(1, usize::MAX - 1, 1, 1, Zero).unwrap();
    assert!(!m.has_lazy_mapping_at_pages(1, usize::MAX - 4096, usize::MAX));
}

#[test]
fn alpha_uses_its_actual_pcb_capability() {
    assert_eq!(manager().alpha_entry().pcb, 99);
}

#[test]
fn tasks_share_vm_and_allocation_metadata_without_duplicating_physical_ownership() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    manager.ensure_vm_space_for_pid(1).unwrap();
    manager.share_address_space(2, 1).unwrap();
    manager
        .vm_space_mut(2)
        .unwrap()
        .record_frame(0x1000, 0xabcd)
        .unwrap();
    manager.vm_space_mut(2).unwrap().set_permissions(0x1000, 0);
    assert_eq!(
        manager.vm_space_mut(1).unwrap().find_frame(0x1000),
        Some(0xabcd)
    );
    assert_eq!(manager.vm_space_mut(1).unwrap().permissions(0x1000), 0);
    manager
        .register_physical_allocation(2, 0x1000, 7, 100, 1)
        .unwrap();
    assert!(manager
        .find_active_physical_allocation_reference(1, 0x1000, 1)
        .is_some());
    assert_eq!(manager.releasable_physical_allocations_for_pid(1).len(), 1);
    assert_eq!(manager.releasable_physical_allocations_for_pid(2).len(), 0);
    assert!(manager.has_memory_peers(1));
    manager.mark_exited(1, 1, 0).unwrap();
    manager.defer_vm_reap(1);
    assert!(manager.memory_entry(2).unwrap().deferred_vm_reap);
    manager.mark_exited(2, 1, 0).unwrap();
    manager.reap_process(2, true).unwrap();
    assert!(!manager.has_memory_peers(1));
    assert_eq!(
        manager.vm_space_mut(1).unwrap().find_frame(0x1000),
        Some(0xabcd)
    );
}

#[test]
fn sparse_vm_walk_includes_retained_prot_none_backing_not_unpopulated_holes() {
    let mut vm = vm_space::VmSpace::new();
    for va in [0x1000, 0x4000, 0x9000] {
        vm.record_frame(va, va + 1).unwrap();
    }
    vm.set_permissions(0x4000, 0);
    assert_eq!(vm.backed_pages(0x2000, 0x9000), [(0x4000, 0)]);
}

#[test]
fn native_root_provenance_never_treats_kernel_or_other_slots_as_notifications() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let types = manager.find_entry_by_pid(1).unwrap().root_types;
    assert!(types.is_notification(21));
    assert!(types.is_ipc(20));
    for slot in [0, 1, 2, 3, 7, 22, 40, 64, 255, 256, usize::MAX] {
        assert!(!types.is_notification(slot));
    }
    assert!(manager.root_types_mut(0).is_err());
    assert!(manager.root_types_mut(999).is_err());
}

#[test]
fn root_slot_overwrite_clears_notification_type_without_touching_adjacent_slots() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    let types = manager.root_types_mut(1).unwrap();
    for slot in [31, 32, 63, 64, 127, 128, 255] {
        types.notification(slot);
        assert!(types.is_notification(slot));
        types.other(slot);
        assert!(types.occupied(slot));
        assert!(!types.is_notification(slot));
        types.ipc(slot);
        assert!(types.is_ipc(slot));
        types.clear(slot);
        assert!(!types.occupied(slot));
    }
    assert!(types.is_notification(21));
    assert!(types.is_ipc(20));
    types.notification(256);
    assert!(!types.occupied(256));
    assert_eq!(core::mem::size_of_val(types), 64);
}

#[test]
fn exited_process_cannot_acquire_new_root_provenance() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    manager.mark_exited(1, 1, 0).unwrap();
    assert!(manager.root_types_mut(1).is_err());
}

#[test]
fn foreign_revocation_can_empty_a_known_slot_without_requiring_a_remove() {
    use std::cell::RefCell;
    let operations = RefCell::new(Vec::new());
    for known in [false, true] {
        operations.borrow_mut().clear();
        nanami_core::process::install_notification_alias(
            known,
            || {
                operations.borrow_mut().push("copy");
                Ok(())
            },
            || {
                operations.borrow_mut().push("remove");
                Err(CapabilityError::PermissionDenied)
            },
        )
        .unwrap();
        assert_eq!(*operations.borrow(), ["copy"]);
    }
}

#[test]
fn only_a_known_live_notification_is_removed_before_replacement() {
    use std::cell::RefCell;
    let occupied = RefCell::new(true);
    let operations = RefCell::new(Vec::new());
    nanami_core::process::install_notification_alias(
        true,
        || {
            operations.borrow_mut().push("copy");
            if *occupied.borrow() {
                return Err(CapabilityError::Fatal); // A9N INIT_FIRST conversion.
            }
            *occupied.borrow_mut() = true;
            Ok(())
        },
        || {
            operations.borrow_mut().push("remove");
            *occupied.borrow_mut() = false;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*operations.borrow(), ["copy", "remove", "copy"]);
}

#[test]
fn occupied_unverified_or_ipc_slot_is_never_deleted_by_notification_transfer() {
    nanami_core::process::install_notification_alias(
        false,
        || Err(CapabilityError::Fatal),
        || {
            panic!("notification API deleted another capability type");
        },
    )
    .unwrap_err();
}

#[test]
fn failed_notification_withdrawal_is_not_followed_by_another_copy() {
    let mut copies = 0;
    let result = nanami_core::process::install_notification_alias(
        true,
        || {
            copies += 1;
            Err(CapabilityError::Fatal)
        },
        || Err(CapabilityError::PermissionDenied),
    );
    assert_eq!(result, Err(CapabilityError::PermissionDenied));
    assert_eq!(copies, 1);
}

#[test]
fn uncertain_authority_keeps_process_backing_and_metadata_quarantined() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    manager.quarantine_resources(1).unwrap();
    manager.mark_exited(1, 0, 5).unwrap();
    assert!(manager.reap_process(1, true).is_err());
    assert!(manager.find_entry_by_pid(1).unwrap().resource_quarantined);
}

#[test]
fn platform_access_is_per_process_and_defaults_to_denied() {
    let mut manager = manager();
    install(&mut manager, 1, 201);
    install(&mut manager, 2, 202);
    assert_eq!(
        manager.entries().map(|entry| entry.pid).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(
        !manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
    manager.set_platform_information_access(1, true).unwrap();
    assert!(
        manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
    assert!(
        !manager
            .find_entry_by_pid(2)
            .unwrap()
            .platform_information_access
    );
    assert!(manager.set_platform_information_access(999, true).is_err());
    manager.set_platform_information_access(1, false).unwrap();
    assert!(
        !manager
            .find_entry_by_pid(1)
            .unwrap()
            .platform_information_access
    );
}

#[test]
fn affinity_diagnostic_requires_explicit_one() {
    assert_eq!(
        nanami_core::process::SAME_CORE_DIAGNOSTIC,
        option_env!("NANAMI_SAME_CORE_DIAGNOSTIC") == Some("1")
    );
}

#[test]
fn affinity_allocation_preserves_topology_and_normal_round_robin() {
    for core_count in [1, 2, 4, 64] {
        let mut manager = ProcessManager::new_alpha(1, 2, 99, 3, 4096, &[], core_count).unwrap();
        for index in 0..core_count * 3 {
            let expected = if nanami_core::process::SAME_CORE_DIAGNOSTIC || core_count == 1 {
                0
            } else {
                1 + index % (core_count - 1)
            };
            assert_eq!(manager.allocate_affinity(), expected);
            assert_eq!(manager.online_core_count(), core_count);
        }
        assert_eq!(manager.alpha_entry().affinity, 0);
    }
}

#[test]
fn active_core_mask_uses_allocated_affinities_not_the_online_count() {
    let mut manager = manager();
    for pid in 1..=6 {
        let affinity = manager.allocate_affinity();
        let root = pid + 200;
        manager
            .install_process(
                pid,
                0,
                root,
                root + 1,
                root + 2,
                root + 3,
                root + 4,
                pid,
                affinity,
                0,
                0x200000,
                0x400000,
            )
            .unwrap();
    }
    let expected = if nanami_core::process::SAME_CORE_DIAGNOSTIC {
        1
    } else {
        0xf
    };
    assert_eq!(manager.active_core_mask(), expected);
    assert_eq!(manager.online_core_count(), 4);
}

fn install(manager: &mut ProcessManager, pid: usize, root: usize) {
    manager
        .install_process(
            pid,
            0,
            root,
            root + 1,
            root + 2,
            root + 3,
            root + 4,
            pid,
            pid % 4,
            0,
            0x200000,
            0x400000,
        )
        .unwrap();
}

#[test]
fn lookup_handles_out_of_order_install_replacement_missing_and_alpha() {
    let mut manager = manager();
    for pid in [55, 3, 901, 2, 16] {
        install(&mut manager, pid, pid + 200);
    }
    for pid in [2, 3, 16, 55, 901] {
        assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, pid + 200);
    }
    assert!(manager.find_entry_by_pid(54).is_none());
    assert!(manager.find_entry_by_pid(usize::MAX).is_none());
    assert_eq!(manager.find_entry_by_pid(0).unwrap().root_node, 1);
    install(&mut manager, 16, 987);
    assert_eq!(manager.find_entry_by_pid(16).unwrap().root_slot, 987);
    assert_eq!(manager.statistics().running, 5);
}

#[test]
fn process_entry_hints_cover_the_hot_set_without_lookup_allocation() {
    let mut manager = manager();
    for pid in (1..=64).rev() { install(&mut manager, pid, 200 + pid); }
    let entries: Vec<_> = manager.entries().filter(|entry| entry.pid != 0).collect();
    let mut old_comparisons = 0;
    for pid in 1..=64 {
        entries.binary_search_by_key(&pid, |entry| { old_comparisons += 1; entry.pid }).unwrap();
        assert!(manager.entry_hint_hit(pid));
    }
    PROTECT_ALLOCATIONS.with(|count| count.set(Some(0)));
    let mut correct = true;
    for pid in 1..=64 {
        correct &= manager.find_entry_by_pid(pid).unwrap().root_slot == 200 + pid;
        correct &= manager.memory_owner_pid(pid) == pid;
        correct &= manager.memory_root_node(pid) == Some(201 + pid);
    }
    let allocations = PROTECT_ALLOCATIONS.with(|count| count.replace(None).unwrap());
    assert!(correct);
    assert_eq!(allocations, 0);
    assert!(old_comparisons > 64 * 4);
    println!("PROCESS-ENTRY-HINTS entries=64 old-pid-comparisons={old_comparisons} verified-hits=64 lookup-allocations=0 metadata-bytes=512");
}

#[test]
fn process_vm_hints_cover_the_hot_set_without_lookup_allocation() {
    let mut manager = manager();
    for pid in (1..=64).rev() {
        install(&mut manager, pid, 200 + pid);
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager.vm_space_mut(pid).unwrap().record_frame(0x1000, pid).unwrap();
    }
    let sorted_pids: Vec<usize> = (1..=64).collect();
    let mut old_comparisons = 0;
    for pid in 1..=64 {
        sorted_pids.binary_search_by_key(&pid, |key| { old_comparisons += 1; *key }).unwrap();
        assert!(manager.vm_hint_hit(pid));
    }
    PROTECT_ALLOCATIONS.with(|count| count.set(Some(0)));
    let mut correct = true;
    for pid in 1..=64 {
        correct &= manager.vm_space(pid).unwrap().find_frame(0x1000) == Some(pid);
        correct &= manager.vm_space_mut(pid).unwrap().find_frame(0x1000) == Some(pid);
    }
    let allocations = PROTECT_ALLOCATIONS.with(|count| count.replace(None).unwrap());
    assert!(correct);
    assert_eq!(allocations, 0);
    assert!(old_comparisons > 64 * 4);
    println!("PROCESS-VM-HINTS entries=64 old-sorted-pid-comparisons={old_comparisons} verified-hits=64 lookup-allocations=0 metadata-bytes=512");
}

#[test]
fn vm_hints_preserve_owner_retarget_reset_and_exact_pid_row_cleanup() {
    let mut manager = manager();
    for pid in [129, 65, 2, 1, usize::MAX] {
        install(&mut manager, pid, if pid == usize::MAX { 777 } else { 200 + pid });
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager.vm_space_mut(pid).unwrap().record_frame(0x1000, pid).unwrap();
    }
    assert!(!manager.vm_hint_hit(129));
    assert!(manager.vm_hint_hit(1));
    assert!(manager.vm_space(193).is_none());
    manager.share_address_space(2, 65).unwrap();
    manager.vm_space_mut(2).unwrap().record_frame(0x2000, 777).unwrap();
    assert_eq!(manager.vm_space(65).unwrap().find_frame(0x2000), Some(777));
    manager.discard_process_artifacts(2, 202); // Remove row2, never its owner65.
    assert!(!manager.vm_hint_hit(2));
    assert_eq!(manager.vm_space(2).unwrap().find_frame(0x2000), Some(777));
    manager.ensure_vm_space_for_pid(2).unwrap();
    install(&mut manager, 2, 444); // New task data points back at its own empty row.
    assert_eq!(manager.vm_space(2).unwrap().find_frame(0x1000), None);
    manager.vm_space_mut(2).unwrap().record_frame(0x1000, 42).unwrap();
    manager.reset_runtime_memory_for_exec(2, 0, 0x200000, 0x400000).unwrap();
    assert!(manager.vm_hint_hit(2));
    assert_eq!(manager.vm_space(2).unwrap().find_frame(0x1000), None);
    manager.share_address_space(1, 65).unwrap();
    manager.mark_exited(65, 1, 0).unwrap();
    manager.reap_process(65, true).unwrap();
    assert!(manager.vm_space(1).is_none());
    install(&mut manager, 65, 888);
    manager.ensure_vm_space_for_pid(65).unwrap();
    manager.vm_space_mut(65).unwrap().record_frame(0x1000, 999).unwrap();
    assert_eq!(manager.vm_space(1).unwrap().find_frame(0x1000), Some(999));
    // Pre-install rows must work without ProcessEntry lookup succeeding.
    manager.ensure_vm_space_for_pid(77).unwrap();
    assert!(manager.find_entry_by_pid(77).is_none());
    manager.vm_space_mut(77).unwrap().record_frame(0x1000, 77).unwrap();
    manager.discard_process_artifacts(77, 3977);
    assert!(manager.vm_space(77).is_none());
    for turn in 1..=100 {
        let pid = 1024 + turn * 64 + 1;
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager.vm_space_mut(pid).unwrap().record_frame(0x1000, pid).unwrap();
        assert_eq!(manager.vm_space(pid).unwrap().find_frame(0x1000), Some(pid));
        assert_eq!(manager.vm_space(129).unwrap().find_frame(0x1000), Some(129));
        assert_eq!(manager.vm_space(usize::MAX).unwrap().find_frame(0x1000), Some(usize::MAX));
        manager.discard_process_artifacts(pid, 3000 + turn);
        assert!(manager.vm_space(pid).is_none());
    }
    assert_eq!(manager.vm_space(1).unwrap().find_frame(0x1000), Some(999));
    assert!(manager.vm_space(0).is_none()); // Bootstrap remains a separate tracker.
}

#[test]
fn process_entry_hint_collisions_shifts_replacement_and_dangling_owner_keep_exact_identity() {
    let mut manager = manager();
    let mut expected = std::collections::BTreeMap::new();
    for (pid, root) in [(129, 329), (1, 201), (66, 266), (2, 202), (65, 265), (usize::MAX, 777)] {
        install(&mut manager, pid, root);
        expected.insert(pid, root);
    }
    assert!(!manager.entry_hint_hit(129));
    assert!(manager.entry_hint_hit(65));
    for (&pid, &root) in &expected { assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, root); }
    install(&mut manager, 65, 999);
    expected.insert(65, 999);
    manager.share_address_space(2, 65).unwrap();
    assert_eq!(manager.memory_root_node(2), Some(1000));
    manager.mark_exited(65, 1, 0).unwrap();
    manager.reap_process(65, true).unwrap();
    expected.remove(&65);
    assert_eq!(manager.memory_root_node(2), None);
    assert!(manager.memory_context(2).is_none());
    assert!(manager.find_entry_by_pid(65).is_none());
    assert_eq!(manager.memory_owner_pid(2), 65);
    install(&mut manager, 65, 888);
    expected.insert(65, 888);
    assert_eq!(manager.memory_root_node(2), Some(889));
    for turn in 1..=100 {
        let pid = 1024 + turn * 64 + 1;
        install(&mut manager, pid, 2000 + turn);
        for (&saved, &root) in &expected { assert_eq!(manager.find_entry_by_pid(saved).unwrap().root_slot, root); }
        assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, 2000 + turn);
        assert!(manager.find_entry_by_pid(pid + 64).is_none());
        manager.mark_exited(pid, 1, 0).unwrap();
        manager.reap_process(pid, true).unwrap();
        assert!(manager.find_entry_by_pid(pid).is_none());
    }
    for pid in [2, 65, 1, 66, 129, usize::MAX] {
        manager.mark_exited(pid, 1, 0).unwrap();
        manager.reap_process(pid, true).unwrap();
        expected.remove(&pid);
        for (&saved, &root) in &expected { assert_eq!(manager.find_entry_by_pid(saved).unwrap().root_slot, root); }
    }
    assert_eq!(manager.statistics().running, 0);
    assert_eq!(manager.find_entry_by_pid(0).unwrap().root_node, 1);
}

#[test]
fn owner_reads_are_current_and_owned_entry_lookups_remain_snapshots() {
    let mut manager = manager();
    install(&mut manager, 31, 231);
    install(&mut manager, 7, 207);
    let before = manager.find_entry_by_pid(31).unwrap();
    assert_eq!(manager.memory_owner_pid(31), 31);
    manager.share_address_space(31, 7).unwrap();
    assert_eq!(manager.memory_owner_pid(31), 7);
    assert_eq!(before.memory_owner, 31);
    let shared = manager.find_entry_by_pid(31).unwrap();
    assert_eq!(shared.memory_owner, 7);
    install(&mut manager, 31, 999);
    assert_eq!(manager.memory_owner_pid(31), 31);
    assert_eq!(shared.root_slot, 231);
    assert_eq!(manager.find_entry_by_pid(31).unwrap().root_slot, 999);
    assert_eq!(manager.memory_owner_pid(usize::MAX), usize::MAX);
    assert_eq!(manager.memory_owner_pid(0), 0);
}

#[test]
fn reap_preserves_other_pid_and_vm_lookups_and_slot_reuse() {
    let mut manager = manager();
    for pid in [10, 7, 3, 8, 15] {
        install(&mut manager, pid, pid + 200);
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager
            .vm_space_mut(pid)
            .unwrap()
            .record_frame(0x1000, pid + 1234)
            .unwrap();
    }
    assert_eq!(
        manager.reap_process(7, true),
        Err(CapabilityError::IllegalOperation)
    );
    for pid in [7, 15, 3] {
        manager.mark_exited(pid, 1, 0).unwrap();
        assert!(manager.find_entry_by_pid(pid).unwrap().exited);
        manager.reap_process(pid, true).unwrap();
        assert!(manager.find_entry_by_pid(pid).is_none());
        assert!(manager.vm_space_mut(pid).is_none());
    }
    for pid in [8, 10] {
        assert_eq!(
            manager.vm_space_mut(pid).unwrap().find_frame(0x1000),
            Some(pid + 1234)
        );
        assert!(manager.find_entry_by_pid(pid).is_some());
    }
    assert_eq!(
        manager.reap_process(0, true),
        Err(CapabilityError::PermissionDenied)
    );
    let (pid, slot) = manager.alloc_process_slot().unwrap();
    install(&mut manager, pid, slot);
    manager.mark_exited(pid, 1, 0).unwrap();
    manager.reap_process(pid, true).unwrap();
    let (next_pid, next_slot) = manager.alloc_process_slot().unwrap();
    assert!(next_pid > pid);
    assert_eq!(slot, next_slot);
}

#[test]
fn exec_reset_and_failed_spawn_discard_do_not_change_other_spaces() {
    let mut manager = manager();
    for pid in [30, 10, 20] {
        install(&mut manager, pid, pid + 200);
        manager.ensure_vm_space_for_pid(pid).unwrap();
        manager
            .vm_space_mut(pid)
            .unwrap()
            .record_frame(4096, pid)
            .unwrap();
        // Repeated ensure must not reset an existing VM.
        manager.ensure_vm_space_for_pid(pid).unwrap();
    }
    manager
        .reset_runtime_memory_for_exec(20, 0, 0x200000, 0x400000)
        .unwrap();
    assert_eq!(manager.vm_space_mut(20).unwrap().find_frame(4096), None);
    manager.discard_process_artifacts(10, 210);
    assert!(manager.vm_space_mut(10).is_none());
    assert_eq!(manager.vm_space_mut(30).unwrap().find_frame(4096), Some(30));
    manager.ensure_vm_space_for_pid(5).unwrap();
    assert_eq!(manager.vm_space_mut(30).unwrap().find_frame(4096), Some(30));
}

#[test]
#[ignore = "host microbenchmark; excludes IPC and insertion/removal"]
fn process_lookup_benchmark() {
    use std::{hint::black_box, time::Instant};
    for count in [16, 64, 256, 1024] {
        let mut manager = manager();
        for pid in 1..=count {
            install(&mut manager, pid, pid + 200);
            manager.ensure_vm_space_for_pid(pid).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            for i in 0..200_000 {
                let pid = black_box((i * 17) % count + 1);
                black_box(manager.find_entry_by_pid(pid).unwrap().address_space);
                black_box(manager.vm_space_mut(pid).unwrap());
            }
            samples.push(start.elapsed().as_nanos() / 200_000);
        }
        samples.sort();
        println!("{count} processes: {} ns per entry+VM lookup", samples[2]);
    }
}

#[test]
fn repeated_install_reap_and_reuse_preserve_lookup_invariants() {
    let mut manager = manager();
    let mut live = std::collections::BTreeMap::new();
    let mut random = 12345u64;
    for operation in 0..1000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let pid = (random >> 32) as usize % 64 + 1;
        if operation % 3 == 0 && live.remove(&pid).is_some() {
            manager.mark_exited(pid, 1, 0).unwrap();
            manager.reap_process(pid, false).unwrap();
            assert!(manager.find_entry_by_pid(pid).is_none());
            assert!(manager.vm_space_mut(pid).is_none());
        } else {
            let root = operation + 200;
            install(&mut manager, pid, root);
            manager.ensure_vm_space_for_pid(pid).unwrap();
            manager
                .vm_space_mut(pid)
                .unwrap()
                .record_frame(4096, root)
                .unwrap();
            live.insert(pid, root);
        }
        assert_eq!(manager.statistics().running, live.len());
        for (&pid, &root) in &live {
            assert_eq!(manager.find_entry_by_pid(pid).unwrap().root_slot, root);
            assert_eq!(
                manager.vm_space_mut(pid).unwrap().find_frame(4096),
                Some(root)
            );
        }
    }
}
