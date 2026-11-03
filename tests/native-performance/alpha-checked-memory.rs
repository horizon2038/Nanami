//! Real copy-request authorization and VmSpace permissions; byte transfer mocked.
use super::*;
const PAGE_SIZE: usize = 4096;
const USER_ACCESS: usize = nanami_uapi::os::OS_MEMORY_COPY_USER_ACCESS;
fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}
struct OsRequestEvent {
    identifier: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
}
struct Alpha {
    processes: ProcessManager,
    copies: Vec<(usize, usize, usize, usize, usize)>,
    fail_copy: bool,
    input: Vec<u8>,
    input_reads: usize,
    fail_input: bool,
    replace_input_on_copy: bool,
}
impl Alpha {
    fn new() -> Self {
        let mut processes = manager();
        install(&mut processes, 1, 201);
        for pid in [2, 3] {
            processes
                .install_process(
                    pid,
                    1,
                    200 + pid,
                    300 + pid,
                    400 + pid,
                    500 + pid,
                    0,
                    pid,
                    0,
                    0,
                    0x200000,
                    0x400000,
                )
                .unwrap();
            processes.ensure_vm_space_for_pid(pid).unwrap();
        }
        Self {
            processes,
            copies: Vec::new(),
            fail_copy: false,
            input: Vec::new(),
            input_reads: 0,
            fail_input: false,
            replace_input_on_copy: false,
        }
    }
    fn page(&mut self, va: usize, permissions: usize) {
        let vm = self.processes.vm_space_mut(2).unwrap();
        vm.record_frame(va, va + 100).unwrap();
        vm.set_permissions(va, permissions);
    }
    fn copy_process_memory(
        &mut self,
        source_pid: usize,
        source: usize,
        destination_pid: usize,
        destination: usize,
        bytes: usize,
    ) -> Result<(), CapabilityError> {
        self.copies
            .push((source_pid, source, destination_pid, destination, bytes));
        if self.replace_input_on_copy {
            self.input.fill(0);
            self.replace_input_on_copy = false;
        }
        if self.fail_copy {
            return Err(CapabilityError::Fatal);
        }
        Ok(())
    }
    fn read_process_memory_into_static_buffer(
        &mut self,
        _: usize,
        address: usize,
        bytes: usize,
    ) -> Result<&[u8], CapabilityError> {
        self.input_reads += 1;
        if self.fail_input || bytes > self.input.len() || address.checked_add(bytes).is_none() {
            return Err(CapabilityError::InvalidArgument);
        }
        Ok(&self.input[..bytes])
    }
    fn request(&mut self, base: usize, bytes: usize, write: bool) -> Result<(), CapabilityError> {
        self.handle_process_memory_copy_request(request(base, bytes), write)
    }
}

fn vector_request(count: usize) -> OsRequestEvent {
    OsRequestEvent {
        identifier: 1,
        arg0: 2,
        arg1: 0x9000,
        arg2: count,
        arg3: 0,
    }
}

fn vector_input(alpha: &mut Alpha, transfers: &[nanami_uapi::os::ProcessMemoryTransfer]) {
    alpha.input = transfers
        .iter()
        .flat_map(|range| [range.target, range.local, range.bytes])
        .flat_map(usize::to_ne_bytes)
        .collect();
}

#[test]
fn vector_copies_use_the_same_authority_permissions_and_direction() {
    use nanami_uapi::os::ProcessMemoryTransfer as Transfer;
    assert_eq!(
        core::mem::size_of::<Transfer>(),
        3 * core::mem::size_of::<usize>()
    );
    for write in [false, true] {
        let mut alpha = Alpha::new();
        alpha.page(0x1000, 3);
        let ranges = [
            Transfer {
                target: 0x1100,
                local: 0x9100,
                bytes: 4,
            },
            Transfer {
                target: 0x1200,
                local: 0x9200,
                bytes: 8,
            },
        ];
        vector_input(&mut alpha, &ranges);
        assert_eq!(
            alpha.handle_process_memory_vector_request(vector_request(2), write),
            Ok(())
        );
        assert_eq!(alpha.input_reads, 1);
        let expected: Vec<_> = ranges
            .iter()
            .map(|range| {
                if write {
                    (1, range.local, 2, range.target, range.bytes)
                } else {
                    (2, range.target, 1, range.local, range.bytes)
                }
            })
            .collect();
        assert_eq!(alpha.copies, expected);
    }
}

#[test]
fn vector_rejects_unauthorized_or_invalid_headers_before_reading_descriptors() {
    for mode in 0..7 {
        let mut alpha = Alpha::new();
        let mut request = vector_request(1);
        let expected = if mode == 6 {
            CapabilityError::PermissionDenied
        } else {
            CapabilityError::InvalidArgument
        };
        match mode {
            0 => request.identifier = 0,
            1 => request.arg0 = 0,
            2 => request.arg1 = 0,
            3 => request.arg2 = 0,
            4 => request.arg2 = nanami_uapi::os::OS_MEMORY_COPY_VECTOR_MAX + 1,
            5 => request.arg3 = 1,
            _ => request.identifier = 3,
        }
        assert_eq!(
            alpha.handle_process_memory_vector_request(request, true),
            Err(expected)
        );
        assert_eq!(alpha.input_reads, 0);
        assert!(alpha.copies.is_empty());
    }
}

