use super::*;

impl Runtime {
    pub fn linux_file(&self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if fd as usize >= LINUX_FD_MAX {
            return None;
        }
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = &self.managed[i];
            if process.pid == pid && process.pcb != 0 {
                let file = process.files[fd as usize];
                return if file.is_open() { Some(file) } else { None };
            }
            i += 1;
        }
        None
    }

    pub fn terminal_id(&self, pid: Word) -> Option<Word> {
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = &self.managed[i];
            if process.pid == pid && process.pcb != 0 {
                return Some(process.terminal_id);
            }
            i += 1;
        }
        None
    }

    pub fn terminal_canonical(&self, pid: Word) -> Option<bool> {
        let mut i = 0usize;
        while i < self.managed.len() {
            let process = &self.managed[i];
            if process.pid == pid && process.pcb != 0 {
                return Some(process.terminal_canonical);
            }
            i += 1;
        }
        None
    }

    pub fn set_linux_file(&mut self, pid: Word, fd: Word, file: LinuxFile) -> bool {
        if fd as usize >= LINUX_FD_MAX {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.files[fd as usize] = file;
        true
    }

    pub fn clear_linux_file(&mut self, pid: Word, fd: Word) -> Option<LinuxFile> {
        if fd as usize >= LINUX_FD_MAX {
            return None;
        }
        let process = self.managed_process_mut(pid)?;
        let old = process.files[fd as usize];
        process.files[fd as usize] = LinuxFile::EMPTY;
        if old.is_open() {
            Some(old)
        } else {
            None
        }
    }

    pub fn allocate_linux_file(
        &mut self,
        pid: Word,
        file: LinuxFile,
        min_fd: Word,
    ) -> Option<Word> {
        let process = self.managed_process_mut(pid)?;
        let mut fd = min_fd as usize;
        if fd >= process.files.len() {
            return None;
        }
        while fd < process.files.len() {
            if !process.files[fd].is_open() {
                process.files[fd] = file;
                return Some(fd as Word);
            }
            fd += 1;
        }
        None
    }

    pub fn set_cwd(&mut self, pid: Word, path: &[u8]) -> bool {
        if path.is_empty() || path.len() >= LINUX_CWD_MAX || path[0] != b'/' {
            return false;
        }
        let Some(process) = self.managed_process_mut(pid) else {
            return false;
        };
        process.cwd = [0; LINUX_CWD_MAX];
        process.cwd[..path.len()].copy_from_slice(path);
        process.cwd_len = path.len();
        true
    }

    pub fn inherit_process_files(&mut self, parent_pid: Word, child_pid: Word) -> bool {
        let Some(parent) = self.managed_process(parent_pid) else {
            return false;
        };
        let (cwd, cwd_len, files) = (parent.cwd, parent.cwd_len, parent.files);
        let Some(child) = self.managed_process_mut(child_pid) else {
            return false;
        };
        child.cwd = cwd;
        child.cwd_len = cwd_len;
        child.files = files;
        true
    }

    pub fn allocate_pipe(&mut self) -> Option<Word> {
        let mut i = 0usize;
        while i < self.pipes.len() {
            if !self.pipes[i].active {
                self.pipes[i] = LinuxPipe {
                    active: true,
                    readers: 1,
                    writers: 1,
                    buffer: [0; LINUX_PIPE_BYTES],
                    read: 0,
                    write: 0,
                    len: 0,
                };
                return Some(i as Word);
            }
            i += 1;
        }
        None
    }

    pub fn pipe(&self, pipe_id: Word) -> Option<LinuxPipe> {
        let index = pipe_id as usize;
        if index >= self.pipes.len() || !self.pipes[index].active {
            return None;
        }
        Some(self.pipes[index])
    }

    pub fn pipe_mut(&mut self, pipe_id: Word) -> Option<&mut LinuxPipe> {
        let index = pipe_id as usize;
        if index >= self.pipes.len() || !self.pipes[index].active {
            return None;
        }
        Some(&mut self.pipes[index])
    }
}
