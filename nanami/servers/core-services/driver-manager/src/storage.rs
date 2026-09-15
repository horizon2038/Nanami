//! One boot-time decision shared by the PCI and USB storage drivers. Publication
//! order must not choose the root disk, and no arbitration is on the I/O path.
pub struct Selection {
    pub pids: [usize; 2],
    reports: [Option<usize>; 2],
    decision: Option<Decision>,
}

enum Decision {
    Selected(usize),
    Ambiguous,
}

impl Selection {
    pub const fn new() -> Self {
        Self {
            pids: [0; 2],
            reports: [None; 2],
            decision: None,
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
        if roots > 2
            || self.reports[index].is_some_and(|previous| previous != 0 && previous != roots)
        {
            return Err(());
        }
        if self.decision.is_none() {
            // An empty scan is not final: USB may enumerate after driver startup.
            // A positive report cannot be withdrawn, and a committed root is
            // never replaced by a later hotplug.
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
            self.decision = if total > 1 {
                Some(Decision::Ambiguous)
            } else {
                self.reports
                    .iter()
                    .position(|&report| report == Some(1))
                    .map(|index| Decision::Selected(self.pids[index]))
            };
        }
        Ok(match self.decision {
            Some(Decision::Ambiguous) => STORAGE_AMBIGUOUS,
            Some(Decision::Selected(winner)) if winner == pid => STORAGE_SELECTED,
            _ => STORAGE_NOT_SELECTED,
        })
    }
}
