//! Distinguish pure Alpha construction from Negoto prepare and Linux fork/exec.
use nanami_client::{registry, timer, vfs};
use nanami_runtime::{ipc, Word};

fn ticks(port: Word) -> (Word, Word) {
    timer::timer_service_monotonic_ticks(port).unwrap()
}
fn ns(start: Word, end: Word, hz: Word) -> u128 {
    (end - start) as u128 * 1_000_000_000 / hz as u128
}

/// Time the real OCI create/start path through a terminal's X11 connection.
/// Host input, UART reporting and session teardown are outside the measurement.
pub fn desktop_startup(client: &super::Client, files: &super::files::Files, timer: Word) {
    let console = super::Console::connect();
    let marker = "/alter/linux/tinycore/rootfs/tmp/desktop-ready";
    let id = "tc-startup-perf";
    for sample in 0..3 {
        assert!(client.command("state", id).is_err());
        if files.stat(marker).is_ok() {
            files.remove(marker).unwrap();
        }
        assert!(files.stat(marker).is_err(), "stale desktop-ready marker");
        let command = serde_json::json!({
            "operation":"create", "id":id,
            "bundle":"/alter/linux/tinycore-desktop", "terminal":console.id
        });
        let (before, hz) = ticks(timer);
        client.call(command).unwrap();
        client.command("start", id).unwrap();
        let mut ready = false;
        for _ in 0..1200 {
            if files.read(marker).is_ok_and(|bytes| bytes == b"ready") {
                ready = true;
                break;
            }
            // The observer must not starve the lower-priority X11 clients.
            timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
        }
        let elapsed = ns(before, ticks(timer).0, hz);
        assert!(ready, "desktop never completed its real X11 client startup");
        // Optional profile diagnostic, sampled only after the timed readiness
        // boundary. The normal profile need not create this file.
        let wait_attempts = files.read("/alter/linux/tinycore/rootfs/tmp/desktop-wait-attempts");
        assert_eq!(client.command("state", id).unwrap()["status"], "running");
        client
            .call(serde_json::json!({"operation":"kill", "id":id, "signal":9}))
            .unwrap();
        client.stopped(id, timer);
        client.command("delete", id).unwrap();
        assert!(client.command("state", id).is_err());
        if let Ok(attempts) = wait_attempts {
            nanami_runtime::println!(
                "STARTUP-WAIT sample={} attempts={}",
                sample,
                alloc::string::String::from_utf8_lossy(&attempts).trim()
            );
        }
        nanami_runtime::println!(
            "STARTUP-PERF negoto-create-start-to-x11-ready sample={} poll-ms=100 ns={}",
            sample,
            elapsed
        );
    }
    nanami_runtime::println!("TINYCORE-STARTUP-PERFORMANCE-PASS samples=3 cleanup=true");
}

fn read_image(vfs_port: Word, shm: Word, capacity: Word, path: &[u8]) -> (Word, Word) {
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
    (image, bytes)
}

/// Untimed regression: a released private page may still be mapped by a peer.
/// Reusing it must allocate different backing, not zero the peer's live data.
pub fn memory_reference_reuse() {
    registry::connect_vfs_service(25).unwrap();
    let port = ipc::process_slot_descriptor(25);
    let (shm, capacity) = vfs::vfs_attach_shared_memory(port, 65536).unwrap();
    let (image, bytes) = read_image(port, shm, capacity, b"/bin/terminal-service");
    let peer = nanami_runtime::request_process_spawn_memory_suspended(image, bytes, 4, 30).unwrap();
    nanami_runtime::request_mapping_release(image, bytes).unwrap();
    let own = nanami_runtime::get_self_pid().unwrap();
    let base = 0x5_0000_0000;
    let scratch = nanami_runtime::request_memory_create(4096).unwrap();
    for round in 0..8 {
        let (address, size) =
            nanami_runtime::request_process_map_anonymous_at(own, base, 4096).unwrap();
        assert_eq!((address, size), (base, 4096));
        let cookie = 0x1234_5678_9000_0000usize + round;
        unsafe {
            core::ptr::write_volatile(base as *mut usize, cookie);
        }
        let alias = nanami_runtime::request_mapping_share(peer, base, 4096).unwrap();
        nanami_runtime::request_mapping_release(base, 4096).unwrap();
        nanami_runtime::request_process_map_anonymous_at(own, base, 4096).unwrap();
        unsafe {
            assert_eq!(core::ptr::read_volatile(base as *const usize), 0);
        }
        nanami_runtime::request_process_memory_read(peer, alias, scratch, 8).unwrap();
        let peer_value = unsafe { core::ptr::read_volatile(scratch as *const usize) };
        assert_eq!(
            peer_value, cookie,
            "reused RAM still mapped by peer, round={round}"
        );
        // Release the peer first; the old deferred reference still owns its RAM.
        nanami_runtime::request_process_mapping_release(peer, alias, 4096).unwrap();
        nanami_runtime::request_mapping_release(base, 4096).unwrap();
    }
    nanami_runtime::request_process_kill(peer, 0).unwrap();
    nanami_runtime::request_process_reap(peer).unwrap();
    nanami_runtime::request_mapping_release(scratch, 4096).unwrap();
    nanami_runtime::println!("RAM-SHARING-REUSE-PASS rounds=8 peer-data=true cleanup=true");
}

