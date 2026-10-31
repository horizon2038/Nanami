#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use nanami_client::exec;
use nanami_client::{registry, terminal, timer, uapi::negoto::*};
use nanami_runtime::{ipc, Word, OS_RESPONSE_OK};
use serde_json::{json, Value};
#[path = "../../../../nanami/servers/apps/negoto/src/files.rs"]
mod files;
mod frame_capture;
mod process_perf;
mod framebuffer {
    pub use crate::frame_capture::Rect;
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("NEGOTO-SMOKE-FAIL {}", info);
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}
#[alloc_error_handler]
fn oom(_: core::alloc::Layout) -> ! {
    panic!("out of memory");
}

struct Client {
    port: Word,
    shm: Word,
    bytes: Word,
}

struct NativeExec {
    port: Word,
    shm: Word,
    bytes: Word,
}
impl NativeExec {
    fn connect() -> Self {
        registry::connect_service("exec-service", 28).unwrap();
        let port = ipc::process_slot_descriptor(28);
        let (shm, bytes) = exec::exec_attach_shared_memory(port, 16 * 1024).unwrap();
        Self { port, shm, bytes }
    }
    fn cli(&self, args: &[&str], timer: Word) {
        let pid = self.spawn_cli(args, None);
        self.wait(pid, timer);
    }
    fn spawn_cli(&self, args: &[&str], terminal_id: Option<Word>) -> Word {
        let path = b"/bin/negoto";
        let mut launch = Vec::new();
        for word in [
            16u64,
            (args.len() + 1) as u64,
            u64::from(terminal_id.is_some()),
        ] {
            launch.extend_from_slice(&word.to_le_bytes());
        }
        for value in core::iter::once("/bin/negoto").chain(args.iter().copied()) {
            launch.extend_from_slice(value.as_bytes());
            launch.push(0);
        }
        if let Some(id) = terminal_id {
            launch.extend_from_slice(format!("NANAMI_TERMINAL_ID={id}").as_bytes());
            launch.push(0);
        }
        assert!(512 + launch.len() <= self.bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(path.as_ptr(), self.shm as *mut u8, path.len());
            core::ptr::copy_nonoverlapping(
                launch.as_ptr(),
                (self.shm + 512) as *mut u8,
                launch.len(),
            );
        }
        exec::exec_spawn_path_arguments(self.port, 0, path.len(), 512, launch.len()).unwrap()
    }
    fn wait(&self, pid: Word, timer: Word) {
        for _ in 0..2000 {
            let (exited, status) = exec::exec_process_status(self.port, pid).unwrap();
            if exited {
                assert_eq!(status, 0, "CLI pid={pid}");
                exec::exec_process_reap(self.port, pid).unwrap();
                return;
            }
            timer::timer_service_sleep_milliseconds(timer, 10).unwrap();
        }
        panic!("CLI did not exit: pid={pid}");
    }
}

