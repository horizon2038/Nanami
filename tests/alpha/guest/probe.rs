#![no_std]
#![no_main]

mod handover;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("DOMAIN-PROBE-FAIL {}", info);
    loop {
        nanami_runtime::yield_now();
    }
}

fn main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let memory = nanami_runtime::request_nanami_info_memory()?;
    assert_eq!(
        memory.total_bytes,
        1 << 29,
        "child does not own the requested partition"
    );
    assert!(memory.free_bytes > 4096 && memory.free_bytes < memory.total_bytes);
    let pid = nanami_runtime::get_self_pid()?;
    assert!(
        a9n_abi::arch::ipc_port::identify(nanami_runtime::OS_PORT_SLOT2_DESCRIPTOR, 0).is_err(),
        "client can spoof its OS authorization identity"
    );
    assert_eq!(nanami_runtime::get_self_pid()?, pid);
    assert!(
        matches!(
            nanami_runtime::request_driver_platform_info(1),
            Err(nanami_runtime::RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED
            ))
        ),
        "unconfigured process received privileged platform metadata"
    );
    assert_eq!(
        nanami_client::registry::register_service_with_pid("domain-probe-fixture")?,
        pid
    );
    // An exact retry neither consumes a second name nor changes its endpoint.
    assert_eq!(
        nanami_client::registry::register_service_with_pid("domain-probe-fixture")?,
        pid
    );
    assert_eq!(
        nanami_client::registry::connect_service_with_pid("domain-probe-fixture", 24)?,
        pid
    );
    assert!(
        a9n_abi::arch::ipc_port::identify(nanami_runtime::ipc::process_slot_descriptor(24), 99)
            .is_err(),
        "service client can spoof another caller"
    );
    let info = nanami_runtime::service_info_by_ordinal(0)?;
    assert_eq!(info.name(), "domain-probe-fixture");
    assert_eq!(info.owner_pid, pid);
    assert!(nanami_runtime::service_info_by_ordinal(1).is_err());
    assert!(
        nanami_client::registry::connect_service("vfs-service", 25).is_err(),
        "unimported parent namespace visible"
    );
    // after= in the parent's manifest guarantees this service already exists
    // there. It must still be absent locally until we spawn our own leaf.
    assert!(nanami_client::registry::connect_service("domain-leaf-fixture", 25).is_err());
    let before = nanami_runtime::request_nanami_info_memory()?.free_bytes;
    assert!(
        nanami_runtime::request_dma(1 << 30).is_err(),
        "allocation escaped the child's RAM budget"
    );
    assert_eq!(
        nanami_runtime::request_nanami_info_memory()?.free_bytes,
        before,
        "failed oversized allocation leaked child RAM"
    );
    assert!(
        nanami_runtime::request_io_port(0x80, 0x80, 26).is_err(),
        "child inherited parent I/O authority"
    );
    let (address, bytes) = nanami_runtime::request_heap(4096)?;
    assert!(bytes >= 4096);
    unsafe {
        (address as *mut u64).write_volatile(0x123456789abcdef0);
    }
    assert_eq!(
        unsafe { (address as *const u64).read_volatile() },
        0x123456789abcdef0
    );
    nanami_runtime::request_notification_port_create(26, 1)?;
    let child = nanami_runtime::request_process_spawn("./modules/domain-leaf")?;
    assert!(child > pid);
    // Block instead of yielding at priority 16 while the priority-4 leaf starts.
    let notification = nanami_runtime::ipc::process_slot_descriptor(26);
    assert_eq!(nanami_runtime::ipc::notification_wait(notification)?, 1);
    assert!(
        matches!(
            nanami_client::registry::register_service("domain-leaf-fixture"),
            Err(nanami_runtime::RequestError::Status(
                nanami_runtime::OS_RESPONSE_PERMISSION_DENIED
            ))
        ),
        "registration stole the leaf's published name"
    );
    assert_eq!(
        nanami_client::registry::connect_service_with_pid("domain-leaf-fixture", 27)?,
        child
    );
    let port = nanami_runtime::ipc::process_slot_descriptor(27);
    let (status, owner, budget) = nanami_runtime::call_service_port(port, 42, 0, 0, 0, 0, 1)?;
    assert_eq!(status, nanami_runtime::OS_RESPONSE_OK);
    assert_eq!(owner, child);
    assert_eq!(budget, memory.total_bytes);
    nanami_runtime::println!(
        "DOMAIN-PROBE-PASS pid={} leaf={} bytes={} no-parent-fallback=true leaf-echo=true",
        pid,
        child,
        memory.total_bytes
    );
    handover::run()?;
    Ok(())
}
nanami_runtime::nanami_entry!(main);
