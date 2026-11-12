//! Populate only confirmed deferred backing; other missing faults retain signals.
use super::{Runtime, Word};

pub(crate) fn materialize_snapshot_fault(runtime: &Runtime, pid: Word,
    address: Word, architecture_code: Word) -> bool {
    const PAGE: Word = 4096;
    if !cfg!(target_arch = "x86_64") || architecture_code & !0x16 != 0 { return false; }
    let page = address & !(PAGE - 1);
    if page == 0 || page.checked_add(PAGE).is_none() { return false; }
    let required = if architecture_code & 16 != 0 { 4 }
        else if architecture_code & 2 != 0 { 2 } else { 1 };
    let Some(permissions) = runtime.mapping_prot(pid, page, PAGE) else { return false; };
    if permissions & !7 != 0 || permissions & required == 0
        || !runtime.memory_access_allowed(pid, page, PAGE, required) { return false; }
    // Alpha only adds PTEs for initialized, pinned RAM with current rights.
    // No guest backing/permissions or borrowed view changes during this call.
    nanami_runtime::request_process_populate_snapshot(pid, page, required) == Ok(true)
}
