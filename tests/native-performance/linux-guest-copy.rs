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
struct Runtime {
    posix_shm: usize,
    posix_shm_size: usize,
    allowed: bool,
    denied: Option<usize>,
}
impl Runtime {
    fn memory_access_allowed(&self, _: Word, address: Word, bytes: Word, _: Word) -> bool {
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
fn map_request_error(_: ()) -> i32 {
    EFAULT
}
mod guest_views {
    use std::cell::Cell;
    thread_local! {
        pub(super) static HIT: Cell<bool> = const { Cell::new(false) };
        pub(super) static CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static WRITE_HIT: Cell<bool> = const { Cell::new(false) };
        pub(super) static WRITE_CALLS: Cell<usize> = const { Cell::new(0) };
        pub(super) static BATCH_WRITE_CALLS: Cell<usize> = const { Cell::new(0) };
    }
    pub(super) fn copy_into_scratch(
        runtime: &super::Runtime,
        _: usize,
        _: usize,
        destination: usize,
        bytes: usize,
    ) -> bool {
        CALLS.with(|calls| calls.set(calls.get() + 1));
        if destination
            .checked_sub(runtime.posix_shm)
            .and_then(|offset| offset.checked_add(bytes))
            .is_none_or(|end| end > runtime.posix_shm_size)
        {
            return false;
        }
        HIT.with(|hit| hit.get())
    }
    pub(super) fn copy_into_owned(
        _: &super::Runtime,
        _: usize,
        source: usize,
        destination: &mut [core::mem::MaybeUninit<u8>],
    ) -> bool {
        CALLS.with(|calls| calls.set(calls.get() + 1));
        if destination.len() > 4096 || !HIT.with(|hit| hit.get()) {
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
        if source.len() > 4096 || !WRITE_HIT.with(|hit| hit.get()) {
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
    pub(super) fn with_private_write<T>(
        _: &Runtime,
        _: Word,
        _: Word,
        _: Word,
        operation: impl FnOnce() -> Result<T, i32>,
    ) -> Result<T, i32> {
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
            if transport.partial_fault && write {
                let prefix = transport.denied.unwrap() - guest;
                unsafe {
                    std::ptr::copy_nonoverlapping(server as *const u8, guest as *mut u8, prefix);
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
    guest_views::WRITE_HIT.with(|hit| hit.set(false));
    guest_views::WRITE_CALLS.with(|calls| calls.set(0));
    guest_views::BATCH_WRITE_CALLS.with(|calls| calls.set(0));
    guest_views::HIT.with(|hit| hit.set(false));
    guest_views::CALLS.with(|calls| calls.set(0));
    TRANSPORT.with(|transport| *transport.borrow_mut() = Transport::default());
    Runtime {
        posix_shm: buffer.as_mut_ptr() as Word,
        posix_shm_size: buffer.len(),
        allowed: true,
        denied: None,
    }
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
    for (bytes, hit) in [(32, false), (16384, true)] {
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
    assert_eq!(
        copyout_owned_many(
            &runtime,
            2,
            &[(base, &source[..]), (base + 4096, &[8u8][..])]
        ),
        Ok(())
    );
    assert_eq!(&output[..4096], &source);
    assert_eq!(output[4096], 8);
    TRANSPORT.with(|transport| {
        assert_eq!(transport.borrow().batches, 1);
        assert_eq!(transport.borrow().calls.len(), 2);
    });
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
