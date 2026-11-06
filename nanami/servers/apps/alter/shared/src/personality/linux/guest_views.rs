//! Supervisor-local RAM leases for typed owned buffers and owned scratch.
//! Arbitrary raw local addresses retain Alpha's checked-copy path.
use super::{Runtime, Word};
use crate::state::page_window::{Effects, GrantError, LeaseEffects, WriteEffects, PAGE};
use core::mem::MaybeUninit;

const ENABLED: bool = match option_env!("NANAMI_GUEST_READ_VIEWS") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    // Other backends retain checked copies until their runtime tests pass.
    None => cfg!(target_arch = "x86_64"),
};
const WRITE_ENABLED: bool = match option_env!("NANAMI_GUEST_WRITE_VIEWS") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    // Other backends retain checked copies until their runtime tests pass.
    None => cfg!(target_arch = "x86_64"),
};
struct NativeEffects<const WRITE: bool>;
impl<const WRITE: bool> LeaseEffects for NativeEffects<WRITE> {
    fn grant(&mut self, pid: Word, page: Word, local: Word) -> Result<(), GrantError> {
        let result = if WRITE {
            nanami_runtime::request_process_write_view(pid, page, local)
        } else {
            nanami_runtime::request_process_read_view(pid, page, local)
        };
        result.map_err(|error| match error {
            nanami_runtime::RequestError::Status(
                nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT
                | nanami_runtime::OS_RESPONSE_PERMISSION_DENIED
                | nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR
                | nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
            ) => GrantError::NotInstalled,
            // Unknown status values are protocol errors, not proof that the
            // remote operation left the VA unmapped.
            _ => GrantError::Uncertain,
        })
    }
    fn release(&mut self, local: Word) -> Result<(), ()> {
        nanami_runtime::request_mapping_release(local, PAGE).map_err(|_| ())
    }
}
impl Effects for NativeEffects<false> {
    fn read(&mut self, local: Word, offset: Word, destination: &mut [MaybeUninit<u8>]) {
        // The lease pins readable WB RAM. Volatile sampling also permits a
        // different guest thread to modify its input while the syscall runs.
        let source = local + offset;
        let output = destination.as_mut_ptr() as *mut u8;
        let mut copied = 0;
        unsafe {
            while copied < destination.len() && (source + copied) & 7 != 0 {
                output
                    .add(copied)
                    .write(core::ptr::read_volatile((source + copied) as *const u8));
                copied += 1;
            }
            while destination.len() - copied >= 8 {
                let value = core::ptr::read_volatile((source + copied) as *const u64);
                (output.add(copied) as *mut u64).write_unaligned(value);
                copied += 8;
            }
            while copied < destination.len() {
                output
                    .add(copied)
                    .write(core::ptr::read_volatile((source + copied) as *const u8));
                copied += 1;
            }
        }
    }
}

impl WriteEffects for NativeEffects<true> {
    fn write(&mut self, local: Word, offset: Word, source: &[u8]) {
        // The supervisor owns source; the writable alias pins ordinary RAM.
        // Volatile stores permit simultaneous guest access without creating
        // a Rust reference to memory owned by a different running process.
        let destination = local + offset;
        let mut copied = 0;
        unsafe {
            while copied < source.len() && (destination + copied) & 7 != 0 {
                core::ptr::write_volatile((destination + copied) as *mut u8, source[copied]);
                copied += 1;
            }
            while source.len() - copied >= 8 {
                let value = (source.as_ptr().add(copied) as *const u64).read_unaligned();
                core::ptr::write_volatile((destination + copied) as *mut u64, value);
                copied += 8;
            }
            while copied < source.len() {
                core::ptr::write_volatile((destination + copied) as *mut u8, source[copied]);
                copied += 1;
            }
        }
    }
}

/// The caller has already applied current personality read permissions.
pub(super) fn copy_into_scratch(
    runtime: &Runtime,
    pid: Word,
    source: Word,
    destination: Word,
    bytes: Word,
) -> bool {
    if !ENABLED || bytes > PAGE || destination < runtime.posix_shm {
        return false;
    }
    let Some(end) = (destination - runtime.posix_shm).checked_add(bytes) else {
        return false;
    };
    if end > runtime.posix_shm_size {
        return false;
    }
    // Runtime owns this mapped scratch range for the complete synchronous
    // request. No borrowed guest pointer is reinterpreted as an owned buffer.
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(destination as *mut MaybeUninit<u8>, bytes) };
    copy_into_owned(runtime, pid, source, buffer)
}

/// The caller has checked current personality permissions. The typed borrow
/// proves ownership of the output; a guest address cannot designate this buffer.
pub(super) fn copy_into_owned(
    runtime: &Runtime,
    pid: Word,
    source: Word,
    destination: &mut [MaybeUninit<u8>],
) -> bool {
    if !ENABLED || destination.len() > PAGE {
        return false;
    }
    let Some(process) = runtime
        .managed_process(pid)
        .filter(|process| !process.exited)
    else {
        return false;
    };
    runtime.read_window.copy_into(
        process.vm_group,
        process.mappings.generation(),
        pid,
        source,
        destination,
        &mut NativeEffects::<false>,
    )
}

pub(super) fn copy_from_scratch(
    runtime: &Runtime,
    pid: Word,
    destination: Word,
    source: Word,
    bytes: Word,
) -> bool {
    if !WRITE_ENABLED || bytes > PAGE || source < runtime.posix_shm {
        return false;
    }
    let Some(end) = (source - runtime.posix_shm).checked_add(bytes) else {
        return false;
    };
    if end > runtime.posix_shm_size {
        return false;
    }
    let buffer = unsafe { core::slice::from_raw_parts(source as *const u8, bytes) };
    copy_from_owned(runtime, pid, destination, buffer)
}

/// Current logical write permission was checked by the caller. A cache miss
/// also requires Alpha's current writable, non-COW private RAM authorization.
pub(super) fn copy_from_owned(
    runtime: &Runtime,
    pid: Word,
    destination: Word,
    source: &[u8],
) -> bool {
    if !WRITE_ENABLED || source.len() > PAGE {
        return false;
    }
    let Some(process) = runtime.managed_process(pid).filter(|process| {
        !process.exited && process.personality == crate::state::OsPersonality::Linux
    }) else {
        return false;
    };
    runtime.write_window.copy_from(
        process.vm_group,
        process.mappings.generation(),
        pid,
        destination,
        source,
        &mut NativeEffects::<true>,
    )
}

pub(super) fn copy_from_owned_many(
    runtime: &Runtime,
    pid: Word,
    buffers: &[(Word, &[u8])],
) -> bool {
    if !WRITE_ENABLED {
        return false;
    }
    let Some(process) = runtime.managed_process(pid).filter(|process| {
        !process.exited && process.personality == crate::state::OsPersonality::Linux
    }) else {
        return false;
    };
    runtime.write_window.copy_from_many(
        process.vm_group,
        process.mappings.generation(),
        pid,
        buffers,
        &mut NativeEffects::<true>,
    )
}

pub(super) fn invalidate(runtime: &Runtime, pid: Word) {
    if !ENABLED && !WRITE_ENABLED {
        return;
    }
    if let Some(process) = runtime.managed_process(pid) {
        process.mappings.invalidate_views();
        if ENABLED {
            runtime
                .read_window
                .clear_owner(process.vm_group, &mut NativeEffects::<false>);
        }
        if WRITE_ENABLED {
            runtime
                .write_window
                .clear_owner(process.vm_group, &mut NativeEffects::<true>);
        }
    }
}