#[test]
fn vector_descriptor_snapshot_failure_never_writes_a_destination() {
    let mut alpha = Alpha::new();
    alpha.fail_input = true;
    assert_eq!(
        alpha.handle_process_memory_vector_request(vector_request(1), true),
        Err(CapabilityError::InvalidArgument)
    );
    assert!(alpha.copies.is_empty());
}

#[test]
fn vector_maximum_count_and_descriptor_address_overflow_are_bounded() {
    use nanami_uapi::os::{ProcessMemoryTransfer as Transfer, OS_MEMORY_COPY_VECTOR_MAX};
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 3);
    let ranges = [Transfer {
        target: 0x1100,
        local: 0x9100,
        bytes: 4,
    }; OS_MEMORY_COPY_VECTOR_MAX];
    vector_input(&mut alpha, &ranges);
    assert_eq!(
        alpha.handle_process_memory_vector_request(vector_request(ranges.len()), true),
        Ok(())
    );
    assert_eq!(alpha.copies.len(), OS_MEMORY_COPY_VECTOR_MAX);
    alpha.copies.clear();
    let mut request = vector_request(1);
    request.arg1 = usize::MAX;
    assert_eq!(
        alpha.handle_process_memory_vector_request(request, true),
        Err(CapabilityError::InvalidArgument)
    );
    assert!(alpha.copies.is_empty());
}

#[test]
fn vector_shared_vm_tasks_observe_the_owners_current_protection() {
    use nanami_uapi::os::ProcessMemoryTransfer as Transfer;
    let mut alpha = Alpha::new();
    alpha.processes.share_address_space(3, 2).unwrap();
    vector_input(
        &mut alpha,
        &[Transfer {
            target: 0x1100,
            local: 0x9100,
            bytes: 4,
        }],
    );
    for permissions in [1, 3, 0, 3] {
        alpha.page(0x1000, permissions);
        let mut request = vector_request(1);
        request.arg0 = 3;
        let expected = if permissions & 2 != 0 {
            Ok(())
        } else {
            Err(CapabilityError::PermissionDenied)
        };
        assert_eq!(
            alpha.handle_process_memory_vector_request(request, true),
            expected
        );
    }
}

#[test]
fn vector_failure_keeps_completed_prefix_and_checks_each_whole_field() {
    use nanami_uapi::os::ProcessMemoryTransfer as Transfer;
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 3);
    alpha.page(0x2000, 1);
    let ranges = [
        Transfer {
            target: 0x1100,
            local: 0x9100,
            bytes: 4,
        },
        Transfer {
            target: 0x1ffc,
            local: 0x9200,
            bytes: 8,
        },
        Transfer {
            target: 0x1200,
            local: 0x9300,
            bytes: 4,
        },
    ];
    vector_input(&mut alpha, &ranges);
    assert_eq!(
        alpha.handle_process_memory_vector_request(vector_request(3), true),
        Err(CapabilityError::PermissionDenied)
    );
    assert_eq!(alpha.copies, [(1, 0x9100, 2, 0x1100, 4)]);
    alpha.copies.clear();
    assert_eq!(
        alpha.handle_process_memory_vector_request(vector_request(3), false),
        Ok(())
    );
    assert_eq!(alpha.copies.len(), 3);
}

#[test]
fn vector_invalid_later_records_do_not_cancel_completed_transfers() {
    use nanami_uapi::os::ProcessMemoryTransfer as Transfer;
    for invalid in [
        Transfer {
            target: 0,
            local: 0x9200,
            bytes: 4,
        },
        Transfer {
            target: 0x1200,
            local: 0,
            bytes: 4,
        },
        Transfer {
            target: 0x1200,
            local: 0x9200,
            bytes: 0,
        },
        Transfer {
            target: 0x1200,
            local: 0x9200,
            bytes: USER_ACCESS,
        },
    ] {
        let mut alpha = Alpha::new();
        alpha.page(0x1000, 3);
        vector_input(
            &mut alpha,
            &[
                Transfer {
                    target: 0x1100,
                    local: 0x9100,
                    bytes: 4,
                },
                invalid,
            ],
        );
        assert_eq!(
            alpha.handle_process_memory_vector_request(vector_request(2), true),
            Err(CapabilityError::InvalidArgument)
        );
        assert_eq!(alpha.copies, [(1, 0x9100, 2, 0x1100, 4)]);
    }
}

