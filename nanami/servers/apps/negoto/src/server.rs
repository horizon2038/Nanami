use crate::{backend::Backend, files::Files};
use alloc::{format, string::String, vec::Vec};
use nanami_client::{
    registry,
    uapi::{alter::*, negoto::*},
    vfs,
};
use nanami_runtime::{
    ipc::{self, ServiceRequest},
    Word, OS_RESPONSE_OK,
};
use negoto_oci::{guest_path, host_path, valid_id, Config, Container, Status};
use serde_json::{json, Value};

struct Client {
    pid: Word,
    address: Word,
    bytes: Word,
}
struct Managed {
    container: Container,
    exit_status: Word,
}
struct Server {
    clients: Vec<Client>,
    containers: Vec<Managed>,
    files: Option<Files>,
    backend: Option<Backend>,
}

pub fn run() -> nanami_runtime::NanamiResult {
    registry::register_service("negoto-service")?;
    let port = ipc::process_slot_descriptor(20);
    let mut server = Server {
        clients: Vec::new(),
        containers: Vec::new(),
        files: None,
        backend: None,
    };
    nanami_runtime::println!("[negoto] online");
    let mut request = ipc::service_receive(port)?;
    loop {
        let reply = server.request(request);
        request = ipc::service_reply_receive(port, reply.0, reply.1, reply.2)?;
    }
}