/// Observe real RAM ownership, not just equal bytes in two independent copies.
/// Both source and children stay suspended throughout this privileged test.
pub fn private_readonly_fork() {
    use nanami_runtime as rt;
    const BYTES: usize = 4 * 1024 * 1024;
    const BASE: usize = 0x5_0000_0000;
    registry::connect_vfs_service(25).unwrap();
    let port = ipc::process_slot_descriptor(25);
    let (shm, capacity) = vfs::vfs_attach_shared_memory(port, 65536).unwrap();
    let (image, bytes) = read_image(port, shm, capacity, b"/bin/terminal-service");
    let source = rt::request_process_spawn_memory_suspended(image, bytes, 4, 30).unwrap();
    rt::request_mapping_release(image, bytes).unwrap();
    let scratch = rt::request_memory_create(BYTES).unwrap();
    unsafe {
        core::ptr::write_bytes(scratch as *mut u8, 0x33, BYTES);
    }
    rt::request_process_map_initialized_at(source, BASE, BYTES, scratch, BYTES).unwrap();
    rt::request_process_mapping_protect(source, BASE, BYTES, 1).unwrap();
    rt::request_process_seal_capabilities(source).unwrap();
    for round in 0..4 {
        let mut costs = [0usize; 2];
        for (mode, cost) in costs.iter_mut().enumerate() {
            let before = rt::request_nanami_info_memory().unwrap().free_bytes;
            let child = if mode == 0 {
                rt::request_process_fork(source, 31, 4).unwrap()
            } else {
                rt::request_process_fork_shared_readonly(source, 31, 4).unwrap()
            };
            let after = rt::request_nanami_info_memory().unwrap().free_bytes;
            *cost = before.saturating_sub(after);
            if mode == 1 {
                assert!(rt::request_process_private_write(
                    child,
                    BASE,
                    BYTES,
                    rt::OS_PRIVATE_WRITE_QUERY
                )
                .unwrap());
                assert!(rt::request_process_memory_write(child, BASE, scratch, 8).is_err());
                assert!(rt::request_process_mapping_protect(child, BASE, 4096, 3).is_err());
                rt::request_process_private_write(child, BASE, 4096, rt::OS_PRIVATE_WRITE_DETACH)
                    .unwrap();
                unsafe {
                    core::ptr::write_volatile(scratch as *mut usize, 0x77);
                }
                rt::request_process_memory_write(child, BASE, scratch, 8).unwrap();
                rt::request_process_memory_read(source, BASE, shm, 8).unwrap();
                assert_eq!(
                    unsafe { core::ptr::read_volatile(shm as *const usize) },
                    0x3333_3333_3333_3333
                );
                rt::request_process_memory_read(child, BASE, shm, 8).unwrap();
                assert_eq!(
                    unsafe { core::ptr::read_volatile(shm as *const usize) },
                    0x77
                );
            }
            rt::request_process_kill(child, 0).unwrap();
            rt::request_process_reap(child).unwrap();
        }
        if round != 0 {
            let saved = costs[0].saturating_sub(costs[1]);
            assert!(saved >= 3 * 1024 * 1024, "RO sharing did not save real RAM");
            rt::println!(
                "PRIVATE-RO-RAM round={} eager={} shared={} saved={}",
                round,
                costs[0],
                costs[1],
                saved
            );
        }
    }
    // The original root may disappear first. A private fork's read-only
    // aliases must remain live, including the last-owner detach fast path.
    let survivor = rt::request_process_fork_shared_readonly(source, 31, 4).unwrap();
    rt::request_process_kill(source, 0).unwrap();
    rt::request_process_reap(source).unwrap();
    rt::request_process_memory_read(survivor, BASE, shm, 8).unwrap();
    assert_eq!(unsafe { core::ptr::read_volatile(shm as *const usize) }, 0x3333_3333_3333_3333);
    rt::request_process_private_write(survivor, BASE, 4096, rt::OS_PRIVATE_WRITE_DETACH).unwrap();
    rt::request_process_memory_write(survivor, BASE, scratch, 8).unwrap();
    rt::request_process_kill(survivor, 0).unwrap();
    rt::request_process_reap(survivor).unwrap();
    rt::request_mapping_release(scratch, BYTES).unwrap();
    rt::println!("PRIVATE-READONLY-FORK-PASS ram=true guarded-write=true isolation=true source-reap=true");
}