#[test]
fn vector_snapshot_survives_descriptor_storage_being_overwritten_during_copy() {
    use nanami_uapi::os::ProcessMemoryTransfer as Transfer;
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 3);
    vector_input(
        &mut alpha,
        &[
            Transfer {
                target: 0x1100,
                local: 0x9100,
                bytes: 4,
            },
            Transfer {
                target: 0x1200,
                local: 0x9200,
                bytes: 8,
            },
        ],
    );
    alpha.replace_input_on_copy = true;
    assert_eq!(
        alpha.handle_process_memory_vector_request(vector_request(2), true),
        Ok(())
    );
    assert_eq!(
        alpha.copies,
        [(1, 0x9100, 2, 0x1100, 4), (1, 0x9200, 2, 0x1200, 8)]
    );
}
fn request(base: usize, bytes: usize) -> OsRequestEvent {
    OsRequestEvent {
        identifier: 1,
        arg0: 2,
        arg1: base,
        arg2: 0x9000,
        arg3: bytes | USER_ACCESS,
    }
}
#[path = "../../nanami/src/nanami_core/alpha/checked_memory.rs"]
mod production;

#[test]
fn read_only_backing_allows_copyin_but_not_copyout() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1);
    assert_eq!(alpha.request(0x1800, 32, false), Ok(()));
    assert_eq!(alpha.copies, [(2, 0x1800, 1, 0x9000, 32)]);
    assert_eq!(
        alpha.request(0x1800, 32, true),
        Err(CapabilityError::PermissionDenied)
    );
    assert_eq!(alpha.copies.len(), 1);
}

#[test]
fn all_pages_are_permission_checked_before_any_byte_transfer() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 3);
    alpha.page(0x2000, 0);
    for write in [false, true] {
        assert_eq!(
            alpha.request(0x1fff, 2, write),
            Err(CapabilityError::PermissionDenied)
        );
    }
    assert!(alpha.copies.is_empty());
    alpha.page(0x2000, 1);
    assert_eq!(alpha.request(0x1fff, 2, false), Ok(()));
    assert_eq!(
        alpha.request(0x1fff, 2, true),
        Err(CapabilityError::PermissionDenied)
    );
}

#[test]
fn current_protection_changes_are_observed_without_a_personality_cache() {
    let mut alpha = Alpha::new();
    for permissions in [3, 0, 1, 7, 0] {
        alpha.page(0x1000, permissions);
        for write in [false, true] {
            let right = if write { 2 } else { 1 };
            let expected = if permissions & right != 0 {
                Ok(())
            } else {
                Err(CapabilityError::PermissionDenied)
            };
            assert_eq!(alpha.request(0x1100, 64, write), expected);
        }
    }
}

#[test]
fn shared_vm_task_checks_the_owners_current_permissions() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 1);
    alpha.processes.share_address_space(3, 2).unwrap();
    let mut call = request(0x1000, 8);
    call.arg0 = 3;
    assert_eq!(
        alpha.handle_process_memory_copy_request(call, true),
        Err(CapabilityError::PermissionDenied)
    );
    alpha.page(0x1000, 3);
    let mut call = request(0x1000, 8);
    call.arg0 = 3;
    assert_eq!(alpha.handle_process_memory_copy_request(call, true), Ok(()));
    assert_eq!(alpha.copies, [(1, 0x9000, 3, 0x1000, 8)]);
}

#[test]
fn checked_reads_do_not_grant_a_foreign_supervisor_access() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 7);
    for (caller, error) in [
        (0, CapabilityError::InvalidArgument),
        (3, CapabilityError::PermissionDenied),
        (999, CapabilityError::PermissionDenied),
    ] {
        let mut call = request(0x1000, 8);
        call.identifier = caller;
        assert_eq!(
            alpha.handle_process_memory_copy_request(call, false),
            Err(error)
        );
    }
    assert!(alpha.copies.is_empty());
    let mut call = request(0x1000, 8);
    call.identifier = 2;
    assert_eq!(
        alpha.handle_process_memory_copy_request(call, false),
        Ok(())
    );
}

#[test]
fn empty_null_unknown_and_overflowing_requests_never_copy() {
    let mut alpha = Alpha::new();
    for (base, bytes) in [(0, 8), (0x1000, 0), (usize::MAX - 1, 4)] {
        assert_eq!(
            alpha.request(base, bytes, false),
            Err(CapabilityError::InvalidArgument)
        );
    }
    let mut call = request(0x1000, 8);
    call.arg0 = 999;
    assert_eq!(
        alpha.handle_process_memory_copy_request(call, false),
        Err(CapabilityError::InvalidArgument)
    );
    let mut call = request(0x1000, 8);
    call.arg2 = 0;
    assert_eq!(
        alpha.handle_process_memory_copy_request(call, false),
        Err(CapabilityError::InvalidArgument)
    );
    assert!(alpha.copies.is_empty());
}

#[test]
fn privileged_copy_and_default_elf_permissions_retain_their_contracts() {
    let mut alpha = Alpha::new();
    alpha.page(0x1000, 0);
    let mut call = request(0x1000, 8);
    call.arg3 &= !USER_ACCESS;
    assert_eq!(
        alpha.handle_process_memory_copy_request(call, false),
        Ok(())
    );
    alpha.page(0x1000, 7);
    assert_eq!(alpha.request(0x1000, 8, true), Ok(()));
    alpha.fail_copy = true;
    assert_eq!(alpha.request(0x1000, 8, false), Err(CapabilityError::Fatal));
}
