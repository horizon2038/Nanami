use core::arch::asm;

use super::Alpha;
use crate::info;

pub(super) const fn ipc_buffer_tls_base(ipc_buffer_va: usize) -> usize {
    // AArch64's a9n_abi reads the IPC buffer address directly from TPIDR_EL0.
    ipc_buffer_va
}

extern "C" fn run_on_relocated_stack(alpha_ptr: *mut Alpha) -> ! {
    let alpha = unsafe { &mut *alpha_ptr };
    info!("[stack] switched to runtime stack");
    alpha.run_event_loop();
}

pub(super) unsafe fn jump_to_relocated_stack(alpha_ptr: *mut Alpha, new_sp: usize) -> ! {
    unsafe {
        asm!(
            "mov x0, {alpha}",
            "and x1, {stack}, #0xfffffffffffffff0",
            "mov sp, x1",
            "mov x29, xzr",
            "br {entry}",
            alpha = in(reg) alpha_ptr,
            stack = in(reg) new_sp,
            entry = in(reg) run_on_relocated_stack as extern "C" fn(*mut Alpha) -> !,
            options(noreturn)
        )
    }
}
