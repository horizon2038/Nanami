//! Production guest-copy helpers; personality permission checks and transport mocked.
#![allow(dead_code)]
extern crate self as nanami_runtime;
use std::cell::RefCell;
type Word = usize;
pub use nanami_uapi::os::{ProcessMemoryTransfer, OS_MEMORY_COPY_VECTOR_MAX};
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
const LINUX_IOVEC_LEN: usize = 16;
const LINUX_IOV_MAX: usize = 16;
const LINUX_MSGHDR_LEN: usize = 56;
const LINUX_PAGE_SIZE: usize = 4096;
struct Runtime {
    cold_views_quiescent: bool,
    private_shared: bool,
    readonly_only: bool,
    private_failure: Option<Word>,
    posix_shm: usize,
    posix_shm_size: usize,
    allowed: bool,
    denied: Option<usize>,
}
impl Runtime {
    fn memory_access_allowed(&self, _: Word, address: Word, bytes: Word, _: Word) -> bool {
        PERMISSION_CHECKS.with(|count| count.set(count.get() + 1));
        bytes == 0
            || (self.allowed
                && self
                    .denied
                    .is_none_or(|bad| bad < address || bad >= address.saturating_add(bytes)))
    }
}
#[derive(Default)]
struct Transport {
    calls: Vec<(bool, usize, usize, usize, usize)>,
    fail: bool,
    denied: Option<usize>,
    partial_fault: bool,
    batches: usize,
}
thread_local! { static TRANSPORT: RefCell<Transport> = RefCell::new(Transport::default()); }
thread_local! { static PERMISSION_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn map_request_error(_: ()) -> i32 {
    EFAULT
}
mod guest_views {
    pub(super) const MAX_WRITE_BUFFERS: usize = 4;
    pub(super) const MAX_CONTIGUOUS_READ_BYTES: usize = 16384;
    use std::cell::Cell;
    thread_local! {
        pub(super) static HIT: Cell<bool> = const { Cell::new(false) };
        pub(super) static COPY_INPUT: Cell<bool> = const { Cell::new(false) };
        pub(super) static CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static WRITE_HIT: Cell<bool> = const { Cell::new(false) };
        pub(super) static CACHED_WRITE_HIT: Cell<bool> = const { Cell::new(false) };
        pub(super) static WRITE_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static BATCH_WRITE_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static COW_PAGE: Cell<Option<usize>> = const { Cell::new(None) };
    }
    fn writable(destination:usize,bytes:usize)->bool {
        WRITE_HIT.with(Cell::get) && !COW_PAGE.with(Cell::get).is_some_and(|page|
            destination < page+4096 && destination.checked_add(bytes).is_none_or(|end|end>page))
    }
    pub(super) fn copy_into_scratch(
        runtime: &super::Runtime,
        _: usize,
        source: usize,
        destination: usize,
        bytes: usize,
    ) -> bool {
        CALLS.with(|calls| calls.set(calls.get() + 1));
        if bytes > MAX_CONTIGUOUS_READ_BYTES || destination
            .checked_sub(runtime.posix_shm)
            .and_then(|offset| offset.checked_add(bytes))
            .is_none_or(|end| end > runtime.posix_shm_size)
        {
            return false;
        }
        let hit = HIT.with(|hit| hit.get());
        if hit && COPY_INPUT.with(|copy| copy.get()) {
            unsafe { core::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes); }
        }
        hit
    }
    pub(super) fn copy_into_owned(
        _: &super::Runtime,
        _: usize,
        source: usize,
        destination: &mut [core::mem::MaybeUninit<u8>],
    ) -> bool {
        CALLS.with(|calls| calls.set(calls.get() + 1));
        if destination.len() > MAX_CONTIGUOUS_READ_BYTES || !HIT.with(|hit| hit.get()) {
            return false;
        }
        for (offset, byte) in destination.iter_mut().enumerate() {
            byte.write(unsafe { core::ptr::read_volatile((source + offset) as *const u8) });
        }
        true
    }
    pub(super) fn copy_from_owned(
        _: &super::Runtime,
        _: usize,
        destination: usize,
        source: &[u8],
    ) -> bool {
        WRITE_CALLS.with(|calls| calls.set(calls.get() + 1));
        if source.len() > 4096 || !writable(destination,source.len()) {
            return false;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(source.as_ptr(), destination as *mut u8, source.len());
        }
        true
    }
    pub(super) fn copy_from_scratch(
        runtime: &super::Runtime,
        pid: usize,
        destination: usize,
        source: usize,
        bytes: usize,
    ) -> bool {
        if source
            .checked_sub(runtime.posix_shm)
            .and_then(|offset| offset.checked_add(bytes))
            .is_none_or(|end| end > runtime.posix_shm_size)
        {
            return false;
        }
        let buffer = unsafe { core::slice::from_raw_parts(source as *const u8, bytes) };
        copy_from_owned(runtime, pid, destination, buffer)
    }
    pub(super) fn copy_from_owned_cached(
        runtime: &super::Runtime, pid: usize, destination: usize, source: &[u8],
    ) -> bool {
        CACHED_WRITE_HIT.with(|hit| hit.get())
            && copy_from_owned(runtime, pid, destination, source)
    }
    pub(super) fn copy_from_scratch_cached(
        runtime: &super::Runtime, pid: usize, destination: usize, source: usize, bytes: usize,
    ) -> bool {
        CACHED_WRITE_HIT.with(|hit| hit.get())
            && copy_from_scratch(runtime, pid, destination, source, bytes)
    }
    pub(super) fn copy_from_owned_many_cached(
        runtime: &super::Runtime, pid: usize, buffers: &[(usize, &[u8])],
    ) -> bool {
        CACHED_WRITE_HIT.with(|hit| hit.get())
            && copy_from_owned_many(runtime, pid, buffers)
    }
    pub(super) fn copy_from_owned_many(
        runtime: &super::Runtime,
        pid: usize,
        buffers: &[(usize, &[u8])],
    ) -> bool {
        BATCH_WRITE_CALLS.with(|calls| calls.set(calls.get() + 1));
        if buffers.len() > 4
            || buffers
                .iter()
                .try_fold(0usize, |sum, (_, source)| {
                    sum.checked_add(source.len()).filter(|total| *total <= 4096)
                })
                .is_none()
            || !WRITE_HIT.with(|hit| hit.get())
            || buffers.iter().any(|(destination,source)| !writable(*destination,source.len()))
        {
            return false;
        }
        buffers
            .iter()
            .all(|(destination, source)| copy_from_owned(runtime, pid, *destination, source))
    }
}
mod private_memory {
    use super::*;
    thread_local! { pub(super) static PREPARED: RefCell<Vec<(Word, Word)>> = const { RefCell::new(Vec::new()) }; }
    thread_local! { pub(super) static INVALIDATIONS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) }; }
    pub(super) fn invalidate_write_origin(_: &Runtime) {
        INVALIDATIONS.with(|count| count.set(count.get() + 1));
    }
    pub(super) fn invalidate_checked_write_origin(runtime: &Runtime, _: Word, _: Word, _: Word) {
        invalidate_write_origin(runtime);
    }
    pub(super) fn needs_checked_write_preparation(runtime: &Runtime, _: Word) -> bool {
        runtime.private_shared && !runtime.readonly_only
    }
    pub(super) fn can_try_cold_write_view(runtime:&Runtime,_:Word)->bool {
        option_env!("NANAMI_COLD_WRITE_VIEWS")!=Some("0") && runtime.cold_views_quiescent
    }
    pub(super) fn with_checked_private_write<T>(
        runtime: &Runtime, pid: Word, address: Word, bytes: Word,
        operation: impl FnOnce() -> Result<T, i32>,
    ) -> Result<T, i32> {
        if needs_checked_write_preparation(runtime, pid) {
            with_private_write(runtime, pid, address, bytes, operation)
        } else { operation() }
    }
    pub(super) fn with_private_write<T>(
        runtime: &Runtime,
        _: Word,
        address: Word,
        bytes: Word,
        operation: impl FnOnce() -> Result<T, i32>,
    ) -> Result<T, i32> {
        if runtime.private_shared {
            PREPARED.with(|p| p.borrow_mut().push((address, bytes)));
            if runtime.private_failure == Some(address) { return Err(EFAULT); }
            guest_views::COW_PAGE.with(|cow| {
                if cow.get().is_some_and(|page|address<page+4096 && address.checked_add(bytes).is_some_and(|end|end>page)) { cow.set(None); }
            });
        }
        operation()
    }
}
pub fn request_process_memory_read_checked(
    pid: Word,
    guest: Word,
    server: Word,
    bytes: Word,
) -> Result<(), ()> {
    transfer(false, pid, guest, server, bytes)
}
pub fn request_process_memory_write_checked(
    pid: Word,
    guest: Word,
    server: Word,
    bytes: Word,
) -> Result<(), ()> {
    transfer(true, pid, guest, server, bytes)
}
pub fn request_process_memory_writev_checked(
    pid: Word,
    transfers: &[ProcessMemoryTransfer],
) -> Result<(), ()> {
    TRANSPORT.with(|transport| transport.borrow_mut().batches += 1);
    for range in transfers {
        transfer(true, pid, range.target, range.local, range.bytes)?;
    }
    Ok(())
}
pub fn request_process_memory_readv_checked(
    pid: Word,
    transfers: &[ProcessMemoryTransfer],
) -> Result<(), ()> {
    TRANSPORT.with(|transport| transport.borrow_mut().batches += 1);
    for range in transfers {
        transfer(false, pid, range.target, range.local, range.bytes)?;
    }
    Ok(())
}
pub fn request_process_memory_write(_: Word, _: Word, _: Word, _: Word) -> Result<(), ()> {
    panic!("guest copies must never select the privileged unchecked request");
}
fn transfer(write: bool, pid: usize, guest: usize, server: usize, bytes: usize) -> Result<(), ()> {
    TRANSPORT.with(|transport| {
        let mut transport = transport.borrow_mut();
        transport.calls.push((write, pid, guest, server, bytes));
        if transport.fail || guest == 0 || guest.checked_add(bytes).is_none() {
            return Err(());
        }
        if transport
            .denied
            .is_some_and(|pointer| guest <= pointer && pointer < guest + bytes)
        {
            // Model a missing later Frame, not just a permission precheck that
            // rejects the whole range before its first byte can be touched.
            if transport.partial_fault {
                let prefix = transport.denied.unwrap() - guest;
                let (source, destination) = if write { (server, guest) } else { (guest, server) };
                unsafe {
                    std::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, prefix);
                }
            }
            return Err(());
        }
        let (source, destination) = if write {
            (server, guest)
        } else {
            (guest, server)
        };
        unsafe {
            std::ptr::copy_nonoverlapping(source as *const u8, destination as *mut u8, bytes);
        }
        Ok(())
    })
}
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/guest_memory.rs"]
mod production;
use production::*;
fn runtime(buffer: &mut [u8]) -> Runtime {
    private_memory::INVALIDATIONS.with(|count| count.set(0));
    private_memory::PREPARED.with(|p| p.borrow_mut().clear());
    guest_views::COPY_INPUT.with(|copy| copy.set(false));
    guest_views::WRITE_HIT.with(|hit| hit.set(false));
    guest_views::COW_PAGE.with(|cow| cow.set(None));
    guest_views::CACHED_WRITE_HIT.with(|hit| hit.set(false));
    guest_views::WRITE_CALLS.with(|calls| calls.set(0));
    guest_views::BATCH_WRITE_CALLS.with(|calls| calls.set(0));
    guest_views::HIT.with(|hit| hit.set(false));
    guest_views::CALLS.with(|calls| calls.set(0));
    TRANSPORT.with(|transport| *transport.borrow_mut() = Transport::default());
    Runtime {
        cold_views_quiescent:false,
        private_shared: false,
        readonly_only: false,
        private_failure: None,
        posix_shm: buffer.as_mut_ptr() as Word,
        posix_shm_size: buffer.len(),
        allowed: true,
        denied: None,
    }
}

