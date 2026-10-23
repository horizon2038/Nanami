#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;

mod backend;
mod cli;
mod files;
mod output;
mod server;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[negoto] panic: {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}
#[alloc_error_handler]
fn oom(_: core::alloc::Layout) -> ! {
    panic!("out of memory");
}

fn main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    nanami_runtime::heap::init_heap(1024 * 1024)?;
    if nanami_runtime::process_arg(1) == Some(b"serve") {
        return server::run();
    }
    let mut out = output::Output::new();
    match cli::run(&mut out) {
        Ok(()) => Ok(()),
        Err(error) => {
            out.write(alloc::format!("negoto: {error}\n").as_bytes());
            Err(nanami_runtime::RequestError::InvalidArgument.into())
        }
    }
}
nanami_runtime::nanami_entry!(main);
