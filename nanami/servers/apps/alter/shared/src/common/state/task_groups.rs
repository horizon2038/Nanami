use super::*;

impl Runtime {
    pub fn set_umask(&mut self, pid: Word, mask: Word) -> Option<Word> {
        let process = self.managed_process(pid)?;
        let previous = process.umask;
        let group = process.fs_group;
        for process in &mut self.managed {
            if process.pid != 0 && process.fs_group == group {
                process.umask = mask & 0o777;
            }
        }
        Some(previous)
    }

    pub fn thread_group(&self, pid: Word) -> Word {
        self.managed_process(pid)
            .map_or(pid, |process| process.tgid)
    }

    pub fn vm_group(&self, pid: Word) -> Word {
        self.managed_process(pid)
            .map_or(pid, |process| process.vm_group)
    }

    pub fn has_live_threads(&self, pid: Word) -> bool {
        let group = self.thread_group(pid);
        self.managed
            .iter()
            .any(|other| other.pid != pid && other.pid != 0 && !other.exited && other.tgid == group)
    }

    pub fn files_shared_with_live_task(&self, pid: Word) -> bool {
        let Some(process) = self.managed_process(pid) else {
            return false;
        };
        self.managed.iter().any(|other| {
            other.pid != pid
                && other.pid != 0
                && !other.exited
                && other.files.same_storage(process.files)
        })
    }

    pub fn share_task_resources(
        &mut self,
        parent: Word,
        child: Word,
        files: bool,
        fs: bool,
        thread: bool,
    ) -> bool {
        let Some(parent_process) = self.managed_process_mut(parent) else {
            return false;
        };
        if !parent_process.mappings.ensure() {
            return false;
        }
        let source = *parent_process;
        let Some(target) = self.managed_process_mut(child) else {
            return false;
        };
        target.mappings.release();
        target.mappings = source.mappings;
        target.vm_group = source.vm_group;
        if files {
            target.files.release();
            target.files = source.files;
        }
        if fs {
            target.fs_group = source.fs_group;
        }
        if thread {
            target.tgid = source.tgid;
            target.parent_pid = source.parent_pid;
            target.job = source.job;
        }
        target.program_break = source.program_break;
        target.mapped_break = source.mapped_break;
        true
    }

    pub fn sync_program_break(&mut self, pid: Word) {
        let Some(source) = self.managed_process(pid).copied() else {
            return;
        };
        for process in &mut self.managed {
            if process.pid != 0 && process.vm_group == source.vm_group {
                process.program_break = source.program_break;
                process.mapped_break = source.mapped_break;
            }
        }
    }
}
