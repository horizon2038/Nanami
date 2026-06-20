#![no_std]
#![no_main]

extern crate alloc;

mod nanami_core;
mod nanami_drivers;
mod nanami_utils;

use core::mem::MaybeUninit;
use nanami_core::alpha::Alpha;
use nun::InitInfo;

nun::entry!(main);

static mut ALPHA: MaybeUninit<Alpha> = MaybeUninit::uninit();

fn main(init_info: &InitInfo) {
    crate::info!(r#" _   _                             _ "#);
    crate::info!(r#"| \ | | __ _ _ __   __ _ _ __ ___ (_)"#);
    crate::info!(r#"|  \| |/ _` | '_ \ / _` | '_ ` _ \| |"#);
    crate::info!(r#"| |\  | (_| | | | | (_| | | | | | | |"#);
    crate::info!(r#"|_| \_|\__,_|_| |_|\__,_|_| |_| |_|_|"#);
    crate::info!("");

    crate::info!("[entry] main entered");
    unsafe {
        ALPHA.write(Alpha::bootstrap(init_info).expect("alpha bootstrap failed"));
        let alpha = ALPHA.assume_init_mut();
        crate::info!("[entry] alpha bootstrap returned");
        alpha.start();
        alpha.switch_to_runtime_stack_and_run();
    }
}
