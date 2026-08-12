use core::arch::asm;

use super::Alpha;
use crate::info;

extern "C" fn run_on_relocated_stack(alpha_ptr: *mut Alpha) -> ! {
    let alpha = unsafe { &mut *alpha_ptr };
    info!("[stack] switched to runtime stack");
    alpha.run_event_loop();
}

pub(super) unsafe fn jump_to_relocated_stack(alpha_ptr: *mut Alpha, new_sp: usize) -> ! {
    unsafe {
        asm!(
            "mov rdi, {alpha}",
            "mov rsp, {stack}",
            "and rsp, -16",
            "sub rsp, 8",
            "mov rbp, rsp",
            "jmp {entry}",
            alpha = in(reg) alpha_ptr,
            stack = in(reg) new_sp,
            entry = in(reg) run_on_relocated_stack as extern "C" fn(*mut Alpha) -> !,
            options(noreturn)
        )
    }
}
