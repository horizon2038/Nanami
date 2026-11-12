//! Supervisor-local RAM leases for typed owned buffers and owned scratch.
//! Arbitrary raw local addresses retain Alpha's checked-copy path.
use super::{Runtime, Word};
use crate::state::page_window::{
    Effects, GrantError, LeaseEffects, WriteEffects, MAX_CONTIGUOUS_WRITE_BYTES, PAGE,
};
use core::mem::MaybeUninit;
pub(super) use crate::state::page_window::{MAX_CONTIGUOUS_READ_BYTES, MAX_WRITE_BUFFERS};

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

#[cfg(target_os = "none")]
static DIAGNOSTIC_WRITE_GRANTS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// MOVSQ samples/stores the same forward sequence of eight-byte words as the
// scalar loop. No SIMD/FPU state is used, and default asm memory effects keep
// these external-process accesses observable to the compiler.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn copy_sampled_words(source: *const u8, destination: *mut u8, words: usize) {
    unsafe {
        core::arch::asm!(
            "cld",
            "rep movsq",
            inout("rsi") source => _,
            inout("rdi") destination => _,
            inout("rcx") words => _,
            options(nostack),
        );
    }
}
impl<const WRITE: bool> LeaseEffects for NativeEffects<WRITE> {
    fn grant(&mut self, pid: Word, page: Word, local: Word) -> Result<(), GrantError> {
        #[cfg(target_os = "none")]
        if WRITE && option_env!("NANAMI_LARGE_COPYOUT_DIAG") == Some("1") {
            DIAGNOSTIC_WRITE_GRANTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
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
            #[cfg(target_arch = "x86_64")]
            if destination.len() - copied >= 2048 {
                let words = (destination.len() - copied) / 8;
                copy_sampled_words((source + copied) as *const u8, output.add(copied), words);
                copied += words * 8;
            }
            while destination.len() - copied >= 32 {
                let input = (source + copied) as *const u64;
                let target = output.add(copied) as *mut u64;
                target.write_unaligned(core::ptr::read_volatile(input));
                target.add(1).write_unaligned(core::ptr::read_volatile(input.add(1)));
                target.add(2).write_unaligned(core::ptr::read_volatile(input.add(2)));
                target.add(3).write_unaligned(core::ptr::read_volatile(input.add(3)));
                copied += 32;
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
            #[cfg(target_arch = "x86_64")]
            if source.len() - copied >= 2048 {
                let words = (source.len() - copied) / 8;
                copy_sampled_words(source.as_ptr().add(copied), (destination + copied) as *mut u8, words);
                copied += words * 8;
            }
            while source.len() - copied >= 32 {
                let input = source.as_ptr().add(copied) as *const u64;
                let target = (destination + copied) as *mut u64;
                core::ptr::write_volatile(target, input.read_unaligned());
                core::ptr::write_volatile(target.add(1), input.add(1).read_unaligned());
                core::ptr::write_volatile(target.add(2), input.add(2).read_unaligned());
                core::ptr::write_volatile(target.add(3), input.add(3).read_unaligned());
                copied += 32;
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

#[cfg(test)]
pub(super) unsafe fn read_native_for_test(source: Word, destination: &mut [MaybeUninit<u8>]) {
    NativeEffects::<false>.read(source, 0, destination);
}

#[cfg(test)]
pub(super) unsafe fn write_native_for_test(destination: Word, source: &[u8]) {
    NativeEffects::<true>.write(destination, 0, source);
}

/// The caller has already applied current personality read permissions.
pub(super) fn copy_into_scratch(
    runtime: &Runtime,
    pid: Word,
    source: Word,
    destination: Word,
    bytes: Word,
) -> bool {
    if !ENABLED || bytes > MAX_CONTIGUOUS_READ_BYTES || destination.checked_add(bytes).is_none() {
        return false;
    }
    let owned = |base: Word, size: Word| base != 0 && size != 0
        && destination.checked_sub(base).and_then(|offset| offset.checked_add(bytes))
            .is_some_and(|end| end <= size);
    if !owned(runtime.posix_shm, runtime.posix_shm_size)
        && !owned(runtime.posix_direct_shm, runtime.posix_direct_shm_size)
    {
        return false;
    }
    // Runtime owns this mapped scratch range for the complete synchronous
    // request, including the delegated POSIX window. No borrowed guest pointer
    // is reinterpreted as an owned buffer.
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
    if !ENABLED || destination.len() > MAX_CONTIGUOUS_READ_BYTES {
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
    copy_from_scratch_inner::<false>(runtime, pid, destination, source, bytes)
}

pub(super) fn copy_from_scratch_cached(
    runtime: &Runtime, pid: Word, destination: Word, source: Word, bytes: Word,
) -> bool {
    copy_from_scratch_inner::<true>(runtime, pid, destination, source, bytes)
}

fn copy_from_scratch_inner<const CACHED: bool>(
    runtime: &Runtime, pid: Word, destination: Word, source: Word, bytes: Word,
) -> bool {
    if !WRITE_ENABLED || bytes > MAX_CONTIGUOUS_WRITE_BYTES || source.checked_add(bytes).is_none() {
        return false;
    }
    // Both windows are owned, mapped RAM for this synchronous request. Direct
    // POSIX reads return the delegated window, not the ordinary control SHM.
    // Keeping that window eligible lets repeated small file reads reuse the
    // same authorized writable guest lease instead of copying through Alpha.
    let owned = |base: Word, size: Word| base != 0 && size != 0
        && source.checked_sub(base).and_then(|offset| offset.checked_add(bytes))
            .is_some_and(|end| end <= size);
    if !owned(runtime.posix_shm, runtime.posix_shm_size)
        && !owned(runtime.posix_direct_shm, runtime.posix_direct_shm_size)
    {
        return false;
    }
    let buffer = unsafe { core::slice::from_raw_parts(source as *const u8, bytes) };
    copy_from_owned_inner::<CACHED>(runtime, pid, destination, buffer)
}

/// Current logical write permission was checked by the caller. A cache miss
/// also requires Alpha's current writable, non-COW private RAM authorization.
pub(super) fn copy_from_owned(
    runtime: &Runtime,
    pid: Word,
    destination: Word,
    source: &[u8],
) -> bool {
    copy_from_owned_inner::<false>(runtime, pid, destination, source)
}

pub(super) fn copy_from_owned_cached(
    runtime: &Runtime, pid: Word, destination: Word, source: &[u8],
) -> bool {
    copy_from_owned_inner::<true>(runtime, pid, destination, source)
}

fn copy_from_owned_inner<const CACHED: bool>(
    runtime: &Runtime, pid: Word, destination: Word, source: &[u8],
) -> bool {
    if !WRITE_ENABLED || source.len() > MAX_CONTIGUOUS_WRITE_BYTES {
        return false;
    }
    let Some(process) = runtime.managed_process(pid).filter(|process| {
        !process.exited && process.personality == crate::state::OsPersonality::Linux
    }) else {
        return false;
    };
    #[cfg(target_os = "none")]
    let grants_before = if option_env!("NANAMI_LARGE_COPYOUT_DIAG") == Some("1") {
        DIAGNOSTIC_WRITE_GRANTS.load(core::sync::atomic::Ordering::Relaxed)
    } else { 0 };
    let copied = if CACHED {
        runtime.write_window.copy_from_cached(
            process.vm_group, process.mappings.generation(), pid,
            destination, source, &mut NativeEffects::<true>,
        )
    } else {
        runtime.write_window.copy_from(
            process.vm_group,
            process.mappings.generation(),
            pid,
            destination,
            source,
            &mut NativeEffects::<true>,
        )
    };
    #[cfg(target_os = "none")]
    if copied && source.len() > 16 * 1024 && option_env!("NANAMI_LARGE_COPYOUT_DIAG") == Some("1")
        && process.image_name[..process.image_name_len].rsplit(|byte| *byte == b'/').next() == Some(&b"xfe"[..])
    {
        use core::sync::atomic::{AtomicUsize, Ordering};
        static REPORTS: AtomicUsize = AtomicUsize::new(0);
        if REPORTS.fetch_add(1, Ordering::Relaxed) < 8 {
            let grants = DIAGNOSTIC_WRITE_GRANTS.load(Ordering::Relaxed).wrapping_sub(grants_before);
            nanami_runtime::println!("[linux.copyout] large-view pid={} bytes={} cached-only={} grants={}", pid, source.len(), CACHED, grants);
        }
    }
    copied
}

pub(super) fn copy_from_owned_many(
    runtime: &Runtime,
    pid: Word,
    buffers: &[(Word, &[u8])],
) -> bool {
    copy_from_owned_many_inner::<false>(runtime, pid, buffers)
}

pub(super) fn copy_from_owned_many_cached(
    runtime: &Runtime, pid: Word, buffers: &[(Word, &[u8])],
) -> bool {
    copy_from_owned_many_inner::<true>(runtime, pid, buffers)
}

fn copy_from_owned_many_inner<const CACHED: bool>(
    runtime: &Runtime, pid: Word, buffers: &[(Word, &[u8])],
) -> bool {
    if !WRITE_ENABLED || buffers.len() > MAX_WRITE_BUFFERS {
        return false;
    }
    let Some(process) = runtime.managed_process(pid).filter(|process| {
        !process.exited && process.personality == crate::state::OsPersonality::Linux
    }) else {
        return false;
    };
    if CACHED {
        return runtime.write_window.copy_from_many_cached(
            process.vm_group, process.mappings.generation(), pid,
            buffers, &mut NativeEffects::<true>,
        );
    }
    runtime.write_window.copy_from_many(
        process.vm_group,
        process.mappings.generation(),
        pid,
        buffers,
        &mut NativeEffects::<true>,
    )
}

pub(super) fn has_cached_owner(runtime: &Runtime, pid: Word) -> bool {
    runtime.managed_process(pid).is_some_and(|process| {
        runtime.read_window.has_owner(process.vm_group)
            || runtime.write_window.has_owner(process.vm_group)
    })
}

/// Request-local identity for a private mmap or a nonwritable protection edit.
/// Complete only after the range-confined hardware and metadata commit.
/// Never carry across dispatch, fork/exec or a failed operation.
pub(super) struct BeforeMmap { owner: Word, generation: Word }

impl BeforeMmap {
    pub(super) fn capture(runtime: &Runtime, pid: Word) -> Option<Self> {
        if !ENABLED && !WRITE_ENABLED { return None; }
        let process = runtime.managed_process(pid).filter(|process|
            !process.exited && process.personality == crate::state::OsPersonality::Linux)?;
        let generation = process.mappings.generation();
        (process.vm_group != 0 && generation != 0).then_some(Self { owner: process.vm_group, generation })
    }

    pub(super) fn completed(self, runtime: &Runtime, pid: Word, base: Word, bytes: Word) {
        self.completed_inner(runtime, pid, base, bytes, "mmap");
    }

    pub(super) fn completed_protection(self, runtime: &Runtime, pid: Word, base: Word, bytes: Word) {
        self.completed_inner(runtime, pid, base, bytes, "protect");
    }

    fn completed_inner(self, runtime: &Runtime, pid: Word, base: Word, bytes: Word, operation: &str) {
        let Some(process) = runtime.managed_process(pid).filter(|process|
            !process.exited && process.personality == crate::state::OsPersonality::Linux
                && process.vm_group == self.owner) else { return; };
        let after = process.mappings.generation();
        let read = if ENABLED { runtime.read_window.preserve_disjoint(self.owner, self.generation, after, base, bytes) } else { 0 };
        let write = if WRITE_ENABLED { runtime.write_window.preserve_disjoint(self.owner, self.generation, after, base, bytes) } else { 0 };
        #[cfg(target_os = "none")]
        if option_env!("NANAMI_MMAP_VIEWS_DIAG") == Some("1") && read + write != 0 {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static REPORTS: AtomicUsize = AtomicUsize::new(0);
            if REPORTS.fetch_add(1, Ordering::Relaxed) < 32 {
                nanami_runtime::println!("[linux.mmap] kept-disjoint-views pid={} read={} write={}", pid, read, write);
            }
        }
        let _ = (read, write);
        #[cfg(target_os = "none")]
        if option_env!("NANAMI_RANGE_VIEWS_DIAG") == Some("1") && read + write != 0 {
            nanami_runtime::print!("[views.keep] op={} pid={} read={} write={}\n", operation, pid, read, write);
        }
        let _ = operation;
    }
}

/// Invalidate the VM generation before any PTE change. Only overlapping local
/// aliases are withdrawn; disjoint Ready leases stay pinned but inert in the
/// old generation. The successful outer operation may rebase those exact
/// identities. Failure never revives them, and uncertain releases stay owned.
pub(super) fn invalidate_range(runtime: &Runtime, pid: Word, base: Word, bytes: Word) {
    if !ENABLED && !WRITE_ENABLED { return; }
    if let Some(process) = runtime.managed_process(pid) {
        process.mappings.invalidate_views();
        if ENABLED {
            runtime.read_window.clear_range(process.vm_group, base, bytes, &mut NativeEffects::<false>);
        }
        if WRITE_ENABLED {
            runtime.write_window.clear_range(process.vm_group, base, bytes, &mut NativeEffects::<true>);
        }
    }
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
