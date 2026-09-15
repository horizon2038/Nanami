#![allow(dead_code)]
extern crate alloc;
extern crate self as nun;

use std::cell::RefCell;

pub type CapabilityDescriptor = usize;
#[path = "../../nanami/src/nanami_utils/avl.rs"]
pub mod avl;
#[path = "../../nanami/src/nanami_utils/static_avl.rs"]
pub mod static_avl;
pub mod nanami_utils {
    pub use crate::{avl, static_avl};
}
#[path = "../../nanami/src/nanami_core/vm_space.rs"]
mod vm_space;
use vm_space::{BootstrapVmSpace, VmSpace, VmTracker};

const EIO: i32 = 5;
const EFAULT: i32 = 14;
const EINVAL: i32 = 22;
#[path = "../../nanami/servers/apps/alter/shared/src/personality/linux/vectored.rs"]
mod vectored;

fn check_page_table_hint(vm: &mut impl VmTracker) {
    const BASE: usize = 0x68000000;
    const REGION: usize = 1 << 21;
    assert!(!vm.page_tables_ready(BASE));
    assert!(!vm.page_tables_ready(usize::MAX));
    // Recording an intermediate table is not proof that every level exists.
    vm.record_page_table(BASE, 1).unwrap();
    assert!(!vm.page_tables_ready(BASE));
    vm.record_frame(BASE + 4096, 7).unwrap();
    assert!(vm.page_tables_ready(BASE));
    assert!(vm.page_tables_ready(BASE + REGION - 1));
    assert!(!vm.page_tables_ready(BASE - 1));
    assert!(!vm.page_tables_ready(BASE + REGION));
    assert_eq!(vm.forget_frame(BASE + 4096), Some(7));
    assert!(vm.page_tables_ready(BASE)); // UNMAP of a frame leaves the table in place.
    vm.record_frame(BASE + REGION, 8).unwrap();
    assert!(vm.page_tables_ready(BASE + REGION));
    assert!(!vm.page_tables_ready(BASE)); // Bounded one-entry hint; misses are safe.
}

#[test]
fn page_table_hint_covers_only_the_last_successfully_mapped_region() {
    check_page_table_hint(&mut VmSpace::new());
}

#[test]
fn page_table_hint_is_address_space_local_and_reset_on_replacement() {
    let mut first = VmSpace::new();
    let second = VmSpace::new();
    first.record_frame(0x1000, 7).unwrap();
    assert!(first.page_tables_ready(0x2000));
    assert!(!second.page_tables_ready(0x2000));
    first = VmSpace::new();
    assert!(!first.page_tables_ready(0x1000));
}

#[test]
fn bootstrap_page_table_hint_has_the_same_lifetime_and_region_rules() {
    // The real bootstrap tracker is deliberately large and allocation-free.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            check_page_table_hint(&mut BootstrapVmSpace::new());
        })
        .unwrap()
        .join()
        .unwrap();
}

struct Transfer {
    result: Result<usize, i32>,
    output: Vec<u8>,
    copies: usize,
    writes: usize,
}

fn transfer(
    source: &[u8],
    iovecs: &[(usize, usize)],
    capacity: usize,
    fail_copy: Option<usize>,
    write_results: &[Result<usize, i32>],
) -> Transfer {
    let buffer = RefCell::new(vec![0xcc; capacity]);
    let mut copies = 0;
    let mut writes = 0;
    let mut output = Vec::new();
    let result = vectored::writev(
        iovecs.iter().copied(),
        capacity,
        |base, offset, len| {
            copies += 1;
            assert!(len != 0 && offset + len <= capacity);
            if fail_copy == Some(copies) {
                // Alpha can copy some bytes before encountering an unmapped page.
                buffer.borrow_mut()[offset..offset + len].fill(0xfe);
                return Err(EFAULT);
            }
            let end = base.checked_add(len).ok_or(EFAULT)?;
            let bytes = source.get(base..end).ok_or(EFAULT)?;
            buffer.borrow_mut()[offset..offset + len].copy_from_slice(bytes);
            Ok(())
        },
        |len| {
            let result = write_results.get(writes).copied().unwrap_or(Ok(len));
            writes += 1;
            assert!(len != 0 && len <= capacity);
            if let Ok(written) = result {
                if written <= len {
                    output.extend_from_slice(&buffer.borrow()[..written]);
                }
            }
            result
        },
    );
    Transfer {
        result,
        output,
        copies,
        writes,
    }
}

