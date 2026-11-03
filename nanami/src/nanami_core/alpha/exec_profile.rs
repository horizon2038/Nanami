//! Opt-in local counter samples for exec/reap diagnosis, not benchmark results.
//! Alpha stays on one core; elapsed ticks include interrupts/descheduling.
const ENABLED: bool = match option_env!("NANAMI_EXEC_PROFILE") {
    Some(value) => value.len() == 1 && value.as_bytes()[0] == b'1',
    None => false,
};

pub(super) fn stamp() -> u64 {
    if !ENABLED {
        return 0;
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let low: u32;
        let high: u32;
        core::arch::asm!("lfence", "rdtsc", "lfence", out("eax") low, out("edx") high,
            options(nostack, preserves_flags));
        (u64::from(high) << 32) | u64::from(low)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let ticks: u64;
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks,
            options(nostack, preserves_flags));
        ticks
    }
}

pub(super) fn report(pid: usize, image_pages: usize, stamps: [u64; 6]) {
    if !ENABLED {
        return;
    }
    let ticks = core::array::from_fn::<_, 5, _>(|i| stamps[i + 1].saturating_sub(stamps[i]));
    crate::force_info!(
        "[exec.profile] pid={} image-pages={} unmap={} space={} plan={} image={} stack={} counter-ticks=true",
        pid, image_pages, ticks[0], ticks[1], ticks[2], ticks[3], ticks[4]
    );
}

pub(super) fn report_reap(pid: usize, references: usize, stamps: [u64; 10]) {
    if !ENABLED {
        return;
    }
    let ticks = core::array::from_fn::<_, 9, _>(|i| stamps[i + 1].saturating_sub(stamps[i]));
    crate::force_info!(
        "[reap.profile] pid={} refs={} invalidate={} revoke={} remove={} tables={} ram={} deferred={} references={} arena={} entry={} counter-ticks=true",
        pid, references, ticks[0], ticks[1], ticks[2], ticks[3], ticks[4], ticks[5], ticks[6], ticks[7], ticks[8]
    );
}

pub(super) fn report_drop(pid: usize, references: usize, reset: u64, phases: [u64; 4]) {
    if !ENABLED {
        return;
    }
    crate::force_info!(
        "[exec.drop.profile] pid={} refs={} reset={} unmap={} remove={} ram={} deferred={} counter-ticks=true",
        pid, references, reset, phases[0], phases[1], phases[2], phases[3]
    );
}
