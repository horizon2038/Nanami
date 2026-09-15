//! One boot-time decision shared by the PCI and USB storage drivers. Publication
//! order must not choose the root disk, and no arbitration is on the I/O path.
pub struct Selection {
    pub pids: [usize; 2],
    reports: [Option<usize>; 2],
}

impl Selection {
    pub const fn new() -> Self {
        Self {
            pids: [0; 2],
            reports: [None; 2],
        }
    }

    pub fn report(&mut self, pid: usize, roots: usize) -> Result<usize, ()> {
        use nanami_services::device::{
            STORAGE_AMBIGUOUS, STORAGE_NOT_SELECTED, STORAGE_PENDING, STORAGE_SELECTED,
        };
        let index = self
            .pids
            .iter()
            .position(|&value| pid != 0 && value == pid)
            .ok_or(())?;
        if roots > 2 || self.reports[index].is_some_and(|previous| previous != roots) {
            return Err(());
        }
        self.reports[index] = Some(roots);
        if self
            .pids
            .iter()
            .zip(self.reports)
            .any(|(&pid, report)| pid != 0 && report.is_none())
        {
            return Ok(STORAGE_PENDING);
        }
        let total: usize = self.reports.iter().flatten().sum();
        Ok(if total > 1 {
            STORAGE_AMBIGUOUS
        } else if total == 1 && roots == 1 {
            STORAGE_SELECTED
        } else {
            STORAGE_NOT_SELECTED
        })
    }
}
