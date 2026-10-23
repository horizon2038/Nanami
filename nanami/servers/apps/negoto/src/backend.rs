use alloc::{format, string::String, vec::Vec};
use nanami_client::{registry, uapi::alter::*};
use nanami_runtime::{ipc, Word, OS_RESPONSE_OK};
use negoto_oci::Config;

pub struct Backend {
    port: Word,
    shm: Word,
    bytes: Word,
}
impl Backend {
    pub fn connect() -> Result<Self, String> {
        registry::connect_service("alter-linux", 25).map_err(|e| format!("Alter/Linux: {e}"))?;
        let port = ipc::process_slot_descriptor(25);
        let (status, shm, bytes) = nanami_runtime::call_service_port(
            port,
            ALTER_REQUEST_CONTROL,
            ALTER_CONTROL_ATTACH_SHARED_MEMORY,
            64 * 1024,
            0,
            0,
            3,
        )
        .map_err(|e| format!("Alter memory: {e}"))?;
        if status != OS_RESPONSE_OK {
            return Err(format!("Alter memory: status {status}"));
        }
        Ok(Self { port, shm, bytes })
    }
    pub fn call(&self, code: Word, first: Word, second: Word) -> Result<(Word, Word), String> {
        let (status, value, detail) =
            nanami_runtime::call_service_port(self.port, code, first, second, 0, 0, 3)
                .map_err(|e| format!("Alter transport: {e}"))?;
        if status != OS_RESPONSE_OK {
            return Err(format!("Alter operation {code:#x}: status {status}"));
        }
        Ok((value, detail))
    }
    pub fn prepare(
        &self,
        config: &Config,
        root: &str,
        executable: &str,
        terminal: Word,
    ) -> Result<Word, String> {
        let mut launch = Vec::new();
        launch.extend_from_slice(&(config.args.len() as u64).to_le_bytes());
        launch.extend_from_slice(&(config.env.len() as u64).to_le_bytes());
        for string in config.args.iter().chain(&config.env) {
            launch.extend_from_slice(string.as_bytes());
            launch.push(0);
        }
        let wire = ExecutionRequest {
            root: root.as_bytes(),
            cwd: config.cwd.as_bytes(),
            executable: executable.as_bytes(),
            launch: &launch,
            flags: if config.readonly {
                ALTER_CONTEXT_READONLY
            } else {
                0
            },
        };
        let output = unsafe { core::slice::from_raw_parts_mut(self.shm as *mut u8, self.bytes) };
        let len = wire
            .encode(output)
            .filter(|len| *len <= 32 * 1024)
            .ok_or("execution parameters exceed Alter limits")?;
        let (status, pid, _) = nanami_runtime::call_service_port(
            self.port,
            ALTER_REQUEST_PREPARE_CONTEXT,
            0,
            len,
            terminal,
            0,
            4,
        )
        .map_err(|e| format!("prepare: {e}"))?;
        if status != OS_RESPONSE_OK || pid == 0 {
            return Err(format!("prepare: status {status}"));
        }
        Ok(pid)
    }
}
