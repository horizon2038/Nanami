//! One validated slot hint for repeated lookups within a syscall. It never
//! owns a process reference, and stale slots are checked before every hit.
use core::cell::Cell;

pub struct ProcessIndex(Cell<usize>);

impl ProcessIndex {
    pub const fn new() -> Self {
        Self(Cell::new(usize::MAX))
    }

    pub fn find<T>(
        &self,
        entries: &[T],
        pid: usize,
        mut identity: impl FnMut(&T) -> Option<usize>,
    ) -> Option<usize> {
        let cached = self.0.get();
        if entries
            .get(cached)
            .is_some_and(|entry| identity(entry) == Some(pid))
        {
            return Some(cached);
        }
        let index = entries
            .iter()
            .position(|entry| identity(entry) == Some(pid))?;
        self.0.set(index);
        Some(index)
    }
}