#[test]
fn bulk_owned_gather_replaces_scalar_requests_and_initializes_only_the_accepted_prefix() {
    use core::mem::MaybeUninit;
    for sizes in [vec![20000, 19000], vec![20000, 7, 19000], vec![20000, 7, 19000, 11]] {
        let sources: Vec<Vec<u8>> = sizes.iter().enumerate()
            .map(|(i, &n)| vec![i as u8 + 1; n]).collect();
        let mut bases = vec![0];
        bases.extend(sources.iter().map(|v| v.as_ptr() as Word));
        // This unused invalid trailing iovec must never be read or validated.
        bases.push(Word::MAX);
        let mut lengths = vec![0]; lengths.extend(&sizes); lengths.push(10);
        let bytes = sizes.iter().sum::<usize>() - 3;
        let mut destination = vec![MaybeUninit::new(0xcc); bytes + 16];
        let r = runtime(&mut [0; 65536]);
        let mut offset = 0;
        for (&base, &length) in bases.iter().zip(&lengths) {
            let count = length.min(bytes - offset);
            copyin_owned(&r, 10, base, &mut destination[8 + offset..8 + offset + count]).unwrap();
            offset += count;
            if offset == bytes { break; }
        }
        let scalar = TRANSPORT.with(|t| t.borrow().calls.len());
        assert_eq!(scalar, sizes.len());
        TRANSPORT.with(|t| *t.borrow_mut() = Transport::default());
        destination.fill(MaybeUninit::new(0xcc));
        assert_eq!(copyin_owned_gather(&r, 10, &bases, &lengths,
            &mut destination[8..8 + bytes]), Some(Ok(())));
        TRANSPORT.with(|t| {
            let t = t.borrow(); assert_eq!(t.batches, 1); assert_eq!(t.calls.len(), sizes.len());
            let mut offset = 0;
            for (i, call) in t.calls.iter().enumerate() {
                assert_eq!(call.0, false); assert_eq!(call.1, 10);
                assert_eq!(call.2, bases[i + 1]);
                assert_eq!(call.3, destination[8 + offset..].as_ptr() as Word);
                offset += call.4;
            }
            assert_eq!(offset, bytes);
        });
        let expected: Vec<u8> = sources.iter().flatten().copied().take(bytes).collect();
        let actual: Vec<u8> = destination.iter().map(|b| unsafe { b.assume_init() }).collect();
        assert_eq!(&actual[8..8 + bytes], expected);
        assert!(actual[..8].iter().chain(&actual[8 + bytes..]).all(|b| *b == 0xcc));
        assert_eq!(guest_views::CALLS.with(|n| n.get()), sizes.len());
        eprintln!("bulk {} bytes/{} fields: scalar requests={} vector requests=1", bytes, sizes.len(), scalar);
    }
}

