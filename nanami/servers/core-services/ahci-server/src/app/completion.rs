//! Poll command completion without reading a second MMIO register every lap.
use super::*;

pub(super) enum Register { Issued, Status }

#[inline]
pub(super) fn wait(
    read: impl FnMut(Register) -> u32,
    progress: impl FnMut() -> bool,
    yield_now: impl FnMut(),
    limit: usize,
) -> Result<(), RequestError> {
    wait_with_backoff(read, progress, yield_now, |count| {
        for _ in 0..count { core::hint::spin_loop(); }
    }, limit)
}

#[inline]
pub(super) fn wait_with_backoff(
    mut read: impl FnMut(Register) -> u32,
    mut progress: impl FnMut() -> bool,
    mut yield_now: impl FnMut(),
    mut relax: impl FnMut(usize),
    limit: usize,
) -> Result<(), RequestError> {
    let mut spin = 0usize;
    let mut issued_reads = 0usize;
    let mut status_reads = 0usize;
    let mut progress_reads = 0usize;
    loop {
        // DMA's private command-header byte count is only a polling hint.
        // Always inspect CI immediately and at least once per 16 iterations,
        // including on HBAs that defer or omit intermediate byte-count updates.
        let check = spin & 15 == 0 || {
            progress_reads += 1;
            progress()
        };
        let complete = if check {
            issued_reads += 1;
            read(Register::Issued) & 1 == 0
        } else {
            false
        };
        // PxIS.TFES is write-one-to-clear (AHCI 1.3.1 section 3.3.5).
        // submit clears it before issuing, and no other request runs here.
        // Recheck while busy, and always check before accepting completion.
        // https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/serial-ata-ahci-spec-rev1-3-1.pdf
        if complete || spin & 15 == 15 {
            status_reads += 1;
            if read(Register::Status) & PORT_IS_TFES != 0 {
                return Err(RequestError::Transport);
            }
        }
        if complete {
            fence(Ordering::Acquire);
            #[cfg(not(test))]
            if option_env!("NANAMI_AHCI_POLL_DIAG") == Some("1") {
                use core::sync::atomic::{AtomicUsize, Ordering};
                static COMMANDS: AtomicUsize = AtomicUsize::new(0);
                static ISSUED_READS: AtomicUsize = AtomicUsize::new(0);
                static STATUS_READS: AtomicUsize = AtomicUsize::new(0);
                static POLLS: AtomicUsize = AtomicUsize::new(0);
                static PROGRESS_READS: AtomicUsize = AtomicUsize::new(0);
                let ci = ISSUED_READS.fetch_add(issued_reads, Ordering::Relaxed) + issued_reads;
                let status = STATUS_READS.fetch_add(status_reads, Ordering::Relaxed) + status_reads;
                let polls = POLLS.fetch_add(spin + 1, Ordering::Relaxed) + spin + 1;
                let progress = PROGRESS_READS.fetch_add(progress_reads, Ordering::Relaxed) + progress_reads;
                let commands = COMMANDS.fetch_add(1, Ordering::Relaxed) + 1;
                if commands <= 8 || commands.is_power_of_two() {
                    nanami_runtime::println!("[ahci.poll] commands={} ci={} is={} polls={} hints={}", commands, ci, status, polls, progress);
                }
            }
            return Ok(());
        }
        spin += 1;
        if spin >= limit {
            return Err(RequestError::Transport);
        }
        if spin & 0x3fff == 0 {
            yield_now();
        }
        // Keep the first completion check immediate. For a busy command,
        // spread subsequent uncached MMIO reads (VM exits under KVM) with
        // bounded CPU relaxation: 1,2,4,...,64 PAUSEs, four polls per step.
        // No sleep/IRQ dependency; CI and the latched TFES remain authoritative.
        relax(1 << ((spin - 1) / 4).min(6));
    }
}
