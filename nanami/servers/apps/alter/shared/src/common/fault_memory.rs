//! Demand publication of initialized owned backing uses common VMA rights.
use crate::state::{OsPersonality, Runtime};
use nanami_runtime::Word;

pub(super) fn materialize(
    runtime: &Runtime, pid: Word, _personality: OsPersonality, address: Word, code: Word,
) -> bool {
    if crate::linux::bus_fault(runtime, pid, address) {
        return false;
    }
    if crate::linux::materialize_private_write_fault(runtime, pid, address, code)
    {
        return true;
    }
    // mmap and its tracked permissions are shared by both personalities.
    // Alpha still validates current initialized backing, ownership and rights.
    crate::linux::materialize_snapshot_fault(runtime, pid, address, code)
        || crate::linux::materialize_stack_fault(runtime, pid, address, code)
}