struct Console {
    port: Word,
    id: Word,
    shm: Word,
    bytes: Word,
}
impl Console {
    fn connect() -> Self {
        registry::connect_terminal_service(27).unwrap();
        let port = ipc::process_slot_descriptor(27);
        let (shm, bytes) = terminal::terminal_attach_shared_memory(port, 4096).unwrap();
        let id = terminal::terminal_create(port, 80, 24).unwrap();
        Self {
            port,
            id,
            shm,
            bytes,
        }
    }
    fn send(&self, bytes: &[u8]) {
        assert!(bytes.len() <= self.bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.shm as *mut u8, bytes.len());
        }
        assert_eq!(
            terminal::terminal_write_input(self.port, self.id, 0, bytes.len()).unwrap(),
            bytes.len()
        );
    }
    fn expect(&self, needle: &[u8], timer: Word) {
        self.collect_until(needle, timer);
    }
    fn collect_until(&self, needle: &[u8], timer: Word) -> Vec<u8> {
        let mut output = Vec::new();
        for _ in 0..2000 {
            let count = terminal::terminal_read_output(self.port, self.id, 0, self.bytes).unwrap();
            assert!(count <= self.bytes);
            let bytes = unsafe { core::slice::from_raw_parts(self.shm as *const u8, count) };
            output.extend_from_slice(bytes);
            assert!(
                !output
                    .windows(b"job control turned off".len())
                    .any(|window| window == b"job control turned off"),
                "interactive shell disabled job control"
            );
            assert!(
                output.len() <= 64 * 1024,
                "unexpected terminal output volume"
            );
            if output.windows(needle.len()).any(|window| window == needle) {
                return output;
            }
            timer::timer_service_sleep_milliseconds(timer, 10).unwrap();
        }
        panic!(
            "interactive output timed out: {}",
            String::from_utf8_lossy(&output)
        );
    }
}
impl Client {
    fn call(&self, command: Value) -> Result<Value, String> {
        let bytes = serde_json::to_vec(&command).unwrap();
        assert!(bytes.len() <= self.bytes);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.shm as *mut u8, bytes.len());
        }
        let (status, len, _) = nanami_runtime::call_service_port(
            self.port,
            NEGOTO_REQUEST_COMMAND,
            bytes.len(),
            0,
            0,
            0,
            2,
        )
        .map_err(|e| format!("IPC {e}"))?;
        assert!(len <= self.bytes);
        let response = unsafe { core::slice::from_raw_parts(self.shm as *const u8, len) };
        let value: Value = serde_json::from_slice(response).unwrap();
        if status != OS_RESPONSE_OK {
            return Err(format!("{value}"));
        }
        Ok(value)
    }
    fn command(&self, operation: &str, id: &str) -> Result<Value, String> {
        self.call(json!({"operation":operation,"id":id}))
    }
    fn create(&self, id: &str, bundle: &str) -> Result<Value, String> {
        self.call(json!({"operation":"create","id":id,"bundle":format!("/alter/linux/bundles/{bundle}"),"terminal":0}))
    }
    fn stopped(&self, id: &str, port: Word) -> Value {
        for _ in 0..2000 {
            let state = self.command("state", id).unwrap();
            if state["status"] == "stopped" {
                return state;
            }
            timer::timer_service_sleep_milliseconds(port, 10).unwrap();
        }
        panic!("container {id} did not exit");
    }
}