/// Untimed regression: two aliases in the final owner must free RAM once.
pub fn memory_alias_retirement() {
    const BYTES: usize = 1024 * 1024;
    const WARMUP: usize = 2;
    const ROUNDS: usize = 4;
    registry::connect_vfs_service(25).unwrap();
    let port = ipc::process_slot_descriptor(25);
    let (shm, capacity) = vfs::vfs_attach_shared_memory(port, 65536).unwrap();
    let (image, bytes) = read_image(port, shm, capacity, b"/bin/terminal-service");
    let mut initial_free = 0;
    for round in 0..WARMUP + ROUNDS {
        if round == WARMUP {
            initial_free = nanami_runtime::request_nanami_info_memory()
                .unwrap()
                .free_bytes;
        }
        let peer =
            nanami_runtime::request_process_spawn_memory_suspended(image, bytes, 4, 30).unwrap();
        let (source, first) = nanami_runtime::request_shared_memory(peer, BYTES).unwrap();
        let second = nanami_runtime::request_mapping_share(peer, source, BYTES).unwrap();
        assert_ne!(first, second);
        unsafe {
            core::ptr::write_volatile(source as *mut usize, 0x1234_5678 + round);
        }
        // Retire the original owner first. The suspended peer now owns both
        // remaining references; neither can be freed by a count==1 snapshot.
        nanami_runtime::request_mapping_release(source, BYTES).unwrap();
        nanami_runtime::request_process_memory_read(peer, second, shm, 8).unwrap();
        assert_eq!(
            unsafe { core::ptr::read_volatile(shm as *const usize) },
            0x1234_5678 + round
        );
        nanami_runtime::request_process_kill(peer, 0).unwrap();
        nanami_runtime::request_process_reap(peer).unwrap();
        nanami_runtime::println!(
            "RAM-ALIAS-ROUND round={} free={}",
            round,
            nanami_runtime::request_nanami_info_memory()
                .unwrap()
                .free_bytes
        );
    }
    let final_free = nanami_runtime::request_nanami_info_memory()
        .unwrap()
        .free_bytes;
    // Permit small late metadata growth, not one leaked MiB per retirement.
    let lost = initial_free.saturating_sub(final_free);
    assert!(
        lost <= 256 * 1024,
        "same-owner aliases leaked RAM: {lost} bytes"
    );
    nanami_runtime::request_mapping_release(image, bytes).unwrap();
    nanami_runtime::println!("RAM-ALIAS-RETIREMENT-PASS rounds=4 aliases=2 cleanup=true");
}

pub fn benchmark(client: &super::Client, timer: Word) {
    registry::connect_vfs_service(25).unwrap();
    let vfs_port = ipc::process_slot_descriptor(25);
    let (shm, capacity) = vfs::vfs_attach_shared_memory(vfs_port, 65536).unwrap();
    let (image, bytes) = read_image(
        vfs_port,
        shm,
        capacity,
        b"/alter/linux/tinycore/rootfs/bin/busybox",
    );
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
    // A second, larger, unmodified Tiny Core ELF. It is never resumed, so
    // interpreter/DSO startup is excluded and kill/reap is timed separately.
    // Keep the VFS window valid until the last read; do not reuse a released
    // session's cached shared-memory address.
    let (image, bytes) = read_image(
        vfs_port,
        shm,
        capacity,
        b"/alter/linux/tinycore/rootfs/usr/local/bin/xfe",
    );
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
            "PROCESS-PERF alpha-xfe sample={} bytes={} spawn-ns={} kill-reap-ns={}",
            sample,
            bytes,
            ns(before, spawned, hz),
            ns(spawned, reaped, hz)
        );
    }
    nanami_runtime::request_mapping_release(image, bytes).unwrap();
    // Include source preparation and final alias cleanup, not only the faster
    // construction interval while the caller keeps its input buffer alive.
    let cases: [(&str, &[u8], usize); 2] = [
        (
            "alpha-lifecycle-busybox-8",
            b"/alter/linux/tinycore/rootfs/bin/busybox",
            8,
        ),
        (
            "alpha-lifecycle-xfe-3",
            b"/alter/linux/tinycore/rootfs/usr/local/bin/xfe",
            3,
        ),
    ];
    for (name, path, count) in cases {
        for sample in 0..3 {
            let (before, hz) = ticks(timer);
            let (image, bytes) = read_image(vfs_port, shm, capacity, path);
            let prepared = ticks(timer).0;
            for _ in 0..count {
                let pid = nanami_runtime::request_process_spawn_memory_fault_handler_suspended(
                    image, bytes, 4, 30,
                )
                .unwrap();
                nanami_runtime::request_process_kill(pid, 0).unwrap();
                nanami_runtime::request_process_reap(pid).unwrap();
            }
            let constructed = ticks(timer).0;
            nanami_runtime::request_mapping_release(image, bytes).unwrap();
            let released = ticks(timer).0;
            nanami_runtime::println!(
                "PROCESS-PERF {} sample={} bytes={} ns={} prepare-ns={} source-release-ns={}",
                name,
                sample,
                bytes,
                ns(before, released, hz),
                ns(before, prepared, hz),
                ns(constructed, released, hz)
            );
        }
    }
    nanami_runtime::request_mapping_release(shm, capacity).unwrap();
    nanami_runtime::println!(
        "TINYCORE-PROCESS-PERFORMANCE-PASS {}",
        alloc::string::String::from_utf8_lossy(&output)
    );
}
