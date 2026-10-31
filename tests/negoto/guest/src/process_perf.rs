//! Distinguish pure Alpha construction from Negoto prepare and Linux fork/exec.
use nanami_client::{registry, timer, vfs};
use nanami_runtime::{ipc, Word};

fn ticks(port: Word) -> (Word, Word) {
    timer::timer_service_monotonic_ticks(port).unwrap()
}
fn ns(start: Word, end: Word, hz: Word) -> u128 {
    (end - start) as u128 * 1_000_000_000 / hz as u128
}
pub fn benchmark(client: &super::Client, timer: Word) {
    registry::connect_vfs_service(25).unwrap();
    let vfs_port = ipc::process_slot_descriptor(25);
    let (shm, capacity) = vfs::vfs_attach_shared_memory(vfs_port, 65536).unwrap();
    let path = b"/alter/linux/tinycore/rootfs/bin/busybox";
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), shm as *mut u8, path.len());
    }
    let fd = vfs::vfs_open(vfs_port, 0, path.len()).unwrap();
    let (_, bytes, _) = vfs::vfs_fstat(vfs_port, fd).unwrap();
    let image = nanami_runtime::request_memory_create(bytes).unwrap();
    let mut offset = 0;
    while offset < bytes {
        let count = vfs::vfs_read(vfs_port, fd, offset, (bytes - offset).min(capacity), 0).unwrap();
        assert!(count != 0 && count <= bytes - offset);
        unsafe {
            core::ptr::copy_nonoverlapping(shm as *const u8, (image + offset) as *mut u8, count);
        }
        offset += count;
    }
    vfs::vfs_close(vfs_port, fd).unwrap();
    for sample in 0..3 {
        let (start, hz) = ticks(timer);
        for _ in 0..8 {
            // Never resume: no interpreter, Linux emulator or POSIX access in
            // this timed region. The native caller owns and reaps every child.
            let pid = nanami_runtime::request_process_spawn_memory_fault_handler_suspended(
                image, bytes, 4, 30,
            )
            .unwrap();
            nanami_runtime::request_process_kill(pid, 0).unwrap();
            nanami_runtime::request_process_reap(pid).unwrap();
        }
        nanami_runtime::println!(
            "PROCESS-PERF alpha-spawn-reap-8 sample={} bytes={} ns={}",
            sample,
            bytes,
            ns(start, ticks(timer).0, hz)
        );
    }
    for sample in 0..3 {
        let (before, hz) = ticks(timer);
        let pid = nanami_runtime::request_process_spawn_memory_fault_handler_suspended(
            image, bytes, 4, 30,
        )
        .unwrap();
        let spawned = ticks(timer).0;
        nanami_runtime::request_process_kill(pid, 0).unwrap();
        nanami_runtime::request_process_reap(pid).unwrap();
        let reaped = ticks(timer).0;
        nanami_runtime::println!(
            "PROCESS-PERF alpha-breakdown sample={} spawn-ns={} kill-reap-ns={}",
            sample,
            ns(before, spawned, hz),
            ns(spawned, reaped, hz)
        );
    }
    nanami_runtime::request_mapping_release(image, bytes).unwrap();
    nanami_runtime::request_mapping_release(shm, capacity).unwrap();
    for sample in 0..6 {
        let id = alloc::format!("tc-create-perf-{sample}");
        let (start, hz) = ticks(timer);
        client.call(serde_json::json!({"operation":"create", "id":id, "bundle":"/alter/linux/tinycore"})).unwrap();
        let elapsed = ns(start, ticks(timer).0, hz);
        client
            .call(serde_json::json!({"operation":"kill", "id":id, "signal":9}))
            .unwrap();
        client.stopped(&id, timer);
        client.command("delete", &id).unwrap();
        nanami_runtime::println!(
            "PROCESS-PERF negoto-create sample={} ns={}",
            sample,
            elapsed
        );
    }
    let native = super::NativeExec::connect();
    let console = super::Console::connect();
    let pid = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/nanami-process-performance",
            "tc-process-perf",
        ],
        Some(console.id),
    );
    let output = console.collect_until(b"PROCESS-PERFORMANCE-PASS", timer);
    native.wait(pid, timer);
    let pid = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/nanami-exec-cache",
            "tc-exec-cache",
        ],
        Some(console.id),
    );
    let freshness = console.collect_until(b"PROCESS-CACHE-FRESHNESS-PASS", timer);
    native.wait(pid, timer);
    nanami_runtime::println!("{}", alloc::string::String::from_utf8_lossy(&freshness));
    nanami_runtime::println!(
        "TINYCORE-PROCESS-PERFORMANCE-PASS {}",
        alloc::string::String::from_utf8_lossy(&output)
    );
}
