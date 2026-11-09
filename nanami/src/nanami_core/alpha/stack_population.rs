//! Raw guests receive their initial stack before resume through their supervisor.
//! Keep the whole zero-backed lazy plan, but populate only its initial tail.
// Keep startup growth within the mapped tail instead of trapping repeatedly
// through the supervisor. The lower stack remains demand-populated; reservation
// size, permissions, zero-before-publication and the legacy opt-out are unchanged.
const STARTUP_PAGES: usize = 32;

pub(super) fn initial_permissions(raw: bool) -> usize {
    if cfg!(target_arch = "x86_64") && raw
        && option_env!("NANAMI_LAZY_RAW_STACK") != Some("0")
    { 3 } else { 7 }
}
pub(super) fn first_page(raw_fault_process: bool, reserved_pages: usize) -> usize {
    if cfg!(target_arch = "x86_64") && raw_fault_process
        && option_env!("NANAMI_LAZY_RAW_STACK") != Some("0")
    {
        reserved_pages.saturating_sub(STARTUP_PAGES)
    } else {
        0
    }
}

pub(super) fn exec_first_page(sealed: bool, has_ipc_buffer: bool, reserved_pages: usize) -> usize {
    // Sealed raw guests retain demand growth below the initial startup tail.
    // Native IPC and unsealed execution keep their full initial population.
    if cfg!(target_arch = "x86_64") && sealed && !has_ipc_buffer
        && option_env!("NANAMI_LAZY_RAW_STACK") != Some("0")
    { reserved_pages.saturating_sub(STARTUP_PAGES) } else { 0 }
}
