//! Demand population of the fixed initial stack, including sparse fork copies.
use super::{Runtime, Word};

const PAGE: Word = 4096;
const BASE: Word = 0x0400_0000;
const END: Word = 0x0404_0000;

pub(crate) fn materialize_stack_fault(
    runtime: &Runtime, pid: Word, address: Word, architecture_code: Word,
) -> bool {
    if !cfg!(target_arch = "x86_64") || !(BASE..END).contains(&address)
        // Only ordinary not-present faults. Protection/reserved-bit/PK/etc.
        // faults must retain their existing signal behavior.
        || architecture_code & !0x16 != 0
    {
        return false;
    }
    let page = address & !(PAGE - 1);
    let Some(permissions) = runtime.mapping_prot(pid, page, PAGE) else { return false; };
    let required = if architecture_code & 16 != 0 { 4 }
        else if architecture_code & 2 != 0 { 2 } else { 1 };
    if permissions & !7 != 0 || permissions & required == 0
        || !runtime.memory_access_allowed(pid, page, PAGE, required)
    { return false; }
    let Some(process) = runtime.managed_process(pid) else { return false; };
    // All tasks sharing this table must discard any prior guest view. Alpha's
    // authorized protection handler materializes an absent zero page and then
    // installs the current permissions; an existing page is handled idempotently.
    // The lazy constructor maps before filling. A different CLONE_VM task
    // must not observe that intermediate page. Use the same acknowledged
    // stop/resume barrier as private writes, including failed construction.
    super::private_memory::with_vm_quiescence(runtime, pid, || {
        process.mappings.invalidate_views();
        nanami_runtime::request_process_mapping_protect(pid, page, PAGE, permissions)
            .map_err(|_| super::EIO)
    }).is_ok()
}