#[test]
fn bulk_owned_gather_keeps_leases_and_scalar_fallback_for_ineligible_shapes() {
    use core::mem::MaybeUninit;
    let mut storage = vec![MaybeUninit::new(0xa5); 65537];
    let r = runtime(&mut [0; 65536]);
    for (bases, lengths, bytes) in [
        (vec![1, 2], vec![16384, 16384], 32768),
        (vec![1], vec![40000], 40000),
        (vec![1, 2, 3, 4, 5], vec![20000, 1, 1, 1, 1], 20004),
        (vec![1, 2], vec![40000, 40000], 65537),
        (vec![1, Word::MAX - 3], vec![20000, 10], 20010),
        (vec![1, 0], vec![20000, 10], 20010),
        (vec![1, 2], vec![20000], 20000),
        (vec![1, 2], vec![20000, 1], 20002),
        (vec![1, 2], vec![20000, 1], 0),
    ] {
        assert_eq!(copyin_owned_gather(&r, 10, &bases, &lengths, &mut storage[..bytes]), None);
    }
    TRANSPORT.with(|t| { assert!(t.borrow().calls.is_empty()); assert_eq!(t.borrow().batches, 0); });
    assert!(storage.iter().all(|b| unsafe { b.assume_init() } == 0xa5));
    assert_eq!(guest_views::CALLS.with(|n| n.get()), 0);
}

#[test]
fn bulk_owned_gather_checks_all_logical_sources_and_keeps_failed_input_private() {
    let sources = [vec![0x31u8; 20000], vec![0x42u8; 19000]];
    let bases = sources.each_ref().map(|s| s.as_ptr() as Word);
    let lengths = [20000, 19000];
    for logical in [true, false] {
        let mut r = runtime(&mut [0; 65536]);
        let mut owned: Vec<u8> = Vec::with_capacity(39000);
        if logical { r.denied = Some(bases[1]); }
        else { TRANSPORT.with(|t| {
            let mut t = t.borrow_mut(); t.denied = Some(bases[1] + 7); t.partial_fault = true;
        }); }
        assert_eq!(copyin_owned_gather(&r, 10, &bases, &lengths,
            &mut owned.spare_capacity_mut()[..39000]), Some(Err(EFAULT)));
        assert!(owned.is_empty());
        TRANSPORT.with(|t| {
            let t = t.borrow(); assert_eq!(t.batches, usize::from(!logical));
            assert_eq!(t.calls.len(), if logical { 0 } else { 2 });
        });
        // Only the transport case initialized this first field, but neither
        // failure changes the owner's public length or private-write state.
        if !logical {
            let spare = owned.spare_capacity_mut();
            assert!(spare[..20000].iter().all(|b| unsafe { b.assume_init() } == 0x31));
            assert!(spare[20000..20007].iter().all(|b| unsafe { b.assume_init() } == 0x42));
        }
        assert!(private_memory::PREPARED.with(|p| p.borrow().is_empty()));
    }
}

#[test]
fn cached_owned_vectors_skip_preparation_and_keep_fault_prefix_on_misses() {
    for cached in [false, true] {
        for denied in [false, true] {
            let mut scratch = [0u8; 32];
            let mut output = [0u8; 32];
            let target = output.as_mut_ptr() as Word;
            let mut r = runtime(&mut scratch);
            r.private_shared = true;
            r.private_failure = Some(target);
            r.denied = denied.then_some(target + 8);
            guest_views::WRITE_HIT.with(|hit| hit.set(true));
            guest_views::CACHED_WRITE_HIT.with(|hit| hit.set(cached));
            let buffers = [(target, &[1u8; 8][..]), (target + 8, &[2u8; 8][..])];
            let result = copyout_owned_many(&r, 7, &buffers);
            assert_eq!(result, if cached && !denied { Ok(()) } else { Err(EFAULT) });
            if cached {
                assert_eq!(&output[..8], &[1; 8]);
                assert_eq!(&output[8..16], &[if denied {0} else {2}; 8]);
                private_memory::PREPARED.with(|p| assert!(p.borrow().is_empty()));
                private_memory::INVALIDATIONS.with(|n| assert!(n.get() > 0));
            } else { assert_eq!(output, [0; 32]); }
            TRANSPORT.with(|t| assert!(t.borrow().calls.is_empty()));
        }
    }
}

#[test]
fn cached_writable_copy_skips_preparation_but_misses_and_permissions_do_not() {
    for scratch in [false, true] {
        for cached in [false, true] {
            let mut source = [37u8; 32];
            let mut output = [0u8; 32];
            let target = output.as_mut_ptr() as Word;
            let mut r = runtime(&mut source);
            r.private_shared = true;
            r.private_failure = Some(target);
            guest_views::WRITE_HIT.with(|hit| hit.set(true));
            guest_views::CACHED_WRITE_HIT.with(|hit| hit.set(cached));
            let result = if scratch {
                copyout_from(&r, 7, target, r.posix_shm, 32)
            } else { copyout_owned(&r, 7, target, &source) };
            assert_eq!(result, if cached { Ok(()) } else { Err(EFAULT) });
            assert_eq!(output, [if cached {37} else {0}; 32]);
            private_memory::PREPARED.with(|p| assert_eq!(p.borrow().len(), usize::from(!cached)));
            private_memory::INVALIDATIONS.with(|n| assert!(n.get() > 0));
            r.allowed = false;
            assert_eq!(copyout_owned(&r, 7, target, &[99; 32]), Err(EFAULT));
            assert_eq!(output, [if cached {37} else {0}; 32]);
        }
    }
}

#[test]
fn quiescent_cold_non_cow_views_omit_preparation_for_owned_scratch_and_vectors() {
    let enabled=option_env!("NANAMI_COLD_WRITE_VIEWS")!=Some("0");
    for kind in 0..3 {
        for quiescent in [false,true] {
            let mut source=[37u8;32];let mut output=[0x99u8;32];
            let base=output.as_mut_ptr() as Word;let mut r=runtime(&mut source);
            r.private_shared=true;r.cold_views_quiescent=quiescent;r.private_failure=Some(base);
            guest_views::WRITE_HIT.with(|hit|hit.set(true));
            let result=match kind {
                0=>copyout_owned(&r,7,base,&source[..16]),
                1=>copyout_from(&r,7,base,r.posix_shm,16),
                _=>copyout_owned_many(&r,7,&[(base,&source[..8]),(base+8,&source[8..16])]),
            };
            let copied=enabled&&quiescent;
            assert_eq!(result,if copied {Ok(())}else{Err(EFAULT)});
            assert_eq!(&output[..16],&[if copied {37}else{0x99};16]);
            assert_eq!(&output[16..],&[0x99;16]);
            private_memory::PREPARED.with(|p|assert_eq!(p.borrow().is_empty(),copied));
            TRANSPORT.with(|t|assert!(t.borrow().calls.is_empty()));
            // Logical denial still precedes all native view/store operations.
            let before=output;r.allowed=false;
            assert_eq!(copyout_owned(&r,7,base,&[99;16]),Err(EFAULT));assert_eq!(output,before);
        }
    }
}