fn main() -> nanami_runtime::NanamiResult {
    ipc::init_ipc_tls()?;
    nanami_runtime::heap::init_heap(1024 * 1024)?;
    registry::connect_service("negoto-service", 23)?;
    registry::connect_timer_service(26)?;
    let timer = ipc::process_slot_descriptor(26);
    let port = ipc::process_slot_descriptor(23);
    let (status, shm, bytes) =
        nanami_runtime::call_service_port(port, NEGOTO_REQUEST_ATTACH, 0, 0, 0, 0, 1)?;
    assert_eq!(status, OS_RESPONSE_OK);
    let client = Client { port, shm, bytes };
    let files = files::Files::connect().unwrap();
    if matches!(
        nanami_runtime::process_arg(1),
        Some(b"--tinycore" | b"--tinycore-desktop" | b"--tinycore-apps" | b"--tinycore-perf" | b"--tinycore-process-perf")
    ) {
        let desktop = nanami_runtime::process_arg(1) == Some(b"--tinycore-desktop");
        let applications = nanami_runtime::process_arg(1) == Some(b"--tinycore-apps");
        if nanami_runtime::process_arg(1) == Some(b"--tinycore-process-perf") {
            process_perf::benchmark(&client, timer);
        } else if applications {
            tinycore_applications(&client, &files, timer);
        } else if nanami_runtime::process_arg(1) == Some(b"--tinycore-perf") {
            frame_capture::benchmark(timer);
            let native = NativeExec::connect();
            let console = Console::connect();
            let pid = native.spawn_cli(
                &[
                    "run",
                    "--bundle",
                    "/alter/linux/nanami-desktop-performance",
                    "tc-performance",
                ],
                Some(console.id),
            );
            let output = console.collect_until(b"DESKTOP-PERFORMANCE-PASS", timer);
            native.wait(pid, timer);
            nanami_runtime::println!(
                "TINYCORE-PERFORMANCE-PASS {}",
                String::from_utf8_lossy(&output)
            );
        } else if desktop {
            tinycore_desktop(&client, &files, timer);
        } else {
            tinycore(&client, &files, timer);
        }
        let (status, _, _) =
            nanami_runtime::call_service_port(port, NEGOTO_REQUEST_DETACH, 0, 0, 0, 0, 1)?;
        assert_eq!(status, OS_RESPONSE_OK);
        nanami_runtime::request_mapping_release(shm, bytes)?;
        if applications {
            nanami_runtime::println!("TINYCORE-APPLICATIONS-PASS windows=true cleanup=true");
        } else if desktop {
            nanami_runtime::println!("TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true");
        } else {
            nanami_runtime::println!("TINYCORE-SMOKE-PASS init=true dynamic=true interactive=true");
        }
        return Ok(());
    }
    let marker = "/alter/linux/bundles/busybox/rootfs/work/result";
    assert!(files.stat(marker).is_err(), "fixture is not fresh");
    assert!(client.command("start", "missing").is_err());
    assert!(client.create("../escape", "busybox").is_err());
    assert!(client.create("unsupported", "unsupported").is_err());
    client.create("box", "busybox").unwrap();
    let state = client.command("state", "box").unwrap();
    assert_eq!(state["status"], "created");
    assert!(state["pid"].as_u64().unwrap() > 0);
    assert_eq!(state["annotations"]["io.negoto.test"], "unmodified-busybox");
    timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    assert!(files.stat(marker).is_err(), "create executed the program");
    assert!(client.create("box", "busybox").is_err());
    assert!(client.command("delete", "box").is_err());
    client.command("start", "box").unwrap();
    assert!(client.command("start", "box").is_err());
    let state = client.stopped("box", timer);
    assert_eq!(state["exitStatus"], 0, "{state}");
    assert_eq!(files.read(marker).unwrap(), b"NEGOTO-BUSYBOX-OK\n");
    client.command("delete", "box").unwrap();
    assert!(client.command("state", "box").is_err());
    assert!(files.stat(marker).is_ok(), "delete removed bundle content");
    nanami_runtime::println!("NEGOTO-LIFECYCLE-PASS create-gated=true argv=true env=true cwd=true fork-exec=true root=true");

    client.create("box", "readonly").unwrap();
    client.command("start", "box").unwrap();
    let state = client.stopped("box", timer);
    assert_eq!(state["exitStatus"], 0, "{state}");
    assert!(files
        .stat("/alter/linux/bundles/busybox/rootfs/work/forbidden")
        .is_err());
    client.command("delete", "box").unwrap();
    nanami_runtime::println!("NEGOTO-READONLY-PASS");

    client.create("box", "hold").unwrap();
    client.command("start", "box").unwrap();
    assert_eq!(client.command("state", "box").unwrap()["status"], "running");
    assert!(client.command("delete", "box").is_err());
    client
        .call(json!({"operation":"kill","id":"box","signal":9}))
        .unwrap();
    assert_eq!(client.stopped("box", timer)["status"], "stopped");
    client.command("delete", "box").unwrap();
    nanami_runtime::println!("NEGOTO-KILL-PASS");

    client.create("signals", "signals").unwrap();
    client.command("start", "signals").unwrap();
    let state = client.stopped("signals", timer);
    assert_eq!(state["exitStatus"], 0, "signal handlers: {state}");
    assert_eq!(
        files
            .read("/alter/linux/bundles/busybox/rootfs/work/signal-result")
            .unwrap(),
        b"signal-ok"
    );
    client.command("delete", "signals").unwrap();
    nanami_runtime::println!("NEGOTO-SIGNALS-PASS handlers=true ignore=true sigreturn=true");
    client.create("signal-abi", "signal-abi").unwrap();
    client.command("start", "signal-abi").unwrap();
    let state = client.stopped("signal-abi", timer);
    assert_eq!(state["exitStatus"], 0, "signal ABI: {state}");
    assert_eq!(
        files
            .read("/alter/linux/bundles/busybox/rootfs/work/signal-abi")
            .unwrap(),
        b"SIGNAL-ABI-PASS\n"
    );
    client.command("delete", "signal-abi").unwrap();
    nanami_runtime::println!("NEGOTO-SIGNAL-ABI-PASS masks=true nested=true altstack=true realtime=true faults=true stop=true wait=true timers=true restart=true");
    nanami_runtime::println!(
        "NEGOTO-ASYNC-SIGNALS-PASS busy-timer=true busy-stop-continue=true gprs=true"
    );

    for _ in 0..20 {
        client.create("box", "true").unwrap();
        client.command("start", "box").unwrap();
        assert_eq!(client.stopped("box", timer)["exitStatus"], 0);
        client.command("delete", "box").unwrap();
    }
    let native = NativeExec::connect();
    let bundle = "/alter/linux/bundles/true";
    native.cli(&["features"], timer);
    native.cli(&["create", "--bundle", bundle, "cli"], timer);
    assert_eq!(client.command("state", "cli").unwrap()["status"], "created");
    native.cli(&["state", "cli"], timer);
    native.cli(&["start", "cli"], timer);
    assert_eq!(client.stopped("cli", timer)["exitStatus"], 0);
    native.cli(&["delete", "cli"], timer);
    for _ in 0..18 {
        native.cli(&["run", "--bundle", bundle, "cli"], timer);
        assert!(client.command("state", "cli").is_err());
    }
    nanami_runtime::println!("NEGOTO-CLI-PASS lifecycle=true run=true detach-reuse=18");
    let console = Console::connect();
    let pid = native.spawn_cli(
        &["run", "--bundle", "/alter/linux/bundles/interactive", "tty"],
        Some(console.id),
    );
    console.expect(b"# ", timer);
    // The expected marker is not contiguous in the input, avoiding echo-only passes.
    console.send(b"printf 'nego''to-interactive-ok\\n'\n");
    console.expect(b"negoto-interactive-ok", timer);
    console.send(b"/bin/busybox true || exit 71; printf 'nego''to-exec-ok\\n'\n");
    console.expect(b"negoto-exec-ok", timer);
    console.send(b"/bin/busybox echo pipe-ok | /bin/busybox cat || exit 72\n");
    console.expect(b"pipe-ok\r\n", timer);
    console.send(b"/bin/busybox sleep 0.05 & fg || exit 73; printf 'nego''to-fg-ok\\n'\n");
    console.expect(b"negoto-fg-ok", timer);
    console.send(b"/bin/busybox sleep 100\n");
    timer::timer_service_sleep_milliseconds(timer, 300).unwrap();
    console.send(b"\x03");
    console.send(b"printf 'nego''to-ctrl-c-ok\\n'\n");
    console.expect(b"negoto-ctrl-c-ok", timer);
    nanami_runtime::println!("NEGOTO-CTRL-C-PASS foreground=true shell-survives=true");
    console.send(b"exit\n");
    native.wait(pid, timer);
    assert!(client.command("state", "tty").is_err());
    nanami_runtime::println!(
        "NEGOTO-INTERACTIVE-PASS prompt=true input=true job-control=true fork-exec=true pipeline=true exit=true cleanup=true"
    );
    let (status, _, _) =
        nanami_runtime::call_service_port(port, NEGOTO_REQUEST_DETACH, 0, 0, 0, 0, 1)?;
    assert_eq!(status, OS_RESPONSE_OK);
    nanami_runtime::request_mapping_release(shm, bytes)?;
    nanami_runtime::println!("NEGOTO-SMOKE-PASS reuse=20 cleanup=true");
    Ok(())
}

