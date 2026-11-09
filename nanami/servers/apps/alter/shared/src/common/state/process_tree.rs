//! Read-only selection of the deepest process for child-first tree teardown.
use super::*;

impl Runtime {
    pub fn deepest_process_in_tree(&self, root_pid: Word) -> Option<ManagedProcess> {
        if root_pid == 0 {
            return None;
        }
        // The table is immutable throughout selection. Retain only the slot
        // until the winner is known, then take the caller's owned snapshot.
        let mut best = None;
        let mut best_depth = 0usize;
        let mut i = 0usize;
        while i < self.managed.len() {
            let entry = &self.managed[i];
            if entry.pid != 0 {
                if let Some(depth) = self.tree_depth(entry.pid, root_pid) {
                    if best.is_none() || depth >= best_depth {
                        best = Some(i);
                        best_depth = depth;
                    }
                }
            }
            i += 1;
        }
        best.map(|index| self.managed[index])
    }

    fn tree_depth(&self, pid: Word, root_pid: Word) -> Option<usize> {
        let mut current = pid;
        let mut depth = 0usize;
        while depth <= self.managed.len() {
            if current == root_pid {
                return Some(depth);
            }
            let process = self.managed_process(current)?;
            if process.pid != process.tgid {
                current = process.tgid;
                depth += 1;
                continue;
            }
            if process.parent_pid == 0 {
                return None;
            }
            current = process.parent_pid;
            depth += 1;
        }
        None
    }
}