#[test]
fn cold_cow_refusal_keeps_the_whole_scalar_unwritten_until_preparation_succeeds() {
    #[repr(align(4096))] struct Pages([u8;8192]);
    for fail in [false,true] {
        let mut source=[37u8;32];let mut output=Box::new(Pages([0x99;8192]));
        let page=output.0.as_mut_ptr() as Word;let target=page+4096-16;
        let mut r=runtime(&mut source);r.private_shared=true;r.cold_views_quiescent=true;
        if fail {r.private_failure=Some(target);}
        guest_views::WRITE_HIT.with(|hit|hit.set(true));guest_views::COW_PAGE.with(|cow|cow.set(Some(page+4096)));
        assert_eq!(copyout_owned(&r,7,target,&source),if fail {Err(EFAULT)}else{Ok(())});
        assert!(output.0[..4096-16].iter().chain(&output.0[4096+16..]).all(|&byte|byte==0x99));
        assert_eq!(&output.0[4096-16..4096+16],&[if fail {0x99}else{37};32]);
        private_memory::PREPARED.with(|p|assert_eq!(*p.borrow(),[(target,32)]));
        if fail {guest_views::COW_PAGE.with(|cow|assert_eq!(cow.get(),Some(page+4096)));}
    }
}

#[test]
fn cold_vector_refusal_preserves_the_old_ordered_prefix_on_later_cow_failure() {
    #[repr(align(4096))] struct Pages([u8;12288]);
    let enabled=option_env!("NANAMI_COLD_WRITE_VIEWS")!=Some("0");
    let mut source=[37u8;32];let mut output=Box::new(Pages([0x99;12288]));let base=output.0.as_mut_ptr() as Word;
    let mut r=runtime(&mut source);r.private_shared=true;r.cold_views_quiescent=true;r.private_failure=Some(base+4096);
    guest_views::WRITE_HIT.with(|hit|hit.set(true));guest_views::COW_PAGE.with(|cow|cow.set(Some(base+4096)));
    let fields=[(base,&source[..8]),(base+4096,&source[8..16]),(base+8192,&source[16..24])];
    assert_eq!(copyout_owned_many(&r,7,&fields),Err(EFAULT));
    assert_eq!(&output.0[..8],&[37;8]);assert!(output.0[8..].iter().all(|&byte|byte==0x99));
    private_memory::PREPARED.with(|p|assert_eq!(*p.borrow(),if enabled {vec![(base+4096,8)]}else{vec![(base,8),(base+4096,8)]}));
}

#[test]
fn cold_copyout_never_admits_unowned_raw_sources_or_oversized_vectors() {
    let mut source=[37u8;32];let foreign=[17u8;32];let mut output=[0x99;32];let base=output.as_mut_ptr() as Word;
    let mut r=runtime(&mut source);r.private_shared=true;r.cold_views_quiescent=true;r.private_failure=Some(base);
    guest_views::WRITE_HIT.with(|hit|hit.set(true));
    assert_eq!(copyout_from(&r,7,base,foreign.as_ptr() as Word,16),Err(EFAULT));assert_eq!(output,[0x99;32]);
    let fields=[(base,&source[..1]);5];
    assert_eq!(copyout_owned_many(&r,7,&fields),Err(EFAULT));assert_eq!(output,[0x99;32]);
}

#[test]
fn posix_write_copy_retains_exact_buffer_and_checked_fallbacks() {
    for (hit, allowed, bytes) in [(true,true,32), (false,true,32), (true,false,32), (true,true,4097), (true,true,8192), (true,true,16384), (true,true,16385), (true,true,0)] {
        let input: Vec<u8> = (0..bytes).map(|i| (i * 37) as u8).collect();
        let mut output = vec![0xabu8; bytes + 16];
        let mut r = runtime(&mut output);
        r.allowed = allowed;
        guest_views::HIT.with(|cached| cached.set(hit));
        guest_views::COPY_INPUT.with(|copy| copy.set(true));
        PERMISSION_CHECKS.with(|count| count.set(0));
        assert_eq!(copyin_posix_write(&r,7,input.as_ptr() as Word,r.posix_shm+8,bytes),Ok(()));
        println!("POSIX-WRITE-SIZE bytes={bytes} permissions={}",PERMISSION_CHECKS.with(|count|count.replace(0)));
        assert_eq!(&output[8..8+bytes],input.as_slice());
        assert!(output[..8].iter().chain(&output[8+bytes..]).all(|b|*b==0xab));
        TRANSPORT.with(|t| assert_eq!(t.borrow().calls.len(),
            usize::from(!(hit && allowed && bytes != 0 && bytes <= guest_views::MAX_CONTIGUOUS_READ_BYTES))));
        TRANSPORT.with(|t| println!("POSIX-WRITE-VIEW hit={hit} allowed={allowed} bytes={bytes} checked-copies={}",t.borrow().calls.len()));
    }
    let input = [7u8; 32];
    let mut output = [0xabu8; 32];
    let r = runtime(&mut output);
    TRANSPORT.with(|t| t.borrow_mut().fail=true);
    assert_eq!(copyin_posix_write(&r,7,input.as_ptr() as Word,r.posix_shm,32),Err(EFAULT));
    assert_eq!(output,[0xab;32]);
}

#[test]
fn single_iovec_storage_keeps_shared_decoder_validation_and_transfer_size() {
    let mut scratch = [0u8; 64];
    let mut r = runtime(&mut scratch);
    let input = [0x1234usize, 27usize];
    let mut bases = [99];
    let mut lengths = [88];
    read_linux_iovecs(&mut r, 1, input.as_ptr() as Word, 1, &mut bases, &mut lengths).unwrap();
    assert_eq!((bases, lengths), ([0x1234], [27]));
    TRANSPORT.with(|t| {
        assert_eq!(t.borrow().calls.len(), 1);
        assert_eq!(t.borrow().calls[0].4, 16);
    });
    TRANSPORT.with(|t| t.borrow_mut().calls.clear());
    assert_eq!(read_linux_iovecs(&mut r, 1, 0, 2, &mut bases, &mut lengths), Err(EINVAL));
    assert_eq!(read_linux_iovecs(&mut r, 1, 0, 1, &mut bases, &mut lengths), Err(EFAULT));
    assert_eq!((bases, lengths), ([0x1234], [27]));
    assert!(TRANSPORT.with(|t| t.borrow().calls.is_empty()));
    let bad = [0usize, 1usize];
    assert_eq!(read_linux_iovecs(&mut r, 1, bad.as_ptr() as Word, 1, &mut bases, &mut lengths), Err(EFAULT));
    assert_eq!((bases, lengths), ([0], [1]));
    TRANSPORT.with(|t| t.borrow_mut().calls.clear());
    assert_eq!(read_linux_iovecs(&mut r, 1, 0, 0, &mut bases, &mut lengths), Ok(()));
    assert!(TRANSPORT.with(|t| t.borrow().calls.is_empty()));
}