fn tinycore_applications(client: &Client, files: &files::Files, timer: Word) {
    let native = NativeExec::connect();
    let console = Console::connect();
    let root = "/alter/linux/tinycore/rootfs/tmp/";
    for name in [
        "desktop-ready",
        "desktop-apps-ok",
        "desktop-apps-step",
        "desktop-apps-failed",
        "pointer-probe.log",
        "desktop-exit",
    ] {
        let path = format!("{root}{name}");
        if files.stat(&path).is_ok() {
            files.remove(&path).unwrap();
        }
    }
    let pid = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/tinycore-desktop",
            "tc-applications",
        ],
        Some(console.id),
    );
    let mut ready = false;
    for _ in 0..1200 {
        if files.stat(&format!("{root}desktop-ready")).is_ok() {
            ready = true;
            break;
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    if !ready {
        let count =
            terminal::terminal_read_output(console.port, console.id, 0, console.bytes).unwrap();
        let bytes = unsafe { core::slice::from_raw_parts(console.shm as *const u8, count) };
        nanami_runtime::println!(
            "[tinycore.apps] startup: {}",
            String::from_utf8_lossy(bytes)
        );
        panic!("application-test desktop did not start");
    }
    nanami_runtime::println!("TINYCORE-APPLICATIONS-READY");
    let mut previous = Vec::new();
    let mut previous_pointer = Vec::new();
    let mut complete = false;
    for turn in 0..6000 {
        if let Ok(pointer) = files.read(&format!("{root}pointer-probe.log")) {
            if pointer != previous_pointer {
                if let Some(line) = pointer
                    .split(|byte| *byte == b'\n')
                    .rev()
                    .find(|line| !line.is_empty())
                {
                    nanami_runtime::println!("{}", String::from_utf8_lossy(line));
                }
                previous_pointer = pointer;
            }
        }
        if let Ok(application) = files.read(&format!("{root}desktop-apps-failed")) {
            let name = String::from_utf8_lossy(&application);
            for suffix in ["log", "tree"] {
                if let Ok(bytes) =
                    files.read(&format!("{root}desktop-apps/{}.{}", name.trim(), suffix))
                {
                    nanami_runtime::println!(
                        "[tinycore.apps] {}.{}: {}",
                        name.trim(),
                        suffix,
                        String::from_utf8_lossy(&bytes)
                    );
                }
            }
            panic!("GUI application failed: {}", name.trim());
        }
        if let Ok(step) = files.read(&format!("{root}desktop-apps-step")) {
            if step != previous {
                nanami_runtime::println!(
                    "TINYCORE-APP-WINDOW {}",
                    String::from_utf8_lossy(&step).trim()
                );
                previous = step;
            }
        }
        if files
            .read(&format!("{root}desktop-apps-ok"))
            .is_ok_and(|bytes| bytes == b"desktop-apps-ok\n")
        {
            complete = true;
            break;
        }
        if turn % 100 == 99 {
            if let Ok(current) = files.read(&format!("{root}desktop-apps-current")) {
                let name = String::from_utf8_lossy(&current);
                if let Ok(bytes) = files.read(&format!("{root}desktop-apps/{}.log", name.trim())) {
                    if !bytes.is_empty() {
                        nanami_runtime::println!(
                            "[tinycore.apps] {}: {}",
                            name.trim(),
                            String::from_utf8_lossy(&bytes)
                        );
                    }
                }
            }
            let count =
                terminal::terminal_read_output(console.port, console.id, 0, console.bytes).unwrap();
            if count != 0 {
                let bytes = unsafe { core::slice::from_raw_parts(console.shm as *const u8, count) };
                nanami_runtime::println!("[tinycore.apps] {}", String::from_utf8_lossy(bytes));
            }
        }
        // Do not let this higher-priority observer saturate VFS/Alpha while
        // the lower-priority GUI clients are loading fonts and libraries.
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(complete, "GUI application smoke test did not complete");
    nanami_runtime::println!("TINYCORE-APPLICATIONS-VERIFIED");
    let mut exited = false;
    for _ in 0..1200 {
        if files.stat(&format!("{root}desktop-exit")).is_ok() {
            exited = true;
            break;
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(exited, "application-test desktop did not exit");
    native.wait(pid, timer);
    assert!(client.command("state", "tc-applications").is_err());
}

fn tinycore_desktop(client: &Client, files: &files::Files, timer: Word) {
    let native = NativeExec::connect();
    let console = Console::connect();
    let syscalls = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/nanami-desktop-syscalls",
            "tc-syscalls",
        ],
        Some(console.id),
    );
    console.expect(b"DESKTOP-SYSCALLS-PASS", timer);
    native.wait(syscalls, timer);
    assert!(client.command("state", "tc-syscalls").is_err());
    nanami_runtime::println!("TINYCORE-DESKTOP-SYSCALLS-PASS");
    let namespace = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/nanami-namespace",
            "tc-namespace",
        ],
        Some(console.id),
    );
    console.expect(b"NAMESPACE-PASS", timer);
    native.wait(namespace, timer);
    assert!(client.command("state", "tc-namespace").is_err());
    nanami_runtime::println!("TINYCORE-NAMESPACE-PASS");
    let mapping = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/nanami-file-mapping",
            "tc-mapping",
        ],
        Some(console.id),
    );
    console.expect(b"FILE-MAPPING-PASS", timer);
    native.wait(mapping, timer);
    assert!(client.command("state", "tc-mapping").is_err());
    nanami_runtime::println!("TINYCORE-FILE-MAPPING-PASS");
    for cycle in 1..=2 {
        tinycore_desktop_session(client, files, timer, &native, &console, cycle);
    }
}

