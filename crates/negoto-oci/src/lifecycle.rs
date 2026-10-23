use crate::{valid_id, Error, Result, OCI_VERSION};
use alloc::{collections::BTreeMap, string::String};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Creating,
    Created,
    Running,
    Stopped,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Created => "created",
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }
}

pub struct Container {
    pub id: String,
    pub bundle: String,
    pub annotations: BTreeMap<String, String>,
    pub status: Status,
    pub pid: usize,
}

impl Container {
    pub fn new(id: &str, bundle: String, annotations: BTreeMap<String, String>) -> Result<Self> {
        if !valid_id(id) || !bundle.starts_with('/') || bundle.contains('\0') {
            return Err(Error("invalid container ID or bundle pathname".into()));
        }
        Ok(Self {
            id: id.into(),
            bundle,
            annotations,
            status: Status::Creating,
            pid: 0,
        })
    }
    pub fn created(&mut self, pid: usize) -> Result<()> {
        if self.status != Status::Creating || pid == 0 {
            return Err(Error("invalid create transition".into()));
        }
        self.pid = pid;
        self.status = Status::Created;
        Ok(())
    }
    pub fn require_start(&self) -> Result<()> {
        if self.status != Status::Created {
            return Err(Error("start requires a created container".into()));
        }
        Ok(())
    }
    pub fn started(&mut self) -> Result<()> {
        self.require_start()?;
        self.status = Status::Running;
        Ok(())
    }
    pub fn observe_exit(&mut self, exited: bool) {
        if exited && self.pid != 0 {
            self.status = Status::Stopped;
        }
    }
    pub fn require_delete(&self) -> Result<()> {
        if self.status != Status::Stopped {
            return Err(Error("delete requires a stopped container".into()));
        }
        Ok(())
    }
    pub fn state_json(&self) -> Result<String> {
        serde_json::to_string(&serde_json::json!({"ociVersion":OCI_VERSION,"id":self.id,"status":self.status.name(),"pid":self.pid,"bundle":self.bundle,"annotations":self.annotations}))
            .map_err(|e| Error(alloc::format!("state: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_does_not_run_and_invalid_operations_do_not_change_state() {
        let mut container = Container::new("test", "/bundle".into(), BTreeMap::new()).unwrap();
        assert!(container.started().is_err());
        container.created(42).unwrap();
        assert_eq!(container.status, Status::Created);
        assert!(container.require_delete().is_err());
        container.started().unwrap();
        assert!(container.started().is_err());
        assert_eq!(container.status, Status::Running);
        container.observe_exit(true);
        assert!(container.require_delete().is_ok());
        let state: serde_json::Value =
            serde_json::from_str(&container.state_json().unwrap()).unwrap();
        assert_eq!(state["pid"], 42);
        assert_eq!(state["status"], "stopped");
    }
}