#[test]
fn writable_view_is_behind_current_permissions_and_requires_an_owned_source() {
    let mut scratch = [0xabu8; 32];
    let mut runtime = runtime(&mut scratch);
    let mut guest = [0x55u8; 32];
    let target = guest.as_mut_ptr() as Word;
    guest_views::WRITE_HIT.with(|hit| hit.set(true));
    runtime.allowed = false;
    assert_eq!(
        copyout_from(&runtime, 7, target, runtime.posix_shm, 16),
        Err(EFAULT)
    );
    assert_eq!(copyout_owned(&runtime, 7, target, &[0xcc; 16]), Err(EFAULT));
    guest_views::WRITE_CALLS.with(|calls| assert_eq!(calls.get(), 0));
    assert_eq!(guest, [0x55; 32]);
    runtime.allowed = true;
    assert_eq!(
        copyout_from(&runtime, 7, target, runtime.posix_shm, 16),
        Ok(())
    );
    assert_eq!(&guest[..16], &[0xab; 16]);
    assert_eq!(copyout_owned(&runtime, 7, target, &[0xcc; 16]), Ok(()));
    assert_eq!(&guest[..16], &[0xcc; 16]);
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
    let foreign_source = [0xddu8; 16];
    assert_eq!(
        copyout_from(&runtime, 7, target, foreign_source.as_ptr() as Word, 16),
        Ok(())
    );
    assert_eq!(&guest[..16], &[0xdd; 16]);
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().calls.len(), 1));
}

#[test]
fn contiguous_scalar_outputs_keep_ownership_and_ordered_faults_on_view_hits() {
    let mut scratch = [0u8; 32];
    let mut runtime = runtime(&mut scratch);
    let mut guest = [55u32; 3];
    let base = guest.as_mut_ptr() as Word;
    let pointers = [base, base + 4, base + 8];
    guest_views::WRITE_HIT.with(|hit| hit.set(true));
    assert_eq!(
        write_guest_u32s(&mut runtime, 7, pointers, [1, 2, 3]),
        Ok(())
    );
    assert_eq!(guest, [1, 2, 3]);
    guest_views::WRITE_CALLS.with(|calls| assert_eq!(calls.get(), 1));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
    guest.fill(55);
    runtime.denied = Some(base + 4);
    assert_eq!(
        write_guest_u32s(&mut runtime, 7, pointers, [4, 5, 6]),
        Err(EFAULT)
    );
    assert_eq!(guest, [4, 55, 55]);
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn cached_copy_is_still_behind_current_personality_permissions() {
    let mut scratch = [0; 32];
    let mut runtime = runtime(&mut scratch);
    guest_views::HIT.with(|hit| hit.set(true));
    runtime.allowed = false;
    assert_eq!(
        copyin_into(&runtime, 7, 4096, runtime.posix_shm, 16),
        Err(EFAULT)
    );
    guest_views::CALLS.with(|calls| assert_eq!(calls.get(), 0));
    runtime.allowed = true;
    assert_eq!(
        copyin_into(&runtime, 7, 4096, runtime.posix_shm, 16),
        Ok(())
    );
    guest_views::CALLS.with(|calls| assert_eq!(calls.get(), 1));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn owned_input_checks_current_permissions_even_when_a_lease_would_hit() {
    let mut scratch = [0xabu8; 32];
    let mut runtime = runtime(&mut scratch);
    let guest = [7u8; 16];
    let mut destination = [core::mem::MaybeUninit::new(99); 16];
    guest_views::HIT.with(|hit| hit.set(true));
    runtime.allowed = false;
    assert_eq!(
        copyin_owned(&runtime, 7, guest.as_ptr() as Word, &mut destination),
        Err(EFAULT)
    );
    guest_views::CALLS.with(|calls| assert_eq!(calls.get(), 0));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
    assert_eq!(
        destination.map(|byte| unsafe { byte.assume_init() }),
        [99; 16]
    );
    assert_eq!(scratch, [0xab; 32]);
}

#[test]
fn raw_destinations_outside_owned_scratch_keep_the_checked_path() {
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    let guest = [7u8; 16];
    let mut destination = [0u8; 16];
    guest_views::HIT.with(|hit| hit.set(true));
    copyin_into(
        &runtime,
        7,
        guest.as_ptr() as Word,
        destination.as_mut_ptr() as Word,
        destination.len(),
    )
    .unwrap();
    assert_eq!(destination, guest);
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().calls.len(), 1));
}

#[test]
fn owned_input_samples_fresh_bytes_directly_without_scratch_or_copy_rpc() {
    let mut scratch = [0xabu8; 32];
    let runtime = runtime(&mut scratch);
    let mut guest = [7u8; 32];
    guest_views::HIT.with(|hit| hit.set(true));
    for value in [7, 21, 98] {
        guest.fill(value);
        let mut destination = Vec::<u8>::with_capacity(guest.len());
        copyin_owned(
            &runtime,
            7,
            guest.as_ptr() as Word,
            destination.spare_capacity_mut(),
        )
        .unwrap();
        unsafe {
            destination.set_len(guest.len());
        }
        assert_eq!(destination, [value; 32]);
        assert_eq!(scratch, [0xab; 32]);
    }
    guest_views::CALLS.with(|calls| assert_eq!(calls.get(), 3));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn owned_input_misses_and_large_transfers_keep_one_checked_request() {
    for (bytes, hit) in [(32, false), (16385, true)] {
        let mut scratch = [0xabu8; 32];
        let runtime = runtime(&mut scratch);
        let guest = vec![7u8; bytes];
        let mut destination = vec![core::mem::MaybeUninit::uninit(); bytes];
        guest_views::HIT.with(|cached| cached.set(hit));
        copyin_owned(&runtime, 7, guest.as_ptr() as Word, &mut destination).unwrap();
        assert!(destination
            .iter()
            .all(|byte| unsafe { byte.assume_init() } == 7));
        assert_eq!(scratch, [0xab; 32]);
        TRANSPORT.with(|transport| {
            assert_eq!(
                transport.borrow().calls,
                [(
                    false,
                    7,
                    guest.as_ptr() as Word,
                    destination.as_mut_ptr() as Word,
                    bytes
                )]
            );
        });
    }
}

#[test]
fn owned_input_failure_never_exposes_uninitialized_vec_capacity() {
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    let guest = [7u8; 32];
    let mut destination = Vec::<u8>::with_capacity(32);
    destination.extend_from_slice(b"old");
    TRANSPORT.with(|transport| transport.borrow_mut().fail = true);
    assert_eq!(
        copyin_owned(
            &runtime,
            7,
            guest.as_ptr() as Word,
            &mut destination.spare_capacity_mut()[..16]
        ),
        Err(EFAULT)
    );
    assert_eq!(destination, b"old");
}

#[test]
fn empty_owned_input_needs_no_permission_check_lease_or_transport() {
    let mut scratch = [0; 32];
    let mut runtime = runtime(&mut scratch);
    runtime.allowed = false;
    assert_eq!(copyin_owned(&runtime, 7, usize::MAX, &mut []), Ok(()));
    guest_views::CALLS.with(|calls| assert_eq!(calls.get(), 0));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn ordered_copyout_batches_valid_ranges_without_touching_shared_scratch() {
    let mut scratch = [0xabu8; 32];
    let runtime = runtime(&mut scratch);
    let input = [4u32, 8, 12];
    let mut output = [0u32; 3];
    let ranges: Vec<_> = (0..3)
        .map(|i| ProcessMemoryTransfer {
            target: unsafe { output.as_mut_ptr().add(i) } as usize,
            local: &input[i] as *const _ as usize,
            bytes: 4,
        })
        .collect();
    assert_eq!(copyout_many(&runtime, 2, &ranges), Ok(()));
    assert_eq!(output, input);
    assert_eq!(scratch, [0xab; 32]);
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().batches, 1));
}

