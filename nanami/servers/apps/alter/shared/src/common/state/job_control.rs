//! Session, process-group and controlling-terminal state owned by Alter.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobControlError {
    NoProcess,
    Permission,
    InvalidArgument,
    ExecutedChild,
    NoTerminal,
    NoSpace,
}

#[derive(Clone, Copy, Debug)]
pub struct ProcessJobControl {
    pub session: Word,
    pub group: Word,
    pub controlling_terminal: Word,
    pub executed: bool,
}
impl ProcessJobControl {
    pub const EMPTY: Self = Self {
        session: 0,
        group: 0,
        controlling_terminal: 0,
        executed: false,
    };
}

#[derive(Clone, Copy)]
pub struct TerminalControl {
    pub id: Word,
    pub session: Word,
    pub foreground: Word,
}
impl TerminalControl {
    pub const EMPTY: Self = Self {
        id: 0,
        session: 0,
        foreground: 0,
    };
}

impl Runtime {
    pub fn initialize_job_control(&mut self, pid: Word, parent_pid: Word) {
        let mut job = self
            .managed_process(parent_pid)
            .map(|parent| parent.job)
            .unwrap_or(ProcessJobControl {
                session: pid,
                group: pid,
                ..ProcessJobControl::EMPTY
            });
        job.executed = false;
        let terminal_id = self.managed_process(pid).unwrap().terminal_id;
        self.managed_process_mut(pid).unwrap().job = job;
        if parent_pid == 0 && terminal_id != 0 {
            // Stdio may refer to a terminal already controlled by another
            // session. Such a descriptor does not confer controlling authority.
            let _ = self.claim_controlling_terminal(pid, terminal_id);
        }
    }

    pub fn visible_job_process(
        &self,
        caller: Word,
        target: Word,
    ) -> Result<&ManagedProcess, JobControlError> {
        let current = self
            .managed_process(caller)
            .ok_or(JobControlError::NoProcess)?;
        let process = self
            .managed_process(target)
            .ok_or(JobControlError::NoProcess)?;
        if current.rootfs.leader != process.rootfs.leader {
            return Err(JobControlError::NoProcess);
        }
        Ok(process)
    }

    pub fn process_group(&self, caller: Word, target: Word) -> Result<Word, JobControlError> {
        let target = if target == 0 { caller } else { target };
        Ok(self.visible_job_process(caller, target)?.job.group)
    }

    pub fn process_session(&self, caller: Word, target: Word) -> Result<Word, JobControlError> {
        let target = if target == 0 { caller } else { target };
        Ok(self.visible_job_process(caller, target)?.job.session)
    }

    pub fn set_process_group(
        &mut self,
        caller: Word,
        target: Word,
        group: Word,
    ) -> Result<(), JobControlError> {
        if group > i32::MAX as Word {
            return Err(JobControlError::InvalidArgument);
        }
        if target > i32::MAX as Word {
            return Err(JobControlError::NoProcess);
        }
        let target = if target == 0 { caller } else { target };
        let group = if group == 0 { target } else { group };
        let current = self
            .managed_process(caller)
            .ok_or(JobControlError::NoProcess)?;
        let process = self.visible_job_process(caller, target)?;
        if target != caller && process.parent_pid != caller {
            return Err(JobControlError::NoProcess);
        }
        if target != caller && process.job.executed {
            return Err(JobControlError::ExecutedChild);
        }
        if process.job.session != current.job.session || process.job.session == target {
            return Err(JobControlError::Permission);
        }
        let session = process.job.session;
        if group != target
            && !self.managed.iter().any(|candidate| {
                candidate.pid != 0
                    && candidate.job.session == session
                    && candidate.job.group == group
            })
        {
            return Err(JobControlError::Permission);
        }
        self.managed_process_mut(target).unwrap().job.group = group;
        Ok(())
    }

    pub fn create_session(&mut self, pid: Word) -> Result<Word, JobControlError> {
        self.managed_process(pid)
            .ok_or(JobControlError::NoProcess)?;
        if self
            .managed
            .iter()
            .any(|process| process.pid != 0 && process.job.group == pid)
        {
            return Err(JobControlError::Permission);
        }
        let job = &mut self.managed_process_mut(pid).unwrap().job;
        job.session = pid;
        job.group = pid;
        job.controlling_terminal = 0;
        Ok(pid)
    }

    pub fn claim_controlling_terminal(
        &mut self,
        pid: Word,
        id: Word,
    ) -> Result<(), JobControlError> {
        let process = self
            .managed_process(pid)
            .ok_or(JobControlError::NoProcess)?;
        if process.job.session == pid
            && process.job.controlling_terminal == id
            && self.controlling_terminal(pid, id).is_ok()
        {
            return Ok(());
        }
        if id == 0 || process.job.session != pid || process.job.controlling_terminal != 0 {
            return Err(JobControlError::Permission);
        }
        if self
            .terminal_controls
            .iter()
            .any(|terminal| terminal.id == id)
        {
            return Err(JobControlError::Permission);
        }
        let group = process.job.group;
        let slot = self
            .terminal_controls
            .iter_mut()
            .find(|terminal| terminal.id == 0)
            .ok_or(JobControlError::NoSpace)?;
        *slot = TerminalControl {
            id,
            session: pid,
            foreground: group,
        };
        for process in &mut self.managed {
            if process.pid != 0 && process.job.session == pid {
                process.job.controlling_terminal = id;
            }
        }
        Ok(())
    }

    pub fn controlling_terminal(
        &self,
        pid: Word,
        id: Word,
    ) -> Result<&TerminalControl, JobControlError> {
        let process = self
            .managed_process(pid)
            .ok_or(JobControlError::NoProcess)?;
        if id == 0 || process.job.controlling_terminal != id {
            return Err(JobControlError::NoTerminal);
        }
        self.terminal_controls
            .iter()
            .find(|terminal| terminal.id == id && terminal.session == process.job.session)
            .ok_or(JobControlError::NoTerminal)
    }

    pub fn set_terminal_foreground(
        &mut self,
        pid: Word,
        id: Word,
        group: Word,
    ) -> Result<(), JobControlError> {
        if group > i32::MAX as Word {
            return Err(JobControlError::InvalidArgument);
        }
        let session = self.controlling_terminal(pid, id)?.session;
        if !self.managed.iter().any(|process| {
            process.pid != 0 && process.job.session == session && process.job.group == group
        }) {
            return Err(JobControlError::Permission);
        }
        self.terminal_controls
            .iter_mut()
            .find(|terminal| terminal.id == id)
            .unwrap()
            .foreground = group;
        Ok(())
    }

    pub fn detach_controlling_terminal(
        &mut self,
        pid: Word,
        id: Word,
    ) -> Result<(), JobControlError> {
        let session = self.controlling_terminal(pid, id)?.session;
        if session == pid {
            self.release_session_terminal(session);
        } else {
            self.managed_process_mut(pid)
                .unwrap()
                .job
                .controlling_terminal = 0;
        }
        Ok(())
    }

    pub fn release_session_terminal(&mut self, session: Word) {
        for terminal in &mut self.terminal_controls {
            if terminal.session == session {
                *terminal = TerminalControl::EMPTY;
            }
        }
        for process in &mut self.managed {
            if process.pid != 0 && process.job.session == session {
                process.job.controlling_terminal = 0;
            }
        }
    }
}
