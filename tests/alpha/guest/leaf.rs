#![no_std]
#![no_main]

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("DOMAIN-LEAF-FAIL {}", info);
    loop {
        nanami_runtime::yield_now();
    }
}

fn main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let pid = nanami_runtime::get_self_pid()?;
    let memory = nanami_runtime::request_nanami_info_memory()?;
    assert_eq!(
        nanami_client::registry::register_service_with_pid("domain-leaf-fixture")?,
        pid
    );
    assert_eq!(
        nanami_client::registry::connect_service_with_pid("domain-leaf-fixture", 24)?,
        pid
    );
    nanami_runtime::println!("DOMAIN-LEAF-PASS pid={} bytes={}", pid, memory.total_bytes);
    if memory.total_bytes == (1 << 29) {
        // Fixture policy: the child's first process is its probe.
        nanami_runtime::request_notification_port_copy(1, 26, 27, 1)?;
        nanami_runtime::ipc::notification_notify(nanami_runtime::ipc::process_slot_descriptor(27))?;
    }
    let port = nanami_runtime::ipc::process_slot_descriptor(20);
    loop {
        let _ = nanami_runtime::ipc::service_receive_event(port)?;
        nanami_runtime::ipc::service_reply(
            port,
            nanami_runtime::OS_RESPONSE_OK,
            pid,
            memory.total_bytes,
        )?;
    }
}
nanami_runtime::nanami_entry!(main);