#[test]
fn ordered_copyout_preserves_prefix_for_personality_and_alpha_failures() {
    for personality in [false, true] {
        let mut scratch = [0; 32];
        let mut runtime = runtime(&mut scratch);
        let input = [4u32, 8, 12];
        let mut output = [99u32; 3];
        let ranges: Vec<_> = (0..3)
            .map(|i| ProcessMemoryTransfer {
                target: unsafe { output.as_mut_ptr().add(i) } as usize,
                local: &input[i] as *const _ as usize,
                bytes: 4,
            })
            .collect();
        if personality {
            runtime.denied = Some(ranges[1].target);
        } else {
            TRANSPORT.with(|transport| transport.borrow_mut().denied = Some(ranges[1].target));
        }
        assert_eq!(copyout_many(&runtime, 2, &ranges), Err(EFAULT));
        assert_eq!(output, [4, 99, 99]);
        TRANSPORT.with(|transport| {
            assert_eq!(transport.borrow().batches, usize::from(!personality));
        });
    }
}

#[test]
fn ordered_copyout_empty_and_zero_length_inputs_retain_scalar_semantics() {
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    assert_eq!(copyout_many(&runtime, 2, &[]), Ok(()));
    assert_eq!(
        copyout_many(
            &runtime,
            2,
            &[ProcessMemoryTransfer {
                target: 0,
                local: 0,
                bytes: 0
            }]
        ),
        Ok(())
    );
    TRANSPORT.with(|transport| {
        assert_eq!(transport.borrow().batches, 0);
        assert!(transport.borrow().calls.is_empty());
    });
}

#[test]
fn three_select_words_use_one_checked_batch_and_keep_overlapping_write_order() {
    for overlap in [false, true] {
        let mut scratch = [0xabu8; 32];
        let runtime = runtime(&mut scratch);
        let mut output = [0x99u8; 32]; let base = output.as_mut_ptr() as Word;
        let values = [1u64.to_ne_bytes(), 2u64.to_ne_bytes(), 4u64.to_ne_bytes()];
        let fields = core::array::from_fn::<_, 3, _>(|i| (base + if overlap { i * 3 } else { i * 8 }, &values[i][..]));
        assert_eq!(copyout_owned_many(&runtime, 2, &fields), Ok(()));
        let mut expected = [0x99u8; 32];
        for i in 0..3 { let start = if overlap { i * 3 } else { i * 8 }; expected[start..start + 8].copy_from_slice(&values[i]); }
        assert_eq!(output, expected); assert_eq!(scratch, [0xab; 32]);
        TRANSPORT.with(|t| { let t = t.borrow(); assert_eq!(t.batches, 1); assert_eq!(t.calls.len(), 3); });
    }
}

#[test]
fn owned_vector_hit_avoids_transport_and_scratch_without_writing_gaps() {
    let mut scratch = [0xabu8; 32];
    let runtime = runtime(&mut scratch);
    let mut output = [99u8; 16];
    let base = output.as_mut_ptr() as Word;
    guest_views::WRITE_HIT.with(|hit| hit.set(true));
    assert_eq!(
        copyout_owned_many(
            &runtime,
            2,
            &[(base + 1, &[4, 5][..]), (base + 8, &[6, 7][..])]
        ),
        Ok(())
    );
    assert_eq!(
        output,
        [99, 4, 5, 99, 99, 99, 99, 99, 6, 7, 99, 99, 99, 99, 99, 99]
    );
    assert_eq!(scratch, [0xab; 32]);
    TRANSPORT.with(|transport| {
        assert_eq!(transport.borrow().batches, 0);
        assert!(transport.borrow().calls.is_empty());
    });
}

#[test]
fn owned_vector_miss_preserves_one_checked_request_and_ordered_fault_prefix() {
    for personality in [false, true] {
        let mut scratch = [0xabu8; 32];
        let mut runtime = runtime(&mut scratch);
        let mut output = [99u8; 16];
        let base = output.as_mut_ptr() as Word;
        if personality {
            runtime.denied = Some(base + 8);
        } else {
            TRANSPORT.with(|transport| transport.borrow_mut().denied = Some(base + 8));
        }
        assert_eq!(
            copyout_owned_many(
                &runtime,
                2,
                &[(base + 1, &[4, 5][..]), (base + 8, &[6, 7][..])]
            ),
            Err(EFAULT)
        );
        assert_eq!(&output[1..3], &[4, 5]);
        assert_eq!(&output[8..10], &[99, 99]);
        assert_eq!(scratch, [0xab; 32]);
        TRANSPORT
            .with(|transport| assert_eq!(transport.borrow().batches, usize::from(!personality)));
        guest_views::BATCH_WRITE_CALLS.with(|calls| {
            assert_eq!(calls.get(), usize::from(!personality));
        });
    }
}

#[test]
fn oversized_owned_vectors_use_one_checked_batch_instead_of_chunking() {
    let mut scratch = [0xabu8; 32];
    let runtime = runtime(&mut scratch);
    let source = [7u8; 4096];
    let mut output = [0u8; 4097];
    let base = output.as_mut_ptr() as Word;
    guest_views::WRITE_HIT.with(|hit| hit.set(true));
    PERMISSION_CHECKS.with(|count| count.set(0));
    assert_eq!(
        copyout_owned_many(
            &runtime,
            2,
            &[(base, &source[..]), (base + 4096, &[8u8][..])]
        ),
        Ok(())
    );
    println!("OWNED-COPYOUT-PREFLIGHT checks={}", PERMISSION_CHECKS.with(|count| count.replace(0)));
    assert_eq!(&output[..4096], &source);
    assert_eq!(output[4096], 8);
    TRANSPORT.with(|transport| {
        assert_eq!(transport.borrow().batches, 1);
        assert_eq!(transport.borrow().calls.len(), 2);
    });
    assert_eq!(scratch, [0xab; 32]);
}

