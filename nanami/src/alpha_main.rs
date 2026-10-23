#![no_std]
#![no_main]

extern crate alloc;
mod nanami_core;
mod nanami_utils;

use core::mem::MaybeUninit;
use nanami_core::alpha::Alpha;
use nanami_uapi::alpha::{name, AlphaEnvironment};

static mut ALPHA: MaybeUninit<Alpha> = MaybeUninit::uninit();

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".section .text", ".global _start", "_start:",
    "lea rsp, [__alpha_stack_end]", "lea rdi, [__alpha_environment]", "call {entry}",
    "ud2", entry = sym alpha_entry
);
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text", ".global _start", "_start:",
    "adrp x1, __alpha_stack_end", "add x1, x1, :lo12:__alpha_stack_end", "mov sp, x1",
    "adrp x0, __alpha_environment", "add x0, x0, :lo12:__alpha_environment", "bl {entry}",
    "brk #0", entry = sym alpha_entry
);

extern "C" fn alpha_entry(environment: *const AlphaEnvironment) -> ! {
    // The shared launcher maps a complete, validated record and all inputs before resume.
    let environment = unsafe { &*environment };
    environment.validate().expect("invalid Alpha environment");
    if environment.retired_launcher.process != 0 {
        nun::arch::process_control_block::suspend(environment.retired_launcher.process)
            .expect("launcher suspension failed");
        crate::force_info!("[boot] launcher suspended; Alpha owns independent address space");
    }
    let platform = &environment.platform;
    crate::force_info!("Nanami v{}", env!("CARGO_PKG_VERSION"));
    crate::force_info!(
        "Kernel: A9N v{}.{}.{}-{}+{}",
        platform.kernel.major,
        platform.kernel.minor,
        platform.kernel.patch,
        name(&platform.kernel.pre_release),
        name(&platform.kernel.build_metadata)
    );
    crate::force_info!(
        "Environment: architecture={} platform={} cores={}",
        name(&platform.architecture),
        name(&platform.platform),
        platform.core_count
    );
    unsafe {
        let storage = core::ptr::addr_of_mut!(ALPHA);
        (*storage).write(Alpha::bootstrap(environment).expect("alpha bootstrap failed"));
        let alpha = (&mut *storage).assume_init_mut();
        alpha.start();
        alpha.switch_to_runtime_stack_and_run();
    }
}
