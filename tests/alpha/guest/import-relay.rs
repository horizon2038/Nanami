#![no_std]
#![no_main]

use nanami_runtime::{self as runtime, service::*, RequestError};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("IMPORT-RELAY-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn run() -> Result<(), RequestError> {
    runtime::ipc::init_ipc_tls()?;
    // This PID is local to the intermediate Alpha, not the root manager or
    // the grandchild client, which intentionally have the same local PID.
    assert_eq!(runtime::get_self_pid()?, 1);
    let memory = runtime::request_nanami_info_memory()?;
    assert_eq!(memory.total_bytes, 1 << 30);
    // A successful downstream import consumes the relay's only upstream
    // Session allowance. It must not be a quota-free forwarding shortcut.
    assert!(connect("middle-door-5", 31).is_err());
    assert!(connect("upstream-only-4", 31).is_err());
    assert!(connect("unshared-parent-9", 31).is_err());
    assert!(connect("child-door-9", 31).is_err());
    assert!(runtime::request_io_port(0x80, 0x80, 25).is_err());
    // Half the delegated memory belongs to the grandchild and must not be
    // counted as free in its parent after the grandchild starts.
    assert!(memory.free_bytes < (1 << 29));
    runtime::println!("IMPORT-RELAY-PASS budget=1073741824 quota=1");
    for _ in 0..3 {
        runtime::println!("I-R-OK");
    }
    Ok(())
}

fn main() -> runtime::NanamiResult {
    if let Err(error) = run() {
        panic!("recursive import relay failed: {:?}", error);
    }
    Ok(())
}

runtime::nanami_entry!(main);