#[test]
fn writev_batches_service_ipc_without_an_extra_copy() {
    let source: Vec<_> = (0..4096).map(|i| i as u8).collect();
    let iovecs: Vec<_> = (0..16).map(|i| (i * 256, 256)).collect();
    let t = transfer(&source, &iovecs, 4096, None, &[]);
    assert_eq!(t.result, Ok(4096));
    assert_eq!(t.output, source);
    assert_eq!(t.copies, 16); // Alpha transfers are unchanged.
    assert_eq!(t.writes, 1); // Service writes: 16 -> 1.
}

#[test]
fn writev_crosses_buffer_and_iovec_boundaries_with_zero_entries() {
    let source: Vec<_> = (0..20000).map(|i| (i * 7) as u8).collect();
    let iovecs = [
        (0, 0),
        (13, 23),
        (777, 4099),
        (usize::MAX, 0),
        (9000, 8888),
        (0, 0),
    ];
    let expected: Vec<_> = iovecs
        .iter()
        .filter(|(_, len)| *len != 0)
        .flat_map(|&(base, len)| source[base..base + len].iter().copied())
        .collect();
    for capacity in [1, 13, 128, 4096, 65536] {
        let t = transfer(&source, &iovecs, capacity, None, &[]);
        assert_eq!(t.result, Ok(expected.len()));
        assert_eq!(t.output, expected);
        assert_eq!(t.writes, expected.len().div_ceil(capacity));
    }
    let t = transfer(&source, &[(usize::MAX, 0), (0, 0)], 128, None, &[]);
    assert_eq!((t.result, t.copies, t.writes), (Ok(0), 0, 0));
}

#[test]
fn writev_fault_commits_only_successfully_staged_bytes() {
    let source = b"abcdefghijklmnop";
    let iovecs = [(0, 4), (4, 4), (8, 4)];
    for failed in 1..=3 {
        let t = transfer(source, &iovecs, 16, Some(failed), &[]);
        let bytes = (failed - 1) * 4;
        assert_eq!(t.result, if bytes == 0 { Err(EFAULT) } else { Ok(bytes) });
        assert_eq!(t.output, source[..bytes]);
    }
    let t = transfer(source, &[(0, 16)], 4, Some(3), &[]);
    assert_eq!(t.result, Ok(8));
    assert_eq!(t.output, source[..8]);
}

#[test]
fn writev_short_write_and_service_failure_return_committed_count() {
    let source = b"abcdefghijklmnop";
    for written in 0..8 {
        let t = transfer(source, &[(0, 5), (5, 11)], 8, None, &[Ok(written)]);
        assert_eq!(t.result, Ok(written));
        assert_eq!(t.output, source[..written]);
        assert_eq!(t.writes, 1);
    }
    for failure in [Err(EIO), Ok(9)] {
        let first = transfer(source, &[(0, 16)], 8, None, &[failure]);
        assert_eq!(first.result, Err(EIO));
        let second = transfer(source, &[(0, 16)], 8, None, &[Ok(8), failure]);
        assert_eq!(second.result, Ok(8));
        assert_eq!(second.output, source[..8]);
    }
}

#[test]
fn writev_invalid_capacity_and_overflow_do_not_issue_unbounded_copies() {
    let t = transfer(b"x", &[(0, 1)], 0, None, &[]);
    assert_eq!((t.result, t.copies, t.writes), (Err(EIO), 0, 0));
    let mut copies = 0;
    let mut writes = 0;
    let result = vectored::writev(
        [(usize::MAX, 2)].into_iter(),
        1,
        |_, _, _| {
            copies += 1;
            Ok(())
        },
        |len| {
            writes += 1;
            Ok(len)
        },
    );
    assert_eq!((result, copies, writes), (Ok(1), 1, 1));
}
