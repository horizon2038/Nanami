#![no_std]
#![no_main]

use nanami_runtime::service::{self, ProviderEvent, Receiver, SESSION_IDENTIFIER_BIT};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("HANDOVER-PROVIDER-FAIL {}", info);
    loop {
        nanami_runtime::yield_now();
    }
}

fn main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let pid = nanami_runtime::get_self_pid()?;
    let interface = if env!("CARGO_BIN_NAME") == "handover-bad" {
        0x7351
    } else {
        0x7350
    };
    let mut receiver = Receiver::register(interface, 28, 29)?;
    // Test-only rendezvous with the explicitly configured manager process.
    nanami_runtime::request_notification_port_copy(1, 26, 27, 1)?;
    nanami_runtime::ipc::notification_notify(nanami_runtime::ipc::process_slot_descriptor(27))?;
    let mut count = 0;
    loop {
        match receiver.receive()? {
            ProviderEvent::Request(request) => {
                assert_ne!(
                    request.identifier & SESSION_IDENTIFIER_BIT,
                    0,
                    "raw PID used as a managed Session identity"
                );
                count += 1;
                let detail = if request.code == 43 {
                    request.identifier
                } else {
                    count
                };
                if request.arg0 == 0x494d5054 {
                    nanami_runtime::println!("IMPORT-PROVIDER-ACCEPTED");
                    nanami_runtime::ipc::notification_notify(
                        nanami_runtime::ipc::process_slot_descriptor(27),
                    )?;
                }
                receiver.reply(nanami_runtime::OS_RESPONSE_OK, pid, detail)?;
            }
            ProviderEvent::Retired => {
                service::retire_provider(receiver.id)?;
                return Ok(());
            }
            ProviderEvent::Event(_) => {}
        }
    }
}
nanami_runtime::nanami_entry!(main);
