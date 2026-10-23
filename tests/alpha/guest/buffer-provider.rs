#![no_std]
#![no_main]

use nanami_runtime::{self as runtime, service::*};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("BUFFER-PROVIDER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn main() -> Result<(), runtime::RequestError> {
    runtime::ipc::init_ipc_tls()?;
    let pid = runtime::get_self_pid()?;
    let mut receiver = Receiver::register_buffered(0x8833, 28, 29)?;
    runtime::request_notification_port_copy(1, 26, 27, 1)?;
    runtime::ipc::notification_notify(runtime::ipc::process_slot_descriptor(27))?;
    let mut last = 0usize;
    loop {
        match receiver.receive()? {
            ProviderEvent::Request(request) => {
                let buffer = session_buffer(SessionId(request.identifier))?;
                assert_eq!(buffer.bytes, 65536);
                assert!(
                    runtime::request_mapping_release(buffer.address, buffer.bytes).is_err(),
                    "legacy release bypassed managed ownership"
                );
                last = buffer.address;
                let words = buffer.address as *mut u64;
                assert_eq!(unsafe { words.read_volatile() }, 0x5142554646455251);
                let detail = if request.code == 43 {
                    buffer.address
                } else {
                    let count = unsafe { words.add(1).read_volatile() } + 1;
                    unsafe {
                        words.add(1).write_volatile(count);
                    }
                    count as usize
                };
                receiver.reply(runtime::OS_RESPONSE_OK, pid, detail)?;
            }
            ProviderEvent::Retired => {
                retire_provider(receiver.id)?;
                assert_ne!(last, 0);
                runtime::println!("BUFFER-STALE-READ-BEGIN pid={} address={:#x}", pid, last);
                // An expected real user page fault is required by the runner.
                // Returning here would prove that stale data access survived.
                // Explicit faulting machine access, not an invalid Rust
                // dereference whose optimization semantics would be undefined.
                let value: usize;
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("mov {value}, qword ptr [{address}]", value = out(reg) value, address = in(reg) last, options(nostack, readonly));
                }
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    core::arch::asm!("ldr {value}, [{address}]", value = out(reg) value, address = in(reg) last, options(nostack, readonly));
                }
                panic!("stale Provider mapping remained readable: {:#x}", value);
            }
            ProviderEvent::Event(_) => {}
        }
    }
}
fn entry() -> runtime::NanamiResult {
    main().expect("buffer Provider operation failed");
    Ok(())
}
runtime::nanami_entry!(entry);
