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
                return;
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
nanami_runtime::nanami_entry!(main);