#[test]
fn owned_vector_count_limits_empty_and_zero_fields_keep_scalar_fallback() {
    let mut scratch = [0xabu8; 32];
    let mut runtime = runtime(&mut scratch);
    assert_eq!(copyout_owned_many(&runtime, 2, &[]), Ok(()));
    TRANSPORT.with(|transport| {
        assert_eq!(transport.borrow().batches, 0);
        assert!(transport.borrow().calls.is_empty());
    });
    const N: usize = OS_MEMORY_COPY_VECTOR_MAX + 1;
    let input = [7u8; N];
    let mut output = [99u8; N];
    let base = output.as_mut_ptr() as Word;
    let vectors: [(Word, &[u8]); N] = std::array::from_fn(|i| (base + i, &input[i..i + 1]));
    runtime.denied = Some(base + N / 2);
    assert_eq!(copyout_owned_many(&runtime, 2, &vectors), Err(EFAULT));
    assert!(output[..N / 2].iter().all(|b| *b == 7));
    assert!(output[N / 2..].iter().all(|b| *b == 99));
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().batches, 0));
    runtime.denied = None;
    assert_eq!(copyout_owned_many(&runtime, 2, &[(0, &[][..]), (base, &[3u8][..])]), Ok(()));
    assert_eq!(output[0], 3);
    assert_eq!(scratch, [0xab; 32]);
}

#[test]
fn copyin_and_copyout_use_the_checked_request_and_exact_owned_buffers() {
    let mut guest = [1u8, 2, 3, 4];
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    let mut owned = [0u8; 4];
    copyin_into(
        &runtime,
        2,
        guest.as_ptr() as Word,
        owned.as_mut_ptr() as Word,
        4,
    )
    .unwrap();
    assert_eq!(owned, guest);
    let response = [5u8, 6, 7, 8];
    copyout_from(
        &runtime,
        2,
        guest.as_mut_ptr() as Word,
        response.as_ptr() as Word,
        4,
    )
    .unwrap();
    assert_eq!(guest, response);
    assert_eq!(scratch, [0; 32]);
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().calls.len(), 2));
}

#[test]
fn ordinary_shm_reads_and_writes_use_the_same_checked_helpers() {
    let mut guest = [7u8, 8, 9, 10];
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    read_target_memory(&runtime, 2, guest.as_ptr() as Word, 4).unwrap();
    assert_eq!(&scratch[..4], &guest);
    scratch[..4].copy_from_slice(&[11, 12, 13, 14]);
    write_target_memory(&runtime, 2, guest.as_mut_ptr() as Word, 4).unwrap();
    assert_eq!(guest, [11, 12, 13, 14]);
    TRANSPORT.with(|transport| {
        let transport = transport.borrow();
        assert_eq!(transport.calls[0].3, runtime.posix_shm);
        assert_eq!(transport.calls[1].3, runtime.posix_shm);
    });
}

#[test]
fn rejected_checked_copies_return_efault_without_touching_bytes() {
    let mut guest = [7u8; 8];
    let mut scratch = [0; 32];
    let runtime = runtime(&mut scratch);
    TRANSPORT.with(|transport| transport.borrow_mut().fail = true);
    assert_eq!(
        read_target_memory(&runtime, 2, guest.as_ptr() as Word, 8),
        Err(EFAULT)
    );
    assert_eq!(
        write_target_memory(&runtime, 2, guest.as_mut_ptr() as Word, 8),
        Err(EFAULT)
    );
    assert_eq!(guest, [7; 8]);
    assert_eq!(scratch, [0; 32]);
}

