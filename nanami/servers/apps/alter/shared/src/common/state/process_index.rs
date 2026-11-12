//! Validated slot hints across alternating PIDs. They never own a process
//! reference, and stale/reused slots are checked before every hit.
use core::cell::Cell;

pub struct ProcessIndex([Cell<usize>; 16]);

impl ProcessIndex {
    pub const fn new() -> Self {
        Self([const { Cell::new(usize::MAX) }; 16])
    }

    pub fn find<T>(
        &self,
        entries: &[T],
        pid: usize,
        mut identity: impl FnMut(&T) -> Option<usize>,
    ) -> Option<usize> {
        // Collisions only restore the linear lookup. Full current identity,
        // rather than the PID bucket or saved slot, proves every hit.
        let hint = &self.0[pid % self.0.len()];
        let cached = hint.get();
        if entries
            .get(cached)
            .is_some_and(|entry| identity(entry) == Some(pid))
        {
            return Some(cached);
        }
        let index = entries
            .iter()
            .position(|entry| identity(entry) == Some(pid))?;
        hint.set(index);
        Some(index)
    }
}
