#![no_std]
#![no_main]
extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use nanami_runtime::{self as runtime, service::*, RequestError, Word};
#[path = "event-queue.rs"]
mod queue;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("EVENT-PROVIDER-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn run() -> Result<(), RequestError> {
    runtime::ipc::init_ipc_tls()?;
    let pid = runtime::get_self_pid()?;
    let mut receiver = Receiver::register_buffered_events(0x8834, 28, 29, 30)?;
    runtime::request_notification_port_copy(1, 26, 27, 1)?;
    runtime::ipc::notification_notify(runtime::ipc::process_slot_descriptor(27))?;
    let mut views: Vec<Word> = Vec::new();
    loop {
        match receiver.receive()? {
            ProviderEvent::Request(request) => {
                // Deliberately do not drain queues on CALL. Data progress must
                // be caused by direct Notification delivery, not IPC polling.
                if request.code == 44 {
                    assert_eq!(views.len(), 1);
                    let data = unsafe { &*(views[0] as *const queue::Queue) };
                    let calls = data.accepted_calls.fetch_add(1, Ordering::AcqRel) + 1;
                    while data.reply_release.load(Ordering::Acquire) == 0 {
                        runtime::yield_now();
                    }
                    receiver.reply(runtime::OS_RESPONSE_OK, pid, calls)?;
                } else {
                    assert_eq!(request.code, 43);
                    receiver.reply(runtime::OS_RESPONSE_OK, pid, 0)?;
                }
            }
            ProviderEvent::Event(runtime::ipc::ServiceEvent::Notification {
                identifier, ..
            }) => {
                if identifier & SERVICE_RESOURCE_NOTIFICATION != 0 {
                    views.clear();
                    let mut ordinal = 0;
                    while let Some(session) = buffer_session(receiver.id, ordinal)? {
                        views.push(session_buffer(session)?.address);
                        ordinal += 1;
                    }
                }
                for &address in &views {
                    let data = unsafe { &*(address as *const queue::Queue) };
                    data.drain(pid);
                }
            }
            ProviderEvent::Retired => {
                let old = runtime::ipc::process_slot_descriptor(30);
                assert!(a9n_abi::arch::notification_port::poll(old).is_err());
                assert!(a9n_abi::arch::notification_port::notify(old).is_err());
                retire_provider(receiver.id)?;
                return Ok(());
            }
            _ => panic!("unexpected event Provider input"),
        }
    }
}
fn main() -> runtime::NanamiResult {
    run().expect("event Provider operation failed");
    Ok(())
}
runtime::nanami_entry!(main);
