use crate::output::Output;
use alloc::{format, string::String, vec::Vec};
use nanami_client::{registry, timer, uapi::negoto::*};
use nanami_runtime::{ipc, Word, OS_RESPONSE_OK};
use serde_json::{json, Value};

pub fn run(out: &mut Output) -> Result<(), String> {
    let args = (1..nanami_runtime::process_argc())
        .map(|index| {
            core::str::from_utf8(nanami_runtime::process_arg(index).unwrap_or_default())
                .map(String::from)
                .map_err(|_| String::from("arguments must be UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some(operation) = args.first().map(String::as_str) else {
        return Err(usage().into());
    };
    if matches!(operation, "help" | "--help") {
        out.write(usage().as_bytes());
        return Ok(());
    }
    if operation == "features" {
        out.write(b"{\"ociVersionMin\":\"1.0.0\",\"ociVersionMax\":\"1.3.0\",\"hooks\":[],\"linux\":{\"namespaces\":[],\"capabilities\":[],\"seccomp\":{\"enabled\":false}},\"annotations\":{\"io.negoto.profile\":\"alter-rootfs-v1\"}}\n");
        return Ok(());
    }
    let mut bundle = None;
    let mut positional = Vec::new();
    let mut index = 1;
    while index < args.len() {
        if matches!(args[index].as_str(), "--bundle" | "-b") {
            index += 1;
            bundle = Some(args.get(index).ok_or("--bundle needs a pathname")?.clone());
        } else if args[index].starts_with('-') {
            return Err(format!("unknown option: {}", args[index]));
        } else {
            positional.push(args[index].clone());
        }
        index += 1;
    }
    let id = positional.first().ok_or(usage())?;
    if !negoto_oci::valid_id(id) {
        return Err("invalid container ID".into());
    }
    let positional_max = if operation == "kill" { 2 } else { 1 };
    if positional.len() > positional_max {
        return Err(usage().into());
    }
    let mut client = Client::connect()?;
    let result = (|| {
        let command = |operation: &str| json!({"operation":operation,"id":id});
        let timer_port = if operation == "run" {
            registry::connect_timer_service(26)
                .map_err(|e| format!("run needs timer-service: {e}"))?;
            Some(ipc::process_slot_descriptor(26))
        } else {
            None
        };
        match operation {
            "create" | "run" => {
                let bundle = bundle.as_ref().ok_or("create/run requires --bundle")?;
                client.call(&json!({"operation":"create","id":id,"bundle":bundle,"terminal":out.terminal_id()}))?;
                if operation == "create" {
                    return Ok(());
                }
                client.call(&command("start"))?;
                let port = timer_port.unwrap();
                loop {
                    let state = client.call(&command("state"))?;
                    if state["status"] == "stopped" {
                        let status = state["exitStatus"].as_u64().unwrap_or(1);
                        client.call(&command("delete"))?;
                        if status != 0 {
                            return Err(format!("container exited with status {status}"));
                        }
                        return Ok(());
                    }
                    timer::timer_service_sleep_milliseconds(port, 50)
                        .map_err(|e| format!("wait: {e}"))?;
                }
            }
            "state" => {
                let state = client.call(&command("state"))?;
                out.write(
                    format!(
                        "{}\n",
                        serde_json::to_string(&state).map_err(|e| format!("state JSON: {e}"))?
                    )
                    .as_bytes(),
                );
                Ok(())
            }
            "kill" => {
                let signal = match positional.get(1).map(String::as_str).unwrap_or("TERM") {
                    "KILL" | "SIGKILL" => 9,
                    "TERM" | "SIGTERM" => 15,
                    value => value.parse::<u64>().map_err(|_| "invalid signal")?,
                };
                client.call(&json!({"operation":"kill","id":id,"signal":signal}))?;
                Ok(())
            }
            "start" | "delete" => client.call(&command(operation)).map(|_| ()),
            _ => Err(usage().into()),
        }
    })();
    let detached = client.detach();
    result?;
    detached
}

struct Client {
    port: Word,
    shm: Word,
    bytes: Word,
}
impl Client {
    fn connect() -> Result<Self, String> {
        registry::connect_service("negoto-service", 23)
            .map_err(|e| format!("negoto-service: {e}; start /bin/negoto serve"))?;
        let port = ipc::process_slot_descriptor(23);
        let (status, shm, bytes) =
            nanami_runtime::call_service_port(port, NEGOTO_REQUEST_ATTACH, 0, 0, 0, 0, 1)
                .map_err(|e| format!("attach: {e}"))?;
        if status != OS_RESPONSE_OK || bytes < NEGOTO_SHM_BYTES {
            return Err("cannot attach Negoto management memory".into());
        }
        Ok(Self { port, shm, bytes })
    }
    fn call(&mut self, command: &Value) -> Result<Value, String> {
        let bytes = serde_json::to_vec(command).map_err(|e| format!("command: {e}"))?;
        if bytes.len() > self.bytes || bytes.len() > NEGOTO_COMMAND_MAX_BYTES {
            return Err("command exceeds 1 KiB".into());
        }
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
        .map_err(|e| format!("command IPC: {e}"))?;
        if len == 0 || len > self.bytes {
            return Err(format!("invalid Negoto response: status {status}"));
        }
        let bytes = unsafe { core::slice::from_raw_parts(self.shm as *const u8, len) };
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("response JSON: {e}"))?;
        if status != OS_RESPONSE_OK {
            return Err(value["error"]
                .as_str()
                .unwrap_or("Negoto operation failed")
                .into());
        }
        Ok(value)
    }
    fn detach(&self) -> Result<(), String> {
        let (status, _, _) =
            nanami_runtime::call_service_port(self.port, NEGOTO_REQUEST_DETACH, 0, 0, 0, 0, 1)
                .map_err(|e| format!("detach: {e}"))?;
        if status != OS_RESPONSE_OK {
            return Err("Negoto detach failed".into());
        }
        nanami_runtime::request_mapping_release(self.shm, self.bytes)
            .map_err(|e| format!("release: {e}"))
    }
}

fn usage() -> &'static str {
    "usage: negoto create|run --bundle /path/to/bundle ID\n       negoto start|state|delete ID\n       negoto kill ID [TERM|KILL|0]\n       negoto features\n"
}
