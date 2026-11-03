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
    TRANSPORT.with(|transport| *transport.borrow_mut() = Transport::default());
    Runtime {
        posix_shm: buffer.as_mut_ptr() as Word,
        allowed: true,
        denied: None,
    }
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