fn tinycore_desktop_session(
    client: &Client,
    files: &files::Files,
    timer: Word,
    native: &NativeExec,
    console: &Console,
    cycle: usize,
) {
    let root = "/alter/linux/tinycore/rootfs/tmp/";
    // Never accept the preceding session's markers as evidence of a restart.
    for name in [
        "desktop-ready",
        "desktop-input-ok",
        "desktop-launch-ok",
        "desktop-info-ok",
        "desktop-exit",
    ] {
        let path = format!("{root}{name}");
        if files.stat(&path).is_ok() {
            files.remove(&path).unwrap();
        }
    }
    let pid = native.spawn_cli(
        &[
            "run",
            "--bundle",
            "/alter/linux/tinycore-desktop",
            "tc-desktop",
        ],
        Some(console.id),
    );
    let mut ready = false;
    // Keep the higher-priority observer from starving the GUI's cold-start
    // work with continual VFS/exec RPCs. All markers still require readback.
    for turn in 0..1200 {
        if files.stat(&format!("{root}desktop-ready")).is_ok() {
            ready = true;
            break;
        }
        if turn % 100 == 99 {
            let count =
                terminal::terminal_read_output(console.port, console.id, 0, console.bytes).unwrap();
            if count != 0 {
                let bytes = unsafe { core::slice::from_raw_parts(console.shm as *const u8, count) };
                nanami_runtime::println!(
                    "[tinycore.gui] console: {}",
                    String::from_utf8_lossy(bytes)
                );
            }
            for name in [
                "Xfbdev.log",
                "waitforX.log",
                "flwm.log",
                "wbar.log",
                "aterm.log",
            ] {
                if let Ok(bytes) = files.read(&format!("{root}{name}")) {
                    if !bytes.is_empty() {
                        nanami_runtime::println!(
                            "[tinycore.gui] {}: {}",
                            name,
                            String::from_utf8_lossy(&bytes)
                        );
                    }
                }
            }
            let (exited, status) = exec::exec_process_status(native.port, pid).unwrap();
            assert!(
                !exited,
                "desktop CLI exited before startup: status={status}"
            );
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(ready, "X server did not accept a real X11 client");
    nanami_runtime::println!("TINYCORE-DESKTOP-READY cycle={}", cycle);
    let mut input = false;
    for _ in 0..1200 {
        if files
            .read(&format!("{root}desktop-input-ok"))
            .is_ok_and(|bytes| bytes == b"desktop-input-ok\n")
        {
            input = true;
            break;
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(input, "X terminal did not execute the keyboard command");
    nanami_runtime::println!("TINYCORE-DESKTOP-INPUT-PASS cycle={}", cycle);
    let mut launched = false;
    for _ in 0..1200 {
        let (exited, _) = exec::exec_process_status(native.port, pid).unwrap();
        assert!(!exited, "closing the initial terminal ended the desktop");
        if files
            .read(&format!("{root}desktop-launch-ok"))
            .is_ok_and(|bytes| bytes == b"desktop-launch-ok\n")
        {
            launched = true;
            break;
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(
        launched,
        "dock Terminal action did not execute a new X client"
    );
    nanami_runtime::println!("TINYCORE-DESKTOP-LAUNCH-PASS cycle={}", cycle);
    let mut information = false;
    for _ in 0..1200 {
        if files
            .read(&format!("{root}desktop-info-ok"))
            .is_ok_and(|bytes| bytes == b"desktop-info-ok\n")
        {
            information = true;
            break;
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(
        information,
        "information launcher did not execute its X terminal command"
    );
    nanami_runtime::println!("TINYCORE-DESKTOP-INFO-PASS cycle={}", cycle);
    let mut exit_requested = false;
    for turn in 0..1200 {
        if files.stat(&format!("{root}desktop-exit")).is_ok() {
            exit_requested = true;
            break;
        }
        if turn % 100 == 99 {
            for name in ["wbar.log", "editor.log"] {
                if let Ok(bytes) = files.read(&format!("{root}{name}")) {
                    if !bytes.is_empty() {
                        nanami_runtime::println!(
                            "[tinycore.gui] {}: {}",
                            name,
                            String::from_utf8_lossy(&bytes)
                        );
                    }
                }
            }
        }
        timer::timer_service_sleep_milliseconds(timer, 100).unwrap();
    }
    assert!(
        exit_requested,
        "dock Exit action did not request session shutdown"
    );
    native.wait(pid, timer);
    assert!(client.command("state", "tc-desktop").is_err());
    assert!(
        files
            .read(&format!("{root}desktop-info-ok"))
            .is_ok_and(|bytes| bytes == b"desktop-info-ok\n"),
        "information launcher did not execute its X terminal command"
    );
    nanami_runtime::println!("TINYCORE-DESKTOP-SESSION-PASS cycle={}", cycle);
}

fn tinycore(client: &Client, files: &files::Files, timer: Word) {
    let native = NativeExec::connect();
    let console = Console::connect();
    let config = "/alter/linux/tinycore-smoke/config.json";
    let original = files.read(config).unwrap();
    assert_eq!(original.first(), Some(&b'{'));
    for (bundle, marker) in [
        ("/alter/linux/nanami-chmod", b"CHMOD-PASS".as_slice()),
        ("/alter/linux/nanami-chmod-readonly", b"CHMOD-READONLY-PASS"),
        ("/alter/linux/nanami-procfs", b"PROCFS-PASS"),
        (
            "/alter/linux/nanami-desktop-syscalls",
            b"DESKTOP-SYSCALLS-PASS",
        ),
    ] {
        let pid = native.spawn_cli(
            &["run", "--bundle", bundle, "tc-permissions"],
            Some(console.id),
        );
        console.expect(marker, timer);
        native.wait(pid, timer);
        assert!(client.command("state", "tc-permissions").is_err());
        assert_eq!(
            files.read(config).unwrap(),
            original,
            "chmod fixture changed an unrelated bundle"
        );
    }
    nanami_runtime::println!(
        "TINYCORE-CHMOD-PASS mode=true aliases=true unlinked=true readonly=true"
    );
    let marker = "/alter/linux/tinycore/rootfs/tmp/tinycore-init-ok";
    assert!(files.stat(marker).is_err(), "fixture is not fresh");
    let pid = native.spawn_cli(
        &["run", "--bundle", "/alter/linux/tinycore-smoke", "tc-smoke"],
        Some(console.id),
    );
    console.expect(b"tinycore-init-ok", timer);
    native.wait(pid, timer);
    assert_eq!(files.read(marker).unwrap(), b"tinycore-init-ok\n");
    assert!(client.command("state", "tc-smoke").is_err());
    nanami_runtime::println!("TINYCORE-INIT-PASS root=true symlinks=true pipe=true fork=true");

    let pid = native.spawn_cli(
        &["run", "--bundle", "/alter/linux/tinycore", "tc-console"],
        Some(console.id),
    );
    console.expect(b"tinycore:/# ", timer);
    console.send(b"ls && ls . && printf 'tinycore-''ls-cwd-ok\\n'\n");
    console.expect(b"tinycore-ls-cwd-ok", timer);
    console.send(
        b"ls / && ls /bin && cd /tmp && ls . && cd / && printf 'tinycore-''ls-paths-ok\\n'\n",
    );
    console.expect(b"tinycore-ls-paths-ok", timer);
    console.send(b"sleep 100 & p=$!; ps > /tmp/procfs-ps; ps -T > /tmp/procfs-threads 2>/tmp/procfs-thread-error; printf '%s\\n' $? > /tmp/procfs-thread-status; cat /tmp/procfs-ps; grep 'sleep 100' /tmp/procfs-ps && kill $p && wait $p; printf 'tinycore-''ps-ok\\n'\n");
    console.expect(b"tinycore-ps-ok", timer);
    let listing = files
        .read("/alter/linux/tinycore/rootfs/tmp/procfs-ps")
        .unwrap();
    assert!(
        listing.windows(9).any(|bytes| bytes == b"sleep 100"),
        "BusyBox ps omitted its sleeping child"
    );
    assert!(
        listing.windows(3).any(|bytes| bytes == b"PID"),
        "BusyBox ps did not print a table"
    );
    let threads = files
        .read("/alter/linux/tinycore/rootfs/tmp/procfs-threads")
        .unwrap();
    let status = files
        .read("/alter/linux/tinycore/rootfs/tmp/procfs-thread-status")
        .unwrap();
    if status == b"0\n" {
        assert!(
            threads.windows(9).any(|bytes| bytes == b"sleep 100"),
            "BusyBox ps -T omitted the child"
        );
    } else {
        let error = files
            .read("/alter/linux/tinycore/rootfs/tmp/procfs-thread-error")
            .unwrap();
        let error = core::str::from_utf8(&error).unwrap();
        nanami_runtime::println!("TINYCORE-PS-T-OPTION: {}", error);
        assert!(
            error.contains("invalid option") || error.contains("unrecognized option"),
            "ps -T failed for a reason other than a disabled BusyBox option"
        );
    }
    // Emit the independently read file, not terminal input echo.
    nanami_runtime::println!(
        "TINYCORE-PS-OUTPUT\n{}",
        core::str::from_utf8(&listing).unwrap()
    );
    nanami_runtime::println!(
        "TINYCORE-PS-PASS numeric-pids=true args=true openat=true fork=true reap=true"
    );
    // Split expected strings so terminal input echo cannot satisfy the check.
    console.send(b"printf 'tinycore-''interactive-ok\\n'\n");
    console.expect(b"tinycore-interactive-ok", timer);
    console.send(b"cat /etc/os-release\n");
    console.expect(b"VERSION_ID=17.1", timer);
    console.send(b"/bin/busybox sleep 0.05 & fg; printf 'tinycore-''fg-ok\\n'\n");
    console.expect(b"tinycore-fg-ok", timer);
    console.send(b"/bin/busybox sleep 100\n");
    timer::timer_service_sleep_milliseconds(timer, 300).unwrap();
    console.send(b"\x03");
    console.send(b"printf 'tinycore-''ctrl-c-ok\\n'\n");
    console.expect(b"tinycore-ctrl-c-ok", timer);
    console.send(b"exit\n");
    native.wait(pid, timer);
    assert!(client.command("state", "tc-console").is_err());
    nanami_runtime::println!(
        "TINYCORE-CONSOLE-PASS release=true job-control=true ctrl-c=true exit=true"
    );
}
nanami_runtime::nanami_entry!(main);
