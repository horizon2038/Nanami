//! Diagnostic adapter: uses existing values only; no additional clock reads.
use super::*;
use crate::common::wait_shape::{Identity, Row, OWNER_LIMIT};
use crate::state::readiness::ReadinessWait;

pub(super) fn note(
    runtime: &mut Runtime, pid: Word, wait: &ReadinessWait, phase: u8,
    timeout_ns: Option<u128>, ready: Option<Word>, trigger: Word, expired: bool,
    result: isize,
) {
    if !runtime.wait_shape.active_for(pid) || !runtime.managed_process(pid).is_some_and(|process| {
        process.image_name.get(..process.image_name_len).is_some_and(|name| {
            name.rsplit(|byte| *byte == b'/').next() == Some(&b"xfe"[..])
        })
    }) {
        return;
    }
    let fd = if wait.count == 0 { -1 } else { wait.entries[0].fd };
    let (socket, peer) = if fd >= 0 {
        runtime.linux_file(pid, fd as Word)
            .filter(|file| file.kind == LinuxFileKind::SocketUnix)
            .and_then(|file| runtime.unix_sockets.get(file.resource))
            .map_or((0, 0), |socket| (socket.id, socket.peer.unwrap_or(0)))
    } else { (0, 0) };
    let sample_row = runtime.wait_shape.rows.len();
    runtime.wait_shape.record(Row {
        pid, syscall: wait.context.number, phase,
        cached_ticks: runtime.monotonic_ticks, hz: runtime.monotonic_tick_hz,
        deadline: wait.deadline, timeout_ns, ready,
        interests: wait.sources, trigger, expired, result, count: wait.count,
        first_fd: fd, socket, peer, lower: None, upper: None,
    });
    if sample_row < runtime.wait_shape.rows.len() && runtime.wait_shape.needs_identity(pid, socket, peer) {
        if let Some(endpoint) = runtime.unix_sockets.get(peer) {
            let address = &endpoint.address;
            let mut identity = Identity::new(pid, socket, peer, sample_row,
                &address.bytes[..address.len]);
            // A creator credential is not proof of current ownership. Match
            // every live descriptor to this exact open-description ID. Retain
            // a bounded owned name prefix and the full descriptor count.
            for process in &runtime.managed {
                if process.pid == 0 || process.exited { continue; }
                for (fd, file) in process.files.iter().enumerate() {
                    if file.kind == LinuxFileKind::SocketUnix && file.resource == peer {
                        identity.add_owner(process.pid, fd,
                            &process.image_name[..process.image_name_len]);
                    }
                }
            }
            // An accepted endpoint can still be pending in the listener. If
            // no FD owns it yet, retry on a later event instead of guessing.
            runtime.wait_shape.capture_identity(identity);
        }
    }
}

pub(super) fn clock(runtime: &mut Runtime, ticks: Word, hz: Word) {
    runtime.wait_shape.sample(ticks, hz);
}

pub(super) fn path_step(runtime: &mut Runtime, pid: Word, step: usize) {
    runtime.wait_shape.path_step(pid, step);
}

pub(super) fn path(runtime: &mut Runtime, pid: Word, bytes: usize) {
    if !runtime.wait_shape.in_gap(pid) { return; }
    // read_c_string already copied and validated this NUL-terminated prefix.
    // Snapshot now, before any path translation or subsequent IPC reuses SHM.
    let value = unsafe { core::slice::from_raw_parts(runtime.posix_shm as *const u8, bytes) };
    runtime.wait_shape.path(pid, value);
}

pub(super) fn report(runtime: &mut Runtime) {
    if !runtime.wait_shape.due(runtime.monotonic_ticks, runtime.monotonic_tick_hz) {
        return;
    }
    let trace = &runtime.wait_shape;
    nanami_runtime::println!("[wait.shape.begin] pid={} rows={} dropped={} allocation-failed={} cached-ticks-not-elapsed=1 clock-consistent={}",
        trace.pid.unwrap_or(0), trace.rows.len(), trace.dropped, trace.allocation_failed as u8, trace.clock_consistent as u8);
    if let Some(identity) = &trace.identity {
        nanami_runtime::println!("[wait.peer] socket={} peer={} sample-row={} descriptors={} address-truncated={} address={:?}",
            identity.socket, identity.peer, identity.sample_row, identity.descriptors,
            identity.address_truncated as u8, &identity.address[..identity.address_len]);
        for (index, owner) in identity.owners[..identity.descriptors.min(OWNER_LIMIT)].iter().enumerate() {
            let image = core::str::from_utf8(&owner.image[..owner.image_len]).unwrap_or("<non-utf8>");
            nanami_runtime::println!("[wait.owner] index={} pid={} fd={} image={} truncated={}",
                index, owner.pid, owner.fd, image, owner.truncated as u8);
        }
    } else {
        nanami_runtime::println!("[wait.peer] unavailable=1");
    }
    for (index, row) in trace.rows.iter().enumerate() {
        nanami_runtime::println!("[wait.shape] seq={} pid={} call={} phase={} cached={} hz={} deadline={:?} timeout-ns={:?} ready={:?} interests={} trigger={} expired={} result={} count={} fd={} socket={} peer={} lower={:?} upper={:?}",
            index, row.pid, row.syscall, row.phase, row.cached_ticks, row.hz,
            row.deadline, row.timeout_ns, row.ready, row.interests, row.trigger,
            row.expired as u8, row.result, row.count, row.first_fd, row.socket, row.peer, row.lower, row.upper);
    }
    nanami_runtime::println!("[wait.shape.end] rows={}", trace.rows.len());
    let total = trace.gap_calls.iter().try_fold(0usize, |sum, count| sum.checked_add(*count));
    let kinds = trace.gap_calls.iter().filter(|count| **count != 0).count();
    nanami_runtime::println!("[wait.gap] start={:?} end={:?} overflow={} entries={:?} kinds={} dispatch-entries=1 closing-poll-included=1",
        trace.gap_start, trace.gap_end, trace.gap_overflow as u8, total, kinds);
    for (number, count) in trace.gap_calls.iter().enumerate() {
        if *count != 0 {
            nanami_runtime::println!("[wait.gap.call] number={} count={}", number, count);
        }
    }
    nanami_runtime::println!("[wait.paths] total={} unique={} dropped={} allocation-failed={} overflow={} last-string-result-only=1",
        trace.path_total, trace.paths.len(), trace.path_dropped, trace.path_allocation_failed as u8, trace.path_overflow as u8);
    nanami_runtime::println!("[wait.path.steps] resolves={} readlinks={} rejected={} overflow={}",
        trace.path_steps[0], trace.path_steps[1], trace.path_steps[2], trace.path_step_overflow as u8);
    for (index, row) in trace.paths.iter().enumerate() {
        let bytes = &row.bytes[..row.len];
        if bytes.iter().all(|byte| (32..=126).contains(byte)) {
            let text = core::str::from_utf8(bytes).unwrap();
            nanami_runtime::println!("[wait.path] index={} first-call={} hits={} ok={} errors={} last={:?} truncated={} text={:?}",
                index, row.first_call, row.hits, row.ok, row.errors, row.last, row.truncated as u8, text);
        } else {
            nanami_runtime::println!("[wait.path] index={} first-call={} hits={} ok={} errors={} last={:?} truncated={} bytes={:?}",
                index, row.first_call, row.hits, row.ok, row.errors, row.last, row.truncated as u8, bytes);
        }
    }
}