impl Server {
    fn request(&mut self, request: ServiceRequest) -> (Word, Word, Word) {
        if request.code == NEGOTO_REQUEST_ATTACH {
            if self.clients.len() == NEGOTO_CLIENT_MAX {
                let mut index = 0;
                while index < self.clients.len() {
                    let client = &self.clients[index];
                    if nanami_runtime::request_process_alive(client.pid) == Ok(false)
                        && nanami_runtime::request_mapping_release(client.address, client.bytes)
                            .is_ok()
                    {
                        self.clients.swap_remove(index);
                    } else {
                        index += 1;
                    }
                }
            }
            if self
                .clients
                .iter()
                .any(|client| client.pid == request.identifier)
                || self.clients.len() == NEGOTO_CLIENT_MAX
            {
                return (nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION, 0, 0);
            }
            return match nanami_runtime::request_shared_memory(request.identifier, NEGOTO_SHM_BYTES)
            {
                Ok((local, peer)) => {
                    self.clients.push(Client {
                        pid: request.identifier,
                        address: local,
                        bytes: NEGOTO_SHM_BYTES,
                    });
                    (OS_RESPONSE_OK, peer, NEGOTO_SHM_BYTES)
                }
                Err(_) => (nanami_runtime::OS_RESPONSE_FATAL, 0, 0),
            };
        }
        let Some(index) = self
            .clients
            .iter()
            .position(|client| client.pid == request.identifier)
        else {
            return (nanami_runtime::OS_RESPONSE_INVALID_DESCRIPTOR, 0, 0);
        };
        let client = &self.clients[index];
        if request.code == NEGOTO_REQUEST_DETACH {
            if nanami_runtime::request_mapping_release(client.address, client.bytes).is_err() {
                return (nanami_runtime::OS_RESPONSE_FATAL, 0, 0);
            }
            self.clients.swap_remove(index);
            return (OS_RESPONSE_OK, 0, 0);
        }
        if request.code != NEGOTO_REQUEST_COMMAND
            || request.arg0 == 0
            || request.arg0 > client.bytes
            || request.arg0 > NEGOTO_COMMAND_MAX_BYTES
        {
            return (nanami_runtime::OS_RESPONSE_INVALID_ARGUMENT, 0, 0);
        }
        // Do not parse directly from memory another core can still modify.
        let mut snapshot = Vec::with_capacity(request.arg0);
        for offset in 0..request.arg0 {
            snapshot
                .push(unsafe { core::ptr::read_volatile((client.address + offset) as *const u8) });
        }
        let command =
            serde_json::from_slice::<Value>(&snapshot).map_err(|e| format!("command JSON: {e}"));
        let result = command.and_then(|command| self.command(command));
        let (status, response) = match result {
            Ok(value) => (OS_RESPONSE_OK, value),
            Err(message) => (
                nanami_runtime::OS_RESPONSE_ILLEGAL_OPERATION,
                json!({"error":message}),
            ),
        };
        let response = serde_json::to_vec(&response).unwrap_or_default();
        let client = &self.clients[index];
        if response.len() > client.bytes {
            return (nanami_runtime::OS_RESPONSE_FATAL, 0, 0);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                response.as_ptr(),
                client.address as *mut u8,
                response.len(),
            );
        }
        (status, response.len(), 0)
    }

    fn command(&mut self, command: Value) -> Result<Value, String> {
        let operation = text(&command, "operation")?;
        let id = text(&command, "id")?;
        if !valid_id(id) {
            return Err("invalid container ID".into());
        }
        if operation == "create" {
            return self.create(id, &command);
        }
        let index = self
            .containers
            .iter()
            .position(|entry| entry.container.id == id)
            .ok_or("container does not exist")?;
        let backend = self
            .backend
            .as_ref()
            .ok_or("Alter/Linux connection is unavailable")?;
        let managed = &mut self.containers[index];
        let pid = managed.container.pid;
        let (exited, exit_status) = backend.call(ALTER_REQUEST_STATUS, pid, 0)?;
        managed.container.observe_exit(exited != 0);
        managed.exit_status = exit_status;
        match operation {
            "state" => {
                let mut state: Value =
                    serde_json::from_str(&managed.container.state_json().map_err(|e| e.0)?)
                        .map_err(|e| format!("state: {e}"))?;
                if managed.container.status == Status::Stopped {
                    state["exitStatus"] = json!(exit_status);
                }
                Ok(state)
            }
            "start" => {
                managed.container.require_start().map_err(|e| e.0)?;
                backend.call(ALTER_REQUEST_START_CONTEXT, pid, 0)?;
                managed.container.started().map_err(|e| e.0)?;
                Ok(json!({}))
            }
            "kill" => {
                if managed.container.status == Status::Stopped {
                    return Err("container is stopped".into());
                }
                let signal = command
                    .get("signal")
                    .and_then(Value::as_u64)
                    .ok_or("invalid signal")?;
                if !matches!(signal, 0 | 9 | 15) {
                    return Err("only signal 0, SIGKILL and SIGTERM are supported".into());
                }
                backend.call(ALTER_REQUEST_SIGNAL_CONTEXT, pid, signal as Word)?;
                Ok(json!({}))
            }
            "delete" => {
                managed.container.require_delete().map_err(|e| e.0)?;
                backend.call(ALTER_REQUEST_DELETE_CONTEXT, pid, 0)?;
                self.containers.swap_remove(index);
                Ok(json!({}))
            }
            _ => Err("unknown operation".into()),
        }
    }

    fn create(&mut self, id: &str, command: &Value) -> Result<Value, String> {
        if self.containers.iter().any(|entry| entry.container.id == id) {
            return Err("container ID already exists".into());
        }
        if self.containers.len() == NEGOTO_CONTAINER_MAX {
            return Err("container limit reached".into());
        }
        let bundle = text(command, "bundle")?;
        if !bundle.starts_with('/') {
            return Err("bundle pathname must be absolute".into());
        }
        let bundle = guest_path("/", bundle).map_err(|e| e.0)?;
        if self.files.is_none() {
            self.files = Some(Files::connect()?);
        }
        let files = self.files.as_ref().unwrap();
        let config =
            Config::parse(&files.read(&format!("{bundle}/config.json"))?).map_err(|e| e.0)?;
        let root = guest_path(&bundle, &config.root).map_err(|e| e.0)?;
        if root == "/" || files.stat(&root)?.2 != vfs::VFS_FILE_TYPE_DIRECTORY {
            return Err("root.path must name a non-native-root directory".into());
        }
        let cwd = host_path(&root, &config.cwd).map_err(|e| e.0)?;
        if files.stat(&cwd)?.2 != vfs::VFS_FILE_TYPE_DIRECTORY {
            return Err("process.cwd is not a directory".into());
        }
        let executable = executable(files, &root, &config)?;
        let terminal = command.get("terminal").and_then(Value::as_u64).unwrap_or(0) as Word;
        let mut container =
            Container::new(id, bundle, config.annotations.clone()).map_err(|e| e.0)?;
        if self.backend.is_none() {
            self.backend = Some(Backend::connect()?);
        }
        let pid = self
            .backend
            .as_ref()
            .unwrap()
            .prepare(&config, &root, &executable, terminal)?;
        container.created(pid).map_err(|e| e.0)?;
        self.containers.push(Managed {
            container,
            exit_status: 0,
        });
        Ok(json!({}))
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing/invalid {field}"))
}

fn executable(files: &Files, root: &str, config: &Config) -> Result<String, String> {
    let first = &config.args[0];
    let paths = if first.contains('/') {
        alloc::vec![guest_path(&config.cwd, first).map_err(|e| e.0)?]
    } else {
        let path = config
            .env
            .iter()
            .find_map(|entry| entry.strip_prefix("PATH="))
            .unwrap_or("/bin:/usr/bin");
        path.split(':')
            .map(|directory| {
                let candidate = if directory.is_empty() {
                    first.clone()
                } else {
                    format!("{directory}/{first}")
                };
                guest_path(&config.cwd, &candidate).map_err(|e| e.0)
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    for guest in paths {
        let host = host_path(root, &guest).map_err(|e| e.0)?;
        if files
            .stat(&host)
            .is_ok_and(|stat| stat.2 == vfs::VFS_FILE_TYPE_REGULAR)
        {
            return Ok(guest);
        }
    }
    Err("process.args[0] was not found inside rootfs".into())
}
