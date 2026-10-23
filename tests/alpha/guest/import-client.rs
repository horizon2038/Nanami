#![no_std]
#![no_main]

use nanami_runtime::{self as runtime, service::*, RequestError, Word};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    runtime::println!("IMPORT-CLIENT-FAIL {}", info);
    loop {
        runtime::yield_now();
    }
}

fn call(
    connection: Connection,
    operation: Word,
    signal: Word,
) -> Result<(Word, Word), RequestError> {
    let (status, owner, detail) =
        runtime::call_service_port(connection.endpoint, operation, signal, 0, 0, 0, 2)?;
    assert_eq!(status, runtime::OS_RESPONSE_OK);
    Ok((owner, detail))
}

fn main() -> runtime::NanamiResult {
    runtime::ipc::init_ipc_tls()?;
    assert_eq!(runtime::request_nanami_info_memory()?.total_bytes, 1 << 29);
    assert!(runtime::request_io_port(0x80, 0x80, 25).is_err());
    assert!(connect("unshared-parent-9", 25).is_err());
    assert!(connect("upstream-only-4", 25).is_err());
    // The recursive fixture's immediate parent uses this alias. It must not
    // leak into the grandchild through an implicit namespace fallback.
    assert!(connect("middle-door-5", 25).is_err());
    assert!(nanami_client::registry::connect_service("child-door-9", 25).is_err());
    let connection = connect("child-door-9", 31)?;
    let (first, badge) = call(connection, 43, 0)?;
    assert_eq!(connection.session.0, SESSION_IDENTIFIER_BIT | 1);
    assert_eq!(badge, SESSION_IDENTIFIER_BIT | 2);
    assert_ne!(badge, connection.session.0);
    assert!(a9n_abi::arch::ipc_port::identify(connection.endpoint, 1).is_err());
    let mut info = a9n_abi::MessageInfo::normal(false, 0, 0);
    let mut identifier = 0;
    assert!(matches!(
        a9n_abi::arch::ipc_port::receive(connection.endpoint, &mut info, &mut identifier),
        Err(a9n_abi::CapabilityError::PermissionDenied)
    ));
    assert_eq!(call(connection, 42, 0x494d5054)?.0, first);
    let mut switched = false;
    for _ in 0..4096 {
        if call(connection, 42, 0)?.0 != first {
            switched = true;
            break;
        }
    }
    assert!(switched, "imported handle did not reach the successor");
    let second = connect("child-door-9", 32)?;
    assert_eq!(call(second, 43, 0)?.1, badge);
    assert!(connect("child-door-9", 33).is_err(), "import quota escaped");
    close(second.session)?;
    close(connection.session)?;
    assert!(call(connection, 42, 0).is_err());
    assert!(
        connect("child-door-9", 33).is_err(),
        "quarantined quota was reused"
    );
    runtime::println!("IMPORT-CLIENT-PASS alias=true no-parent-fallback=true scoped-session=true stable-handle=true wo-client=true quota=2 closed=true");
    // A9N's debug ABI writes one character per syscall. Repeated short terminal
    // tokens survive other boot processes interleaving long serial lines; all
    // assertions above must still complete before any success token is emitted.
    for _ in 0..3 {
        runtime::println!("I-C-OK");
    }
    Ok(())
}

runtime::nanami_entry!(main);
