//! Linux 64-bit sysinfo layout, populated from live Nanami accounting.
use super::{clocks, copyout_from, map_request_error, Runtime, Word, EFAULT};

pub(super) fn sysinfo(runtime: &mut Runtime, pid: Word, output: Word) -> Result<Word, i32> {
    const BYTES: usize = 112;
    if output == 0 || !runtime.memory_access_allowed(pid, output, BYTES, 2) {
        return Err(EFAULT);
    }
    clocks::refresh_clock(runtime)?;
    let memory = nanami_runtime::request_nanami_info_memory().map_err(map_request_error)?;
    let processes = nanami_runtime::request_nanami_info_process().map_err(map_request_error)?;
    let hz = runtime.monotonic_tick_hz;
    // Linux rounds a fractional second of boottime upwards for sysinfo.
    let uptime = runtime.monotonic_ticks / hz + Word::from(runtime.monotonic_ticks % hz != 0);
    let mut bytes = [0u8; BYTES];
    bytes[0..8].copy_from_slice(&(uptime as u64).to_le_bytes());
    bytes[32..40].copy_from_slice(&(memory.total_bytes as u64).to_le_bytes());
    bytes[40..48].copy_from_slice(&(memory.free_bytes as u64).to_le_bytes());
    bytes[80..82].copy_from_slice(&(processes.running.min(u16::MAX as Word) as u16).to_le_bytes());
    bytes[104..108].copy_from_slice(&1u32.to_le_bytes());
    // No swap or separate high-memory zone. Load averages and shared/buffer
    // subdivisions have no accounting source yet, so remain zero, not invented
    // from the count of live (possibly blocked) processes or total used memory.
    copyout_from(runtime, pid, output, bytes.as_ptr() as Word, BYTES)?;
    Ok(0)
}
