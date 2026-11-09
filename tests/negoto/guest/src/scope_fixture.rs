//! Functional exercise of the opt-in scope, with a standard-prctl control.
use super::*;

pub fn run(client: &Client, files: &files::Files, timer: Word) {
    let enabled = match nanami_runtime::process_arg(2) {
        Some(b"active") => true,
        Some(b"off") => false,
        _ => panic!("scope fixture requires active/off"),
    };
    let native = NativeExec::connect();
    let console = Console::connect();
    let paths = ["/alter/linux/tinycore/rootfs/tmp/scope-count.bin",
                 "/alter/linux/tinycore/rootfs/tmp/scope-time.bin"];
    for path in paths {
        if files.stat(path).is_ok() { files.remove(path).unwrap(); }
        assert!(files.stat(path).is_err(), "stale scope report");
    }
    assert!(client.command("state", "scope-fixture").is_err());
    let pid = native.spawn_cli(&[
        "run", "--bundle", "/alter/linux/nanami-syscall-scope", "scope-fixture",
    ], Some(console.id));
    let marker: &[u8] = if enabled {
        b"SCOPE-FIXTURE-PASS enabled=1"
    } else {
        b"SCOPE-FIXTURE-PASS enabled=0"
    };
    console.expect(marker, timer);
    native.wait(pid, timer);
    assert!(client.command("state", "scope-fixture").is_err());
    for (mode, path) in paths.into_iter().enumerate() {
        if enabled {
            scope_packet::validate(&files.read(path).unwrap(), mode as u64);
        } else {
            assert!(files.read(path).is_err(), "inactive fixture produced a report");
        }
    }
    // Test-only post-workload persistence. Reuse the existing Files connection
    // (slot24); attaching a second shared buffer would invalidate that client.
    nanami_client::vfs::vfs_sync(ipc::process_slot_descriptor(24)).unwrap();
    nanami_runtime::println!("SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled={} readout={}",
        usize::from(enabled), if enabled { 2 } else { 0 });
}