#[test]
fn personality_rejects_unbacked_reservations_before_issuing_a_copy() {
    let mut scratch = [0; 8];
    let mut runtime = runtime(&mut scratch);
    runtime.allowed = false;
    assert_eq!(read_target_memory(&runtime, 2, 0x1000, 4), Err(EFAULT));
    assert_eq!(write_target_memory(&runtime, 2, 0x1000, 4), Err(EFAULT));
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn zero_length_copies_do_not_issue_a_service_request() {
    let mut scratch = [0; 8];
    let runtime = runtime(&mut scratch);
    copyin_into(&runtime, 999, 0, 0, 0).unwrap();
    copyout_from(&runtime, 999, usize::MAX, 0, 0).unwrap();
    TRANSPORT.with(|transport| assert!(transport.borrow().calls.is_empty()));
}

#[test]
fn null_and_overflowing_guest_addresses_preserve_efault() {
    let mut scratch = [0; 8];
    let runtime = runtime(&mut scratch);
    assert_eq!(read_target_memory(&runtime, 2, 0, 4), Err(EFAULT));
    assert_eq!(
        write_target_memory(&runtime, 2, usize::MAX - 1, 4),
        Err(EFAULT)
    );
    assert_eq!(scratch, [0; 8]);
}

#[test]
fn adjacent_scalar_outputs_use_one_checked_copy_without_shm_staging() {
    let mut guest = [99u32; 3];
    let mut scratch = [0; 16];
    let mut runtime = runtime(&mut scratch);
    let base = guest.as_mut_ptr() as Word;
    write_guest_u32s(&mut runtime, 2, [base, base + 4, base + 8], [7, 8, 9]).unwrap();
    assert_eq!(guest, [7, 8, 9]);
    assert_eq!(scratch, [0; 16]);
    TRANSPORT.with(|transport| {
        let transport = transport.borrow();
        assert_eq!(transport.calls.len(), 1);
        assert_eq!((transport.calls[0].0, transport.calls[0].4), (true, 12));
    });
}

#[test]
fn aliases_and_noncontiguous_outputs_keep_scalar_order() {
    let mut guest = [99u32; 3];
    let mut scratch = [0; 16];
    let mut runtime = runtime(&mut scratch);
    let base = guest.as_mut_ptr() as Word;
    write_guest_u32s(&mut runtime, 2, [base + 8, base + 4, base], [7, 8, 9]).unwrap();
    assert_eq!(guest, [9, 8, 7]);
    write_guest_u32s(&mut runtime, 2, [base; 3], [10, 11, 12]).unwrap();
    assert_eq!(guest[0], 12);
    TRANSPORT.with(|transport| assert_eq!(transport.borrow().calls.len(), 6));
}

#[test]
fn a_later_contiguous_fault_preserves_the_successful_prefix_and_untouched_suffix() {
    for partial_fault in [false, true] {
        for failed in 0..3 {
            let mut guest = [99u32; 3];
            let mut scratch = [0; 16];
            let mut runtime = runtime(&mut scratch);
            let base = guest.as_mut_ptr() as Word;
            TRANSPORT.with(|transport| {
                let mut transport = transport.borrow_mut();
                transport.denied = Some(base + failed * 4);
                transport.partial_fault = partial_fault;
            });
            assert_eq!(
                write_guest_u32s(&mut runtime, 2, [base, base + 4, base + 8], [7, 8, 9]),
                Err(EFAULT)
            );
            assert_eq!(&guest[..failed], &[7, 8, 9][..failed]);
            assert!(guest[failed..].iter().all(|&value| value == 99));
            TRANSPORT.with(|transport| assert_eq!(transport.borrow().calls.len(), 2 + failed));
        }
    }
}

#[test]
fn scalar_output_fallback_rejects_null_and_overflow_without_later_stores() {
    let mut guest = [99u32; 3];
    let mut scratch = [0; 16];
    let mut runtime = runtime(&mut scratch);
    let base = guest.as_mut_ptr() as Word;
    assert_eq!(
        write_guest_u32s(&mut runtime, 2, [base, 0, base + 8], [7, 8, 9]),
        Err(EFAULT)
    );
    assert_eq!(guest, [7, 99, 99]);
    assert_eq!(
        write_guest_u32s(&mut runtime, 2, [Word::MAX - 1, base, base + 8], [7, 8, 9]),
        Err(EFAULT)
    );
    assert_eq!(guest, [7, 99, 99]);
    write_guest_u32s(&mut runtime, 2, [], []).unwrap();
}

#[test]
fn cow_copyout_prepares_scalar_owned_and_scratch_destinations_before_stores() {
    for owned in [false, true] {
        let mut scratch = [0x42u8; 32];
        let mut r = runtime(&mut scratch);
        r.private_shared = true;
        let mut output = [0x99u8; 16];
        let target = output.as_mut_ptr() as Word;
        if owned { copyout_owned(&r, 1, target, &scratch[..16]).unwrap(); }
        else { copyout_from(&r, 1, target, r.posix_shm, 16).unwrap(); }
        assert_eq!(output, [0x42; 16]);
        private_memory::PREPARED.with(|p| assert_eq!(*p.borrow(), [(target, 16)]));
        output.fill(0x99);
        r.private_failure = Some(target);
        assert_eq!(copyout_owned(&r, 1, target, &[0x55; 16]), Err(EFAULT));
        assert_eq!(output, [0x99; 16]);
    }
}

#[test]
fn readonly_alias_copyout_uses_the_checked_guard_and_keeps_permission_fault_prefixes() {
    for hit in [false, true] {
        let mut scratch = [0x42u8; 32]; let mut r = runtime(&mut scratch);
        r.private_shared = true; r.readonly_only = true;
        let mut output = [0x99u8; 32]; let base = output.as_mut_ptr() as Word;
        r.private_failure = Some(base);
        guest_views::WRITE_HIT.with(|value| value.set(hit));
        copyout_owned(&r, 1, base, &scratch[..8]).unwrap();
        copyout_from(&r, 1, base + 8, r.posix_shm, 8).unwrap();
        assert_eq!(&output[..16], &[0x42; 16]);
        copyout_owned_many(&r, 1, &[(base, &[7; 8]), (base + 8, &[8; 8])]).unwrap();
        private_memory::INVALIDATIONS.with(|count| assert!(count.get() > 0));
        output.fill(0x99); r.denied = Some(base + 8);
        assert_eq!(copyout_owned_many(&r, 1, &[(base, &[7; 8]), (base + 8, &[8; 8])]), Err(EFAULT));
        assert_eq!(&output[..8], &[7; 8]); assert_eq!(&output[8..], &[0x99; 24]);
        private_memory::PREPARED.with(|p| assert!(p.borrow().is_empty()));
    }
}

#[test]
fn cow_vector_copyout_keeps_committed_prefix_when_a_later_detach_fails() {
    for owned in [false, true] {
        let mut scratch = [0u8; 32];
        let mut r = runtime(&mut scratch);
        r.private_shared = true;
        let mut output = [99u32; 2049];
        let base = output.as_mut_ptr() as Word;
        r.private_failure = Some(base + 4096);
        let bytes = [7u32.to_ne_bytes(), 8u32.to_ne_bytes(), 9u32.to_ne_bytes()];
        let fields = core::array::from_fn::<_, 3, _>(|i| (base + i * 4096, bytes[i].as_slice()));
        let transfers = fields.map(|(target, source)| ProcessMemoryTransfer {
            target, local: source.as_ptr() as Word, bytes: source.len(),
        });
        let result = if owned { copyout_owned_many(&r, 1, &fields) }
            else { copyout_many(&r, 1, &transfers) };
        assert_eq!(result, Err(EFAULT));
        assert_eq!((output[0], output[1024], output[2048]), (7, 99, 99));
        private_memory::PREPARED.with(|p| assert_eq!(*p.borrow(), [(base, 4), (base + 4096, 4)]));
        TRANSPORT.with(|t| assert_eq!(t.borrow().batches, 0));
    }
}

#[test]
fn cow_owned_fields_on_one_page_prepare_once_and_keep_overlapping_store_order() {
    #[repr(align(4096))]
    struct Page([u32; 1024]);
    for hit in [false, true] {
        let mut scratch=[0u8;32]; let mut r=runtime(&mut scratch); r.private_shared=true;
        let mut output=Box::new(Page([99;1024])); let base=output.0.as_mut_ptr() as Word;
        guest_views::WRITE_HIT.with(|v| v.set(hit));
        let values=[7u32.to_ne_bytes(),8u32.to_ne_bytes(),9u32.to_ne_bytes()];
        let fields=[(base+12,values[0].as_slice()),(base+4,values[1].as_slice()),(base+12,values[2].as_slice())];
        copyout_owned_many(&r,1,&fields).unwrap();
        assert_eq!((output.0[0],output.0[1],output.0[3]),(99,8,9));
        private_memory::PREPARED.with(|p| { std::println!("COW-OWNED-PAGE hit={hit} preparations={}",p.borrow().len());assert_eq!(*p.borrow(),[(base+4,12)]); });
        TRANSPORT.with(|t| assert_eq!(t.borrow().batches,usize::from(!hit)));
    }
}

#[test]
fn same_page_cow_preparation_failure_and_denied_field_preserve_store_prefix() {
    #[repr(align(4096))]
    struct Page([u32;1024]);
    for denied in [false,true] {
        let mut scratch=[0u8;32];let mut r=runtime(&mut scratch);r.private_shared=true;
        let mut output=Box::new(Page([99;1024]));let base=output.0.as_mut_ptr() as Word;
        let bytes=[7u32.to_ne_bytes(),8u32.to_ne_bytes(),9u32.to_ne_bytes()];
        let fields=[(base+4,bytes[0].as_slice()),(base+8,bytes[1].as_slice()),(base+12,bytes[2].as_slice())];
        if denied {r.denied=Some(base+8);} else {r.private_failure=Some(base+4);}
        assert_eq!(copyout_owned_many(&r,1,&fields),Err(EFAULT));
        assert_eq!((output.0[1],output.0[2],output.0[3]),(if denied {7} else {99},99,99));
        private_memory::PREPARED.with(|p| assert_eq!(*p.borrow(),[(base+4,if denied {4} else {12})]));
        TRANSPORT.with(|t| assert_eq!(t.borrow().batches,0));
    }
}
