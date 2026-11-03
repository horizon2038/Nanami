# Nanami / Alter optimization cycles

Optimize user-level implementation work without removing service boundaries,
permission checks or lifecycle guarantees. Avoid Kernel changes. Aim for at
least 5% lower elapsed time per cycle on a declared Tiny Core workload; record
failed candidates too. Tiny Core exercises Alter, filesystems, process/VM
management and nested graphics, but is not a measurement of every native
application, physical driver or GPU workload.

Use identical timed guest fixtures, sequential QEMU boots, and before/after
controls. Do not run builds or other VMs concurrently with timed samples.
Retain ordinary logging, correctness checks and workload sizes. Report workloads
separately rather than hiding a regression behind a weighted synthetic score.
Same-core handler counter samples include descheduling; they are diagnostic
stage timings, not proof of expensive IPC transport or application speedup.

## Opt-in stage profiling

Build with `NANAMI_EXEC_PROFILE=1` to log Alpha's exec teardown,
address-space recreation, planning, image materialization and stack
materialization stages. Build with `NANAMI_ALTER_PROFILE=1` to accumulate
Alter syscall handler counts and elapsed counter ticks, flushing the totals
when a guest exits. Both settings are compile-time options and are disabled
by default; neither requires `NANAMI_PIPELINE_TRACE`.

The same `NANAMI_EXEC_PROFILE` switch also reports reap stages: copy-window
invalidation, root revoke/remove, page-table release, physical RAM release,
deferred release, ownership-reference removal, arena reset and entry removal.
Reap reports are per process; a deferred VM owner's final cleanup can produce
its own report after the last peer is reaped. Default builds retain no samples
or profiling output.

Alter totals cover all guests served since the previous flush. The `trigger`
PID identifies the exit that caused the flush, not ownership of every sample.
The counters include service work, interrupts and descheduling; they are not
exclusive CPU time or measurements of IPC transport overhead. Counter units
must be calibrated before converting them to time, and samples from different
cores must not be compared without verifying counter synchronization.

Leave these options unset in builds used for before/after speed comparisons.

## Cycle 1 candidate: final permissions on initial page publication

Baseline: commit `75f9c9b`, Nanami 0.7.0. Four-vCPU QEMU/TCG, normal input and
network devices, `qemu-smoke.py --tinycore-process-perf --timeout 240`.
Run order: before 1, after 1, after 2, before 2. Three timed samples per boot;
the same integer-only harness runs Tiny Core's unmodified dynamic BusyBox.
Fork/exec timings include waiting and teardown, not only page construction.

The experiment mapped new private pages directly with their final permissions,
instead of publishing RWX and then changing permissions. It retained PROT_NONE
backing without a temporary accessible leaf, kept the legacy two-Word request,
and used an additive three-Word request for final permissions. Existing-page
copy/protect and service boundaries were unchanged.

Per-boot medians, milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight fork/wait | 88.926 | 89.914 | 91.158 | 85.056 |
| Eight fork/exec/wait | 2500.383 | 2468.935 | 2450.487 | 2363.137 |

Pooling six samples per variant gives 88.865 -> 90.536 ms for fork/wait and
2456.564 -> 2465.335 ms for fork/exec/wait. This does not establish an improvement,
let alone a 5% improvement. Native Alpha controls and the before reboot also
show host/TCG variation. Fewer mapping operations alone are not enough evidence
to justify adding another service operation.

Decision: reject this candidate and restore the production implementation.
The added API and implementation are not retained. Diagnostic logs are
`/private/tmp/nanami-opt-cycle1-{before1,after1,after2,before2}.log`; their
`Test logs:` directories contain the complete serial output and success markers.
The rejected patch is retained at `/private/tmp/nanami-opt-cycle1-rejected.patch`.
Its full host suite passed 1,012 tests (nine opt-in tests ignored), and focused
AddressSanitizer tests passed 38 tests. Correctness passing is not a performance
acceptance criterion.

Next investigation: separate Alpha exec's per-page unmap/cap removal,
address-space recreation and image/stack materialization. Do not change their
ordering until invalidation, shared backing, rollback and frame ownership have
been audited and the expensive stage has been measured.

## Exec diagnosis

A separate diagnostic image enabled both stage profilers and completed the
four-vCPU Tiny Core process fixture. Across 24 normal BusyBox execs the
syscall-handler totals included 384 mmap calls taking 1,538,598,000 counter
ticks, 24 exec calls taking 481,046,000 ticks and 48 fork calls taking
132,250,000 ticks. These totals include service work and descheduling, not
only local emulation instructions. They do not account for all application
elapsed time, and do not establish that IPC transport is expensive.

The raw log is `/private/tmp/nanami-opt-cycle1-handler-profile.log`; its
`Test logs:` directory retains the individual samples. This profiling build
is not used for before/after comparisons.

## Cycle 2: borrow single-packet AF_UNIX receive payloads

Previously every receive copied queued bytes into a fresh Vec before the
checked guest copyout. A single-packet receive now borrows the immutable
queued bytes. Multiple stream packets are still coalesced so the change does
not add copyout requests across packet boundaries. Packet consumption and
queued-rights release still happen only after payload and metadata copyout
succeed. SCM_RIGHTS, peek, truncation, short reads and failure rollback retain
the same rules. There is no new service operation or endpoint.

The host fixture checks the exact queued source address and zero temporary
allocation for a small payload with a 64-KiB receive buffer. It also checks
partial-packet offsets, a later iovec's copy failure without queue consumption,
and unchanged coalesced-copyout counts. AF_UNIX and readiness suites passed
53 tests.

Four-vCPU QEMU/TCG, identical `--tinycore-perf --normal-devices` fixture, three
samples per boot, sequential before/after/after/before order:

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 2,048 AF_UNIX round trips | 386.939 | 365.836 | 369.118 | 365.069 |
| 512 existing-path stat calls (control) | 161.535 | 139.289 | 138.444 | 146.544 |
| 512 missing-path stat calls (control) | 108.835 | 114.961 | 107.479 | 109.591 |

Pooling six samples gives 376.199 -> 367.477 ms for AF_UNIX, about 2.3% lower.
The before reboot overlaps the after results, and the unchanged stat control
also moves. This does not establish a 5% end-to-end speedup. The allocation
and intermediate-payload-copy removal are directly checked, not inferred
from these timings. Logs: `/private/tmp/nanami-opt-cycle2-{before1,after1,after2,before2}.log`.

## Cycle 3 candidate: multi-key VM page indexes (rejected)

Replacing the heap-backed per-process AVL indexes with BTreeMap reduced the
host fixture's 4,096-page frame-index footprint from 4,096 allocations /
163,840 live bytes to 681 allocations / 139,968 bytes. A local Release
insert/find/remove benchmark also improved at larger map sizes. The static,
pre-heap bootstrap tracker and the permissions index were unchanged.

Those local results did not carry through to the Tiny Core process fixture.
Four-vCPU QEMU/TCG, three samples per boot, after/before/after/before order:

| Workload median (ms) | After 1 | Before 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight fork/wait | 83.502 | 81.612 | 82.297 | 91.426 |
| Eight fork/exec/wait | 2369.473 | 2326.810 | 2460.778 | 2332.324 |
| 2,048 getresuid, ordinary VMAs | 529.237 | 498.267 | 548.478 | 504.972 |
| 2,048 getresuid, 128 extra VMAs | 531.678 | 528.383 | 496.433 | 517.572 |

Pooling six samples gives 2329.567 -> 2386.850 ms for fork/exec (2.5% slower)
and 504.549 -> 532.937 ms for ordinary getresuid/copyout (5.6% slower).
The memory saving does not justify retaining a candidate with these application
results. Restore the original AVL representation. The later allocation-free
range visitor is retained separately for mapping-release validation; it does
not replace the indexes.

Logs: `/private/tmp/nanami-opt-cycle3-{after1,before1,after2,before2}.log` and
`/private/tmp/nanami-opt-cycle3-vm-bench.log`. The experimental patch is archived
at `/private/tmp/nanami-opt-cycle3-rejected.patch`, not applied to production.

## Cycle 4: mapping-release ownership snapshots

Sparse release previously searched the complete per-process physical ownership
list for every virtual page during validation, searched it again during UNMAP,
and searched again when moving each reference to deferred storage. The first
two phases now share one sorted, deduplicated snapshot. Strict coverage is
checked by its count; sparse coverage checks only backed VM leaves (including
PROT_NONE) against the snapshot. Unpopulated holes require no per-page walk.
UNMAP still runs in ascending address order, and no reference becomes deferred
until all requested UNMAPs succeed. Newly registered extents are removed in
reverse address order to avoid repeatedly skipping the rest of the same extent
in the ownership list. Arbitrarily ordered ownership lists can still incur
linear searches during this final unlink phase; this is not a claim that every
release is O(k log k).

Authority checks, exact multi-page extents, shared-memory dispatch and frame
ownership rules remain unchanged. Range rounding and end-address overflow are
rejected before mutation. The production handler is exercised directly with
only capability operations mocked: strict holes, missing references under
PROT_NONE, failed copy-window invalidation, partial UNMAP failure/retry, exact
extent slot order, shared-VM ownership, duplicate references and a terabyte
of mostly unpopulated VA. No limits are reduced and no IPC operation is added.

The full native suite passed 1,019 tests (ten opt-in tests ignored). Tiny Core
measurements use the unchanged process fixture and profile-disabled images;
the before image retains the cycle-2 AF_UNIX change, isolating this release
change from the reverted BTreeMap experiment.

The dedicated Release host benchmark (no MAP/UNMAP IPC) measures ownership
snapshot/lookup/unlink work, with fixture construction outside the timer:

| Pages | Old median | Snapshot median |
| ---: | ---: | ---: |
| 128 | 114.791 us | 21.791 us |
| 1,024 | 6.260 ms | 0.178 ms |
| 4,096 | 55.689 ms | 0.328 ms |

Reproduce with `cargo test --manifest-path tests/native-performance/Cargo.toml
--offline --release --test core-process allocation_index::range_release_bookkeeping_cost
-- --ignored --nocapture --test-threads=1` (as a single command).

The four-vCPU whole-process measurements do **not** establish a speedup:

| Workload median (ms) | After 1 | Before 1 | Before 2 | After 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight fork/wait | 79.549 | 80.216 | 80.478 | 92.848 |
| Eight fork/exec/wait | 2265.252 | 2167.675 | 2281.846 | 2454.574 |
| 2,048 getresuid, ordinary VMAs | 491.934 | 453.321 | 499.586 | 570.765 |
| 2,048 getresuid, 128 extra VMAs | 484.183 | 485.148 | 507.438 | 505.839 |

Pooled fork/exec is about 6% slower, while the unchanged ordinary-copyout
workload also moves by about 5%. Neither those controls nor the local benchmark
justify claiming a 5% application improvement. Retain the distinction between
proven elimination of repeated ownership work and pending end-to-end
performance acceptance. Logs: `/private/tmp/nanami-opt-cycle4-{after1,before1,before2,after2}.log`
and `/private/tmp/nanami-opt-cycle4-range-bench.log`.

Focused AddressSanitizer checks passed 80 tests (two opt-in tests ignored).
The four-vCPU desktop regression passed two sessions in one boot, including
real input, Terminal/Editor/Information launch and Exit/restart cleanup.
The second Information screenshot after its command contains Tiny Core's
release text and a working shell, not just an empty window. Log:
`/private/tmp/nanami-opt-cycle4-desktop.log`.

## Cycle 5: minimal x86_64 trapped-syscall replies

The current A9N fault ABI updates exactly the register prefix in the reply;
untransmitted registers remain unchanged. Its INVALID_KERNEL_CALL fault PC
and the transmitted RIP are both read from the same stopped register context.
Alter previously rewrote RIP to that identical PC and returned RAX through RIP
(16 Words) for every ordinary Linux syscall result.

An explicit trapped-snapshot reply encoder now returns only RAX (one Word)
when Linux's PC is unchanged. A changed PC retains the existing 16-Word form;
FreeBSD still returns through RFLAGS (18 Words) for its carry flag. Signal and
parked-context editing retain the generic encoder and its old register counts.
AArch64 already used an X0-only reply and is unchanged. The incoming fault
still carries the complete context; no Kernel, fastpath or ABI change is made.

The host test uses the real x86_64 encoder and models the Kernel's actual prefix
assignment, checking negative/zero/positive results, untouched registers,
changed-PC fallback, generic signal-context counts and FreeBSD carry handling.
The full native suite passed 1,023 tests (ten opt-in tests ignored), the focused
reply/signal ASan tests passed 20 tests, and Alter/Linux AArch64 and
Alter/FreeBSD x86_64 Release checks passed. End-to-end measurements are kept
separate from these word-count and correctness checks.

Four-vCPU QEMU/TCG, normal input/network devices, unchanged `--tinycore-perf`
fixture, three samples per boot, sequential before/after/after/before order.
The before image includes the same AF_UNIX receive and mapping-release changes;
the isolated difference is the trapped-reply encoder.

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 512 existing-path stat calls | 159.706 | 139.287 | 143.745 | 152.623 |
| 512 missing-path stat calls | 111.233 | 116.545 | 112.955 | 120.790 |
| 2,048 AF_UNIX round trips | 432.129 | 379.885 | 399.517 | 392.284 |

Pooling six samples gives 155.408 -> 142.129 ms (about 8.5% lower) for
existing-path stat, 119.559 -> 114.750 ms (about 4.0%) for missing-path stat,
and 393.569 -> 387.432 ms (about 1.6%) for AF_UNIX. Both before-boot stat
medians exceed both after-boot medians; AF_UNIX samples overlap appreciably.
The cycle's 5% target is observed on this particular stat workload, not on
AF_UNIX or all desktop activity. These results do not demonstrate that process
startup or the complete desktop is 8.5% faster. The ordinary frame-workload
controls are retained in the logs; no frame algorithm was changed.

All four runs passed the real Tiny Core init/dynamic/interactive fixture.
While checking for benchmark interference, a process-name/CPU sample during
the after run showed only its QEMU process and no Cargo/rustc/Clang process;
this one observation is not proof that the host had no other activity.
Logs: `/private/tmp/nanami-opt-cycle5-{before1,after1,after2,before2}.log`.

The cycle-5 image also passed the four-vCPU desktop fixture twice in one boot,
including launcher input, Terminal/Editor/Information and Exit/restart cleanup.
Log: `/private/tmp/nanami-opt-cycle5-desktop.log`. A separate process-fixture
run passed but had no new matched before run, so its startup timings do not
establish an improvement (`/private/tmp/nanami-opt-cycle5-process.log`).

## Cycle 6: request-local ELF source ownership snapshots

The ELF staging reader previously searched the source process's full physical
ownership list for every page copied into Alpha's image buffer. Multi-page
requests now reuse the existing sorted ownership-range snapshot; scalar
initializer records keep the old lookup without allocating a snapshot.
This is metadata only, not a cache of source bytes or a longer-lived reference.
Each page's current Frame is still resolved before using the hint. A missing
hint or a Frame mismatch falls back to the original live lookup, including
when resolution materializes a source page after the snapshot was taken.
Foreign Frames retain the existing transient mapping path. Size limits,
copying, publication and cleanup ordering are unchanged; there is no new
service operation, persistent cache or Kernel change.

Eight tests exercise the real staging reader with real ownership bookkeeping
and mocked Frame mapping: exact source bytes among 4,000 unrelated references,
unaligned boundaries, scalar allocation avoidance, newly materialized and
replaced backing, shared-VM ownership, foreign-Frame failure and invalid ranges.
The full native suite passed 1,031 tests (ten opt-in tests ignored); the
core-process ASan suite passed 60 tests (two opt-in tests ignored), and the
AArch64 Alpha Release check passed.

The process fixture now also reads Tiny Core's unmodified 1,547,968-byte Xfe
ELF outside the timer, creates a suspended process, and times construction
separately from kill/reap. It never resumes this process: this measures Alpha
construction, not dynamic linking or graphical Xfe startup. Its first sample
also includes cold mappings, unlike its two later samples. The original
BusyBox, Negoto, fork/exec and executable-freshness checks are retained.
The VFS shared window is kept alive until this last read in **both** variants;
do not treat the extended fixture as identical to older process-fixture runs.

Four-vCPU QEMU/TCG, normal input/network devices, profiling disabled,
sequential before/after/after/before order. Three samples per workload per boot
(six for Negoto create). Per-boot medians in milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| BusyBox Alpha construction | 3.799 | 3.546 | 3.988 | 4.367 |
| BusyBox kill/reap (control) | 22.458 | 24.232 | 23.114 | 25.153 |
| Xfe Alpha construction | 8.516 | 10.709 | 8.276 | 10.536 |
| Xfe kill/reap (control) | 52.356 | 65.532 | 52.443 | 60.376 |
| Eight fork/wait | 84.008 | 87.621 | 89.385 | 94.297 |
| Eight fork/exec/wait | 2399.821 | 2590.771 | 2359.833 | 2579.241 |
| 2,048 getresuid, ordinary VMAs | 507.089 | 521.148 | 505.979 | 512.268 |
| 2,048 getresuid, 128 extra VMAs | 533.657 | 652.863 | 508.171 | 508.083 |

Pooled BusyBox construction is 4.155 -> 3.821 ms (8.0% lower), but the per-boot
samples overlap and controls move too. Pooled Xfe construction is 9.834 ->
9.874 ms, effectively unchanged. Separating the two warm Xfe samples per boot
gives 8.823 -> 8.658 ms (1.9% lower), also overlapping. Pooled fork/exec is
2508.257 -> 2489.259 ms (0.8% lower), and ordinary getresuid is 509.679 ->
510.063 ms. These data do not establish a 5% application or desktop startup
improvement. Keep the proven removal of repeated list searches distinct from
an end-to-end speedup. One process-name/CPU sample during the second after run
showed only its QEMU process and no Cargo/rustc/Clang process; that is not proof
of an otherwise idle host throughout all four runs.

All four runs passed, including the executable-freshness checks. Logs:
`/private/tmp/nanami-opt-cycle6-{before1,after1,after2,before2}.log`,
`/private/tmp/nanami-v0.7.1-release-native-tests.log`,
`/private/tmp/nanami-opt-cycle6-asan.log` and
`/private/tmp/nanami-opt-cycle6-arm-check.log`.

## Cycle 7 candidate: bounded VMA access hints (rejected after cycle 9)

Do not simply remove Alter's permission check in favor of Alpha's checked
copy. In particular, a PROT_NONE reservation may have personality metadata
without an allocated Frame. That alternative was rejected before timing;
the production copyin/copyout and pathname checks are retained unchanged.

Instead the existing lazy MappingTable allocation stores two allowed intervals,
one per read/write permission. A miss scans the same unsorted VMA list and
finds the nearest disallowing interval on either side of the requested range.
Subsequent requests wholly inside that gap need no VMA scan. Every successful
push, remove or clear invalidates both hints. CLONE_VM aliases share the
same metadata allocation and invalidation; process snapshots remain one Word.
There are 32 additional bytes per allocated VMA table, no extra allocation,
and no cached page existence, backing, contents or Alpha authorization.
Combined/other permission masks use the uncached predicate. Alpha still
performs its checked-copy permission and Frame lookup; no service operation,
request count or Kernel fastpath changes.

New differential tests compare the real implementation against the old
predicate for overlapping, unsorted VMAs and against a separate page model.
They also cover unbacked PROT_NONE reservations, gap boundaries, both permission
hints, shared aliases, clear/remove/reallocation and VM-local cache ownership.
The unchanged production Alpha copy-request handler is in a testable module;
its tests cover authority, full permission preflight, read-only/PROT_NONE,
shared-VM permissions and the privileged-copy distinction. Real guest-copy
helpers are also tested for checked request selection and error propagation.
The full native suite passed 1,049 tests (ten opt-in tests ignored), the focused
ASan suite passed 86 (three opt-in tests ignored), and the Alter/Linux
AArch64 Release check passed.

The dedicated Release host test compares the old predicate with the cached
implementation, alternating addresses over an otherwise unchanged VMA set.
Three samples each, 500,000 checks per sample; fixture construction is excluded.

| VMA count | Linear median | Cached median |
| ---: | ---: | ---: |
| 8 | 3.379 ms | 0.563 ms |
| 32 | 13.744 ms | 0.499 ms |
| 128 | 43.828 ms | 0.496 ms |

This is local permission-bookkeeping work only, not syscall or desktop time.
The rejected candidate and its benchmark are archived in
`/private/tmp/nanami-opt-cycle9-vma-cache-rejected.patch`. The retained benchmark
compares the original predicate with the actual runtime implementation; it no
longer labels the removed optimization as an active cache.

Four-vCPU QEMU/TCG, normal devices, profile-disabled images, unchanged extended
process fixture, sequential before/after/after/before order. The before image
includes cycle 6; the after image adds only these permission hints and the
behavior-preserving copy-handler module split. Three samples per boot:

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight fork/wait | 101.469 | 85.070 | 83.665 | 90.973 |
| Eight fork/exec/wait | 2578.563 | 2366.164 | 2418.590 | 2459.978 |
| 2,048 getresuid, ordinary VMAs | 545.352 | 594.557 | 523.571 | 496.434 |
| 2,048 getresuid, 128 extra VMAs | 569.510 | 550.842 | 500.737 | 512.725 |
| BusyBox Alpha kill/reap (control) | 25.663 | 21.625 | 22.217 | 27.427 |
| Xfe Alpha construction (control) | 9.448 | 14.696 | 9.052 | 9.303 |

Pooled fork/exec is 2543.657 -> 2410.938 ms (5.2% lower), but the unchanged
native kill/reap control is also 25.742 -> 22.115 ms. Ordinary getresuid is
515.825 -> 569.366 ms (10.4% **higher**), and fragmented getresuid is
541.251 -> 522.820 ms (3.4% lower). The results do not establish a consistent
5% application improvement and do not justify ignoring the ordinary-copyout
regression. At this stage end-to-end performance acceptance was pending; the local removal
of repeated scans is not substituted for that gate.

All four process runs passed, including executable freshness. Logs:
`/private/tmp/nanami-opt-cycle7-cache-{before1,after1,after2,before2}.log`,
`/private/tmp/nanami-opt-cycle7-cache-native.log`,
`/private/tmp/nanami-opt-cycle7-cache-asan.log`,
`/private/tmp/nanami-opt-cycle7-cache-arm-alter.log` and
`/private/tmp/nanami-opt-cycle7-cache-vma-bench.log`.

The four-vCPU desktop fixture passed both sessions in one boot, including
real input, Terminal/Editor/Information commands, Exit and restart cleanup.
The second Information screenshot contains Tiny Core's release text and a
working shell. Log: `/private/tmp/nanami-opt-cycle7-cache-desktop.log`.

## Cycle 8: coalesce adjacent scalar syscall outputs

getresuid/getresgid already fetch one current ID from POSIX, but then made
three separate checked Alpha copies even when libc placed the three outputs
in consecutive u32 slots. A small const-sized guest-output helper now uses
one checked copy for adjacent, ascending, non-null outputs. It does not cache
credentials or change the POSIX query, Kernel, fault ABI or IPC fastpath.
Noncontiguous/aliased outputs retain scalar argument order. If the combined
copy fails, scalar stores are retried in that same order, preserving successful
earlier outputs and the untouched suffix when a later pointer faults.
There is no new service request, dynamic allocation or persistent state.

Native tests use the real guest-copy helper and check one request for adjacent
outputs, exact bytes without SHM staging, reverse/alias order, null/overflow,
and each failing output both with and without an already-copied prefix.
The full native suite passed 1,053 tests (ten opt-in tests ignored), the focused
guest-copy ASan suite passed ten tests, and the Alter/Linux AArch64 Release
check passed. The real integer-only Linux ABI fixture also now exercises
getresuid/getresgid on contiguous/reversed outputs, null and protected-page
boundaries, including an unaligned u32 that straddles a protected page.
The performance workload itself was not changed or rebuilt for the comparison.

Four-vCPU QEMU/TCG, normal devices, profile-disabled images, identical extended
process fixture, before/after/after/before order. Both variants include the
cycle-7 VMA hints; the measured difference is the scalar-output helper and its
use in getresuid/getresgid. Three samples per workload per boot:

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 2,048 getresuid, ordinary VMAs | 503.650 | 264.877 | 274.584 | 496.985 |
| 2,048 getresuid, 128 extra VMAs | 530.804 | 266.998 | 291.728 | 505.329 |
| Eight fork/wait | 74.149 | 86.335 | 81.210 | 79.791 |
| Eight fork/exec/wait | 2105.129 | 2471.324 | 2303.828 | 2288.435 |
| BusyBox Alpha kill/reap (control) | 18.992 | 24.071 | 22.485 | 22.165 |

Pooled getresuid is 498.647 -> 267.335 ms (46.4% lower) at ordinary VMA count,
and 527.262 -> 269.049 ms (49.0% lower) with 128 additional VMAs. Every after
sample in these two workloads is lower than every corresponding before sample.
This clears the cycle's 5% target for these complete emulated syscall loops,
not for desktop FPS or application startup. The gain removes repeated
copy-request work, including validation and Frame lookup; it is not evidence
that Kernel IPC transport or the POSIX abstraction itself was inefficient.

Pooled fork/exec is instead 2187.956 -> 2395.351 ms (9.5% higher), while the
unchanged native kill/reap control is 20.569 -> 23.312 ms (13.3% higher).
Do not attribute these movements to the adjacent-output helper, ignore them,
or claim a process-startup improvement. Process startup remains a separate
measurement and optimization gate.

All four timed runs passed, including executable freshness. Logs:
`/private/tmp/nanami-opt-cycle8-{before1,after1,after2,before2}.log`,
`/private/tmp/nanami-opt-cycle8-native.log`,
`/private/tmp/nanami-opt-cycle8-asan.log` and
`/private/tmp/nanami-opt-cycle8-arm-alter.log`.

The updated integer-only ABI fixture passed in four-vCPU QEMU, followed by
Tiny Core init, stock dynamic BusyBox, private root/symlinks, fork/exec/pipes,
real procfs/ps, interactive job control, Ctrl-C and cleanup. Its image was
rebuilt only after all timed comparisons, so compiling or running these
additional checks did not overlap the performance runs. Log:
`/private/tmp/nanami-opt-cycle8-validation.log`.

The latest validation image also passed two full four-vCPU desktop sessions
in one boot, including real input, Terminal/Editor/Information, Exit/restart
and cleanup. Log: `/private/tmp/nanami-opt-cycle8-desktop.log`.

## Cycle 9: reap diagnosis and final VMA-hint acceptance check

The opt-in Alpha profiler now separates reap's existing stages. A diagnostic
four-vCPU process run found RAM-release work dominating the sampled reaps:

| Owned references per process | Samples | Root-revoke median ticks | RAM-release median ticks |
| ---: | ---: | ---: | ---: |
| 67 | 28 | 37,000 | 8,269,500 |
| 204 | 27 | 52,000 | 24,053,000 |
| 914 | 24 | 202,000 | 119,233,000 |

RAM release includes collecting releasable references and calling the physical
allocator for each extent. The source shows that every `PhysicalAllocator::free`
searched the complete global allocation Vec from the beginning. The stage data
are elapsed same-core counter ticks, not calibrated nanoseconds or exclusive
CPU time. They identify where to investigate, not IPC transport latency.
Log: `/private/tmp/nanami-opt-cycle9-profile.log` (its `Test logs:` directory
contains the complete `[reap.profile]` samples). This diagnostic image is not
used for before/after performance comparisons.

For the VMA-hint decision, retain the cycle-8 adjacent-output change in both
variants and extend the integer-only fixture with 256 additional PROT_NONE VMAs
as well as the original zero/128 cases. The configured VMA limit remains 512;
fixture creation/removal is outside each timed loop. Both variants use the
same extended guest binary, profile-disabled Alpha and normal devices.
Four-vCPU QEMU/TCG, before/after/after/before, three samples per workload per boot:

| Workload median (ms) | No cache 1 | Cache 1 | Cache 2 | No cache 2 |
| --- | ---: | ---: | ---: | ---: |
| 2,048 getresuid, ordinary VMAs | 277.046 | 275.501 | 290.421 | 270.205 |
| 2,048 getresuid, 128 extra VMAs | 277.018 | 277.493 | 297.632 | 282.139 |
| 2,048 getresuid, 256 extra VMAs | 293.668 | 275.938 | 281.568 | 285.573 |
| Eight fork/exec/wait | 2334.520 | 2465.940 | 2542.733 | 2309.094 |
| BusyBox Alpha kill/reap (control) | 21.831 | 24.722 | 24.248 | 21.916 |

Pooled ordinary getresuid is 273.370 -> 282.816 ms (3.5% higher), 128-VMA is
280.255 -> 279.804 ms (0.2% lower), and 256-VMA is 289.620 -> 278.252 ms
(3.9% lower). Fork/exec is 2321.807 -> 2476.387 ms (6.7% higher), while the
unchanged native kill/reap control also moves. These results do not establish
a consistent 5% benefit, and the larger-case local gain does not justify
ignoring the other results or retaining 32 bytes per VMA table and extra cache
invalidation machinery on their strength.

Decision: remove the VMA-hint implementation and restore the original predicate
and MappingTable representation. Keep the additional protection, alias and
unsorted-range regressions. The removed candidate and its historical benchmark
are archived at `/private/tmp/nanami-opt-cycle9-vma-cache-rejected.patch`.
All four process runs passed including executable freshness; logs:
`/private/tmp/nanami-opt-cycle9-vma-{before1,after1,after2,before2}.log`.

## Cycle 10: newest-allocation fast lookup for physical release

PhysicalAllocator kept a Vec of allocation records and searched it from the
beginning for every free. Process physical-ownership references are inserted
in creation order and enumerated newest-first for ordinary cleanup. Releasing
one such page still skipped the long-lived allocations for every process.

Keep the Vec, allocation/free order, buddy pools, ownership and error rules.
First check whether the final record is the requested non-reserved extent;
otherwise use the original full checked search. There is no index, cache,
capacity change, extra allocation or persistent metadata. Partial extents and
older frees retain the fallback. This improves the frequent newest-first case;
it does **not** make arbitrary free order O(1) or remove its worst-case linear
search. No Kernel or IPC fastpath code changes.

New tests compile the real allocator and cover 4,096-page newest/oldest order,
head/middle/tail partial frees, a moved record after swap-remove, invalid and
reserved ranges, RAM/device pool separation and 20,000 mixed operations against
an independent page/extent model. The full native suite passed 1,059 tests
(11 opt-in tests ignored), focused ASan passed 120 (four opt-in tests ignored),
and the AArch64 Alpha Release check passed. The original allocator also passed
the same six correctness tests through `NANAMI_PHYSICAL_ALLOCATOR_SOURCE`.

A Release host benchmark builds the real old/new allocator from the respective
source, creates long-lived one-page allocations, allocates a child cohort,
then times only its newest-first release. Three samples; no Kernel, capabilities
or IPC. Fixture construction and memory-info checks are outside the timer:

| Long-lived allocations | Child pages | Old median | New median |
| ---: | ---: | ---: | ---: |
| 4,096 | 204 | 515.833 us | 4.292 us |
| 4,096 | 914 | 2592.709 us | 17.458 us |
| 16,384 | 204 | 2065.209 us | 3.917 us |
| 16,384 | 914 | 16684.000 us | 18.125 us |

The old larger sample varies appreciably; these are local bookkeeping timings,
not a whole-system improvement ratio. The ignored `physical-allocator` test
`recent_process_pages_among_long_lived_allocations` reproduces the benchmark.
The preserved baseline source is `/private/tmp/nanami-opt-cycle10-allocator-before.rs`.

Whole-process comparison: four-vCPU QEMU/TCG, normal devices, profiling disabled,
identical extended zero/128/256-VMA fixture, sequential before/after/after/before.
Both variants have the cycle-8 adjacent-output optimization and **no** VMA
permission cache. The isolated difference is the physical free lookup.
Three samples per workload per boot; medians in milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight Alpha construction + kill/reap | 231.574 | 33.599 | 37.086 | 205.592 |
| BusyBox Alpha construction only | 3.893 | 3.728 | 3.734 | 3.693 |
| BusyBox Alpha kill/reap | 24.905 | 0.431 | 0.385 | 22.104 |
| Xfe Alpha kill/reap | 57.860 | 0.769 | 0.723 | 52.466 |
| Eight fork/wait | 99.658 | 23.503 | 23.767 | 95.035 |
| Eight fork/exec/wait | 2502.783 | 2038.345 | 1964.269 | 2327.577 |
| 2,048 getresuid, ordinary VMAs | 280.101 | 290.061 | 319.540 | 270.982 |
| 2,048 getresuid, 128 extra VMAs | 282.944 | 303.317 | 327.368 | 271.592 |
| 2,048 getresuid, 256 extra VMAs | 314.666 | 312.888 | 321.346 | 267.250 |

Pooled Alpha construction+cleanup is 223.827 -> 36.188 ms (83.8% lower),
BusyBox kill/reap 23.454 -> 0.392 ms (98.3% lower), Xfe kill/reap 55.282 ->
0.752 ms (98.6% lower), fork/wait 96.386 -> 23.635 ms (75.5% lower), and
fork/exec/wait 2430.637 -> 2009.157 ms (17.3% lower). Both after-boot medians
for fork/exec are below both before-boot medians; teardown/fork samples are
well separated. The cycle's 5% target is observed for these lifecycle workloads.
Construction alone is 3.872 -> 3.731 ms and overlaps; do not describe the
teardown reduction as a 98% improvement to construction, application startup,
or all desktop work.

The unrelated copyout loops instead rise by about 10.0%, 14.3% and 10.0%
respectively. Their implementation and request count are unchanged here, but
that fact is not evidence that the measured regressions can be ignored or
attributed to host noise. Follow-up measurement is required; this is not a
claim that every syscall or overall throughput improved uniformly.

All four process runs passed, including executable freshness. The latest
four-vCPU desktop regression passed both sessions in one boot, including
real input, Terminal/Editor/Information, Exit/restart and cleanup. Logs:
`/private/tmp/nanami-opt-cycle10-{before1,after1,after2,before2}.log`,
`/private/tmp/nanami-opt-cycle10-{before,after}-bench.log`,
`/private/tmp/nanami-opt-cycle10-before-unit.log`,
`/private/tmp/nanami-opt-cycle10-final-native.log`,
`/private/tmp/nanami-opt-cycle10-final-asan.log`,
`/private/tmp/nanami-opt-cycle10-arm-alpha.log` and
`/private/tmp/nanami-opt-cycle10-desktop.log`.

Next gates: measure real Tiny Core desktop startup through its fresh
`desktop-ready` marker (not host keyboard automation); recheck ordinary
copyout with matched controls; and evaluate older/interleaved process frees
separately rather than calling the allocator generally constant-time.

## Cycle 11: real desktop startup and isolated syscall controls

The new `--tinycore-startup-perf` probe calls Negoto's actual OCI create/start
service operations for the existing Tiny Core desktop bundle. Timing ends only
after reading the exact `ready` bytes written by an aterm child after its X11
connection. Each session removes the previous marker before starting, checks
that the context is still running, then signals, stops and deletes it before
reusing the same container ID. Teardown, UART reporting, probe initialization
and host keyboard automation are outside the timed region. This measures real
session initialization through the first terminal connection, not completed
rendering of every client, input latency or graphical FPS. Polling every 100 ms
keeps the higher-priority observer from starving the desktop and limits timing
resolution accordingly.

Both variants use the same probe and rootfs fixture, with profiling disabled.
The only production difference is cycle 10's newest-allocation free lookup.
Four-vCPU QEMU/TCG, normal devices, sequential before/after/after/before boots:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session in a fresh boot | 16556.778 | 12725.842 | 12509.773 | 16631.501 |
| Second session | 18580.347 | 13640.145 | 13521.539 | 18718.022 |
| Third session | 18905.767 | 13719.618 | 13722.246 | 18707.422 |

First-session medians are 16594.140 -> 12617.808 ms (24.0% lower). Pooling
only the four later sessions per variant gives 18712.722 -> 13679.882 ms
(26.9% lower). All 12 sessions completed real X11 startup and forced-stop /
delete / same-ID reuse. This supplies the application-startup evidence that
cycle 10's construction/reap microbenchmarks alone could not establish.

Images: `/private/tmp/nanami-opt-cycle11-{before,after}.img`. Logs in execution
order: `/private/tmp/nanami-opt-cycle11-startup-before1.log`,
`/private/tmp/nanami-opt-cycle11-startup-after2.log`,
`/private/tmp/nanami-opt-cycle11-startup-after3.log`,
`/private/tmp/nanami-opt-cycle11-startup-before2.log`. Each log names its full
serial-output directory. The earlier `startup-after1` run was exploratory and
is not included in the table, because the control fixture was rebuilt afterwards.

For the outstanding copyout question, the integer-only process fixture now
includes fixed 2,048-call getpid and getuid controls, then the same checked
getresuid outputs with zero/128/256 extra VMAs, before fork/exec changes the
process tree. This separates basic emulation, the existing POSIX credential
query and the additional checked copyout. It changes benchmark order, not
production syscall behavior; new results must not silently replace the older
post-fork measurements. Both variants use the identical updated fixture.

The x86_64 checked-copy request handler's disassembly is identical between the
two Alpha binaries. Copy transfer and current-frame resolution have the same
instructions except relocated calls to the same memcpy/memmove functions.
This rules out an added branch in those local handlers, not a regression in
their entire service/scheduling path. Binaries and disassembly are retained as
`/private/tmp/nanami-opt-cycle11-{before,after}-alpha.elf`,
`/private/tmp/nanami-opt-cycle11-{before,after}-copy.asm` and
`/private/tmp/nanami-opt-cycle11-{before,after}-transfer.asm`.

Updated process fixture, three samples per boot, before/after/after/before:

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid, 2,048 calls | 38.000 | 38.539 | 39.499 | 37.175 |
| getuid, 2,048 calls | 155.320 | 152.795 | 149.852 | 156.947 |
| getresuid, ordinary VMAs | 269.270 | 281.456 | 270.886 | 271.117 |
| getresuid, 128 extra VMAs | 275.340 | 283.351 | 271.530 | 270.862 |
| getresuid, 256 extra VMAs | 277.373 | 274.056 | 273.690 | 277.327 |
| Eight fork/wait | 82.345 | 23.573 | 21.672 | 83.126 |
| Eight fork/exec/wait | 2283.258 | 1870.318 | 1863.812 | 2286.755 |

Pooled ordinary copyout is 270.194 -> 276.171 ms (2.2% higher), 128-VMAs
274.655 -> 272.934 ms (0.6% lower), and 256-VMAs 277.350 -> 273.775 ms
(1.3% lower). The getpid control rises 3.0% and getuid falls 3.1%. This does
not reproduce the earlier 10-14% increases under this isolated fixture order,
but is not a fix or explanation of the older post-fork measurements.
Fork/wait is 82.736 -> 23.464 ms (71.6% lower); fork/exec/wait is 2285.007 ->
1867.934 ms (18.3% lower). All four runs completed the executable-freshness
checks. Logs: `/private/tmp/nanami-opt-cycle11-process-{before1,after1,after2,before2}.log`.

The physical-free host benchmark now prepares newest-first, oldest-first and
alternating-end release orders outside its timed region. It keeps the same
4,096/16,384 long-lived records and 204/914 child pages and checks that every
sample returns the expected free-page total. Sequential before/after/after/
before executions use the same production allocator with only the lookup
changed. For 16,384 long-lived records and 914 child pages, pooled medians:

| Release order | Before (ms) | After (ms) |
| --- | ---: | ---: |
| Newest first | 9.422 | 0.009 |
| Oldest first | 8.910 | 4.605 |
| Alternating ends | 8.908 | 8.733 |

The alternating-end case still scales with unrelated records and shows no
5% gain. Individual smaller cases vary substantially between executions;
these host-only numbers do not establish general allocator latency, IPC
latency or desktop speedup. Logs: `/private/tmp/nanami-opt-cycle11-free-{before,after,after2,before2}.log`.

To avoid attributing the copyout difference to the changed fixture order,
repeat the original post-fork fixture on the original cycle 10 images:
`cycle9-no-cache.img` versus `cycle10-after.img`, again before/after/after/before.

| Original post-fork workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getresuid, ordinary VMAs | 272.948 | 274.479 | 269.438 | 273.318 |
| getresuid, 128 extra VMAs | 271.910 | 274.284 | 274.840 | 276.538 |
| getresuid, 256 extra VMAs | 272.812 | 271.811 | 277.013 | 275.297 |
| Eight fork/wait | 82.564 | 21.055 | 24.746 | 90.157 |
| Eight fork/exec/wait | 2255.726 | 1834.291 | 1863.765 | 2428.376 |

Pooled copyout is 273.133 -> 272.200, 275.486 -> 274.562, and 274.055 ->
273.856 ms, respectively (all within 0.4%). The original 10-14% increases
are not reproduced with those exact images and that exact fixture order.
Their cause remains unestablished; do not call a nonexistent copyout code
change a fix or attribute all variation to host noise. The lifecycle gains
repeat: fork/wait 87.647 -> 22.901 ms (73.9% lower) and fork/exec/wait
2342.855 -> 1850.090 ms (21.0% lower). All four runs completed executable
freshness checks. Logs: `/private/tmp/nanami-opt-cycle11-postfork-{before1,after1,after2,before2}.log`.

Final checks: 1,059 native tests passed (11 opt-in benchmarks ignored), six
physical-allocator AddressSanitizer tests passed (one benchmark ignored), both
release-order benchmark variants passed every free-page assertion, Python
syntax/argument checks passed, and `git diff --check` is clean. Source changes
in this cycle add measurement and regression coverage; the retained production
optimizer is unchanged from cycle 10. A9N source and IPC fastpath remain
unchanged. Logs: `/private/tmp/nanami-opt-cycle11-final-native.log` and
`/private/tmp/nanami-opt-cycle11-final-asan.log`.

Next: profile the real desktop startup rather than inferring it from generic
syscall counts. The remaining ordinary-fork VMA inheritance loop also calls
`add_mapping` once per already validated parent entry, rescanning a growing
child table; evaluate an independent active-prefix copy without changing
CLONE_VM aliasing or permission/failure rules. Continue observing copyout
separately from lifecycle gains.

## Cycle 12: independent fork VMA metadata initialization

Ordinary fork previously replayed every parent VMA through `add_mapping` on
the growing child table. That repeatedly searched and coalesced metadata
that the parent had already validated. Initialize the child's independent
table by copying only the active parent prefix instead: VMA bookkeeping is
linear in the inherited entry count rather than quadratic merge work.
Physical-page cloning, file inheritance, signal state, clone TID stores,
resume ordering and failure cleanup remain in their existing paths.

`MappingTable::initialize_from` accepts only a fresh, unallocated destination.
An allocated table can belong to CLONE_VM peers even when its active length
is zero; accepting that destination could mutate unrelated shared state.
The ordinary-fork caller already installs `MappingTable::EMPTY`, and allocation
failure leaves it empty and retryable. Empty sources remain allocation-free;
nonempty sources still require one allocation of the existing Storage type.
Neither the one-Word table handle, its capacity, process snapshot size nor
per-process metadata layout grows. CLONE_VM still explicitly shares the
original table through `share_task_resources`, not this copy operation.
No service operation, ABI, Kernel source or IPC fastpath changes.

Native coverage runs the real MappingTable and Runtime mapping methods. It
checks full-capacity independent copies, parent/child permission changes and
release, unsorted overlapping metadata and exact ordering, empty-source lazy
allocation, rejection of existing/shared destinations, invalid PIDs, forced
allocation failure and retry. Existing shared-table and differential permission
tests remain. The final full suite passes 1,063 tests (12 opt-in benchmarks
ignored), focused AddressSanitizer suites pass 50 tests, and Alter/Linux's
AArch64 Release check passes. Logs:
`/private/tmp/nanami-opt-cycle12-final2-{native,asan,arm}.log`.

The opt-in host test repeats 256 inheritance/release operations per sample,
comparing the actual bulk initialization with the former per-entry
registration algorithm. Three samples, median microseconds per inheritance,
including the same zeroed Storage allocation and release:

| Active VMAs | Repeated registration | Bulk initialization |
| --- | ---: | ---: |
| 8 | 0.250 | 0.280 |
| 32 | 0.501 | 0.297 |
| 128 | 5.514 | 0.292 |
| 512 | 74.021 | 0.376 |

There is no established small-table gain; do not extrapolate the large-table
ratios to application startup or all fork work. This benchmark excludes IPC,
page copying and emulated process scheduling. Log:
`/private/tmp/nanami-opt-cycle12-final-vma-bench.log`.

The Tiny Core integer-only process fixture now additionally keeps 128 or
256 fragmented PROT_NONE VMAs live while performing eight fork/wait cycles.
Every child checks EFAULT for checked copyout into an inherited unbacked
reservation before exit. Creation/removal of these VMAs remains outside the
timed loops. The same guest, validation work, workloads, mapping limit and
normal-device/four-vCPU configuration are used for both variants.

Before tightening the destination precondition, the first candidate's
real desktop startup comparison completed all 12 X11 sessions in sequential
before/after/after/before boots. First-session medians were 12864.296 ->
12769.071 ms; later-session medians 13675.983 -> 13876.161 ms. This does not
show a 5% desktop improvement. Those measurements describe the initial
candidate, not the final binary; they are retained rather than being used to
claim a graphical speedup. Logs:
`/private/tmp/nanami-opt-cycle12-startup-{before1,after1,after2,before2}.log`.
The initial process comparison is in `cycle12-process-*` logs; the final
candidate is measured again separately after the precondition review.

Final-code comparison: `/private/tmp/nanami-opt-cycle12-before.img` versus
`/private/tmp/nanami-opt-cycle12-final.img`, sequential before/after/after/
before boots, three samples per workload per boot, profilers disabled:

| Workload median (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Eight ordinary fork/wait | 22.092 | 21.813 | 22.000 | 23.471 |
| Eight fork/wait, 128 extra VMAs | 23.837 | 22.514 | 24.584 | 24.208 |
| Eight fork/wait, 256 extra VMAs | 25.928 | 25.565 | 22.853 | 26.382 |
| Eight fork/exec/wait | 1976.315 | 1857.748 | 1863.934 | 1864.968 |
| getpid, 2,048 calls | 37.876 | 39.962 | 38.214 | 37.862 |
| getuid, 2,048 calls | 153.693 | 155.327 | 151.898 | 153.410 |
| getresuid, ordinary VMAs | 271.882 | 270.436 | 272.380 | 271.841 |
| getresuid, 128 extra VMAs | 276.599 | 270.252 | 271.256 | 272.374 |
| getresuid, 256 extra VMAs | 277.365 | 272.417 | 273.537 | 280.772 |

Pooled eight-fork latency is 23.159 -> 21.907 ms for ordinary VMAs (5.4%
lower), 24.023 -> 23.692 ms for 128 extras (1.4% lower), and 25.946 ->
22.865 ms for 256 extras (11.9% lower). The 256-VMA case exceeds the cycle's
5% target, with both after-boot medians below both before-boot medians, but
the first after boot overlaps some individual before samples. This is not
proof of a 5% improvement for all fork work. Fork/exec is 1937.147 ->
1862.770 ms (3.8% lower). Ordinary checked copyout is effectively unchanged:
271.862 -> 271.938 ms; the extra-VMA copyout cases are 0.9% and 1.8% lower.
getpid rises 3.2%, while getuid falls 0.6%.

Unchanged Alpha-only control work also varies: eight construction/reap cycles
34.162 -> 33.523 ms, BusyBox construction alone 3.598 -> 3.657 ms, Xfe
construction 9.308 -> 8.648 ms. Negoto create is 7.488 -> 7.257 ms. These
paths do not execute the changed fork VMA initializer; do not attribute every
movement to it. Alpha's current executable also compares byte-identical to
the retained cycle 11 Alpha. All four runs complete executable freshness and
child protection checks. Logs:
`/private/tmp/nanami-opt-cycle12-final-process-{before1,after1,after2,before2}.log`.

Retain the small ordinary-fork implementation for independent, linear metadata
inheritance and the measured fragmented-fork improvement. It adds no cache,
index, capacity reduction or new service path. Overall desktop startup remains
a separate optimization target, rather than being inferred from these results.

The final image passes the full four-vCPU desktop regression twice in one
boot: raw Linux desktop syscall/namespace/file-mapping fixtures, real X11
input, Terminal/Editor/Information launches, Exit, fresh restart and complete
context cleanup. Log: `/private/tmp/nanami-opt-cycle12-final-desktop.log`;
full serial output and screenshots:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-nfqeiu6c`.
The final source matches the source used to build that image, and
`git diff --check` and the A9N working tree are clean.

## Cycle 13: profile real startup; reject pooled initialization-source hints

First run the existing real desktop startup fixture with
`NANAMI_EXEC_PROFILE=1 NANAMI_ALTER_PROFILE=1`. Default builds keep both
disabled. Three sessions completed. Summing the emitted Alter flushes gives:

| Syscall | Calls | Handler elapsed counter ticks |
| --- | ---: | ---: |
| mmap (9) | 2,448 | 6,950,183,000 |
| execve (59) | 76 | 5,682,077,000 |
| clone (56) | 71 | 3,632,236,000 |
| openat (257) | 1,540 | 699,396,000 |
| wait4 (61) | 123 | 653,001,000 |

These are global totals flushed on ordinary guest exits, not per-PID ownership
or a complete exclusive CPU account. The last force-killed clients' unflushed
tail can be absent. Samples include service waits, interrupts and descheduling;
do not infer expensive IPC transport from them or compare diagnostic-build
startup latency with the normal-build performance results below.

Alpha separately emitted 81 exec reports: drop-old-mappings 5,782,920,000
ticks out of 6,251,917,000 reported stage ticks (about 92.5%). This stage
includes VM bookkeeping reset, UNMAP, cap removal, physical free and deferred
release. Its total alone does not identify which substep dominates. Alpha and
Alter counter domains must not be pooled. Diagnostic image/log:
`/private/tmp/nanami-opt-cycle13-profile.img`,
`/private/tmp/nanami-opt-cycle13-profile.log`; complete serial:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-v_zqvkcf`.

The candidate reused the existing stable pool initialization window for owned
bulk source pages instead of rotating every source through the smaller
process-copy window. It added no persistent cache or extra payload copy.
A request-local ownership snapshot was limited to sources above one page and
at most 4 MiB; larger inputs retained the original path rather than acquiring
a new API limit. Every source still resolved its current Frame; mismatched or
foreign ownership, small inputs and a source/destination cache-slot collision
kept the existing copy-window fallback. The pinned destination could not be
evicted while its bytes were being initialized.

The candidate passed 1,072 native tests (12 benchmarks ignored), 89 focused
AddressSanitizer tests (two benchmarks ignored), the AArch64 Alpha Release
check and all 12 timed X11 sessions. Added tests exercised current-frame
replacement, foreign pool references, shared-VM ownership, nonaligned source
chunks, collision/MAP/UNMAP failure handling and input beyond the hint budget.
Correctness and fewer potential mapping operations were not the speed gate.

Normal builds, identical guest fixture, four-vCPU QEMU/TCG with normal devices,
sequential before/after/after/before boots:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 12291.372 | 12497.303 | 12612.739 | 12810.249 |
| Second session | 13608.800 | 13485.024 | 13623.435 | 13847.475 |
| Third session | 13499.723 | 13716.703 | 13548.793 | 13713.586 |

First-session medians are 12550.811 -> 12555.021 ms (0.03% higher);
later-session medians 13661.193 -> 13586.114 ms (0.55% lower). This does not
establish a meaningful startup improvement or the cycle's 5% target. Do not
attribute the remaining time to a component merely because this candidate
failed to improve it. Logs:
`/private/tmp/nanami-opt-cycle13-startup-{before1,after1,after2,before2}.log`.

Decision: reject the extra source-window/ownership-hint implementation and
its candidate-specific mocks/tests. The retained production implementation
is restored exactly to cycle 12. The rejected patch is saved at
`/private/tmp/nanami-opt-cycle13-source-window-rejected.patch`; its tested
candidate image remains `/private/tmp/nanami-opt-cycle13-after.img` and is
not the final image. Default output binaries were rebuilt after restoring:
`/private/tmp/nanami-opt-cycle13-restored.img`. Alpha compares byte-identical
to the prior validated default Alpha executable. Restored validation passes
1,063 native tests (12 benchmarks ignored), 80 focused ASan tests (two
benchmarks ignored), and the AArch64 Alpha Release check. Logs:
`/private/tmp/nanami-opt-cycle13-restored-{build,native,asan,arm}.log`.
A9N and IPC fastpath remain unchanged.

Next: split the measured exec-drop stage into reset, UNMAP, cap removal,
physical free and deferred-release costs before choosing the next change.
The allocator already uses TLSF for Alpha heap bookkeeping; do not replace
it again. The separate physical allocator still scans its allocation Vec
when the newest-record hint misses, as the earlier mixed-order host cases
also demonstrate. If runtime substage data confirms that scan as dominant,
evaluate a compact exact-base index without adding duplicate per-record
metadata: an intrusive u32 chain link may fit the existing 64-bit record
padding, with a small bucket array. Preserve partial-free fallback, reserved
records, RAM/device separation, swap-remove updates and recoverable index
allocation failure. Index encoding must remain an optimization, not impose
a new allocation-count limit. Measure metadata bytes as well as latency.

## Cycle 14: compact physical-allocation lookup index

Split Alpha's opt-in exec-drop report into runtime reset, page UNMAP, cap
removal, physical RAM release and deferred release. Three real desktop startup
sessions emitted 81 reports covering 99,267 references:

| Substage | Elapsed local counter ticks |
| --- | ---: |
| Runtime reset | 46,321,000 |
| UNMAP | 86,221,000 |
| Cap removal | 59,951,000 |
| Physical RAM release | 5,432,306,000 |
| Deferred release | 739,009,000 |

Physical release accounts for about 85.4% of the reported substage sum;
UNMAP plus cap removal accounts for about 2.3%. These are elapsed diagnostic
samples, including interrupts and descheduling, not exclusive CPU time or
nanoseconds. Deferred release can also free RAM. The RAM stage includes
MemoryManager's existing delegation checks, so this table alone does not
attribute all of it to the allocation Vec. The previous newest-record hint's
mixed-order host results motivate testing that specific scan next.
Diagnostic image/log: `/private/tmp/nanami-opt-cycle14-profile.img` and
`/private/tmp/nanami-opt-cycle14-profile.log`; full serial:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-c49ox0w4`.

The candidate retains the authoritative allocation Vec and both buddy pools.
Exact-base lookups use a chained bucket index after the newest-record hint.
Interior partial frees and absent/unencodable index entries retain the original
validated linear fallback. Reserved records remain unfreeable; independent
adjacent extents are not merged. RAM/device placement and permission behavior
are unchanged. Removing a record unlinks both it and the last record before
swap-remove, then reindexes the moved record before any partial split records
are appended.

One-based u32 Vec indices use existing padding: AllocationRecord remains 24
bytes on both supported 64-bit architectures. The additional Vec descriptor
is 24 bytes per allocator. Bucket storage starts at 128 bytes and grows at
four records per bucket, using roughly one to two bytes per indexed record
between growth points. Requested bucket capacity is capped at 256 KiB, not
the number of allocations; reaching the cap merely increases chain lengths.
u32 link encoding exhaustion likewise leaves records eligible for the linear
fallback. A failed optional index allocation preserves existing chains and
does not fail an otherwise valid physical allocation. No new protocol, Kernel
operation, IPC fastpath, persistent ownership cache or page-copy work is added.

The public-API allocator suite preserves the baseline-source override, while
a separate private-invariant harness checks bucket growth, head/middle/tail
collisions, random swap-remove orders, partial splits, duplicate reserved
records, absent-index rebuild, initial/growth allocation failure and retry,
u32 encoding boundaries and more than 262,144 records at the bucket cap.
Full native validation passes 1,071 tests (12 opt-in benchmarks ignored),
focused AddressSanitizer suites pass 105 tests (three benchmarks ignored),
and Alpha's AArch64 Release check passes. Default x86_64 Alpha has no RDTSC
instructions or exec-profile symbols. Logs:
`/private/tmp/nanami-opt-cycle14-{native,asan,arm,build}.log`.

The isolated host release benchmark compares the prior newest-record hint
with the candidate, three samples per order. With 16,384 older live records
and 914 child-page releases, median bookkeeping time is:

| Release order | Before (ms) | Candidate (ms) |
| --- | ---: | ---: |
| Newest first | 0.010334 | 0.025625 |
| Oldest first | 4.606083 | 0.057250 |
| Alternating ends | 8.854042 | 0.083542 |

The already-fast tail case becomes slower, while mixed-order scans improve
substantially. These are only allocator bookkeeping measurements, excluding
Kernel calls, page copying and process construction. They do not decide
acceptance without matched real-startup and process-level measurements.
Logs: `/private/tmp/nanami-opt-cycle14-host-{before,after}.log`.

Default-build real startup comparison, identical guest fixture and normal
devices on four-vCPU QEMU/TCG, sequential before/after/after/before boots:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 12500.009 | 6624.193 | 6530.308 | 12512.814 |
| Session 2 | 13726.582 | 7215.194 | 7012.220 | 13432.978 |
| Session 3 | 13634.147 | 7055.490 | 7114.172 | 13525.059 |

First-session medians are 12506.412 -> 6577.251 ms (47.4% lower);
later-session medians are 13579.603 -> 7084.831 ms (47.8% lower). Both after
boots are consistently below both before boots; all 12 X11 startup sessions
complete their kill/delete/same-ID reuse checks. This exceeds the cycle's 5%
startup target, not a claim that every syscall or arbitrary workload becomes
47% faster. The observer polls at 100 ms; host keyboard injection, UART reports
and session cleanup are outside the timed interval. Images:
`/private/tmp/nanami-opt-cycle13-restored.img` (before),
`/private/tmp/nanami-opt-cycle14-after.img` (candidate). Logs:
`/private/tmp/nanami-opt-cycle14-startup-{before1,after1,after2,before2}.log`.

The same images then ran sequential before/after/after/before process-fixture
boots, with no concurrent VM, build, test or host benchmark. Each row is the
per-boot median in milliseconds (three samples except six Negoto creates):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 23.263 | 21.761 | 22.426 | 22.596 |
| fork-128-vmas-8 | 22.699 | 22.507 | 22.848 | 22.775 |
| fork-256-vmas-8 | 22.931 | 22.714 | 21.767 | 22.622 |
| fork-exec-8 | 1853.218 | 789.637 | 786.301 | 1963.457 |
| getpid-2048 | 39.411 | 38.200 | 39.752 | 38.653 |
| getuid-2048 | 154.551 | 155.286 | 157.211 | 152.488 |
| copyout-normal | 267.797 | 273.828 | 273.629 | 273.226 |
| copyout-128-vmas | 273.423 | 275.786 | 275.057 | 273.398 |
| copyout-256-vmas | 273.597 | 278.306 | 276.175 | 271.596 |
| alpha-spawn-reap-8 | 32.111 | 32.888 | 32.213 | 32.056 |
| alpha-breakdown-spawn | 3.841 | 3.640 | 3.459 | 3.600 |
| alpha-breakdown-reap | 0.398 | 0.386 | 0.378 | 0.425 |
| alpha-xfe-spawn | 8.260 | 8.397 | 8.341 | 8.344 |
| alpha-xfe-reap | 0.627 | 0.771 | 0.637 | 0.541 |
| negoto-create | 6.822 | 7.744 | 7.476 | 8.218 |

Pooling the two boots of each variant, eight fork/exec/wait operations fall
from 1916.363 to 787.969 ms (58.9% lower). Ordinary fork/wait falls 3.6%; the
extra-VMA fork cases change by less than 1%. These results do not establish
another 5% improvement for ordinary fork metadata or pure construction.
Alpha construction/reap as a whole rises 1.5%, isolated BusyBox construction
falls 1.3%, and isolated Xfe construction rises 0.4%. Negoto create falls 1.0%.
getpid, getuid and the three checked-copyout cases rise by 1.4%, 2.2%, 1.1%,
0.7% and 2.0%, respectively; keep them as observed control variation, not
as evidence of a syscall acceleration. All runs finish protection/freshness
checks. Logs:
`/private/tmp/nanami-opt-cycle14-process-{before1,after1,after2,before2}.log`.

The tail-index maintenance tradeoff remains visible: Xfe-only kill/reap rises
from 0.551 to 0.680 ms (23.4%, about 0.13 ms). BusyBox-only kill/reap instead
falls 0.412 to 0.384 ms. Do not hide this isolated regression behind the
desktop/fork-exec gains or claim all destruction paths improve. The candidate
is selected for the substantial real desktop and exec improvement, with this
small absolute teardown cost explicitly retained for subsequent investigation.

The candidate also passes the full default-device, four-vCPU desktop
regression twice in one boot: desktop syscall fixtures, X11/FLWM/wbar,
real keyboard/mouse input and window dragging, Terminal/Editor/Information
launches, Exit and fresh restart with complete execution-context cleanup.
Log: `/private/tmp/nanami-opt-cycle14-desktop.log`; full serial and screenshots:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-lxnqb37b`.
The actual input screenshot was also visually inspected. Production allocator
source has not changed since the tested image build. Retain the compact index
for the measured real-startup and fork/exec improvement, with existing
ownership/permission behavior and the explicit teardown tradeoff above.

Finally repeat the same Alpha-only diagnostic configuration (Alter profiling
disabled). It emits the same 81 exec-drop reports and 99,267 references:

| Exec-drop substage | Before ticks | After ticks |
| --- | ---: | ---: |
| reset | 46,321,000 | 42,008,000 |
| unmap | 86,221,000 | 66,607,000 |
| remove | 59,951,000 | 52,155,000 |
| ram | 5,432,306,000 | 30,454,000 |
| deferred | 739,009,000 | 5,593,000 |

The physical-release sample sum falls from 5,432,306,000 to 30,454,000
counter ticks. This corroborates the targeted scan removal; it is not an
additional normal-build speed ratio. The two diagnostic runs are not an
ABBA timing comparison, and other stage movements cannot all be assigned to
the index. After-index broad exec reports sum to drop-old-mappings
318,931,000, address-space creation 5,137,000, planning 990,000, image-page
materialization 390,465,000 and stack-page materialization 129,978,000 ticks.
The broad drop stage also includes the new diagnostic report's UART cost;
do not mistake that reporting overhead for remaining normal-build work.
Log/image: `/private/tmp/nanami-opt-cycle14-profile-after{.log,.img}`;
full serial:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-qmogee5q`.

Next: measure the remaining mmap/exec page-materialization work with the
profiling overhead kept outside the sampled intervals. Keep pure construction,
copyout, and the small tail-release case as separate controls rather than
assuming the desktop startup improvement applies to them.

Both profilers are disabled again in the final default build:
`/private/tmp/nanami-opt-cycle14-final.img`, build log
`/private/tmp/nanami-opt-cycle14-final-build.log`. Restored Alpha is byte-identical
to the default candidate used for all normal performance and GUI checks
(SHA-256 `390fc358d5f8c8e955ff519f9e5e786c799962bedbaaa0374026b67cfb08ff91`)
and contains no RDTSC instructions. The allocator source hash remains
`212321cc9cc3327eccd1abf1e72dccfcd1a78f05dab163994d660940e1f9d8bf`.
Final `git diff --check` passes, A9N's working tree is clean, and no test VM
or build process remains live.

## Cycle 15: final permissions for internal private-page cloning

Refresh the real desktop diagnosis after cycle 14 with both existing opt-in
profilers. The emitted Alter flushes include mmap (2,152 calls / 5,892,265,000
handler counter ticks), clone (57 / 2,916,944,000) and exec (62 / 1,038,866,000).
These are partial global totals flushed on ordinary exits; force-killed tails
can be absent. They include service work and descheduling, not exclusive CPU
or IPC transport cost. Do not compare this diagnostic build's elapsed startup
time with normal performance samples. Log/image:
`/private/tmp/nanami-opt-cycle15-profile-before{.log,.img}`; full serial:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-i7anh45h`.

Source review finds that internal private-page cloning creates a new page
with ALL permissions, looks up the Frame again, then changes permissions to
the already-known source permissions. The candidate passes those permissions
into the shared private-page constructor instead. Existing destination pages
retain the original checked copy/protect path, and sparse virtual holes stay
unpopulated. This is not cycle 1's rejected protocol extension: no service
operation, request word, Kernel ABI or IPC fastpath is added. Anonymous/file
mmap callers retain the existing default constructor and protocol; this
candidate does not claim to optimize every mmap path.

The strict memory mapper accepts an explicit Attribute through an internal
helper while retaining its default-ALL entry point and strict error behavior.
The constructor validates permission bits before mutation, initializes the
same bytes once, then publishes the new leaf with its final permissions.
For retained PROT_NONE backing it ensures the intermediate page tables and
records ownership/zero permissions without publishing an accessible leaf.
Ensuring those tables matters: record_frame updates the last-region readiness
hint, which must not claim that nonexistent intermediate tables are ready.
Failed nonzero MAPs still record the potentially installed leaf before
rollback, with nonzero permissions so cleanup must confirm UNMAP. Existing
partial allocation ownership and deferred-reuse rules are unchanged.

The real constructor's tests cover all permission masks 0..7 and exact copied
contents, NONE backing across a 2-MiB boundary and a subsequent mapping using
the table hint, invalid permission rejection before allocation/reservation,
partial installed-leaf failures and NONE page-table creation failure with
complete ownership accounting. The real MemoryManager fixture also checks
the new strict helper's READ/RW/RX attributes. Full native validation passes
1,076 tests (12 benchmarks ignored); focused ASan suites pass 104 tests
(two benchmarks ignored); Alpha's AArch64 Release check passes. Logs:
`/private/tmp/nanami-opt-cycle15-{native,asan,arm,build}.log`.

Normal-build process comparison, four-vCPU QEMU/TCG with the same guest fixture
and normal devices, sequential before/after/after/before boots with no other
VM/build/test/benchmark running. Per-boot medians, milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 21.670 | 21.398 | 20.950 | 21.020 |
| fork-128-vmas-8 | 22.102 | 22.062 | 21.524 | 22.401 |
| fork-256-vmas-8 | 21.779 | 23.804 | 22.536 | 22.801 |
| fork-exec-8 | 765.705 | 770.252 | 780.966 | 779.793 |
| getpid-2048 | 38.715 | 37.954 | 39.135 | 37.768 |
| getuid-2048 | 153.775 | 156.796 | 156.333 | 153.630 |
| copyout-normal | 273.111 | 272.091 | 271.239 | 271.130 |
| copyout-128-vmas | 277.124 | 275.284 | 274.609 | 274.635 |
| copyout-256-vmas | 274.201 | 273.706 | 278.458 | 276.362 |
| alpha-spawn-reap-8 | 34.546 | 33.598 | 32.769 | 35.396 |
| alpha-breakdown-spawn | 4.080 | 3.592 | 3.521 | 3.687 |
| alpha-breakdown-reap | 0.430 | 0.450 | 0.367 | 0.485 |
| alpha-xfe-spawn | 8.046 | 8.058 | 8.687 | 8.453 |
| alpha-xfe-reap | 0.711 | 0.542 | 0.676 | 0.659 |
| negoto-create | 7.204 | 7.780 | 6.764 | 6.790 |

Pooling the six samples per variant, ordinary fork/wait is 21.377 -> 21.174 ms
(0.9% lower), fork/exec/wait is 774.049 -> 780.747 ms (0.9% higher), 128-extra-VMA
fork is 22.252 -> 21.882 ms (1.7% lower), and 256-extra-VMA fork is
21.874 -> 23.221 ms (6.2% higher). The targeted cloning change does not establish
a 5% process-workload improvement. Unchanged Alpha construction/reap controls
also move (34.971 -> 33.183 ms), so do not count their 5.1% movement as proof
that the clone optimization works. Likewise the BusyBox construction control's
7.5% reduction does not execute the new private-page clone entry point.
getpid and ordinary/128-extra-VMA copyout are less than 1% lower; getuid and
256-extra-VMA copyout are 1.9% and 0.8% higher. All runs pass freshness and
protection checks. Logs:
`/private/tmp/nanami-opt-cycle15-process-{before1,after1,after2,before2}.log`.

Real desktop startup, same normal builds/fixtures/devices and sequential
before/after/after/before order, milliseconds:

| OCI create/start to X11 client | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6520.508 | 6512.536 | 6528.661 | 6538.468 |
| Session 2 | 7119.510 | 7013.594 | 7111.593 | 7128.226 |
| Session 3 | 7125.727 | 7124.804 | 7121.393 | 7122.754 |

First-session medians are 6529.488 -> 6520.599 ms (0.14% lower), and later
sessions are 7124.241 -> 7116.493 ms (0.11% lower). The differences are below
the observer's 100-ms polling interval and do not establish a meaningful
startup gain. All 12 X11 sessions complete kill/delete/same-ID reuse checks.
Logs: `/private/tmp/nanami-opt-cycle15-startup-{before1,after1,after2,before2}.log`.
Before image: `/private/tmp/nanami-opt-cycle14-final.img`; tested candidate:
`/private/tmp/nanami-opt-cycle15-after.img`.

Decision: reject the candidate, including the private constructor/strict-map
extensions and their candidate-specific test changes. This cycle does not
meet the 5% target, and the 256-extra-VMA fork result is worse. Fewer potential
Kernel calls do not justify the extra implementation without a real speedup.
The candidate is archived at
`/private/tmp/nanami-opt-cycle15-final-permissions-rejected.patch`.
Restore all five touched source/test files exactly to the pre-candidate state;
cycle 14's retained physical-allocation index and earlier accepted changes
remain intact. The rejected image is not the final/default artifact.

Next: directly separate private-page construction into reservation/bookkeeping,
physical allocation, initialization-window lookup/MAP, byte initialization and
target publication before choosing another candidate. mmap remains the largest
emitted handler total, but that total alone is not proof of which substep
dominates. In particular, inspect allocator validation/carving work and repeated
frame/node lookups rather than treating the number of IPC operations as cost.

Restoration verification: all five source/test files compare byte-identical
to their pre-candidate snapshots. The rebuilt default Alpha also compares
byte-identical to the retained cycle 14 executable (SHA-256
`390fc358d5f8c8e955ff519f9e5e786c799962bedbaaa0374026b67cfb08ff91`).
Final normal image: `/private/tmp/nanami-opt-cycle15-restored.img`.
Restored validation passes 1,071 native tests (12 benchmarks ignored),
99 focused ASan tests (two benchmarks ignored), and the AArch64 Alpha Release
check. Logs: `/private/tmp/nanami-opt-cycle15-restored-{build,native,asan,arm}.log`.
`git diff --check` passes, A9N remains unchanged, and all test/build processes
have terminated. No rejected mapping implementation remains in production.

## Cycle 16: locate and evaluate page-initialization copy cost

Temporarily split private-page construction into reservation/bookkeeping,
physical allocation/cap copy, destination initialization-window mapping,
fill, and target publication. Accumulate per-request samples; emit aggregate
totals at exec/reap, not per page. A second diagnostic splits fill into current
source Frame lookup, source-window mapping, actual byte copy and suffix zeroing.
Kind 0 has no source, kind 1 is a one-page initializer with a source, and kind 2
has a source and multiple destination pages. Kind 1 is not synonymous with fork,
nor kind 2 with file mmap. Counters are elapsed local ticks including
descheduling and timestamp overhead, not exclusive CPU time or nanoseconds.

Both diagnostic runs complete three real X11 startup/reuse sessions. In the
more detailed run, 240 reports cover 40,273 anonymous pages, 92,241 source-one
pages and 152,641 source-bulk pages; all completed and no recorded request
failed. Early validation failures are outside these completion aggregates.
The emitted initializers include native Alpha work as well as Linux mappings.
The two profiling configurations have slightly different counts; do not pool
them or treat their differences as normal-build performance results.

| Detailed source work | Lookup ticks | Source-map ticks | Copy ticks | Zero ticks |
| --- | ---: | ---: | ---: | ---: |
| Source, one page | 24,129,000 | 491,470,000 | 1,815,666,000 | 9,162,000 |
| Source, multiple pages | 44,861,000 | 743,312,000 | 2,123,576,000 | 57,882,000 |

For source-one work, reservation/allocation/destination-window/publication
samples sum to 145,798,000 / 295,103,000 / 432,793,000 / 182,813,000 ticks;
for source-bulk they are 84,560,000 / 497,751,000 / 727,925,000 / 308,582,000.
The actual byte-copy interval is the largest sampled component, rather than
source lookup alone. This is diagnostic evidence to choose a candidate, not
an application speedup claim. Logs/images:
`/private/tmp/nanami-opt-cycle16-profile{.log,.img}` and
`/private/tmp/nanami-opt-cycle16-profile-detail{.log,.img}`; full serial:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-bsrjd_p9`
and `.../nanami-negoto-qemu-8e4y2qj9`. Temporary instrumentation is removed
from production and archived at
`/private/tmp/nanami-opt-cycle16-initialization-profile.patch`.

Generated code shows that the original compiler-builtins memcpy already uses
REP MOVSB/MOVSQ, not a byte-at-a-time Rust loop. A first array-copy prototype
compiled to SSE and is not booted or accepted. The evaluated candidate instead
uses plain x86_64 GPR loads/stores for 32-byte blocks, with compiler memcpy for
the shorter tail. It uses neither SIMD/FPU nor a CPU feature probe. The unsafe
contract is the same valid/nonoverlapping ordinary-memory ranges as before;
unaligned qword accesses are supported on x86_64. Early-clobber outputs keep
the four temporary registers distinct from both pointers, and the asm declares
memory effects without changing flags or the stack.

The primitive lives under Alpha's architecture adapter. Other architectures
retain compiler copy_nonoverlapping. Only private-page initialization uses the
candidate; global memcpy, Kernel, liba9n, protocol and ownership rules are not
changed. Generated x86_64 code for the constructor has no SIMD/FPU instructions.
The private-page host tests add source offsets and prefix/tail boundaries,
including source-page crossings with an unaligned destination continuation.
Full native validation passes 1,072 tests (12 benchmarks ignored); focused
ASan suites pass 100 tests (two benchmarks ignored); the AArch64 Release check
passes. Host ASan exercises the AArch64 fallback and common constructor, not
the inline x86 assembly. Logs:
`/private/tmp/nanami-opt-cycle16-{native,asan,arm,build}.log`.

The native QEMU probe also includes the exact architecture primitive. Before
any process benchmark timer, it checks 4,824 length/source-offset/destination-
offset combinations, including short tails and 4-KiB page copies, verifying
contents and both destination guards. Both comparison images contain the same
rebuilt probe and execute this untimed check; the runner requires its success
marker. Baseline Alpha compares byte-identical to cycle 14/15's retained normal
Alpha (SHA-256 `390fc358d5f8c8e955ff519f9e5e786c799962bedbaaa0374026b67cfb08ff91`).
No normal benchmark includes the temporary profiling code.

Sequential four-vCPU normal-device process-fixture boots with no concurrent
VM/build/test/benchmark. Per-boot medians, milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 20.418 | 20.482 | 21.535 | 23.011 |
| fork-128-vmas-8 | 21.157 | 22.992 | 21.913 | 23.557 |
| fork-256-vmas-8 | 22.332 | 21.596 | 22.296 | 21.958 |
| fork-exec-8 | 764.296 | 770.146 | 785.599 | 773.079 |
| getpid-2048 | 38.677 | 38.713 | 38.105 | 37.628 |
| getuid-2048 | 158.121 | 153.518 | 152.683 | 155.795 |
| copyout-normal | 272.779 | 271.744 | 273.864 | 270.529 |
| copyout-128-vmas | 275.679 | 272.458 | 273.685 | 272.748 |
| copyout-256-vmas | 276.883 | 274.898 | 274.853 | 273.788 |
| alpha-spawn-reap-8 | 31.629 | 31.030 | 31.379 | 30.379 |
| alpha-breakdown-spawn | 3.572 | 3.447 | 3.371 | 3.673 |
| alpha-breakdown-reap | 0.389 | 0.391 | 0.373 | 0.411 |
| alpha-xfe-spawn | 8.135 | 7.820 | 8.014 | 7.949 |
| alpha-xfe-reap | 0.607 | 0.589 | 0.578 | 0.585 |
| negoto-create | 7.056 | 6.706 | 6.848 | 6.856 |

Pooled ordinary fork/wait is 22.213 -> 21.273 ms (4.2% lower), but before-boot
medians span 20.418 to 23.011 ms and overlap both after boots. Fork/exec/wait
is 772.366 -> 777.873 ms (0.7% higher). The two fragmented-fork cases change
by less than 2%, and the unchanged syscall/copyout controls by less than 3%.
The BusyBox pure-construction control falls 5.6%, but does not execute the
changed source-initialization copy routine; do not count that as this candidate's
success. These measurements do not establish a 5% targeted workload gain.
All four runs complete the exact native x86 copy primitive's guard/content
fixture, child protection checks and executable freshness checks. Logs:
`/private/tmp/nanami-opt-cycle16-process-{before1,after1,after2,before2}.log`.

The same matched images then run real desktop startup in sequential
before/after/after/before boots, milliseconds:

| OCI create/start to X11 client | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6522.187 | 6549.020 | 6536.095 | 6522.365 |
| Session 2 | 7140.723 | 7019.346 | 7102.336 | 7222.135 |
| Session 3 | 7132.461 | 7141.584 | 7131.444 | 7218.407 |

First-session medians are 6522.276 -> 6542.558 ms (0.31% higher), and later
sessions are 7179.565 -> 7116.890 ms (0.87% lower). These differences are below
the 100-ms observer interval, not an established 5% startup gain. All 12 real
X11 sessions finish their kill/delete/same-ID reuse checks. Logs:
`/private/tmp/nanami-opt-cycle16-startup-{before1,after1,after2,before2}.log`.

Decision: reject the low-level copy implementation. Remove the new architecture
adapter, constructor change and candidate-specific host/guest copy fixtures;
restore the normal probe/runner as well. The removed file was created only
for this experiment and is recoverable from the archived candidate:
`/private/tmp/nanami-opt-cycle16-gpr-copy-rejected.patch`. Both that archive and
the separate temporary diagnostic archive pass a non-mutating git apply check
against the restored tree. Earlier accepted optimizations, especially cycle
14's physical-allocation index, remain unchanged. Do not infer a physical
Intel speedup, or an exclusive CPU-copy bottleneck, from the diagnostic counter
ratios or the QEMU-only experiment. No low-level copy override is retained.

Next: inspect why these copies/materializations are required, and where
fork/exec or mapping lifetime can safely avoid them. Source review finds that
sys_vfork delegates to the ordinary copying sys_fork before parking its parent;
however the detailed startup log has no emitted syscall-58 total. Do not assert
that this vfork entry point currently dominates Tiny Core. Inspect actual clone
flags/callers and the existing shared-VM lifecycle first. Any copy avoidance
must preserve snapshot/private-write behavior, permissions, backing lifetime,
checked writes, exec separation and cleanup, without relying on a new Kernel
fastpath or silently weakening existing memory contracts.

Restored normal artifact: `/private/tmp/nanami-opt-cycle16-restored.img`.
Alpha is byte-identical to the retained pre-experiment executable (SHA-256
`390fc358d5f8c8e955ff519f9e5e786c799962bedbaaa0374026b67cfb08ff91`).
The original probe is rebuilt too. Restored validation passes 1,071 native
tests (12 benchmarks ignored), 99 focused ASan tests (two benchmarks ignored),
and the AArch64 Alpha Release check. Logs:
`/private/tmp/nanami-opt-cycle16-restored-{probe-build,build,native,asan,arm}.log`.
Temporary instrumentation and candidate copy/fixture symbols are absent from
the restored production/probe source. git diff --check passes, A9N remains
unchanged, and all test/build processes have terminated.
+
## Cycle 17: bounded set-associative process-copy mappings

Check actual fork usage before changing snapshot semantics. Three diagnostic
startup sessions report 78 requests: 75 ordinary syscall-56 clones with flags
0x1200011, and three shared-VM clones with flags 0x3d0f00. BusyBox normally has
1,202 backed pages at these forks; aterm has 1,837. The ordinary-clone reports
cover 92,055 backed pages. No inherited memfd mapping is reported in this run,
so copying then rebinding MAP_SHARED pages is not established as its bottleneck.
Do not substitute shared-VM behavior for ordinary private forks. Temporary fork,
clone3, memfd and permission-count instrumentation is removed before the normal
comparison. Diagnostic log: /private/tmp/nanami-opt-cycle17-profile.log; full
serial: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-voj537ve.

Instead change only Alpha's existing process-copy alias cache. Replace the
16-entry linear search with 512 sets of four entries (2,048 aliases), hashing
the canonical memory-owner PID and complete current Frame descriptor. Hits and
victim selection examine at most four entries. Per-set round-robin eviction
still protects a pinned source while mapping its destination. No page contents
are cached, private snapshots still copy all their bytes, and every caller
still resolves the current Frame and checks its existing authority/permissions.

A 256-bucket u16 owner-count table makes invalidation of an uncached owner return
without scanning 2,048 entries. These are hash buckets, not a PID limit: collisions
only cause an extra scan, and matching still uses the full owner PID. Entries
count before MAP, including a failed MAP that may have installed a PTE, and
leave the count only after confirmed UNMAP. Failed cleanup remains tracked and
prevents cap revocation or RAM reuse. Shared-VM peers use the same owner key.
The entry-count bound fits u16. No ownership registry, protocol or Kernel change
is introduced.

The fixed alias range is 0x66000000..0x66800000, below the separate initialization
window at 0x67000000. Cache metadata is bounded below 52 KiB per Alpha; populated
aliases also need the existing on-demand page tables. This does not allocate an
8-MiB data buffer or limit guest pages/processes. This additional bounded cache
space is a tradeoff for retaining repeated fork sources and guest-copy mappings;
it is not a claim that larger caches are free.

The production-source cache harness now checks a resident 64-page working set,
10,000 pinned-source destinations, shared-VM owner reuse/invalidation, identical
descriptors in independent spaces, owner-bucket collisions, failed-MAP accounting,
eviction UNMAP failure/retry, invalidation before reuse and window/metadata bounds.
The full native suite passes 1,077 tests (12 benchmarks ignored); focused ASan
suites pass 110 tests (two benchmarks ignored), and the AArch64 Alpha Release
check succeeds. Logs: /private/tmp/nanami-opt-cycle17-cache-{native-all,asan,arm}.log.
Default Alpha has no RDTSC/RDTSCP instructions; both profilers are disabled.

Run the same immutable guest fixture and four-vCPU, normal-device QEMU/TCG
configuration sequentially before/after/after/before, with no simultaneous VM,
build, test or host benchmark. Baseline is cycle 16's restored normal image;
only Alpha's cache implementation differs in the candidate. All 12 OCI startup
sessions complete kill/delete/same-ID reuse checks:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6632.963 | 6320.236 | 6329.542 | 6528.202 |
| Session 2 | 7122.026 | 6731.932 | 6829.514 | 7110.636 |
| Session 3 | 7240.761 | 6819.817 | 6829.231 | 7119.671 |

First-session medians are 6580.583 -> 6324.889 ms (3.9% lower); later-session
medians are 7120.849 -> 6824.524 ms (4.2% lower). Both candidate boots are below
both baseline boots, but neither startup metric establishes this cycle's 5%
whole-startup target. The observer still polls every 100 ms, and cleanup/UART
reporting remain outside the timed interval. Logs:
/private/tmp/nanami-opt-cycle17-startup-{before1,after1,after2,before2}.log.

Then run the process fixture in the same sequential order. Each entry below is
a per-boot median in milliseconds (three samples, except six Negoto creates):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 23.990 | 20.375 | 18.292 | 21.855 |
| fork-128-vmas-8 | 22.196 | 19.543 | 18.769 | 22.669 |
| fork-256-vmas-8 | 22.253 | 20.217 | 18.807 | 21.493 |
| fork-exec-8 | 788.179 | 725.018 | 723.112 | 768.687 |
| getpid-2048 | 38.852 | 38.781 | 39.645 | 37.856 |
| getuid-2048 | 156.319 | 153.514 | 151.482 | 153.208 |
| copyout-normal | 273.365 | 276.135 | 273.685 | 271.775 |
| copyout-128-vmas | 269.813 | 273.902 | 274.199 | 273.264 |
| copyout-256-vmas | 277.135 | 275.891 | 274.472 | 273.780 |
| alpha-spawn-reap-8 | 33.372 | 31.598 | 32.089 | 31.867 |
| alpha-breakdown-spawn | 3.711 | 3.724 | 3.542 | 3.664 |
| alpha-breakdown-reap | 0.395 | 0.389 | 0.364 | 0.389 |
| alpha-xfe-spawn | 8.345 | 8.149 | 8.077 | 7.934 |
| alpha-xfe-reap | 0.748 | 0.586 | 0.544 | 0.623 |
| negoto-create | 6.947 | 6.717 | 7.193 | 8.135 |

Pooling both boots, eight ordinary fork/wait operations fall from 22.923 to
19.334 ms (15.7% lower); eight fork/exec/wait operations fall from 778.433 to
724.768 ms (6.9% lower). The 128- and 256-VMA fork cases fall 13.4% and 11.1%.
Both candidate boot medians are below both baseline boot medians for each of
these four workloads. Retain the candidate for these repeatable process-level
gains, which exceed 5%; do not represent the separate ~4% desktop-startup result
as meeting that threshold or as a universal syscall speedup.

The getpid control is 1.5% higher, getuid is 2.6% lower, and ordinary/128-VMA
copyout are 1.0%/0.5% higher; 256-VMA copyout is 0.2% lower. Keep these small
movements visible rather than claiming every checked-copy path improves.
Construction and isolated teardown also vary; Negoto create does not improve
consistently between both boots despite a lower pooled median. The fixture's
protection and executable-cache freshness assertions pass in every run.
Logs: /private/tmp/nanami-opt-cycle17-process-{before1,after1,after2,before2}.log.

Candidate image: /private/tmp/nanami-opt-cycle17-cache.img; build log:
/private/tmp/nanami-opt-cycle17-cache-build.log. Normal Alpha SHA-256:
a7b64dade4b9fbb0883b24c07bfb12c2a1002b7b03ee25124ca0250d53330313.
Baseline image: /private/tmp/nanami-opt-cycle16-restored.img. A9N's source and
IPC fastpaths are unchanged.

The same candidate passes the full four-vCPU, normal-device desktop regression:
two real X11/FLWM/wbar sessions in one boot, keyboard/mouse input, window dragging,
Terminal/Editor/Information launches, Exit and fresh restart with execution-context
cleanup. Both sessions emit their input, launch, information and session PASS
markers, followed by TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true.
The captured input screenshot was visually inspected too. Log:
/private/tmp/nanami-opt-cycle17-cache-desktop.log; full serial and screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-_xp8jmp8.
Temporary fork diagnostics are removed; the existing opt-in profilers remain
disabled in the normal build. No Kernel change is retained. The tested normal
image remains available at the candidate path above; git diff --check passes.

Next: investigate the remaining page-materialization and checked-copy work,
keeping the direct syscall and pure-construction controls separate from the
fork/exec gains. Do not turn a measured mapping-cache benefit into an assumption
that private contents may be shared or that permission validation is redundant.
+
## Cycle 18: directly consume the selected buddy block (rejected)

Review finds that PhysicalAllocator::allocate_any selects a first-fit normal
buddy block, then calls allocate_at. That path repeats containment and carve
searches for the same block. Test a candidate that consumes the selected first
block directly and uses the existing add_range for its unallocated suffix.
Preserve the smallest-sufficient-order/first-entry choice, swap-remove and suffix
split order, ordinary allocation records, RAM-only allocation and all explicit
allocate_at/device paths. No new cache, metadata, ownership rule, protocol or
Kernel change is introduced.

An internal differential test performs 6,000 deterministic allocate/free
operations against the original selection followed by allocate_at, comparing
every returned range, free-page count and complete free-list order. A public
API test checks byte rounding, zero rejection and no Device fallback. Both
pass, as do 1,079 native tests (12 ignored), 107 focused ASan tests (three
ignored) and the AArch64 Alpha Release check. These prove the tested semantics,
not an application speedup. Logs:
/private/tmp/nanami-opt-cycle18-allocate-{native-all,asan,arm}.log.

Compare against cycle 17's accepted normal image with the exact same guest
fixture, four-vCPU QEMU/TCG and normal devices. Run sequential
before/after/after/before, with no competing build, test, VM or host benchmark.
Process measurements (per-boot medians in milliseconds):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 18.986 | 17.355 | 18.739 | 20.245 |
| fork-128-vmas-8 | 19.534 | 18.917 | 19.051 | 19.516 |
| fork-256-vmas-8 | 19.980 | 18.309 | 18.521 | 19.221 |
| fork-exec-8 | 714.976 | 739.219 | 735.194 | 720.332 |
| getpid-2048 | 37.896 | 39.897 | 37.798 | 38.826 |
| getuid-2048 | 157.398 | 151.945 | 152.193 | 156.781 |
| copyout-normal | 273.886 | 277.518 | 274.613 | 273.855 |
| copyout-128-vmas | 275.215 | 272.900 | 277.579 | 273.965 |
| copyout-256-vmas | 276.280 | 273.780 | 276.424 | 277.246 |
| alpha-spawn-reap-8 | 33.036 | 30.083 | 32.335 | 30.710 |
| alpha-breakdown-spawn | 3.549 | 3.489 | 3.539 | 3.483 |
| alpha-breakdown-reap | 0.400 | 0.374 | 0.432 | 0.385 |
| alpha-xfe-spawn | 8.389 | 7.909 | 7.986 | 7.994 |
| alpha-xfe-reap | 0.555 | 0.594 | 0.564 | 0.590 |
| negoto-create | 7.204 | 7.272 | 8.003 | 7.924 |

Pooled ordinary fork/wait improves from 19.616 to 18.442 ms (6.0%), but eight
fork/exec/wait operations regress from 717.654 to 737.207 ms (2.7%). Both
candidate fork/exec boot medians exceed both baseline medians. 128-/256-VMA
fork improves 2.8%/4.9%. getpid is essentially unchanged; the other small
control and construction/teardown movements are not evidence of an overall
improvement. Logs:
/private/tmp/nanami-opt-cycle18-process-{before1,after1,after2,before2}.log.

The separate real startup comparison also completes all 12 kill/delete/same-ID
reuse sessions, but gets slower:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6331.494 | 6539.474 | 6430.639 | 6221.311 |
| Session 2 | 6825.885 | 7057.945 | 7053.305 | 6527.146 |
| Session 3 | 6932.311 | 7041.473 | 7043.552 | 7024.012 |

First-session medians rise from 6276.403 to 6485.057 ms (3.3%); later sessions
rise from 6879.098 to 7048.429 ms (2.5%). The 100-ms observer and normal timing
boundaries are unchanged. Logs:
/private/tmp/nanami-opt-cycle18-startup-{before1,after1,after2,before2}.log.

Reject the candidate: a smaller source-level path and one improved fork metric
do not justify the repeatable exec and desktop-startup regressions. These
measurements do not establish their lower-level cause; do not attribute the
regression to IPC, Kernel code or allocation-order changes. The differential
test establishes equal choices for its tested operation sequences, not a
universal explanation of instruction/layout/cache effects in the real build.

Archive the complete candidate and tests at
/private/tmp/nanami-opt-cycle18-direct-allocation-rejected.patch. Restore all
three files byte-for-byte to the accepted pre-experiment source, and check the
archive with git apply --check against that restored tree. Retain cycle 17's
mapping cache and all earlier accepted optimizations; no direct-allocation
candidate or extra test-only API remains. Candidate image/build log:
/private/tmp/nanami-opt-cycle18-allocate{.img,-build.log}.

Next: investigate ELF page contents rather than equating fewer allocator
searches with a faster system. Alpha's fill_lazy_page currently zeroes the
whole page before copying file-backed spans over it. Evaluate initializing
file spans and uncovered gaps once, preserving PT_LOAD overlay order, BSS,
IPC/TLS fixup, errors and initialization before publication. This is distinct
from the already optimized private mmap/fork initializer and from sharing
private frames or changing Kernel mappings.

Restored verification passes 1,077 native tests (12 benchmarks ignored),
105 focused ASan tests (three benchmarks ignored), and the AArch64 Alpha
Release check. Logs: /private/tmp/nanami-opt-cycle18-restored-{native,asan,arm}.log.
The normal image is rebuilt at /private/tmp/nanami-opt-cycle18-restored.img
with both profilers disabled. Its Alpha executable is byte-identical to the
accepted cycle-17 executable (SHA-256
a7b64dade4b9fbb0883b24c07bfb12c2a1002b7b03ee25124ca0250d53330313);
the allocator source is likewise restored to SHA-256
212321cc9cc3327eccd1abf1e72dccfcd1a78f05dab163994d660940e1f9d8bf.
Three additional real startup/reuse sessions pass, reporting 6327.680,
6732.316 and 6835.711 ms. These are a restoration smoke check, not a new
optimization comparison. Log: /private/tmp/nanami-opt-cycle18-restored-startup.log;
full serial: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-hzsuvn6g.
git diff --check and the rejected patch's git apply --check pass. A9N remains
unchanged, and all owned benchmark/build/test processes have completed.
+
## Cycle 19: ELF content initialization without a preceding full zero pass (rejected)

Evaluate the distinct ELF initializer, not the private mmap/fork initializer
already changed earlier. Move fill_lazy_page to a small pure page-contents
module. For the first intersecting file span, copy those bytes and zero only
the prefix/suffix gaps; a full file page has no preceding zero pass. Later
file spans retain header order and overwrite earlier bytes. BSS does not erase
earlier file data, matching the old initial-zero-then-file-overlays behavior.
Pages without file bytes still receive a full zero pass, followed by the same
IPC/TLS word fixup. Source/image-end arithmetic and source-slice bounds remain
checked. Host pointer arithmetic uses relative guest-page offsets.

The candidate also passes immutable ELF kind metadata by reference in the
exec-page loop and fills the temporary page before publishing the target PTE
in both ELF construction paths. These are tested together; do not attribute a
whole-build timing difference to zero elision alone. No protocol, privilege,
ABI, SIMD/FPU primitive or Kernel change is introduced.

A new harness compiles the production fill routine with the real ELF segment
and ABI types, using a minimal mapping-kind wrapper. Its independent bytewise
oracle starts with zeros and applies file overlays in order. Eight tests cover
anonymous/BSS/absent pages, full/partial spans, gaps, reversed/overlapping headers,
TLS-word boundaries, invalid offsets/overflow, high guest VAs and 5,000 randomized
segment sets. Guard bytes surround every output page. Failure checks compare
error status and guards, not unspecified unpublished contents. All eight pass;
the full native suite passes 1,085 tests (12 ignored), focused ASan suites pass
118 tests (two ignored), and AArch64 Alpha Release checking succeeds. Logs:
/private/tmp/nanami-opt-cycle19-page-{native-all,asan,arm}.log.

Use the same guest fixture, four-vCPU QEMU/TCG and normal devices, with
sequential before/after/after/before boots and no concurrent VM, build, test or
host benchmark. The baseline is cycle 18's exact restored cycle-17 image.
All 12 real desktop startup sessions complete kill/delete/same-ID reuse:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6333.773 | 6451.240 | 6541.583 | 6326.425 |
| Session 2 | 6719.126 | 6921.321 | 6931.158 | 6927.242 |
| Session 3 | 6920.524 | 7043.196 | 6919.203 | 6936.050 |

First-session medians rise from 6330.099 to 6496.412 ms (2.6%); later-session
medians are 6923.883 -> 6926.240 ms (effectively unchanged). The observer still
polls every 100 ms. No 5% startup improvement is established. Logs:
/private/tmp/nanami-opt-cycle19-startup-{before1,after1,after2,before2}.log.

Then run the same process fixture in the same order (per-boot medians in ms):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 18.454 | 18.964 | 18.256 | 20.243 |
| fork-128-vmas-8 | 20.088 | 20.253 | 20.555 | 19.342 |
| fork-256-vmas-8 | 18.854 | 19.227 | 17.340 | 19.195 |
| fork-exec-8 | 710.113 | 754.551 | 741.372 | 715.181 |
| getpid-2048 | 37.783 | 38.683 | 38.238 | 37.608 |
| getuid-2048 | 155.566 | 154.085 | 155.831 | 156.744 |
| copyout-normal | 270.524 | 274.302 | 271.304 | 273.539 |
| copyout-128-vmas | 272.010 | 278.807 | 271.200 | 278.050 |
| copyout-256-vmas | 275.855 | 281.321 | 275.858 | 277.920 |
| alpha-spawn-reap-8 | 33.919 | 33.541 | 30.857 | 32.465 |
| alpha-breakdown-spawn | 3.637 | 3.597 | 3.491 | 3.567 |
| alpha-breakdown-reap | 0.396 | 0.368 | 0.417 | 0.383 |
| alpha-xfe-spawn | 8.277 | 8.186 | 8.340 | 8.198 |
| alpha-xfe-reap | 0.569 | 0.538 | 0.572 | 0.552 |
| negoto-create | 6.625 | 7.643 | 7.024 | 7.879 |

Pooled fork/exec/wait rises from 712.647 to 747.962 ms (5.0%), with both
candidate boot medians above both baseline boot medians. Isolated BusyBox
construction improves only 0.9%; Xfe construction is 0.3% higher. The combined
eight-create/reap metric has a 4.7% lower pooled median but is not consistently
better between both boots. Ordinary-fork's lower pooled median is not a reliable
ELF-initialization result: this copying-fork workload does not call the changed
initializer, and one candidate boot is slower than the first baseline boot.
128-VMA fork is 3.4% higher. Keep the remaining small syscall/copyout/teardown
movements as separate controls, not an overall score. Protection and executable
cache-freshness assertions pass in all four process runs. Logs:
/private/tmp/nanami-opt-cycle19-process-{before1,after1,after2,before2}.log.

Reject this combined candidate. Fewer page writes and passing bytewise tests do
not establish a faster system; the measured exec regression is repeatable and
the intended construction gain is small. These comparisons do not establish
which source change or lower-level code/layout effect causes the regression.
No claim about physical Intel speed follows from these QEMU measurements.

Archive all six candidate/refactor/test files in
/private/tmp/nanami-opt-cycle19-page-contents-rejected.patch. Restore the four
pre-existing files exactly, including earlier accepted edits, and remove only
the new candidate module/harness. The archive passes git apply --check against
the restored tree. No part of this experiment, including its publication-order
change, is retained. Candidate image: /private/tmp/nanami-opt-cycle19-page.img;
Alpha SHA-256: ea8b34b905ba06a6bfceee056064c96cc9f475a1cd7a4f12d5ad378af6b60a8b.

Next: inspect the unmeasured ELF input path before assuming page filling remains
the right target. Existing exec stage samples begin in replace_process_image_from_memory,
after read_process_memory_into_static_buffer has copied the entire incoming ELF
and after parsing. That reader maps ordinary owned source pages through the same
initialization window used for destination pages. Shared-cache contention and
input-copy costs are hypotheses to measure, not established bottlenecks. Any
source-window change must retain current-Frame/ownership validation, permanent
pool-cap lifetime rules, foreign/shared fallbacks and failure cleanup; do not
replace the private input snapshot with an unsafe mutable borrowed slice.

Restored verification passes 1,077 native tests (12 benchmarks ignored),
110 focused ASan tests (two benchmarks ignored), and the AArch64 Alpha Release
check. Logs: /private/tmp/nanami-opt-cycle19-restored-{native,asan,arm}.log.
Rebuild the normal image with both profilers disabled:
/private/tmp/nanami-opt-cycle19-restored.img (build log with -build.log suffix).
Alpha is byte-identical to the accepted cycle-17 executable (SHA-256
a7b64dade4b9fbb0883b24c07bfb12c2a1002b7b03ee25124ca0250d53330313).
Three additional real startup/reuse sessions pass (6233.491, 6740.575 and
6731.194 ms); these are restoration smoke checks, not an optimization result.
Log: /private/tmp/nanami-opt-cycle19-restored-startup.log; full serial:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-uv4gxrd3.
The rejected patch's git apply --check and git diff --check pass. A9N remains
unchanged; all owned VM/build/test sessions are terminal.

## Cycle 20: input and mapping-cache diagnosis; release checkpoint

Diagnostic-only builds measured ELF input ownership/mapping/copy stages and
process-copy invalidation during three-session Tiny Core startup/reuse runs.
The last build also replayed initialization-window accesses against larger
tag arrays without changing the actual 1,024-entry initialization window.
These instrumented startup times are not normal-build performance results.
Logs: /private/tmp/nanami-opt-cycle20-profile.log,
/private/tmp/nanami-opt-cycle20-cache-profile.log and
/private/tmp/nanami-opt-cycle20-model-profile.log.

At the user's release checkpoint, archive the nine temporary source/harness
changes in /private/tmp/nanami-opt-cycle20-diagnostic-only.patch and restore
each file byte-for-byte to the accepted pre-diagnostic state. The archive
passes git apply --check against that restored tree. No cycle-20 counters,
model arrays, invalidation logging or capacity change is retained. Cycle 17
remains the latest accepted optimization; do not report diagnostic observations
as another speedup. The previously documented opt-in exec/syscall profilers
remain disabled by default.

Nanami 0.7.1 is a patch release of internal user-level optimizations, with no
Kernel or public API change. The restored native suite passes 1,077 tests
(12 opt-in benchmarks ignored): /private/tmp/nanami-v0.7.1-release-native.log.
The x86_64 Release image builds with both profilers unset:
/private/tmp/nanami-v0.7.1-release-check.img (build log with -build.log suffix).
Alpha remains byte-identical to the accepted cycle-17 binary (SHA-256
a7b64dade4b9fbb0883b24c07bfb12c2a1002b7b03ee25124ca0250d53330313).
A9N's worktree and the existing Kernel ABI remain unchanged.

## Cycle 21: larger bounded pool-frame initialization cache

Revisit capacity using cycle 20's actual access sequence, not only a synthetic
working set. At its last complete diagnostic record, 328,533 initialization
lookups have 272,496 misses with 1,024 entries. Replaying the same accesses with
2,048 tags predicts 211,306 misses (22.5% fewer); 4,096 tags predict 161,324.
All 1,104 complete model records have equal total lookups across the three
sizes. These models do not change actual mappings, account for failed mapping
operations, or prove an application speedup. Serial log:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-fcpcm8os/serial.log.

Test 2,048 entries, keeping the same direct indexing, pool-Frame identity and
MAP/UNMAP failure tracking. A hit still obtains the current permanent pool
Frame descriptor; page contents are never cached or shared as a private fork.
Every new owner receives the existing zero/fill work. No Kernel change, new
service operation, permission shortcut, COW or SIMD primitive is introduced.
A compile-time bound keeps the aliases below the separate staging window.

The alias range grows from 0x67000000..0x67400000 to
0x67000000..0x67800000. Metadata grows from 16 to 32 KiB per Alpha; this is
not an 8-MiB data-buffer allocation. Populated aliases also consume existing
on-demand page tables and BootstrapVmSpace frame records. The latter remains
a fixed 16,384-entry tracker: the common 8,192-page heap, 1,024-page stack,
2,048-entry process-copy window and 2,048-entry initialization window total
13,312 records, leaving 3,072 for other mappings. This resource tradeoff is
why the first candidate is 2,048 entries rather than consuming the entire
16-MiB address gap with 4,096 aliases. Cache capacity is not a guest-page or
PID-count limit, but it is not free mapping capacity either.

The production-source harness passes eight tests, adding resident reuse above
the former capacity, exact metadata/address bounds, and failed MAP followed
by failed cleanup/retry. Collision, unowned-page and existing failure checks
are retained. The full native suite passes 1,080 tests (12 opt-in benchmarks
ignored); focused ASan suites pass 107 tests (two ignored); the AArch64 Alpha
Release check passes. Logs:
/private/tmp/nanami-opt-cycle21-init-{native-all,asan,arm}.log.

Compare normal, profiler-disabled images with the same immutable guest fixture,
four-vCPU QEMU/TCG and normal devices. All measured boots run sequentially,
without concurrent builds, tests, benchmarks or VMs. Baseline is
/private/tmp/nanami-v0.7.1-release-check.img (accepted cycle-17 Alpha).
The startup comparison runs before/after/after/before; all 12 sessions pass
fresh X11-client startup, kill/delete and same-container-ID reuse:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6225.670 | 6217.893 | 6124.963 | 6225.057 |
| Session 2 | 6631.212 | 6806.842 | 6715.023 | 6735.193 |
| Session 3 | 6822.660 | 6532.566 | 6725.811 | 6817.556 |

First-session medians are 6225.364 -> 6171.428 ms (0.9% lower), and later
sessions 6776.375 -> 6720.417 ms (0.8% lower). Differences are smaller than
the observer's 100-ms polling interval; they do not establish a 5% startup
gain or even a precise sub-percent speedup. Keep cleanup and UART reporting
outside the timed interval as before. Logs:
/private/tmp/nanami-opt-cycle21-startup-{before1,after1,after2,before2}.log.

The process fixture first runs in the same ABBA order. Its pooled fork/exec
median improves 8.5%, but 128-extra-VMA fork is 5.8% higher. Run another
baseline/candidate pair rather than hiding that regression. Full per-boot
medians in milliseconds (three samples per boot, except six Negoto creates):

| Workload | Before 1 | After 1 | After 2 | Before 2 | Before 3 | After 3 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| getpid-2048 | 39.178 | 37.705 | 37.021 | 37.766 | 38.061 | 37.813 |
| getuid-2048 | 152.866 | 157.384 | 152.632 | 151.132 | 154.689 | 153.292 |
| copyout-normal | 272.555 | 271.767 | 274.054 | 274.016 | 271.800 | 272.691 |
| copyout-128-vmas | 273.102 | 273.023 | 272.553 | 276.625 | 270.218 | 272.455 |
| copyout-256-vmas | 277.300 | 272.968 | 272.501 | 274.069 | 276.225 | 274.444 |
| fork-8 | 20.046 | 19.389 | 18.436 | 20.278 | 18.127 | 19.657 |
| fork-exec-8 | 712.678 | 663.877 | 655.819 | 729.881 | 705.228 | 671.146 |
| fork-128-vmas-8 | 18.944 | 19.589 | 19.911 | 18.461 | 20.318 | 19.059 |
| fork-256-vmas-8 | 19.057 | 18.767 | 18.692 | 18.813 | 18.409 | 19.352 |
| alpha-spawn-reap-8 | 30.898 | 32.943 | 31.390 | 31.728 | 31.293 | 32.657 |
| alpha-breakdown-spawn | 3.535 | 3.579 | 3.578 | 3.531 | 3.593 | 3.592 |
| alpha-breakdown-reap | 0.378 | 0.371 | 0.429 | 0.380 | 0.370 | 0.461 |
| negoto-create | 7.165 | 6.679 | 6.556 | 7.364 | 7.079 | 8.508 |
| alpha-xfe-spawn | 7.990 | 8.394 | 7.938 | 8.612 | 8.544 | 7.983 |
| alpha-xfe-reap | 0.615 | 0.563 | 0.576 | 0.640 | 0.600 | 0.607 |

Across all nine samples per variant, eight fork/exec/wait operations fall
719.654 -> 670.091 ms (6.9%). Every candidate boot median is below every
baseline boot median. This exceeds the cycle's 5% target for this complete
process-lifecycle workload, not for the entire desktop or every syscall.
Ordinary eight-fork latency is 3.3% lower, but boot results overlap; do not
claim a repeatable 5% gain there. Xfe construction and cleanup have lower pooled
medians too, but construction boot medians overlap.

Tradeoffs remain visible: 128-VMA fork rises 18.944 -> 19.718 ms (4.1%,
0.774 ms per eight forks), and 256-VMA fork is 1.5% higher. The initial
128-VMA separation does not hold for the third pair, but its pooled increase
is still reported. Native Alpha eight-create/reap rises 31.293 -> 32.657 ms
(4.4%, 1.364 ms), and isolated BusyBox construction is 1.2% higher. These
are not established lower-level explanations of the change. Do not assert
that a larger cache improves all native construction or fragmented-fork work.
The getpid/getuid and three checked-copyout controls change by less than 1%
each over all six boots. Negoto-create's pooled change is below 1%, despite
a larger apparent gain in the first ABBA comparison. No weighted combined
score is used. All six process runs pass permission and executable-cache
freshness checks. Logs:
/private/tmp/nanami-opt-cycle21-process-{before1,after1,after2,before2,before3,after3}.log.

Retain the small capacity change for the repeatable fork/exec gain, with the
bounded memory cost and slower controls explicitly recorded. This does not
complete the overall optimization goal or turn the diagnostic miss reduction
into a claim of a 22.5% application improvement. No physical Intel timing claim
follows from QEMU/TCG results.

The candidate also passes two real desktop sessions in one boot, including
X11/FLWM/wbar, USB mouse/title dragging, keyboard input, Terminal/Editor/
Information launch, Exit, resource cleanup and fresh restart. Both sessions
emit INPUT/LAUNCH/INFO/SESSION-PASS, followed by
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. The captured input
screenshot is visually inspected. Log:
/private/tmp/nanami-opt-cycle21-init-desktop.log. Full serial and screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-7x5z13u5.

Normal candidate image: /private/tmp/nanami-opt-cycle21-init.img; build log:
/private/tmp/nanami-opt-cycle21-init-build.log. Alpha SHA-256:
1396a3529c01c006799e39c689aabffd6b2d0d26fbcd465d128ef4c3f3c1c1a6.
Both existing profilers stay unset; no cycle-20 diagnostic code is restored.
A9N's source/ABI and IPC fastpaths remain unchanged.

Next: investigate invalidation work rather than increasing capacities again.
The cycle-20 trace repeatedly invalidates all of a process's copy aliases on
mprotect, even though backing Frames remain owned. Audit whether current-Frame
resolution plus checked-copy permission validation permit retaining those
privileged Alpha aliases across protection changes. Test PROT_NONE, read-only
denial, shared-VM ownership, rollback, and subsequent unmap/reap before removing
any invalidation. Lifetime invalidation before Frame removal, revoke or RAM
reuse must remain. This is an investigation, not an implemented shortcut.

## Cycle 22: protection-only copy-alias retention (rejected)

Audit the distinction between guest PTE permissions and privileged Alpha copy
aliases. handle_mapping_protect_request retains the backing Frame record and
allocation across every permissions change, including PROT_NONE. The existing
protect_process_page helper already has no copy-window invalidation when used
by the private-copy path. Checked copy requests separately validate current
VmSpace permissions before using a cached alias; raw supervisor/fork copies
intentionally keep access to retained backing. Current-Frame lookup and all
backing-release/exec/reap invalidation remain mandatory.

Read-only review of A9N's address_space::operation_unmap and both supported
HAL implementations confirms that guest Frame unmapping updates the selected
address-space leaf; it does not mutate the Frame capability's identity.
Do not confuse retaining a privileged alias with granting a guest access to
its protected PTE. No Kernel source or ABI change is made.

The candidate removes the full-PID invalidation from the protection request,
with explanatory comments only. No new cache, state field or protocol is added.
A production-source harness combines real ProcessManager, copy-cache,
protection and checked-request code with capability/PTE bookkeeping mocks
and mocked byte transfer. Four added tests cover all permission masks, checked
read/write refusal with live cached aliases, shared-VM canonical ownership,
protection rollback, and failed alias cleanup blocking cap removal/reuse.
All four fail against the original full-invalidation behavior and pass against
the candidate; this red check proves the intended alias-retention assertions
are exercised, not that the old implementation violates its old contract.

The candidate passes 1,084 native tests (12 ignored), 111 focused ASan tests
(two ignored) and the AArch64 Alpha Release check. Logs:
/private/tmp/nanami-opt-cycle22-retain-{native-final,asan-final,arm}.log.
These mock tests establish bookkeeping/authorization behavior, not hardware
TLB correctness or application speed.

Compare the accepted cycle-21 image with the candidate using the unchanged
guest fixture, four-vCPU QEMU/TCG and normal devices, before/after/after/before.
All timed boots are sequential with no simultaneous build, test or VM.
Both profilers are unset. All 12 real OCI desktop startup/reuse sessions pass:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6228.097 | 6118.542 | 6109.350 | 6225.423 |
| Session 2 | 6606.723 | 6497.655 | 6699.285 | 6701.658 |
| Session 3 | 6728.357 | 6621.938 | 6726.073 | 6819.503 |

First-session medians fall 6226.760 -> 6113.946 ms (1.8%); later sessions
6715.008 -> 6660.612 ms (0.8%). Later-session boot results overlap, and these
small differences are near the unchanged 100-ms observer interval. They do
not establish the cycle's 5% startup target. Logs:
/private/tmp/nanami-opt-cycle22-startup-{before1,after1,after2,before2}.log.

Run the process fixture in the same order. Per-boot medians, milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 38.563 | 39.607 | 40.181 | 37.892 |
| getuid-2048 | 151.020 | 152.832 | 155.540 | 154.773 |
| copyout-normal | 273.830 | 272.291 | 271.116 | 272.144 |
| copyout-128-vmas | 271.849 | 271.907 | 271.949 | 273.052 |
| copyout-256-vmas | 273.342 | 272.847 | 274.088 | 274.379 |
| fork-8 | 19.620 | 19.873 | 19.258 | 18.334 |
| fork-exec-8 | 668.498 | 671.681 | 663.931 | 658.458 |
| fork-128-vmas-8 | 18.917 | 19.236 | 18.688 | 20.020 |
| fork-256-vmas-8 | 19.093 | 18.512 | 18.936 | 18.579 |
| alpha-spawn-reap-8 | 32.586 | 31.898 | 32.277 | 31.071 |
| alpha-breakdown-spawn | 3.523 | 3.922 | 3.693 | 3.500 |
| alpha-breakdown-reap | 0.373 | 0.423 | 0.421 | 0.377 |
| negoto-create | 7.278 | 6.605 | 6.851 | 6.641 |
| alpha-xfe-spawn | 8.136 | 8.096 | 8.480 | 7.877 |
| alpha-xfe-reap | 0.549 | 0.522 | 0.680 | 0.566 |

Pooled eight fork/exec/wait operations are 664.751 -> 671.022 ms (0.9%
higher), ordinary eight-fork is 3.1% higher, and the 128-/256-VMA fork cases
are 2.7%/1.3% lower. None establishes a 5% targeted process improvement.
getpid is instead 3.7% higher; getuid and the three copyout controls change
by less than 0.4%. Native Alpha construction/reap as a whole changes by less
than 1%, but isolated BusyBox construction rises 3.510 -> 3.720 ms (6.0%),
with both candidate boot medians above both baselines. Its isolated cleanup
is 0.375 -> 0.422 ms (12.5%, 47 microseconds). Xfe construction and teardown
also have higher pooled medians, with overlapping per-boot results. Keep
these controls visible; fewer explicit mapping operations are not evidence
that the compiled system is faster. These data do not establish the
lower-level cause of changes to otherwise unchanged workloads.

Reject the candidate: the intended startup gain is small, process performance
does not meet the target, and the construction/getpid increases do not justify
retaining the change on source-level work reduction alone. This is not proof
that permission-only alias retention is semantically wrong or that IPC
transport is the bottleneck. All process fixtures complete their existing
PROT_NONE and executable-cache-freshness assertions. Logs:
/private/tmp/nanami-opt-cycle22-process-{before1,after1,after2,before2}.log.

Archive the complete runtime/comment/mock-harness candidate in
/private/tmp/nanami-opt-cycle22-protection-retention-rejected.patch and restore
all three files byte-for-byte to their cycle-21 state. The archive passes
git apply --check against the restored tree. Candidate timing image remains
/private/tmp/nanami-opt-cycle22-retain.img (build log with -build.log suffix);
candidate Alpha SHA-256:
ba5ce0a099007e08a16588cb19e4bae648b7c445797fe6c6155fc5d755401063.

Retain one independent Linux ABI regression in memory-events.c. It first
populates a copy alias via eventfd read, switches the buffer to read-only,
checks allowed copyin and denied copyout without consuming the event, then
restores the sentinel and exercises PROT_NONE. Existing checks also require
a real read-only CPU store to terminate with SIGSEGV, beyond-EOF access with
SIGBUS, and retained bytes through fork/remap. The test passes with the restored
implementation on both one and four CPUs, without a NIC. Logs:
/private/tmp/nanami-opt-cycle22-memory-short-smp{1,4}.log. Full serial/screen dirs:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-tq0f__g4 and
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-3kor7z0a.

The first two functional attempts never executed the guest. Their temporary
basename nanami-opt-cycle22-memory-events exceeded ALTER_IMAGE_NAME_MAX=24,
so legacy launch parsing rejected it and the host observer waited for a PID
that was never created. The first run's screenshot confirms spawn failed 1;
the second attempt also timed out with a NIC attached. This is not evidence
of a Kernel deadlock, NIC failure or a failing memory-events assertion.
Use the short basename memory-events instead. Add host-runner preflight
validation for empty/overlong UTF-8 basenames, and check parser rejection
(exit 2) before VM startup plus Python compilation. The final valid run
exercises the runner after this validation was added and passes:
/private/tmp/nanami-opt-cycle22-memory-final-smp1.log; serial/screens:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-f750axc_.

Restored verification passes 1,080 native tests (12 ignored), 107 focused ASan
tests (two ignored), and the AArch64 Alpha Release check. Logs:
/private/tmp/nanami-opt-cycle22-restored-{native,asan,arm}.log.
The normal short-fixture image is /private/tmp/nanami-opt-cycle22-memory.img,
rebuilt after the first, invalidly named candidate run was terminal. Its build
log is /private/tmp/nanami-opt-cycle22-memory-short-build.log. Fixture binary:
/private/tmp/nanami-opt-cycle22-fixture.zl92WQ/memory-events.
Current Alpha and nanami/alpha.elf are byte-identical to accepted cycle 21:
1396a3529c01c006799e39c689aabffd6b2d0d26fbcd465d128ef4c3f3c1c1a6.
The accepted Tiny Core image remains /private/tmp/nanami-opt-cycle21-init.img.

Reclaim about 1.25 GiB by deleting only four owned, terminal-job temporary
images: cycle18-allocate, cycle19-page, cycle20-profile and cycle20-cache-profile
under /private/tmp/nanami-opt-*. Their archived source patches and logs are
retained; these diagnostic/rejected images are rebuildable. No accepted
baseline, current candidate timing image, user image or source is deleted.

Next: target lifetime invalidation more precisely rather than dropping the
protection safeguard again. Cycle 20 shows a one-page shared-memory release
discarding up to 1,784 unrelated copy aliases for the same owner. Investigate
invalidating the exact retiring Frame descriptors through their existing
four-way hash sets, including failed-MAP entries and canonical VM owners,
before any target cap is removed. Never return after only the first matching
slot: pending failed entries can coexist with a ready alias. Retain full
owner invalidation for exec/reap and all destructive paths not yet audited.
This investigation is not an implemented optimization.

## Cycle 23: exact-Frame shared-memory alias invalidation (rejected)

Follow cycle 20's one-page shared-memory release that discarded 1,784 cached
aliases for the owner. Audit SharedMapping's copied/mapped counters first:
rollback may have copied more caps than it mapped. Complete cleanup of every
copied retiring cap's Alpha alias before unmapping any guest page or removing
any cap. The cap descriptors are derived from the canonical memory entry's
root and the actual copied-slot range, not a guessed VA/PFN interval.

The candidate adds a private batch invalidator accepting a lazy iterator of
Frame descriptors. It resolves the canonical VM owner and checks the owner
bucket once. An empty bucket skips enumeration entirely. Each Frame examines
only its existing four-way hash set and matches both full PID and descriptor,
including failed-MAP/unready entries. All matching entries are cleared; do
not stop after the first ready alias. Stop enumerating when the owner bucket
becomes empty; bucket collisions only cause extra work, never skipped cleanup.
No temporary descriptor array, new cache state/capacity or protocol is added.

Only remove_shared_mapping calls the batch invalidator, using mapping.copied.
The same pre-cleanup barrier covers release and failed-install rollback.
Failed cleanup keeps caps/backing live and stops destruction. Full-owner
invalidation remains on mprotect, generic mapping release, exec and reap.
No Kernel, permission-validation, IPC-fastpath, private-copy or FPU change
is introduced.

Three new production-source lifecycle assertions fail with the original
full-owner purge and pass with the candidate: retaining unrelated owner/peer
aliases, not blocking one region on another alias's cleanup failure, and
canonical shared-VM owner cleanup. Additional tests cover all-retiring-alias
cleanup before any guest PTE/cap removal, equal-Frame owner hash collisions,
failed-MAP cleanup/retry, both ready and failed aliases of one key, uncached
owner short-circuiting, empty batches and stopping enumeration after the last
owner alias is removed. The duplicate-key test constructs the state through
real cache operations and two failures, not private-array injection.

The final batch candidate passes 1,091 native tests (12 opt-in benchmarks
ignored), 129 focused ASan tests (two ignored), and the AArch64 Alpha Release
check. The scoped cleanup does not weaken current-Frame checks or change the
cache's metadata/address bounds. These bookkeeping mocks do not prove hardware
TLB behavior. Logs:
/private/tmp/nanami-opt-cycle23-frame-{native-all-final,asan-final,arm}.log.
Initial singular-helper experiments are superseded by this batch build.

Compare normal builds using the same immutable fixture, four-vCPU QEMU/TCG,
normal devices and sequential before/after/after/before boots. No build, test,
benchmark or other VM overlaps timed samples. Both profilers remain unset.
Baseline is the accepted cycle-21 image. All 12 actual desktop-startup
sessions pass kill/delete and same-container-ID reuse:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6215.576 | 6124.945 | 6230.735 | 6235.135 |
| Session 2 | 6620.913 | 6713.447 | 6809.558 | 6608.006 |
| Session 3 | 6727.726 | 6825.487 | 6919.197 | 6718.152 |

First-session medians are 6225.356 -> 6177.840 ms (0.8% lower), below the
observer's unchanged 100-ms polling resolution. Later sessions instead rise
6669.532 -> 6817.523 ms (2.2%); both candidate warm-boot medians exceed both
baseline warm-boot medians. This does not meet the 5% startup target.
Logs: /private/tmp/nanami-opt-cycle23-startup-{before1,after1,after2,before2}.log.

The same-order process comparison reports these per-boot medians in ms
(three samples per workload, except six Negoto creates):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 38.310 | 37.959 | 40.799 | 38.763 |
| getuid-2048 | 156.572 | 158.318 | 153.348 | 156.468 |
| copyout-normal | 268.202 | 275.671 | 273.303 | 272.618 |
| copyout-128-vmas | 273.104 | 278.115 | 273.885 | 273.498 |
| copyout-256-vmas | 274.611 | 277.350 | 275.479 | 278.150 |
| fork-8 | 18.099 | 20.931 | 17.843 | 18.777 |
| fork-exec-8 | 676.162 | 664.687 | 668.620 | 666.957 |
| fork-128-vmas-8 | 20.662 | 19.363 | 21.013 | 19.437 |
| fork-256-vmas-8 | 21.342 | 18.840 | 21.362 | 20.546 |
| alpha-spawn-reap-8 | 31.330 | 34.702 | 32.116 | 31.800 |
| alpha-breakdown-spawn | 3.440 | 3.702 | 3.670 | 3.450 |
| alpha-breakdown-reap | 0.357 | 0.397 | 0.463 | 0.371 |
| negoto-create | 6.490 | 7.196 | 7.327 | 6.572 |
| alpha-xfe-spawn | 7.821 | 8.318 | 7.990 | 8.775 |
| alpha-xfe-reap | 0.570 | 0.651 | 0.668 | 0.617 |

Pooled eight fork/exec/wait operations fall only 671.560 -> 668.208 ms (0.5%),
with overlapping boot medians. Ordinary eight-fork rises 18.438 -> 19.821 ms
(7.5%); boot results overlap. The 128-VMA fork median is 4.5% higher.
256-VMA fork has an attractive pooled 8.4% reduction, but the second candidate
boot is above both baselines: this is not a consistently reproduced target
gain. Do not select that one pooled result as an overall score.

Native Alpha eight-create/reap instead rises 31.565 -> 33.854 ms (7.3%);
isolated BusyBox construction rises 3.445 -> 3.686 ms (7.0%), with both
candidate boot medians above both baselines. Negoto create rises 6.572 ->
7.327 ms (11.5%) with the same separation. BusyBox/Xfe isolated cleanup also
has higher pooled medians. Xfe construction's lower pooled median overlaps
between boots, so it is not independent evidence of a broad gain.
getpid and checked copyout controls are around 1% higher, while getuid is
0.4% lower. All process runs finish their PROT_NONE and executable-cache
freshness assertions. Logs:
/private/tmp/nanami-opt-cycle23-process-{before1,after1,after2,before2}.log.

Reject the complete candidate. More retained aliases and fewer explicit
UNMAPs do not by themselves establish a faster system; neither startup nor
fork/exec reaches the target and repeated construction regressions matter.
These measurements do not establish whether cache pressure, bookkeeping,
binary layout, scheduling or another effect causes the changes. Do not
attribute the regression to IPC transport or Kernel changes without evidence.

Archive all four runtime/harness changes in
/private/tmp/nanami-opt-cycle23-frame-invalidation-rejected.patch and restore
them byte-for-byte to their pre-experiment state, retaining every earlier
accepted optimization and cycle 22's independent ABI/runner regressions.
The archive passes git apply --check against the restored tree. Candidate
timing image: /private/tmp/nanami-opt-cycle23-frame.img; build log:
/private/tmp/nanami-opt-cycle23-frame-build-final.log. Candidate Alpha SHA-256:
4058e5624bc3cc5c1cb6aef390673e84fa0e388d9ff5d8f8629617aa8592de58.
No batch invalidator, scoped shared-memory policy, extra mock fields or
candidate assertions remain in the normal implementation.

Restore verification passes 1,080 native tests (12 ignored), 118 focused
ASan tests (two ignored, including the existing copy-window target in this
run) and the AArch64 Alpha Release check. Logs:
/private/tmp/nanami-opt-cycle23-restored-{native,asan,arm}.log.
Rebuild the normal Tiny Core image with both profilers unset:
/private/tmp/nanami-opt-cycle23-restored.img (build log with -build.log suffix).
Current Alpha and nanami/alpha.elf are byte-identical to accepted cycle 21:
1396a3529c01c006799e39c689aabffd6b2d0d26fbcd465d128ef4c3f3c1c1a6.
Three further real startup/reuse sessions pass: 6228.553, 6712.163 and
6721.812 ms. These are restoration checks, not a new speed comparison.
Log: /private/tmp/nanami-opt-cycle23-restored-startup.log; full serial:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-0akxbcgy.
A9N remains unchanged and all owned build/test/VM handles are terminal.

Also remove the accidental tracked-bytecode difference created by cycle 22's
Python syntax check. Its index was unchanged and it was clean before that
owned generation step; preserve the generated copy in the existing task temp
directory, then restore only that bytecode file. No user source/index changes
are reverted.

Next: stop assuming a more selective mapping cache is the right bottleneck.
Read the current raw-fork path before redesigning it. Alter's fork.rs now
calls request_process_fork; Alpha threads.rs create_raw_task explicitly skips
ELF parsing, pathname lookup, initial stack and zero-fill work, then invokes
clone_process_vm for the backed-page snapshot. Do not reintroduce a claim of
duplicate ELF initialization in ordinary fork.

Re-profile this current clone/exec implementation before a larger change:
cycle 16's page-stage totals predate the accepted copy/initialization-cache
changes. A possible user-level mechanism to evaluate is an immutable private
snapshot with deferred guest Frame/PTE construction, retaining eager fallback
under bounded snapshot-storage pressure. It must preserve nested fork,
PROT_NONE, shared memfd replacement, partial unmap/mprotect, privileged
copying, fault handling, rollback and final/shared-VM reap. This is an
unimplemented option, not a promised performance result.

Do not implement naive live-parent-frame COW solely by changing PTEs to RO.
The x86 do_syscall implementation still has a direct valid CAPABILITY_CALL
route (-1), and raw-task construction places AddressSpace/Frame-node caps
in the child root. Final guest authority and every write path need a full
audit before claiming isolation with shared physical backing. No Kernel
modification or COW implementation follows from these preliminary read-only
observations.

## Cycle 24: re-profile the accepted private-fork path

The release checkpoint retains cycle 21's accepted implementation and Nanami
0.7.1. Revalidate the current source rather than repeating rejected cache or
permission policies. Ordinary Linux fork calls request_process_fork, which
creates a suspended raw task with its own VM and clones backed pages. It does
not load the ELF, look up its pathname or create an initial stack again.
Shared-VM threads still use their separate path.

Add temporary instrumentation under the existing NANAMI_EXEC_PROFILE opt-in.
Time backed-page enumeration and clone construction separately. Within the
private-page constructor, accumulate reservation/preparation, physical
allocation and cap copying, destination initialization-window lookup/MAP,
source initialization, and target publication. Reset counters immediately
before clone_process_vm and drain after it, without logging inside a page
loop. Thus unrelated mmap/initializer requests cannot contaminate a clone's
page totals. All counters belong to Alpha's serialized request and are
diagnostic elapsed local ticks, including interruptions, descheduling and
timestamp overhead, not exclusive CPU time or calibrated nanoseconds.

Use four-vCPU QEMU/TCG with the normal PS/2 and virtio-net device configuration,
an unchanged Tiny Core fixture, and no other build/test/VM during each run.
Only Alpha profiling is enabled; Alter profiling is unset. Do not compare
these instrumented application timings to normal-build performance samples.

The process fixture completes freshness and protection checks. It emits 103
ordinary fork reports, each with 67 pages (6,901 in total), and every report's
counted initializer pages equal its backed-page count. Aggregate ticks:

| Clone work | Process fixture | Real desktop startup |
| --- | ---: | ---: |
| Backed-page enumeration | 1,175,000 | 15,190,000 |
| Whole page-clone loop | 87,453,000 | 3,275,501,000 |
| Constructor preparation | 10,297,000 | 130,332,000 |
| Allocation / cap copying | 18,078,000 | 272,373,000 |
| Destination staging mapping | 5,540,000 | 338,172,000 |
| Page initialization | 33,864,000 | 2,120,511,000 |
| Target publication | 13,816,000 | 178,063,000 |

The real startup fixture finishes three actual X11 startup/kill/delete/same-ID
reuse sessions. It reports 75 private clones: 72 with 1,202 pages and three
with 1,837 pages, totaling 92,055 pages (377,057,280 bytes, about 360 MiB).
All counted-page assertions match. Initialization is about 64.7% of the
whole clone-loop elapsed sample, whereas preparation is about 4.0%. Merely
batching preparation would not remove most of this work.

The same startup run emits 81 exec/drop reports, with 99,267 retired page
references. The exec-drop reset/unmap/remove/RAM/deferred sums are
49,566,000 / 65,548,000 / 52,272,000 / 33,371,000 / 5,597,000 ticks.
The broad exec-drop interval also contains its UART report, and therefore
must not be mistaken for an uninstrumented teardown cost. These are current
measurements after the allocation index and both mapping-cache improvements,
not cycle 16's older baseline.

A second diagnostic splits source initialization into current Frame lookup,
source-copy-window lookup/MAP, actual copy, and zero-tail work. It repeats the
three real startup/reuse sessions, with the same 75 clone sizes and 92,055
counted pages. Aggregate ticks:

| Clone / initializer work | Detailed startup sample |
| --- | ---: |
| Whole clone loop | 3,362,348,000 |
| Whole initialization stage | 2,228,801,000 |
| Source Frame lookup | 23,042,000 |
| Source mapping lookup/MAP | 325,502,000 |
| Actual copy interval | 1,857,706,000 |
| Zero-tail interval | 6,746,000 |

The copy interval is about 83.4% of the initialization sample and 55.3% of
the whole clone-loop sample. The separately instrumented run has additional
timestamp/counter overhead; do not pool it with the first run or claim an
application speedup from either. In particular, scheduling wait inside a copy
interval is not proof that the CPU executed memcpy for that whole duration.
No evidence here attributes this cost to IPC transport or changes its fastpath.

Sources are archived as two independently replayable patches:
- /private/tmp/nanami-opt-cycle24-profile.patch
- /private/tmp/nanami-opt-cycle24-profile-detail.patch

Both patches cover only memory_requests.rs, memory_pages.rs and exec_profile.rs.
Images/build logs have the same /private/tmp/nanami-opt-cycle24-profile and
-profile-detail prefixes. Full serial directories:
- Process: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-d0rvc6xk
- Startup: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-7yfz8gay
- Detailed startup: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-4s7lvidg

All three runs exit successfully. The temporary instrumentation is then
removed, restoring all three source files byte-for-byte. Both archived patches
pass git apply --check against that restored source. The default Alpha and
nanami/alpha.elf retain cycle 21's SHA-256:
1396a3529c01c006799e39c689aabffd6b2d0d26fbcd465d128ef4c3f3c1c1a6.
This is a diagnostic cycle, not a retained speedup or a claim of reaching 5%.

Next: evaluate safe avoidance of ordinary private-fork page construction and
copying, not another cache-capacity tweak or the rejected copy-assembly path.
An immutable Alpha-owned private snapshot with deferred guest Frame/PTE
construction remains an option to design and test, not an implementation.
Capturing it still copies all live parent bytes; it cannot be advertised as
zero-copy. Its possible benefit must exceed capture, later materialization,
metadata and extra memory pressure on both fork/exec and long-lived children.

Do not turn ordinary fork into CLONE_VM. Do not share mutable parent Frames
using only read-only PTEs while assuming Linux code cannot invoke raw A9N
capability operations. Any live-frame COW design must first establish actual
guest capability isolation. A user-level immutable snapshot must instead stay
inaccessible to the guest, preserve nested forks, PROT_NONE and checked
privileged writes, integrate with partial unmap/mprotect and shared memfd
replacement, and survive exec, rollback, deferred shared-VM reap and allocation
failure. Use a bounded optimization budget with eager fallback, never a new
process/mapping limit. No Kernel or new memory semantics are introduced by
this diagnosis.

Restored verification passes 1,080 native tests (12 opt-in benchmarks ignored),
118 focused ASan tests (two ignored), and the AArch64 Alpha Release check.
Normal x86_64 Alpha contains no RDTSC/RDTSCP instructions. Logs:
/private/tmp/nanami-opt-cycle24-restored-{native,asan,arm,build}.log.
The normal image /private/tmp/nanami-opt-cycle24-restored.img completes three
fresh startup/kill/delete/reuse sessions: 6235.200, 6614.387 and 6823.335 ms.
These verify restoration and are not a new paired speed comparison. Log:
/private/tmp/nanami-opt-cycle24-restored-startup.log; full serial directory:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-20vi4bs5.

After all diagnostic VM/build handles finish, delete only the two diagnostic
images generated in this cycle, cycle24-profile.img and
cycle24-profile-detail.img under /private/tmp (about 642 MiB of logical image
data). Their source patches, build logs and serial logs remain available for
reproduction. Keep the accepted normal image and earlier baselines. A9N's
working tree remains clean; no version or release metadata changes in this
diagnostic cycle.

## Cycle 25: batch construction of contiguous private-fork pages

Cycle 24 finds that byte copying is the largest sampled private-fork component.
A live-parent-frame COW change cannot be justified by read-only PTEs alone:
current raw tasks have mapping capabilities, and remote invalidation completion
would also need an explicit guarantee. Do not silently change the guest
capability boundary, Kernel or ordinary-fork memory semantics to obtain a
benchmark improvement. Immutable user-level capture with deferred guest pages
remains a larger unimplemented option, not a zero-copy shortcut.

Evaluate an immediate preparation optimization while retaining every private
page copy. Move the existing VM-clone helpers into memory_clone.rs, without
changing the external service protocol. A new, empty destination (the normal
suspended raw-fork child) partitions the sorted backed-page list into contiguous
VA runs and calls the existing private-page constructor once per run, rather
than once per page. Different per-page permissions do not split a run; after
construction, each page receives its own original protection before the child
can be resumed. Sparse/unbacked reservations stay sparse, and retained
PROT_NONE backing is copied with its contents and final NONE protection.

A nonempty or absent destination VmSpace retains the original generic page
copy/protect path. Aligned range clones and overlapping copies also retain
their existing traversal and memmove order. CLONE_VM is unchanged. All pages
still acquire independent physical backing; there is no live-frame sharing,
new metadata cache, page-count limit, IPC operation or Kernel change. The
preparation batch does not claim to reduce copied bytes or eliminate the
largest component found by cycle 24.

The production traversal harness initially adds nine checks: contiguous mixed-permission
runs and private writes in both directions, sparse holes, existing-destination
fallback, canonical shared-VM source, empty/missing destinations, constructor
failure, protection failure with owned child backing, a 520-page run crossing
Frame-node boundaries, and both overlap directions. Five candidate-specific
assertions fail with the original traversal and pass after batching. Page
allocation/copy/protection is modeled in this harness; existing private-page
constructor tests separately exercise real multi-page rollback after partial
allocation/MAP failures. Neither mock establishes hardware TLB behavior.

The candidate passes 1,089 native tests (12 opt-in benchmarks ignored), 127
focused ASan tests (two ignored), and the AArch64 Alpha Release check.
Logs: /private/tmp/nanami-opt-cycle25-{clone-red,clone-green,batch-native,batch-asan,batch-arm}.log.
Both profilers are unset in normal comparison images. Candidate image:
/private/tmp/nanami-opt-cycle25-batch.img; final build log:
 /private/tmp/nanami-opt-cycle25-batch-build-final.log.
Alpha SHA-256:
f4b6a705a7b15994f191c7a7eef52297f2813dc7e919ec5eac0ca89130e8f285.
Baseline is cycle 24's restored normal image, identical to accepted cycle 21.

Normal process comparison uses four-vCPU QEMU/TCG, normal devices, identical
immutable guest fixtures, and sequential before/after/after/before boots. No
build, test, host benchmark or other VM overlaps these samples. Per-boot
medians in milliseconds (three samples each, six Negoto creates):

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 38.245 | 38.341 | 38.321 | 39.177 |
| getuid-2048 | 155.879 | 154.370 | 153.468 | 153.393 |
| copyout-normal | 275.225 | 270.404 | 275.311 | 271.757 |
| copyout-128-vmas | 274.242 | 274.270 | 279.071 | 272.472 |
| copyout-256-vmas | 278.933 | 273.715 | 276.254 | 275.000 |
| fork-8 | 19.699 | 17.300 | 16.021 | 18.337 |
| fork-exec-8 | 676.121 | 677.814 | 669.365 | 690.257 |
| fork-128-vmas-8 | 19.527 | 18.903 | 18.787 | 18.160 |
| fork-256-vmas-8 | 18.628 | 18.572 | 18.960 | 19.040 |
| alpha-spawn-reap-8 | 32.230 | 31.294 | 32.745 | 31.658 |
| alpha-breakdown-spawn | 3.723 | 3.668 | 3.560 | 3.545 |
| alpha-breakdown-reap | 0.437 | 0.374 | 0.425 | 0.399 |
| negoto-create | 7.546 | 8.409 | 6.848 | 7.024 |
| alpha-xfe-spawn | 8.206 | 8.454 | 8.622 | 8.056 |
| alpha-xfe-reap | 0.619 | 0.611 | 0.555 | 0.632 |

Pooled ordinary fork/wait falls 19.018 -> 17.190 ms
(9.6%); both candidate boot medians are below both baseline medians. Fork/exec
instead falls only 1.4%, and the 128-/256-extra-VMA fork medians change by less
than 1%. Keep those workloads separate rather than declaring a broad 9.6%
process speedup. All four runs finish child PROT_NONE and executable-cache
freshness assertions.

Controls include Alpha native spawn/reap +1.6%, Negoto create +8.5% pooled,
and Xfe-only construction +4.4%. Negoto boot results overlap substantially,
but both Xfe construction boot medians are above both baselines (about 0.36 ms
pooled absolute difference). These controls do not execute the new raw-fork
batch constructor. Do not attribute their variation to that constructor
without evidence, and do not hide them when assessing the whole candidate.
getpid/getuid and checked-copyout controls change by around 1% or less.
Logs: /private/tmp/nanami-opt-cycle25-process-{before1,after1,after2,before2}.log.

Real desktop startup uses the same normal images/devices and sequential
before/after/after/before boots, with no overlapping build, test or other VM.
All 12 startup/kill/delete/same-ID reuse sessions pass. Milliseconds:

| OCI create/start to X11 client | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6328.764 | 6315.806 | 6226.938 | 6329.649 |
| Session 2 | 6708.609 | 6709.339 | 6811.473 | 6820.413 |
| Session 3 | 6732.844 | 6938.075 | 6710.734 | 6834.018 |

Cold medians are 6329.207 -> 6271.372 ms (0.9% lower); later sessions are
6776.629 -> 6761.104 ms (0.2% lower). Both differences are below the observer's
unchanged 100-ms polling interval and do not establish a 5% desktop-startup
gain. The consistently faster ordinary-fork fixture must not be reported as
an equivalent whole-desktop speedup. Logs:
/private/tmp/nanami-opt-cycle25-startup-{before1,after1,after2,before2}.log.

Add a tenth traversal regression after timed comparisons: 64 deterministic
sparse/mixed-permission layouts compare the batched snapshot's parent/child
contents and resulting protection metadata with the unchanged generic
page-at-a-time clone path. Both read-only/NONE and writable permissions are
included; this checks results rather than only constructor-call counts.
Final native validation passes 1,090 tests (12 ignored), and focused ASan
passes 128 (two ignored). Logs:
/private/tmp/nanami-opt-cycle25-batch-{native,asan}-final.log.

Accept the run-batching candidate for the consistently reproduced ordinary
fork/wait reduction of 9.6%, with the above construction-control tradeoffs.
This is a targeted process optimization, not a claim that desktop startup,
fork/exec or every construction path improves by 5%. Keep the large byte-copy
cost and the Xfe/Negoto control observations as separate follow-up work.

The candidate also passes two complete desktop sessions in one normal-device
four-vCPU boot: X11/FLWM/wbar, actual keyboard/mouse input and title dragging,
Terminal/Editor/Information launches, Exit and fresh restart with full context
cleanup. The first input screenshot is visually inspected. Log:
/private/tmp/nanami-opt-cycle25-batch-desktop.log; full serial/screens:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-j21qioxu.
No performance claim is based on this untimed functional run.

Additionally compile the current memory-events.c and signals-x86_64.S as an
integer-only Linux fixture. It is byte-identical to the previously built
cycle-22 fixture (SHA-256 d620379a6cd52be897f53ad0bee8e711a02e7c2c74d7ffb0660053b358b90537),
which is embedded in the separate small test image. It passes on one and four
CPUs: warm-alias copyin/copyout permissions, protected CPU accesses, retained
PROT_NONE/fork/remap bytes and beyond-EOF signaling. Logs:
/private/tmp/nanami-opt-cycle25-memory-smp{1,4}.log. Full serial directories:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-v37dx5za
and /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-upnqo050.
Fresh fixture: /private/tmp/nanami-opt-cycle25-fixture.oj44eT/memory-events.
These are functional checks, not timed comparisons.

The accepted normal image is /private/tmp/nanami-opt-cycle25-batch.img.
Default Alpha and nanami/alpha.elf retain the measured f4b6a705... SHA-256
above, including after the small fixture build; neither contains RDTSC/RDTSCP.
The candidate runtime/traversal-test patch is archived at
/private/tmp/nanami-opt-cycle25-batch-candidate.patch and passes a reverse
git apply check against the current tree. Earlier accepted work is preserved.
No A9N, IPC-fastpath, public API, version or release metadata is changed.

After both small memory-test VM handles finish, remove only their generated
cycle25-memory.img and cycle25-memory-rootfs.img from /private/tmp. Retain the
accepted full Tiny Core image, both older accepted baselines, source/fixture
patches, binaries and logs. The removed validation images are rebuildable
using the recorded fixture and normal build command; no user image is removed.

Next: do not repeat the already rejected final-permission helper, cache
retention or copy-assembly experiments as if they were new. The remaining
large copy volume still calls for a lifetime/backing design or a separately
measured change to how much private data is constructed. Any deferred private
snapshot must preserve independent writes and current guest authority; it
still pays capture costs and must be compared against this accepted batching
baseline, not against an older page-at-a-time fork.

## Cycle 26: current-Frame input staging

Start from cycle 25's accepted contiguous-fork baseline. Inspect input staging
rather than adding a new executable-image cache protocol. Existing immutable
Alter file snapshots already validate content identity; Alpha still needs a
coherent input copy. A new persistent image API is not justified by the input
stage costs measured here.

Temporarily profile read_process_memory_into_static_buffer under the existing
NANAMI_EXEC_PROFILE switch. Group completed reads into at-most-one-page and
larger inputs. Accumulate snapshot preparation, current Frame lookup, physical
ownership lookup, source mapping, byte copying and temporary cleanup. Drain
global totals only at exec/reap boundaries, outside page loops. The trigger PID
identifies the flush, not the source owner. Errors before completion are not
included. The diagnostic ticks include timestamp overhead, interrupts and
descheduling; they are not nanoseconds or exclusive CPU time.

One normal-device four-vCPU diagnostic run completes three real desktop
startup/kill/delete/same-ID reuse sessions. Totals are:

| Input work | Small records | Larger inputs |
| --- | ---: | ---: |
| Completed reads | 2,872 | 94 |
| Input bytes | 45,952 | 48,133,986 |
| Snapshot preparation ticks | 255,000 | 6,575,000 |
| Current Frame lookup ticks | 5,870,000 | 3,451,000 |
| Ownership lookup ticks | 112,950,000 | 3,426,000 |
| Source mapping ticks | 3,954,000 | 35,983,000 |
| Copy ticks | 2,127,000 | 17,775,000 |
| Cleanup ticks | 399,000 | 690,000 |

Every small read is 16 bytes: the existing initialized-map request's two-Word
descriptor. The scalar ownership lookup scans the caller's physical-allocation
chain, taking about 90% of these small-stage samples. The current Frame was
already resolved immediately before that scan. Small-stage time is still a
small part of whole startup; do not convert this ratio into a 90% application
speedup or assert that it proves the cycle's 5% application target.

The temporary two-file instrumentation is archived at
/private/tmp/nanami-opt-cycle26-staging-profile.patch, then removed byte-for-byte
before normal builds. Its git apply check passes against the restored source.
Build/run logs: /private/tmp/nanami-opt-cycle26-profile-{build,startup}.log.
Full serial:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-yhri83px.
The terminal-job diagnostic image is deleted after measurement; logs/patch
remain reproducible.

### Small-only prototype (rejected)

First route at-most-one-page reads through the existing process-copy mapping
cache, retaining the old ownership-snapshot/pool-mapping path for larger reads.
Each source page still resolves its current Frame before cache lookup. Also
require the scalar fallback's slot descriptor to match the current Frame
before using allocation metadata; the former snapshot-only check could fall
through to a mismatched scalar record at the same VA.

Add tests for zero ownership walks with thousands of newer records, unaligned
cross-page records and missing-page failure order, shared-VM/materialized
sources, cache mapping failure and a current foreign Frame differing from old
allocation metadata. Six checks independently fail against the original
reader. The test-only global buffer mutex recovers a previous assertion's
poison so a deliberately failing baseline does not hide subsequent cases.
A further bulk mismatch regression guards the fallback check. The prototype
passes 1,096 native tests (12 ignored), 134 focused ASan tests (two ignored)
and the AArch64 Alpha Release check.

The four normal process-fixture boots finish protection/freshness checks.
Pooled fork/wait is 19.915 -> 21.102 ms (+6.0%), with substantial baseline
reboot variation and overlapping boot medians; fork/exec is 676.184 ->
678.619 ms (+0.4%). 128-/256-extra-VMA forks rise 3.2%/4.8%. The unrelated
Alpha spawn/reap control falls 8.8%, so it is not proof of a targeted gain;
Negoto create falls 4.2%, and Xfe construction rises 2.3%. Do not select one
favorable control as a successful 5% optimization. Reject this small-only
split rather than retaining a second staging policy without a measured gain.
Logs: /private/tmp/nanami-opt-cycle26-process-{before1,after1,after2,before2}.log.
Archive: /private/tmp/nanami-opt-cycle26-small-rejected.patch.
Its 5b42a32e... normal Alpha and temporary comparison image are not the final
artifact; the terminal-job image is deleted before building the next candidate.

### Unified current-Frame candidate

The next candidate instead uses the existing process-copy cache for every
input page. Resolve the current Frame, map/reuse its tracked alias and copy
only the requested bytes to the same bounded static staging buffer. Remove
the ownership snapshot allocation/sort, scalar ownership fallback, separate
pool interpretation, temporary-window offsets and per-read temporary cleanup.
There is no new cache, capacity, metadata array, public API or Kernel change.
Large inputs still stream page-by-page through the bounded cache; its capacity
does not become an input-size or guest-page limit. Source bytes are read anew,
not cached. Existing maximum input size and empty-read behavior remain.

Validate full source-range addition before walking any pages, including small
overflowing ranges. Resolve each Frame after any lazy materialization; stale
physical allocation records cannot choose the input data. Alias lifetime and
failed-MAP/UNMAP retention use the existing production copy-window mechanism,
which is covered separately by its owner/collision/pinning/failure regressions.

Adapt the real staging-reader fixture to require no snapshot or ownership
search for both small and bulk inputs, exact partial/boundary bytes, fresh
mapping after materialization, canonical shared-VM ownership, successful
foreign-Frame reads without an Alpha-pool descriptor, stale-metadata rejection
and mapping-error propagation. The candidate passes 1,096 native tests
(12 ignored), 134 focused ASan tests (two ignored), and the AArch64 Alpha
Release check. Logs: /private/tmp/nanami-opt-cycle26-direct-{native,asan,arm,build}.log.
Normal image: /private/tmp/nanami-opt-cycle26-direct.img.
Alpha SHA-256:
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b.

Normal comparisons use four-vCPU QEMU/TCG, normal devices, immutable matched
fixtures and before/after/after/before boots. No build, test, host benchmark
or other VM overlaps timed samples. All process runs finish protection and
executable-cache freshness checks. Initial per-boot medians:

| Workload (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 38.747 | 38.611 | 38.263 | 39.376 |
| getuid-2048 | 156.123 | 152.789 | 152.534 | 155.355 |
| copyout-normal | 272.412 | 271.746 | 274.594 | 265.055 |
| copyout-128-vmas | 274.877 | 277.662 | 352.090 | 275.463 |
| copyout-256-vmas | 277.611 | 277.017 | 281.124 | 276.625 |
| fork-8 | 20.159 | 17.938 | 18.454 | 20.264 |
| fork-exec-8 | 669.533 | 668.508 | 670.616 | 673.348 |
| fork-128-vmas-8 | 18.843 | 18.631 | 19.483 | 18.609 |
| fork-256-vmas-8 | 18.264 | 18.201 | 19.463 | 18.409 |
| alpha-spawn-reap-8 | 34.289 | 25.747 | 23.917 | 31.875 |
| alpha-breakdown-spawn | 3.785 | 2.770 | 2.517 | 3.783 |
| alpha-breakdown-reap | 0.400 | 0.459 | 0.371 | 0.392 |
| negoto-create | 7.199 | 7.321 | 7.232 | 8.113 |
| alpha-xfe-spawn | 8.228 | 5.496 | 5.658 | 9.080 |
| alpha-xfe-reap | 0.597 | 0.575 | 0.584 | 0.765 |

The first pooled native eight-create/reap median is 33.263 -> 24.832 ms
(25.3% lower); Xfe-only construction is 8.654 -> 5.577 ms (35.6% lower).
Fork/exec changes by less than 1%. Ordinary fork's 10.0% pooled reduction is
not by itself an established broad improvement: controls vary between boots.
The 128-extra-VMA copyout control instead has a conspicuous candidate-second-boot
slowdown (352.090 / 432.949 / 319.669 ms). Retain all samples, including this
outlier boot; do not discard it or attribute it to IPC/cache behavior without
evidence. Logs:
/private/tmp/nanami-opt-cycle26-direct-process-{before1,after1,after2,before2}.log.

Run another independent same-image/order process comparison to assess that
control and reproduce the construction improvement:

| Workload (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 40.866 | 38.062 | 38.388 | 39.683 |
| getuid-2048 | 152.921 | 153.762 | 155.892 | 155.609 |
| copyout-normal | 274.672 | 269.295 | 273.381 | 276.115 |
| copyout-128-vmas | 275.947 | 270.188 | 270.466 | 274.705 |
| copyout-256-vmas | 273.949 | 273.952 | 275.157 | 274.839 |
| fork-8 | 17.433 | 16.954 | 17.223 | 17.483 |
| fork-exec-8 | 666.477 | 680.560 | 666.884 | 679.752 |
| fork-128-vmas-8 | 18.573 | 18.787 | 18.215 | 19.144 |
| fork-256-vmas-8 | 18.131 | 18.908 | 18.339 | 18.072 |
| alpha-spawn-reap-8 | 32.229 | 23.704 | 22.425 | 34.052 |
| alpha-breakdown-spawn | 3.517 | 2.581 | 2.538 | 3.727 |
| alpha-breakdown-reap | 0.383 | 0.418 | 0.376 | 0.513 |
| negoto-create | 7.938 | 7.046 | 7.114 | 7.075 |
| alpha-xfe-spawn | 7.989 | 5.453 | 5.060 | 8.212 |
| alpha-xfe-reap | 0.593 | 0.583 | 0.525 | 0.653 |

In this recheck, the 128-VMA copyout boot medians are 275.947/274.705 ms before
and 270.188/270.466 after; the large slowdown is not reproduced. Native
create/reap remains 31.0% lower pooled, and Xfe construction 34.3% lower,
with both candidate construction medians below both baselines. Ordinary fork
instead improves only 2.1% pooled, so do not promote the first cohort's 10%
value as a reproducible improvement to a path this reader does not execute.
Keep the earlier copyout anomaly as unresolved variation, not a proven fixed
bug or a claimed cache/TLB diagnosis. Logs:
/private/tmp/nanami-opt-cycle26-recheck-process-{before1,after1,after2,before2}.log.

Same-order actual desktop startup/reuse comparison, milliseconds:

| OCI create/start to X11 client | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6343.460 | 6225.288 | 6122.147 | 6129.159 |
| Session 2 | 6701.232 | 6616.353 | 6612.160 | 6609.064 |
| Session 3 | 6733.410 | 6737.719 | 6627.860 | 6736.437 |

All 12 startup/kill/delete/same-ID reuse sessions pass. Cold medians are
6236.310 -> 6173.718 ms (1.0% lower), later sessions 6717.321 -> 6622.107 ms
(1.4% lower). Both absolute differences are below the observer's 100-ms polling
interval and are not evidence of a 5% whole-desktop startup improvement.
Logs: /private/tmp/nanami-opt-cycle26-direct-startup-{before1,after1,after2,before2}.log.

### Complete input/child lifecycle comparison

Source review explains a relevant native-construction case: request_memory_create
registers a reclaimable multi-page owned/shared extent as one allocation record.
The old reader's single-page snapshot/find filters do not match that extent,
so each input page takes the temporary MAP/COPY/UNMAP fallback. The existing
cache can instead reuse those current Frame aliases across repeated spawns.
Its source aliases must still be destroyed at input release.

Constructor-only timing can therefore move some source UNMAP work past the
timer, even if no cleanup is lost. Extend the same guest fixture to include
VFS open/stat/read/close, owned input allocation/zeroing/fill, eight BusyBox
or three Xfe suspended child create/kill/reap operations, and final source
mapping release. Report preparation and source-release intervals separately.
Each of three samples loads a fresh source, and UART reporting is outside the
timer. The children are never resumed: this does not time Linux interpreter
or application execution. Existing process/freshness checks remain intact.

Build both variants with the identical extended probe, SHA-256
0c40e2d2c0b867e2cdce8a4ee643e9e49d03fb2d8dfc3ae3091066cec2daeea2.
Alpha remains byte-identical to the accepted f4b6a705... baseline and the
85374862... candidate respectively. Matched images:
/private/tmp/nanami-opt-cycle26-lifecycle-{before,after}.img.
Same normal-device four-vCPU sequential ABBA boots:

| Workload (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 39.208 | 40.257 | 38.254 | 38.439 |
| getuid-2048 | 150.746 | 156.548 | 156.676 | 156.623 |
| copyout-normal | 271.448 | 271.285 | 274.884 | 273.073 |
| copyout-128-vmas | 277.324 | 275.666 | 273.515 | 274.678 |
| copyout-256-vmas | 279.257 | 275.438 | 273.988 | 273.791 |
| fork-8 | 18.505 | 19.362 | 17.536 | 17.075 |
| fork-exec-8 | 665.871 | 669.914 | 687.705 | 670.099 |
| fork-128-vmas-8 | 18.953 | 19.714 | 19.692 | 18.673 |
| fork-256-vmas-8 | 18.572 | 18.314 | 19.937 | 18.789 |
| alpha-spawn-reap-8 | 30.414 | 24.010 | 27.946 | 31.381 |
| alpha-breakdown-spawn | 3.611 | 2.710 | 2.624 | 3.461 |
| alpha-breakdown-reap | 0.443 | 0.381 | 0.389 | 0.413 |
| negoto-create | 7.205 | 7.789 | 7.681 | 6.915 |
| alpha-xfe-spawn | 8.411 | 5.180 | 5.546 | 8.467 |
| alpha-xfe-reap | 0.676 | 0.628 | 0.703 | 0.602 |
| alpha-lifecycle-busybox-8 | 36.997 | 29.890 | 30.493 | 36.672 |
| alpha-lifecycle-busybox-8-prepare | 2.896 | 3.066 | 2.871 | 2.716 |
| alpha-lifecycle-busybox-8-source-release | 0.577 | 0.862 | 0.880 | 0.494 |
| alpha-lifecycle-xfe-3 | 37.660 | 30.876 | 30.360 | 37.019 |
| alpha-lifecycle-xfe-3-prepare | 10.952 | 10.505 | 10.154 | 10.451 |
| alpha-lifecycle-xfe-3-source-release | 1.017 | 1.834 | 1.796 | 1.001 |

Complete BusyBox lifecycle pooled medians are 36.904 -> 30.192 ms (18.2%
lower), and Xfe 37.626 -> 30.573 ms (18.7% lower). Both candidate boot medians
are below both corresponding baselines. This exceeds 5% for these declared
native construction lifecycles, including input preparation and cleanup.
Source release itself increases 0.536 -> 0.871 ms for BusyBox (+0.335 ms) and
1.004 -> 1.815 ms for Xfe (+0.811 ms); keep this explicit instead of calling
all stages faster. Preparation changes +5.3%/-1.8% respectively, already
included in the complete lifecycle figures.

Other controls remain mixed: in the extended fixture Negoto create rises
12.6% pooled with both candidate boot medians above both baselines, whereas
the two preceding cohorts show about 7.1% lower pooled results. Extended
fork/exec rises 1.5%, 128-extra-VMA fork 4.7%, and getpid/getuid around 2-3%.
Do not infer a uniform speedup from the repeatable native lifecycle gain,
or assume an unmeasured cause for these changes. None of the three cohorts
is silently removed or merged into a favorable synthetic score. Logs:
/private/tmp/nanami-opt-cycle26-lifecycle-process-{before1,after1,after2,before2}.log.

Accept the unified reader for the repeatable 18.2%/18.7% reductions in complete
native image lifecycles, including source preparation and final cleanup, and
for removing the input reader's redundant ownership interpretation. This is
not acceptance of the rejected small-only split, a 5% desktop-startup claim,
or evidence that every syscall/control improves. Retain the explicit cleanup
cost and mixed controls above as follow-up observations.

Add three final regressions: a well-formed four-page owned extent with no
page_count=1 records, repeated same-Frame reads observing changed data rather
than cached contents, and small-range overflow rejection before mapping.
Final validation passes 1,099 native tests (12 opt-in benchmarks ignored),
137 focused ASan tests (two ignored), and the AArch64 Alpha Release check.
Logs: /private/tmp/nanami-opt-cycle26-direct-{native,asan,arm}-final.log.

The accepted image also passes two complete normal-device four-vCPU desktop
sessions: X11/FLWM/wbar, real keyboard/mouse input and title dragging,
Terminal/Editor/Information launch, Exit and fresh restart with context cleanup.
The second input screenshot is visually inspected. Log:
/private/tmp/nanami-opt-cycle26-direct-desktop.log; full serial/screens:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-vrk2dosz.
These are untimed functional checks, not performance comparisons.

The current integer-only memory-events fixture separately passes on one and
four CPUs (warm copy permissions, protected CPU stores, PROT_NONE/fork/remap
contents and beyond-EOF signaling). Logs:
/private/tmp/nanami-opt-cycle26-memory-smp{1,4}.log. Full serial directories:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-7_yoinpm
and /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-alter-test-38kxqv6x.

The final/default Alpha and nanami/alpha.elf still match the measured
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b
binary, including after the memory-fixture build and a comment-only rebuild.
No RDTSC/RDTSCP instruction remains in normal Alpha. The input-size comment
now describes the retained 16-MiB limit rather than an obsolete contiguous
staging-map requirement; the runtime and public limit are unchanged.
The accepted full Tiny Core image is:
/private/tmp/nanami-opt-cycle26-lifecycle-after.img.
The matched extended-fixture baseline remains:
/private/tmp/nanami-opt-cycle26-lifecycle-before.img.
Candidate runtime, traversal-test and probe changes are archived at
/private/tmp/nanami-opt-cycle26-direct-candidate.patch; a reverse git apply
check passes against the current tree. The original probe source is also
preserved at /private/tmp/nanami-opt-cycle26-probe-before.rs.

Remove only owned, terminal-job temporary images: this cycle's diagnostic,
rejected small-only and old-fixture direct images; the older rejected
cycle23-frame.img after checking it has no open handles; and the two small
cycle26-memory validation images after both memory-test VMs finish. Source
patches, fixtures and logs remain available to regenerate them. No user image
or accepted latest baseline is removed, and no live image is overwritten.
Earlier accepted optimizations remain intact. A9N stays clean; IPC fastpaths,
public protocol, Nanami version and release metadata are unchanged.

Next: keep the temporary-buffer lifetime contract under review before adding
any persistent Alpha executable-image cache. Inspect whether eager memory
spawns retain Image lazy-mapping metadata that points into the mutable static
staging buffer; verify actual uses before changing it. Also investigate
copyout variability with matched evidence instead of declaring a cache/TLB or
IPC cause. Do not repeat rejected cache-retention, permission-constructor or
copy-assembly experiments as new candidates.

## Cycle 27: request-local eager image plan (rejected)

Audit the staging-buffer concern left by cycle 26 against current code before
changing ownership or fault behavior. A successful spawn already eagerly fills
all image pages and calls drop_image_lazy_mappings_for_pid before returning.
Exec passes a local Image kind directly to its page constructor and registers
only the Zero stack mapping. The suspected retained Image reference into the
mutable staging buffer is therefore not established by these successful paths.
Do not label it a demonstrated lifetime bug or add a persistent ELF cache to
solve an unverified problem.

The actual redundancy is in fresh spawn: it temporarily registers the eager
Image plan in the global lazy-mapping Vec, then materialize_lazy_page looks up
the current VM and scans that registry for every image page. Evaluate a small
candidate that keeps the same Image value local and reuses the existing exec
page constructor, renamed materialize_image_page. Its allocation, ownership
registration, full zero/file-overlay initialization and IPC/TLS fixup are
unchanged. The destination is a fresh VM; strict MAP errors now propagate as
they already do for exec, instead of accepting IllegalOperation through the
generic lazy mapper. Stack lazy mappings and actual fault handling remain
unchanged. No new cache, protocol, capacity, Kernel change or deferred work.

This is not cycle 19's rejected zero-elision/page-initialization candidate:
there is no fill_lazy_page change, segment-order change or changed fill/PTE
publication order. It only removes fresh-spawn registry traffic and the
already-populated-page check. Existing source snapshots and capability
authority are preserved.

Also inspect allocation-only overlap/freshness checks using backed_pages to
produce whole Vecs. The ordinary fresh destinations/ranges are empty, so an
early-exit query alone is not yet a convincing hot Tiny Core optimization.
Leave that separate change unimplemented rather than bundle an unmeasured
tree-query refactor into this experiment.

Build the default Release with both profiling options disabled. Alpha hash:
34920d45563473039ba9ee5bf8a075242f0465609a859d8b65eec01722142a2e.
The unchanged extended lifecycle probe remains
0c40e2d2c0b867e2cdce8a4ee643e9e49d03fb2d8dfc3ae3091066cec2daeea2.
Baseline: /private/tmp/nanami-opt-cycle26-lifecycle-after.img (85374862...).
Candidate: /private/tmp/nanami-opt-cycle27-eager.img.
Archive: /private/tmp/nanami-opt-cycle27-eager-candidate.patch; reverse apply
check succeeds against the candidate source. All earlier dirty changes remain.
The native suite passes 1,099 tests (12 ignored), and AArch64 Alpha Release
checking passes. These existing host tests do not directly model the whole
spawn constructor; the actual guest comparison exercises that production path.
Logs: /private/tmp/nanami-opt-cycle27-{native,arm,eager-build}.log.

Normal-device four-vCPU sequential before/after/after/before process runs,
with no simultaneous build, host benchmark or other VM:

| Workload (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| getpid-2048 | 38.589 | 38.467 | 40.486 | 38.639 |
| getuid-2048 | 157.194 | 155.800 | 153.913 | 150.807 |
| copyout-normal | 272.028 | 272.304 | 272.524 | 269.395 |
| copyout-128-vmas | 273.015 | 272.887 | 279.136 | 274.046 |
| copyout-256-vmas | 273.077 | 277.068 | 272.945 | 275.031 |
| fork-8 | 18.183 | 18.271 | 17.938 | 17.145 |
| fork-exec-8 | 665.429 | 664.390 | 659.604 | 669.737 |
| fork-128-vmas-8 | 18.317 | 18.930 | 18.505 | 18.115 |
| fork-256-vmas-8 | 18.453 | 18.360 | 18.533 | 19.726 |
| alpha-spawn-reap-8 | 24.096 | 26.513 | 23.549 | 23.150 |
| alpha-breakdown-spawn | 2.576 | 2.928 | 2.576 | 2.526 |
| alpha-breakdown-kill-reap | 0.366 | 0.535 | 0.385 | 0.396 |
| negoto-create | 7.700 | 6.594 | 8.593 | 7.117 |
| alpha-xfe-spawn | 5.513 | 5.008 | 4.910 | 5.528 |
| alpha-xfe-kill-reap | 0.576 | 0.592 | 0.572 | 0.625 |
| alpha-lifecycle-busybox-8 | 29.294 | 29.344 | 27.981 | 28.656 |
| alpha-lifecycle-busybox-8-prepare | 2.646 | 2.849 | 2.726 | 2.806 |
| alpha-lifecycle-busybox-8-source-release | 0.719 | 0.706 | 0.791 | 0.749 |
| alpha-lifecycle-xfe-3 | 30.365 | 29.326 | 29.565 | 29.808 |
| alpha-lifecycle-xfe-3-prepare | 10.510 | 10.105 | 10.374 | 10.178 |
| alpha-lifecycle-xfe-3-source-release | 1.792 | 1.662 | 1.753 | 1.647 |

Pooled Xfe construction is 5.5205 -> 4.959 ms (10.2% lower), with both
candidate boot medians below both baselines. Complete Xfe input/three-child/
cleanup lifecycle is only 30.0225 -> 29.4455 ms (1.9% lower); complete BusyBox
lifecycle changes 29.122 -> 28.916 ms (0.7% lower). Keep the whole input
preparation and source release costs inside these figures.

Important contrary controls: native eight-create/reap rises 10.6% pooled,
BusyBox construction 9.6%, and Negoto create 5.3%. Both candidate native
create/reap boot medians do not beat both baselines. Other controls range
between a 4.4% lower 256-VMA fork and a 4.3% higher getpid; fork/exec changes
only -1.1%. Do not infer a uniform gain from the Xfe-only construction result,
attribute unrelated movement to the removed registry search, or silently drop
the slow candidate boot. Full raw samples and serial-directory paths remain in
/private/tmp/nanami-opt-cycle27-process-{before1,after1,after2,before2}.log.

For disk space, remove only three obsolete, owned, terminal benchmark images
after lsof confirms no open handle: cycle21-init.img, cycle23-restored.img and
cycle24-restored.img. Their logs/source archives remain regenerable; keep both
cycle26 matched lifecycle images and cycle25-batch.img. No user image is removed.

Normal-device four-vCPU sequential actual desktop startup/reuse comparison:

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| Session 1 | 6118.750 | 6235.308 | 6136.874 | 6130.723 |
| Session 2 | 6716.631 | 6749.506 | 6537.158 | 6705.307 |
| Session 3 | 6630.493 | 6940.963 | 6826.229 | 6726.202 |

All 12 X11-ready / kill / delete / same-ID reuse sessions pass. First-session
medians change 6124.737 -> 6186.091 ms (+1.0%), later sessions
6710.969 -> 6787.868 ms (+1.1%). Neither improves by 5%; both absolute deltas
are smaller than the 100-ms observation interval. Do not call these small
differences a demonstrated startup regression or speedup. Logs:
/private/tmp/nanami-opt-cycle27-startup-{before1,after1,after2,before2}.log.

Reject this candidate. The repeatable Xfe construction-only improvement does
not justify mixed smaller-image results and essentially unchanged complete
lifecycles/startup. Revert only this cycle's process_requests.rs edits using
apply_patch; compare the whole file byte-for-byte to the saved starting source.
That comparison succeeds and all earlier accepted changes remain. The archived
candidate passes git apply --check against the restored tree. Do not keep a
source refactor merely because it eliminates a theoretically redundant lookup.

Rebuild the default image with both profilers disabled. The resulting Alpha
and nanami/alpha.elf both return exactly to
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b,
the measured cycle-26 accepted binary. Restored verification passes 1,099
native tests (12 ignored), 137 focused ASan tests (two ignored), and AArch64
Alpha Release checking. Logs:
/private/tmp/nanami-opt-cycle27-restored-{build,native,asan,arm}.log.
Restored image: /private/tmp/nanami-opt-cycle27-restored.img.
After authoritative session completion and lsof verification, delete only the
rejected cycle27-eager.img; keep its source patch, build log and all timing/
serial logs. The experiment is reproducible without retaining that disk image.
No A9N, public protocol, version or release metadata change is retained.

Next: expand beyond construction timings to actual desktop steady-state work.
Current Alter sends periodic fbdev present notifications, and Honoka's
drain_window_damage invokes fbdev_damage::capture to sample the live surface
into its stable image. That has existing exact-byte and sparse-damage host
tests, but their ignored host benchmark is not guest FPS. First measure the
guest notification/capture/compose path during idle and real X11 redraw before
selecting a change. Unmodified fbdev clients do not publish dirty rectangles;
do not improve a benchmark by suppressing required updates, lowering refresh
rate or rereading live producer pixels after comparison. Native damage-queue
clients and fallback rendering must keep their existing contracts.

## Cycle 28: real desktop capture profiling and a rejected row-major candidate

Move beyond process construction, as cycle 27 proposes. Source review confirms
the unmodified Tiny Core Xfbdev path: Alter periodically notifies presentation;
Honoka samples the shared image into a stable capture, identifies exact damaged
tiles, composes changed rectangles and presents them. Existing NANAMI_FB_PROFILE
wall measurements start after capture, so they cannot attribute this scan.

Temporarily extend that opt-in profiler to time each fbdev capture with the
existing monotonic timer. Emit 120-call batches with changed-call and rectangle
counts, total and maximum elapsed microseconds. Preserve partial capture totals
when the existing render report resets. This adds clock RPCs and includes
preemption/scheduling; it is diagnostic wall time, not exclusive CPU time or
frame latency. Do not attribute its differences to IPC, CPU caches or TLBs.
The complete two-file instrumentation is archived in
/private/tmp/nanami-opt-cycle28-capture-profile.patch and is removed afterward.

Run the real four-vCPU normal-device Tiny Core desktop through two sessions
with X11/FLWM/wbar, physical-input injection, title dragging, Terminal/Editor/
Information launch, Exit and same-ID restart. Baseline and candidate both pass.
Their probe is identical (0c40e2d2...); Alpha remains 85374862....
Logs /private/tmp/nanami-opt-cycle28-{profile,row-profile}-desktop.log.
Full serial/screens:
- /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-ot7z1x00
- /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-03hjogr5

The baseline reports 93 complete capture batches (11,160 calls), 688 changed
calls, and 32,598,296 elapsed microseconds. Its 20 fully unchanged batches have
a median 3,579.429 us/capture. Post-ready compose/present reports commonly
total only a few milliseconds per roughly two-second interval. These spans
do not cover the same exclusive CPU budget; they identify a path to examine,
not a proven percent of total system CPU or all latency. UART and the extra
timer requests also affect this diagnostic execution.

Evaluate row-major strips of 32 tile columns. A fixed u32 dirty mask tracks
columns while rows are sampled in source order; further strips handle any
width without heap scratch storage or a resolution limit. The same pixels
are sampled once, stable captured values are written, tiles remain 32x16 and
the exact same rectangle sequence is emitted, including runs crossing strips.
No update rate, damage accuracy, notification path, FPU support, GPU protocol
or Kernel code is changed.

A differential test extends beyond 1,024 pixels: widths 0/1/31/32/33/1023/1024/
1025/2049, both 64-bit alignment parities, partial bottom tiles, padding and
twelve moving patterns. Compare complete captured bytes and exact rectangle
order against the previous accepted capture. Existing scalar-reference,
single-pixel, idle and full-update tests also pass. The candidate passes 1,100
native tests (12 ignored), 34 focused ASan tests (one ignored), and AArch64
Honoka Release checking. The first-session Editor screenshot is visually
inspected and displays the expected Terminal, editor and launcher.

Diagnostic candidate results look promising: also 93 complete capture batches,
mean elapsed time 2,920.994 -> 2,545.153 us/capture, and median of fully unchanged
batches 3,579.429 -> 3,047.867 us/capture (20 before / 19 after). Input/update
timing and scheduling are not identical fixed work, so these are not accepted
speedups. Inspect a diagnosis-free, controlled capture workload as well.

Freeze the accepted 2026-11-01 capture loop as a test-only baseline. Extend the
existing same-VM frame fixture to four samples, using implementation orders
before/after, after/before, after/before, before/after. Both buffers reset
outside the timer. Independently assert exact expected damage areas: 0 idle,
61,440 for one changed pixel per each of 120 frames, 57,600,000 for complete
updates. This improves the former always-baseline-first / paired-count-only
fixture. Pixel writes for sparse/full producer work remain inside the timer
equally for both implementations; capture buffers are ordinary native RAM.
No compositor, X11 round trip, display scanout or FPS measurement is included.

Run two independent normal-device, four-vCPU QEMU/TCG boots, sequentially with
no overlapping VM/build/host benchmark and all profilers disabled. Candidate
Honoka is 24de3e72..., probe 607669b5..., Alpha remains 85374862....
All 24 frame rows per boot and the Linux metadata/AF_UNIX checks pass:

| 120-frame workload (ms) | Baseline boot 1 | Candidate boot 1 | Baseline boot 2 | Candidate boot 2 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 60.861 | 61.782 | 61.356 | 61.271 |
| One changed pixel/frame | 61.092 | 61.180 | 60.763 | 61.326 |
| Entire image changes | 116.849 | 187.710 | 115.883 | 186.921 |

Pooled eight-sample medians are 61.1925 -> 61.286 ms for idle (+0.2%),
60.895 -> 61.199 ms for sparse (+0.5%), and 116.1195 -> 187.327 ms for full
updates (+61.3%). The full-update slowdown repeats in both boots despite
passing correctness tests. Reject the row-major candidate. Favorable actual
desktop diagnostic aggregates do not outweigh this reproducible regression.
Logs: /private/tmp/nanami-opt-cycle28-frame-perf-{1,2}.log.

A brief explicit-four-store source alternative is also discarded without a
speed claim. The inspected row candidate binary already lowers the existing
32-byte array write to four scalar movq stores (0x1012384..0x101238f), not a
memcpy helper call or an extra source reread. It does not establish a removable
copy. The proper function-boundary disassembly is archived in
/private/tmp/nanami-opt-cycle28-row-asm.log. Do not present hypothetical source
simplification as measured optimization.

Restore fbdev_damage.rs, profile.rs and compositor.rs byte-for-byte to their
saved starting sources using apply_patch. Those comparisons pass. All prior
accepted optimizations remain. The rejected full source/test candidate is
archived in /private/tmp/nanami-opt-cycle28-row-candidate.patch; a runtime-only
archive, /private/tmp/nanami-opt-cycle28-row-runtime-rejected.patch, passes
git apply --check against the final restored source. The temporary profiler
archive also passes git apply --check. No runtime change from this cycle remains.

Retain the wider/unaligned differential regression, the fixed accepted capture
snapshot (fbdev-capture-baseline.rs), the balanced-order guest benchmark and its
independent damage-area assertions. The guest's before=true now denotes that
accepted snapshot, not the older pre-batching reference retained separately for
host correctness. Historical 2026-10-31 speedup tables do not describe this new
comparison. With production restored to that same algorithm, an independent
normal control boot gives baseline/current medians (120 frames): idle
61.668/60.8515 ms, sparse 61.1465/61.333 ms, full 116.051/115.9465 ms. Do not
claim a new speedup from those small same-algorithm differences. All fixed
work/area assertions and Linux performance fixture checks pass. Log:
/private/tmp/nanami-opt-cycle28-final-control.log.

Final validation passes 1,100 native tests (12 ignored), 34 targeted ASan tests
(one ignored), and AArch64 Honoka Release checking. Logs:
/private/tmp/nanami-opt-cycle28-final-{native,asan,arm,probe-build,build}.log.
The normal final image is /private/tmp/nanami-opt-cycle28-final.img, with
Honoka 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c,
probe 746d81929b9e11f4f2edf2401f6415ccedc37c83d5a6dc0fb99fe5c09dd868d1,
and unchanged accepted Alpha
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b.
There is no profiling RDTSC/RDTSCP in normal Honoka. A9N remains clean; public
protocols, release metadata and version are unchanged.

For disk space, remove only owned obsolete/test images after their authoritative
sessions finish and lsof confirms no open handle: cycle25-batch.img,
cycle27-restored.img, cycle28-profile.img, cycle28-row-profile.img and
cycle28-row.img. Keep cycle26's matched before/after lifecycle images and the
cycle28 final normal image. Source archives, raw timing logs and screenshots
are preserved, so the deleted experiments can be regenerated.

Next: obtain lower-observer-effect local-counter spans in the actual capture,
composition and presentation paths before choosing another change. Existing
diagnostics::local_stamp can be used opt-in without clock RPCs, comparing only
the same core's endpoints and reporting ticks rather than inventing a
frequency. Such elapsed ticks still include preemption and are not exclusive
CPU time. Keep the controlled full-update workload in every candidate gate;
do not repeat row-major traversal or explicit stores as untested speedups.

## Cycle 29: same-core desktop spans and two rejected comparison variants

Revalidate cycle 28's useful but observer-heavy diagnostic. Temporarily replace
Honoka's opt-in Profile clock queries with diagnostics::local_stamp. Add capture
spans and report capture/compose/present counters together after 120 captures
or renders, including idle notifications that produce no dirty rectangles.
Report only raw elapsed counter ticks; do not assume a frequency or compare
different cores' timestamps. The tested Honoka PID 14 is configured with fixed
affinity=1, priority=24. These are elapsed regions including interruptions and
time waiting in real rendering/display calls, not exclusive CPU cycles or an
IPC-transport cost measurement.

The existing clock timer, alarm scheduling, input handling and refresh rate
are untouched. Capture samples include local damage callback work. Reporting
occurs after measured spans; it still perturbs later scheduling. The temporary
two-file change is archived as
/private/tmp/nanami-opt-cycle29-local-profile.patch; no part is retained.

Run the real normal-device four-vCPU desktop through two complete sessions:
X11/FLWM/wbar, title drag, real input, Terminal/Editor/Information, Exit and
same-ID restart all pass. Log:
/private/tmp/nanami-opt-cycle29-local-profile-desktop.log.
Full serial/screens:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-3qedqb97.

There are 94 complete reports: 11,280 captures, 702 changed calls, 1,292 renders;
capture 42,376,804,000 ticks, compose 1,202,447,000 and present 778,912,000 ticks.
Eighteen reports contain no capture changes. The scan remains a large sampled
region without measurement clock IPC. This does not establish its share of all
system CPU, prove a cause for cycle 28's different units/measurements, or turn
raw TSC units into nanoseconds. Partial report tails and uninstrumented work
are not included. Do not explain this as expensive IPC transport or a proven
cache/TLB problem.

Evaluate a distinct, smaller comparison candidate. Preserve tile traversal,
the per-row alignment check and the eight-pixel outer loop, but compare four
u64 words independently using a constant inner loop. Only changed words are
written, reducing simultaneous sampled live values. Every source word is still
sampled once; all outputs use sampled values, and captured contents/rectangles
remain exact. This is not cycle 28's explicit-store spelling change: comparison
grouping and which matching words are rewritten also change. No FPU/SIMD, new
ISA requirement, cache, protocol or Kernel support is introduced.

The candidate passes 1,100 native tests (12 ignored), 34 targeted ASan tests
(one ignored) and AArch64 Honoka Release checking. Logs:
/private/tmp/nanami-opt-cycle29-words-{native,asan,arm}.log.
Honoka hash: 0f72b966..., probe: d484a648..., Alpha unchanged at 85374862....
Source archive: /private/tmp/nanami-opt-cycle29-words-candidate.patch.

Two independent sequential four-vCPU normal-device QEMU/TCG boots use cycle
28's frozen accepted baseline, balanced four-sample orders, 120 frames and
independent expected damage-area assertions. No build, host benchmark or
other VM overlaps either timed run:

| 120-frame workload (ms) | Baseline boot 1 | Candidate boot 1 | Baseline boot 2 | Candidate boot 2 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 86.082 | 95.054 | 86.410 | 96.936 |
| One changed pixel/frame | 86.490 | 95.227 | 86.665 | 95.889 |
| Entire image changes | 195.815 | 176.950 | 196.786 | 179.083 |

The independent-word candidate slows idle by 10.4% / 12.2% and sparse updates
by 10.1% / 10.6%, while full updates improve 9.6% / 9.0%. Reject it: a full-motion
gain does not justify slowing the much more frequent unchanged/small-update
path observed in the real desktop. All correctness/area and Linux metadata/
AF_UNIX checks pass. Logs:
/private/tmp/nanami-opt-cycle29-words-frame-{1,2}.log.

Next evaluate short-circuit comparison within the existing four-word sample.
Keep every source read, the group-wide captured-value write and all traversal
unchanged. Compare captured words until the first mismatch; after one mismatch,
the other captured reads/XOR/OR work is unnecessary to decide that the group is
dirty. This could favor changed groups but may penalize unchanged ones, so use
the same idle/sparse/full gate instead of assuming fewer source operations wins.

The focused fbdev/compositor suite passes 34 tests (one ignored). Honoka is
68ed66a4..., probe 9e07ed46..., Alpha remains 85374862....
Archive: /private/tmp/nanami-opt-cycle29-short-candidate.patch.
Same controlled two-boot comparison:

| 120-frame workload (ms) | Baseline boot 1 | Candidate boot 1 | Baseline boot 2 | Candidate boot 2 |
| --- | ---: | ---: | ---: | ---: |
| Idle | 61.889 | 64.147 | 61.320 | 65.142 |
| One changed pixel/frame | 60.436 | 64.514 | 61.380 | 64.966 |
| Entire image changes | 114.175 | 110.504 | 114.489 | 111.615 |

Idle slows 3.6% / 6.2%, sparse updates 6.7% / 5.8%; full updates improve only
3.2% / 2.5%. Reject this variant as well. No 5% improvement is established, and
the common workload gets worse. Logs:
/private/tmp/nanami-opt-cycle29-short-frame-{1,2}.log.
Do not pool the independent-word and short-circuit artifacts: even the unchanged
source baseline differs in absolute timing (about 86 vs 61 ms idle). Their
generated caller context/layout differs; these observations do not establish
the cause. Each within-artifact comparison has its own balanced samples.

Restore fbdev_damage.rs, profile.rs and compositor.rs byte-for-byte to the
saved starting sources using apply_patch. All three comparisons pass; both
candidate archives and the profiler archive pass git apply --check afterward.
Default rebuilding returns exactly to cycle 28's accepted binaries:
Honoka 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c,
probe 746d81929b9e11f4f2edf2401f6415ccedc37c83d5a6dc0fb99fe5c09dd868d1,
Alpha 853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b.
Restored native tests: 1,100 passed (12 ignored); focused ASan: 34 passed (one
ignored); AArch64 Honoka Release checking passes. Logs:
/private/tmp/nanami-opt-cycle29-restored-{probe-build,build,native,asan,arm}.log.
Image: /private/tmp/nanami-opt-cycle29-restored.img.
There is no retained runtime/protocol/Kernel/version/release change.

Remove only owned terminal-job diagnostic/candidate images after authoritative
completion and lsof checks: cycle29-local-profile.img, cycle29-words.img and
cycle29-short.img. Keep all patches, raw timing/serial logs and the restored
normal image. No user image is deleted; deleted experiments are regenerable.

Next change the level of investigation instead of repeating scalar capture
variants. Source review of profile/usr/local/bin/nanami-desktop finds seven
separate ln invocations rebuilding fixed FLWM menu links on every launch,
plus one ln invocation for the default picture alias (eight setup commands).
The generated pre-boot root has no root/.wmx directory. These are default
configuration artifacts, whereas rootfs/menu packaging is already the Tiny Core
builder's responsibility. Investigate producing default menu links in the
image builder so normal startup does not launch these configuration helpers.
Do not claim a startup gain yet, silently discard user menu customization, or
write through archive/profile symlink parents. Verify fresh-build provenance,
console-vs-desktop separation, real cold/start/restart times, launcher actions
and failure handling before adopting a packaging change. No Kernel change is
needed for that investigation.

## Cycle 30: construct default desktop links in the image builder

Move the seven fixed FLWM menu aliases and the default picture alias into
graphical Tiny Core rootfs assembly. These are configuration defaults, not
per-session work: the original script executed eight separate ln helpers on
every desktop start. Keep the existing directory setup and all X11 readiness,
client startup and cleanup behavior. Do not change the pinned upstream ELFs,
Alpha, Alter, Honoka, Kernel, public protocol, or IPC fastpath.

desktop.configure_links reuses the existing safe merge routine. It rejects a
symlink root or parent directory and conflicting aliases/files rather than
following or replacing them on the host; identical aliases are idempotent.
Install defaults before copying the profile, so an upstream root alias is
rejected before profile files could be copied through it. Only graphical
assembly creates these aliases, recording all eight in manifest.json under
profile_symlinks. Rebuilding a fresh rootfs reinstalls defaults; restarting an
existing desktop preserves guest menu and picture customizations.

The builder passes 26 tests, including six new checks for exact defaults,
repeat installation, destination/root symlink rejection, conflict preservation,
console separation, manifest records and removal of runtime link creation.
Fresh offline assembly succeeds from the original pinned rootfs/TCZ archives:
/private/tmp/nanami-opt-cycle30.0Ozbcq/fresh.
Tests: /private/tmp/nanami-opt-cycle30-builder-final-tests.log.

Build a matched baseline by cloning that freshly assembled overlay, removing
only the eight new aliases (and their now-empty directories), and restoring
the original desktop startup script. All packages, bundle configuration and
normal native binaries remain identical. The immutable timing probe is
746d81929b9e11f4f2edf2401f6415ccedc37c83d5a6dc0fb99fe5c09dd868d1;
Alpha remains 853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b
and Honoka remains 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c.
Both images are built with all profilers unset, the same 256-MiB rootfs size
and the same build options:
/private/tmp/nanami-opt-cycle30-{before,after}.img.

Run --tinycore-startup-perf --normal-devices --smp 4, sequentially in
before/after/after/before order, with no concurrent builds, VMs, tests or
benchmarks. Each boot measures three OCI create/start operations until the
real aterm child writes a fresh X11-ready marker, then kills/deletes the
execution context outside the timer before reusing the same ID. All twelve
sessions pass readiness, cleanup and ID reuse. Polling resolution is 100 ms.

| OCI create/start to X11 client (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 6243.989 | 5317.954 | 5219.436 | 6239.086 |
| Second session | 6702.715 | 5588.260 | 5697.821 | 6703.038 |
| Third session | 6627.798 | 5720.628 | 5820.869 | 6822.621 |
| Boot median | 6627.798 | 5588.260 | 5697.821 | 6703.038 |

Pooled medians are 6665.2565 -> 5643.0405 ms (15.3% lower). First-session
medians are 6241.5375 -> 5268.695 ms (15.6% lower); later-session medians
are 6702.8765 -> 5709.2245 ms (14.8% lower). Every candidate sample is below
every baseline sample. Both baseline boots log 100 process starts and both
candidate boots 76, consistent with eight fewer helpers per desktop start.
This is the actual desktop startup workload, not a general application/CPU
throughput improvement. Logs:
/private/tmp/nanami-opt-cycle30-startup-{before1,after1,after2,before2}.log.
Serial directories, in that order:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-720t45bo,
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-y2ctrnng,
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-iuy4k4th,
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-znvy27q4.

Extend the full GUI fixture, outside any timed comparison, to run
tests/alter-linux/desktop-links.sh in the real X terminal. The first session
checks all default aliases, customizes the Clock menu alias and replaces the
picture alias with a regular file. The second session checks the other defaults
and both customizations after desktop Exit/restart. The native observer removes
stale markers and independently reads each session's success file through VFS;
input echo is not evidence. Only --with-tests packages this script. The normal
desktop contains no new test helper or runtime subprocess for this check.

The expanded full desktop fixture passes both normal-device four-vCPU sessions
in one boot, including the raw desktop-syscall, namespace and file-mapping
regressions; real USB input; Terminal, Editor and Information launch; initial
terminal close; Exit/restart cleanup; and both customization readbacks. Log:
/private/tmp/nanami-opt-cycle30-gui-desktop.log. Serial and screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-srs4w8oc.
This untimed image includes freshly compiled raw fixtures and the expanded
native observer (SHA-256
2e6eef2c3829db0c3d64606b41138b2de32d4f6e59268600ac051f3702dab8f0).
Its Alpha/Honoka hashes still match the timing images. Keep the verified normal
image /private/tmp/nanami-opt-cycle30-gui.img. Shell syntax and git diff --check
pass; the real packaging tests remain 26/26. The unchanged production runtime
also passed all 1,100 native regressions at the preceding release checkpoint
(12 opt-in benchmarks ignored): /private/tmp/nanami-v0.7.1-release-native-tests.log.

Accept the packaging change for its repeatable 15.3% actual-startup reduction,
with no claim that rendering throughput or every application improved. A9N's
worktree stays clean. After authoritative completion and lsof checks, remove
only the two owned timing images (before/after, about 642 MiB) to recover disk
space. Fresh/baseline overlays, original script, build/timing/serial logs and
the verified GUI image remain available to regenerate the comparisons. No
user image, source file or live image is removed.

## Cycle 31: bounded small-file snapshot working set

Revalidate cycle 30's retained implementation. The previous goal turn made
progress with a measured 15.3% real-startup reduction and two full GUI sessions;
there is no repeated blocking condition. Keep the full optimization objective,
service boundaries, ownership checks and Kernel unchanged.

First use NANAMI_ALTER_PROFILE=1 with a temporary clone-call hook. Three real
startup/kill/delete/reuse sessions complete. All 51 logged ordinary clone calls
use flags 0x1200011 (48 BusyBox, three aterm); none requests CLONE_VM or VFORK.
Three clone3 attempts come from FLWM; their first argument is a user pointer,
not a decoded flag value. Do not silently turn ordinary fork into a shared VM.
Alpha still rejects exec while a VM has live peers; adding true shared-vfork
exec requires a separate detachment design.

The 1,155 flushed syscall reports include mmap: 2,069 calls / 5,149,126,000
local ticks; clone: 47 / 2,180,192,000; execve: 52 / 806,935,000; openat:
1,247 / 580,804,000; recvmsg: 1,863 / 392,151,000. Reports cover all guests
since the previous exit-triggered flush, not just the trigger PID. Live guests
can leave unflushed totals at shutdown, explaining different clone counts.
These are elapsed handler spans including IPC waits/descheduling, not exclusive
CPU time, calibrated nanoseconds or evidence of slow IPC transport. Log:
/private/tmp/nanami-opt-cycle31-alter-profile-startup.log; serial:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-kj51h3zi.

A second diagnostic adds mmap classes and cache lookup records. It completes
the same three sessions. Flushed anonymous/private-file/shared-file counts are
329 / 1,716 / 39; their ticks are 421,101,000 / 7,104,267,000 / 149,925,000.
The extra per-file UART records are inside some mmap spans, so do not compare
these totals with the first diagnostic as a performance change. There are
2,593 cache lookup records across 66 identities: loader 79 hits/437 misses,
mapping 1,657 hits/420 misses. Replaying the exact access sequence through the
original eight-slot LRU matches every observed hit/miss. Its cacheable snapshot
fills read 146,274,190 bytes. A capacity/admission model predicts fewer rereads
with more small-file slots; that model is a candidate selector, not a speedup.
Log: /private/tmp/nanami-opt-cycle31-mapping-profile-startup.log; serial:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-r_j1dlje.
Instrumentation is archived in /private/tmp/nanami-opt-cycle31-mapping-profile.patch
against the pre-candidate sources. Restore fault.rs, syscall_profile.rs and
loader.rs byte-for-byte before building any normal comparison image.

The candidate admits up to 48 snapshots while keeping retained data at the
original 16-MiB bound and individual cacheable files at 2 MiB. Admission counts
actual retained capacities and page-rounded/geometric growth, choosing the
least-recently-used fitting slot. It never shrinks a reusable buffer. If no
slot fits, use the existing uncached execution/interpreter buffer, not a new
process or file-size restriction. The standard Alpha heap service returns the
requested 4-KiB-rounded grant. The byte budget covers retained cache buffers,
not transient growth, existing deferred physical releases or uncached buffers.
The existing geometric-growth bound is preserved.

Capacity-constrained admission can select a recent large buffer rather than
the oldest small one. Therefore PT_INTERP loading explicitly passes the live
main ELF address as a protected victim exclusion. A hit remains valid, but
a miss cannot overwrite/free that outstanding executable snapshot. This is
an internal loader parameter, not a new wire operation. File tokens, fstat
validation, read-failure ownership and private guest copies remain unchanged.
Cache metadata stays below 64 KiB (production-type test), not 48 times 2 MiB
of eagerly allocated RAM.

The original nine loader regressions still pass. Three new production-source
tests cover a 32-file resident working set with fresh bytes, large-file churn
at the 16-MiB bound, and a full budget where the main ELF is the only fitting
victim: the interpreter must use its separate fallback buffer and preserve the
main bytes. Full native tests pass 1,103 (12 opt-in benchmarks ignored), the
12 cache tests pass AddressSanitizer, and AArch64 Alter/Linux Release checking
passes. Logs: /private/tmp/nanami-opt-cycle31-cache-{native-all,asan,arm}.log.
Formatting and git diff --check pass.

Build paired normal images from the same fresh cycle-30 overlay and unchanged
2e6eef2c3829db0c3d64606b41138b2de32d4f6e59268600ac051f3702dab8f0 probe.
All profilers are unset. Baseline Alter/Linux and FreeBSD hashes are
c496cbe9cb313e60487febd7ebc934d601cdc50c38f762371d3c3875ecb04728 /
212ee62baf2d698f376d6b8cc34eef45b47639573248c3145bdb8d4fee28382b;
candidate hashes are
58596c9f1271e8b5b680ca3e5a676ff07ab3e44355e227bf7cb814259f49cada /
c6e1f716df51b1ac0590d00c7a6cc69a0c8ce1837823f3af9b2d83ab3bc5e28f.
Alpha stays 853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b;
Honoka stays 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c.

Run two consecutive ABBA blocks with --tinycore-startup-perf --normal-devices
--smp 4, sequentially without concurrent builds, benchmarks, tests or VMs.
All 24 fresh X11-client startup/kill/delete/same-ID reuse sessions pass.
Milliseconds, including every observation:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 5411.056 | 5210.956 | 5120.571 | 5418.508 |
| Second | 5698.451 | 5401.008 | 5408.271 | 5712.055 |
| Third | 5810.737 | 6320.028 | 5505.737 | 5808.187 |

| Session | Before 3 | After 3 | After 4 | Before 4 |
| --- | ---: | ---: | ---: | ---: |
| First | 5309.505 | 5128.252 | 5120.399 | 5316.865 |
| Second | 5603.226 | 5515.141 | 5511.798 | 5709.518 |
| Third | 5713.501 | 5606.611 | 5514.668 | 5820.742 |

The first block alone gives a 5.3% pooled median reduction. Repeating it lowers
the combined result to 5703.9845 -> 5457.004 ms (4.3% lower), not a proven 5%
cycle gain. Cold medians are 5363.9605 -> 5124.4115 ms (4.5%); later-session
medians 5712.778 -> 5513.233 ms (3.5%). Every candidate boot median is below
every baseline boot median, but one candidate third session is slower at
6320.028 ms; retain it and do not attribute its cause without evidence.
Polling resolution is 100 ms. These observations concern startup, not every
application, renderer or hardware target. Logs:
/private/tmp/nanami-opt-cycle31-startup-{before1,after1,after2,before2,before3,after3,after4,before4}.log.
Serial directories, in that order:
jzrvah4w / rco6uv1g / 21etlome / p55lvt5v /
p5o9x3vr / 7wvl120w / cr6bjxl0 / 0tk89a0d,
each under /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-.

The final normal-device four-vCPU GUI fixture passes both complete sessions
in one boot: desktop syscalls, namespace/file mappings, USB keyboard/mouse
input, Terminal/Editor/Information launch, initial-terminal close, Exit/restart,
fresh marker readback and preserved menu/picture customizations. Log:
/private/tmp/nanami-opt-cycle31-gui-desktop.log; serial/screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-ua4jrfk0.
Keep /private/tmp/nanami-opt-cycle31-gui.img as the verified current image and
cycle30-gui.img as the previous accepted baseline. The candidate source/test
archive /private/tmp/nanami-opt-cycle31-cache-candidate.patch passes reverse
git apply --check against the current tree. Default fault/profile sources are
byte-identical to the starting sources, and the normal Alter binary contains
none of the temporary clone/map/image profiling strings. A9N remains clean.

Retain the bounded cache change for the consistently lower actual-startup
boot medians, reduced duplicate file work and explicit lifetime/budget checks.
The observed 4.3% improvement is partial progress: it does not meet the 5%
cycle aim, establish a universal speedup, or complete the optimization goal.
Remaining private-page construction/copying and rendering work still need
separate measurement. No Kernel, IPC fastpath, external protocol or release
version changes are made in this cycle.

After authoritative termination/lsof checks, remove the two owned diagnostic
images and both timing images from this cycle. Also remove the older owned
cycle29-restored.img, superseded by the retained cycle30/31 GUI images. Source
archives, fresh/baseline overlays, build/test/serial logs and both latest GUI
images remain; the removed temporary images are regenerable. No user source,
user image or live VM image is removed.

## Cycle 32: combined VM page records (deferred at the v0.7.1 checkpoint)

Test a generic AVL payload holding each backed page's Frame and permissions
together, retaining a sparse permission map only for unbacked entries. This
removes a second lookup for protected pages without changing the bootstrap
index, page-table hints, external protocol, or Kernel. Independent production-
source tests check 12,000 mixed operations against a separate model, including
permission-only entries, descriptor replacement, sparse ranges, predicate order,
and full-word descriptors and attributes. All three tests pass on both versions.

The compact candidate uses 48 rather than 40 bytes per default-permission AVL
node: 20% more metadata for that workload. At 16,384 protected pages, measured
allocation storage falls from 1,216,384 to 786,432 bytes (35.3% less). These are
host allocator footprints, not application timings or total system RAM. An
earlier Option-payload variant used 56-byte nodes and was not guest-tested.

Build matched normal images with the same cycle-31 Alter binaries, cycle-30
desktop overlay, native observer, and freshly verified raw process fixtures.
Run startup and process comparisons sequentially in before/after/after/before
order, with normal devices and four vCPUs. All twelve startup/cleanup/reuse
sessions and all four process suites pass. Startup milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 5211.813 | 5002.637 | 5101.783 | 5212.506 |
| Second | 5403.170 | 5485.122 | 5495.182 | 5502.448 |
| Third | 5618.690 | 5502.564 | 5495.001 | 5521.435 |

Pooled startup medians are 5452.809 -> 5490.0615 ms (0.7% higher).
Process medians, pooling six observations per workload in milliseconds:

| Workload | Before | Candidate |
| --- | ---: | ---: |
| Alpha spawn/reap, eight children | 24.012 | 24.667 |
| Complete BusyBox image lifecycle, eight children | 29.0905 | 29.4445 |
| Complete Xfe image lifecycle, three children | 31.155 | 30.2255 |
| getpid, 2,048 calls | 38.975 | 39.085 |
| getuid, 2,048 calls | 154.8295 | 155.307 |
| Credential copyout, ordinary VM | 273.709 | 270.4595 |
| Credential copyout, 128 VMAs | 272.9985 | 273.167 |
| Credential copyout, 256 VMAs | 276.3605 | 275.507 |
| Fork, eight children | 18.502 | 18.176 |
| Fork/exec, eight children | 669.584 | 678.993 |
| Fork with 128 VMAs, eight children | 20.3305 | 18.4765 |
| Fork with 256 VMAs, eight children | 20.822 | 18.822 |

Fragmented-VM fork improves by 9.1%/9.6%, but ordinary startup and fork/exec do
not improve. Defer the production change at the user's release checkpoint:
the mixed result and larger default-page metadata do not justify shipping it
as a general startup improvement. Restore only this cycle's AVL/VmSpace edits
byte-for-byte to their pre-cycle copies, preserving all accepted earlier work.
Keep the independent VM contract tests, source overrides, and opt-in footprint
benchmark. Archive the deferred source delta in
/private/tmp/nanami-opt-cycle32-release-deferred.patch.

Comparison logs: /private/tmp/nanami-opt-cycle32-startup-{before1,after1,after2,before2}.log
and /private/tmp/nanami-opt-cycle32-process-{before1,after1,after2,before2}.log.
The candidate passed 1,106 native tests, 106 relevant AddressSanitizer tests,
and AArch64 Alpha Release checking. The first ARM check ran out of disk space;
after removing only its unused generated cache, the cached-target retry passed.
This is not a Kernel or IPC fastpath change or a completed optimization goal.

The release checkpoint's restored sources pass all 1,106 native regressions
(13 opt-in benchmarks ignored), with the documented/CI single-threaded test
setting. The Tiny Core builder passes 26 tests and the normal x86_64 Alpha
Release build succeeds. Its SHA-256 is again
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b,
matching the previously verified cycle-31 GUI image. The default alpha.elf is
refreshed from that build. The initial host invocation omitted the required
test-thread setting and failed in shared heap-test mocks; the prescribed serial
run passes without changing production or test logic. Logs:
/private/tmp/nanami-v0.7.1-final-release-native-serial.log and
/private/tmp/nanami-v0.7.1-final-release-alpha.log.

## Cycle 33: initialize PT_INTERP backing once (rejected)

The preceding goal turn makes progress by finalizing the release checkpoint,
excluding cycle 32's unaccepted VM-index experiment, and passing the restored
regressions. Revalidate the actual worktree and terminal build/VM state before
continuing. Keep the full optimization objective, Kernel, service boundaries,
and ordinary private-memory semantics unchanged.

The current private-file mmap path already initializes new backing directly.
PT_INTERP installation still allocates/zeros the complete virtual span and then
copies file segments over it. Try the existing initialized-map operation for
ordered, page-disjoint interpreter segments: copy each file prefix straight
into private backing and zero only its remainder, including gaps. One bounded
4-KiB local buffer handles an unaligned leading page without exposing file
padding before p_vaddr. Do not add a snapshot protocol or share live guest RAM.
Keep the original zero-span/ordered-file-write implementation for overlapping,
shared-boundary-page or unsorted segment layouts. BSS must not erase bytes
written by another file segment in those layouts.

The real pinned Tiny Core ld-linux-x86-64.so.2 has four disjoint page ranges,
including an unaligned final data segment, and qualifies for the candidate.
Six production-source tests exercise preparation and installation with only
synchronous Alpha operations modeled. Independent expected bytes use the old
zero-span/ordered-file-write rule, not the candidate planner. Strengthened
fixtures use distinct file offsets in overlapping writes (equal offsets would
hide ordering mistakes), shared boundary pages with disjoint file bytes, pure
fileless BSS, 256 generated layouts and failure at every request boundary.
The six correctness checks pass both the restored implementation and the
archived candidate. An additional ignored probe reports work counts only:
on its small two-segment fixture, zeroed backing falls from 36,864 to 15,636
bytes, but requests rise from three (one allocation/two writes) to four
initialized maps. This is not a timing benchmark or an application speedup.

The initial candidate passes 1,112 native regressions (13 ignored), six focused
ASan tests, AArch64 Alter/Linux Release checking and a complete normal x86_64
image build. Use the same cycle-32 overlay, freshly verified raw process
fixtures and unchanged 2e6eef2c3829db0c3d64606b41138b2de32d4f6e59268600ac051f3702dab8f0
native observer in both images. Baseline Alter/Linux/FreeBSD hashes are
58596c9f1271e8b5b680ca3e5a676ff07ab3e44355e227bf7cb814259f49cada /
c6e1f716df51b1ac0590d00c7a6cc69a0c8ce1837823f3af9b2d83ab3bc5e28f;
candidate hashes are
d315dfc4893f1535bfcdfa1e0f88241a032d05c4f8e85677ca0db2e0dcdb6ecc /
d8d6627a05eb4f3c979e7c59e6bd6c328b6451d598821775aeda79c7a63e529b.
Alpha stays 853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b;
Honoka stays 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c.

Run each comparison in before/after/after/before order, sequentially with
normal devices, four vCPUs and no concurrent builds, tests, benchmarks or VMs.
All four process suites and all twelve startup/kill/delete/same-ID reuse
sessions pass. Startup milliseconds, retaining every observation:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 5322.049 | 5232.020 | 5105.272 | 5120.519 |
| Second | 5496.370 | 5600.698 | 5591.417 | 5412.282 |
| Third | 5602.698 | 5599.457 | 5498.813 | 5617.901 |

Pooled startup medians are 5454.326 -> 5545.115 ms (1.7% higher), not a
demonstrated improvement. The observer interval remains 100 ms. Pooled process
medians, six observations per workload, milliseconds:

| Workload | Before | Candidate |
| --- | ---: | ---: |
| Alpha spawn/reap, eight children | 29.3275 | 25.1785 |
| Complete BusyBox image lifecycle, eight children | 28.906 | 28.4075 |
| Complete Xfe image lifecycle, three children | 31.5365 | 31.381 |
| getpid, 2,048 calls | 40.457 | 39.1525 |
| getuid, 2,048 calls | 156.4285 | 156.9395 |
| Credential copyout, ordinary VM | 273.926 | 274.7575 |
| Credential copyout, 128 VMAs | 274.8635 | 276.689 |
| Credential copyout, 256 VMAs | 280.367 | 274.5375 |
| Fork, eight children | 19.7535 | 18.3905 |
| Fork/exec, eight children | 685.8245 | 698.284 |
| Fork with 128 VMAs, eight children | 19.6445 | 19.718 |
| Fork with 256 VMAs, eight children | 18.992 | 18.522 |

Do not attribute the apparently faster Alpha control to the interpreter change
when that construction does not execute PT_INTERP and Alpha's binary is
unchanged. Controls vary too; keep the 62.527-ms baseline getpid observation.
The targeted complete fork/exec workload is 1.8% higher and actual startup
does not improve. Removing redundant bytes is insufficient acceptance evidence
for 87 additional production lines and a second placement path. Reject the
candidate rather than advertise a 5% gain or reduce workload/semantics.

Restore dynamic.rs byte-for-byte from the pre-cycle copy, preserving cycle 31's
protected interpreter-cache admission. Keep six independent ELF-content/error
regressions and the ignored work-count probe, with the Cargo test target and
README entry. The candidate-only source patch is archived at
/private/tmp/nanami-opt-cycle33-interpreter-candidate.patch and passes
git apply --check against restored sources. Both versions and derived test
harnesses remain in /private/tmp/nanami-opt-cycle33.p9tyJ8.

The restored tree passes all 1,112 native tests (14 opt-in probes ignored),
six focused ASan tests, AArch64 Alter/Linux Release checking, formatting/diff
checks and the normal x86_64 image build. Default Alpha, Alter/Linux, FreeBSD
and Honoka hashes match their pre-cycle values exactly. No Kernel, IPC
fastpath, protocol, release version or mutable-source sharing change remains.
Logs: /private/tmp/nanami-opt-cycle33-{native-all,dynamic-asan,arm,after-build}.log,
/private/tmp/nanami-opt-cycle33-{process,startup}-{before1,after1,after2,before2}.log,
and /private/tmp/nanami-opt-cycle33-restored-{native,asan,arm,build}.log.
The full serial directories are recorded by each QEMU log's Test logs line.

Before building, remove only the terminal cycle-32 rejected image and the older
cycle-30 GUI image, superseded by the retained cycle-31 verified GUI image.
After authoritative completion and lsof checks, also remove this cycle's
rejected candidate image and regenerate the normal restored image. Keep source
archives, overlays, logs and the latest verified images; no user files or live
images are removed. All deleted temporary images are regenerable.

The rebuilt normal image /private/tmp/nanami-opt-cycle33-restored.img also
passes three fresh X11-ready startup/cleanup/reuse sessions: 5004.026,
5280.808 and 5305.504 ms. These verify restoration, not a new paired speedup.
Log: /private/tmp/nanami-opt-cycle33-restored-startup.log; full serial directory:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-txu1akxo.
Keep this matched-fixture image and the cycle-31 full-GUI-verified image.
After lsof confirms no open handle, remove the now-superseded cycle-32 baseline
image as well; its sources, overlay and comparison logs remain available.

This cycle does not meet the 5% aim or complete the goal. Do not repeat the
PT_INTERP zero-elision path as an assumed bottleneck. A distinct next candidate
to measure is reducing per-page dynamic AVL node allocation/free traffic while
preserving the existing word payload, search order and sparse-page semantics;
that is not cycle 32's larger combined payload or cycle 3's BTree replacement.
Any representation change still needs memory-footprint and real process/startup
comparisons before acceptance, not only an attractive host microbenchmark.

## Cycle 34: dynamic AVL node pools (rejected)

The preceding goal turn makes progress by rejecting cycle 33 after controlled
fork/exec/startup measurements, adding independent ELF regressions and restoring
the accepted runtime. Revalidate current sources, running jobs and storage.
Keep word-sized keys/values, AVL search/range behavior, sparse-page ownership,
bootstrap's fixed tree, Kernel and external protocols unchanged.

First implement safe stable indexes into lazily allocated 64-node blocks,
returning a block when it becomes empty. Public-API/model checks pass, but
each traversal now resolves a block and slot. Its sparse-retention probe is
unfavorable, so do not advance this variant to QEMU. Next try ordinary pointer
links with an optional pool of at most eight 64-node blocks per index (20 KiB
of node backing on the tested 64-bit targets). Full/failed pools use the original
single-node Box allocation; empty blocks return their backing. A private block
tag occupies existing node padding; full-Word keys and descriptors are not
narrowed or packed. Node size remains 40 bytes.

For this second prototype, pooled Vec buffers never grow after publishing node
pointers. Raw buffer pointers avoid creating a mutable reference to the entire
buffer while live node links exist. The tree alone owns pool and fallback
nodes; Drop unlinks/frees both. Internal Send/Sync implementations preserve
the old public bounds. These are additional ownership invariants and unsafe
machinery, not a reason to accept the optimization without measured benefit.

Extend the production-source VM fixture with five generic AVL regressions:
20,000 mixed operations over ordinary and full-Word keys against an independent
model; sorted/reverse insertion, deletion rotations and reuse; storage returned
before dropping an empty tree; ownership moved to another thread followed by
two shared readers; and small-node insertion under larger-allocation denial.
The candidate succeeds at 256 insertions in each denial case (>=64-byte and
>=512-byte allocations), recording 256 failed optional requests before falling
back in each case. The baseline passes the same public contracts without
needing those optional requests. Existing VM permission/range tests still pass.

Expand the ignored footprint probe to non-power-of-two counts and add a churn
probe leaving one key per 64 original keys. Counts are allocation layouts from
the host allocator, not total system RAM or guest page-copy costs:

| VM/tree metadata probe | Original | Block indexes | Bounded pointer pool |
| --- | ---: | ---: | ---: |
| Allocations at 4,096 default-permission pages | 4,096 | 69 | 3,594 |
| Allocated bytes at those 4,096 pages | 163,840 | 165,888 | 164,160 |
| Retained bytes after shrinking 4,096 keys to 64 | 2,560 | 124,928 | 23,000 |
| AvlTree object bytes | 8 | 40 | 40 |
| VmSpace object bytes | 48 | 112 | 112 |

The initial short host timings vary substantially. Compile frozen baseline and
candidate executables and rerun them in ABBA order without concurrent builds,
tests or VMs. Controls also move. A host load observation reaches 55.70 on this
10-CPU machine, with unrelated OS/application activity; no external process is
stopped or retuned. Do not interpret these noisy host timings as an application
speedup or a causal regression. Exact counts/retention remain useful. Alpha's
real fixed mapped heap already uses TLSF (nanami_utils/heap.rs); node pooling
does not remove an asymptotically slow allocator.

The pointer candidate passes 1,117 native regressions (15 opt-in probes ignored),
the relevant existing ASan suites plus its latest eight VM-index checks, and
AArch64 Alpha Release checking. Because the date changes, rebuild both normal
images on 2026-11-02. The paired Kernel binary SHA-256 is identical:
312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.
Baseline/candidate Alpha hashes are
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b /
992c35b305f8e582738e6713e48d7db0830e10ed5c661216b5e7ea339f6c4b4b.
Alter/Linux stays 58596c9f1271e8b5b680ca3e5a676ff07ab3e44355e227bf7cb814259f49cada,
Honoka 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c,
and the matched native observer
2e6eef2c3829db0c3d64606b41138b2de32d4f6e59268600ac051f3702dab8f0.
Use the unchanged cycle-32 overlay and raw fixtures in both images, with all
profilers unset. Kernel source, IPC fastpath and service request paths do not
change.

Run process and startup comparisons separately in before/after/after/before
order, normal devices and four vCPUs, without concurrent builds, tests or other
VMs. Record host load before/after every boot. All four process suites and
all twelve startup/kill/delete/same-ID reuse sessions pass. Process medians,
pooling six observations per workload, milliseconds:

| Workload | Before | Candidate |
| --- | ---: | ---: |
| Alpha spawn/reap, eight children | 23.601 | 25.546 |
| Complete BusyBox image lifecycle, eight children | 28.2875 | 29.107 |
| Complete Xfe image lifecycle, three children | 29.816 | 30.8795 |
| getpid, 2,048 calls | 39.5315 | 40.5805 |
| getuid, 2,048 calls | 154.0715 | 156.7125 |
| Credential copyout, ordinary VM | 250.388 | 269.431 |
| Credential copyout, 128 VMAs | 275.8085 | 272.0015 |
| Credential copyout, 256 VMAs | 270.925 | 276.193 |
| Fork, eight children | 17.0655 | 20.510 |
| Fork/exec, eight children | 669.456 | 732.5645 |
| Fork with 128 VMAs, eight children | 19.002 | 18.6515 |
| Fork with 256 VMAs, eight children | 18.2445 | 18.569 |

Startup milliseconds, including all observations:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 5018.485 | 5524.067 | 5004.012 | 5104.819 |
| Second | 5187.361 | 5497.726 | 5290.941 | 5280.831 |
| Third | 5590.420 | 5700.870 | 5698.399 | 5398.613 |

Pooled startup medians are 5234.096 -> 5510.8965 ms (5.3% higher); fork/exec
medians are 669.456 -> 732.5645 ms (9.4% higher). Host load and controls vary,
so do not claim a precise causal slowdown or expensive IPC transport. There is
no evidence of the requested improvement to justify the added code, sparse
retention or unsafe pool ownership. Reject both variants and restore avl.rs
byte-for-byte. Retain the five independent API/resource regressions and the
ignored churn/retention probe, not a speculative allocator implementation.

Archives /private/tmp/nanami-opt-cycle34-{block-index,pointer-pool}.patch both
pass git apply --check against restored sources. Source copies and frozen host
harnesses remain in /private/tmp/nanami-opt-cycle34.Vvdku7. Logs:
/private/tmp/nanami-opt-cycle34-{before,block,pool}-bench.log,
/private/tmp/nanami-opt-cycle34-host-{before1,after1,after2,before2}.log,
/private/tmp/nanami-opt-cycle34-{process,startup}-{before1,after1,after2,before2}.log
and their corresponding -host.log load records. Full serial directories are
identified by each QEMU log's Test logs line.

The restored sources pass 1,117 native tests (15 ignored), 111 focused ASan
tests (five ignored), AArch64 Alpha Release checking and the full normal x86_64
image build. Default Alpha and all untouched service hashes match baseline;
the paired Kernel hash remains unchanged. No new unsafe production code,
Kernel/ABI change, version change or pooling representation remains. Logs:
/private/tmp/nanami-opt-cycle34-restored-{native,asan,arm,build}.log.

The old cycle-33 normal image is renamed/rebuilt as the new dated baseline,
then retained as /private/tmp/nanami-opt-cycle34-restored.img after restoration.
Remove only this cycle's terminal rejected candidate image after lsof checks
(about 321 MiB); source/test/log archives remain reproducible. Keep the older
cycle-31 full-GUI-verified image too. No user or live VM image is deleted.

The rebuilt normal image passes three more fresh startup/cleanup/reuse
sessions at 4908.454, 5185.678 and 5291.620 ms. This is restored-state
verification, not a new before/after improvement claim. Log:
/private/tmp/nanami-opt-cycle34-restored-startup.log; full serial directory:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-_bkxkdhf.
Formatting/diff checks pass, avl.rs matches its pre-cycle copy exactly, and
A9N's working tree remains clean.

This cycle does not meet the 5% aim or complete the goal. Stop treating another
AVL pool as an assumed high-leverage improvement. The larger remaining option
is reducing actual private page construction/copying with immutable Alpha-owned
snapshots and deferred materialization, not writable parent-frame aliases.
Before implementation, audit real fault routing, nested fork, PROT_NONE,
privileged copies, shared-memory replacement, partial unmap/protection and
snapshot lifetime/budget fallback. Keep the architecture generic and Kernel
unchanged; do not turn ordinary fork into CLONE_VM or weaken ownership checks.

## Cycle 35: Alpha-private capture audit and eager staging experiment

The preceding optimization cycle made progress by measuring and rejecting two
node-pool representations, restoring the accepted runtime, and adding resource
regressions. The intervening release-message request confirms v0.7.1; it does
not authorize treating the optimization objective as complete. Revalidate the
current working tree and keep Kernel, IPC fastpath and external protocols intact.

Audit the actual private-fork, mapping and fault paths before introducing
deferred backing:

- Alpha's native stack is 64 pages at 0x04000000, ending at 0x04040000.
  Alter/Linux uses that same 256-KiB interval. It is not a second, unused stack
  that can simply be omitted from every Linux process.
- Alpha already uses TLSF and has a 32-MiB heap. A retained snapshot payload
  therefore needs a global admission budget and optional eager fallback, not
  an unbounded allocation per child or another allocator layered over TLSF.
- Raw Linux children send faults to Alter's identified service port, not
  Alpha's native demand-page handler. Alter currently turns non-syscall faults
  into synchronous Linux signals, and its local FaultEvent drops the wire
  architecture fault code. Deferral cannot work merely by withholding a PTE.
- The runtime wire decoder preserves the architecture fault code. x86_64 sends
  its page-fault error code; AArch64 sends ESR_EL1, with separate instruction
  and data abort reasons. An x86 present-bit test cannot be reused as an ARM
  demand-fault classifier. Ordinary protection and invalid-address faults
  must still reach Linux signal handling.
- Kernel address_space::complete_address_space_update performs the HAL update
  before sending remote invalidation IPIs; a later IPI error returns FATAL.
  Thus a failed MAP is not proof that no leaf was installed. Moreover,
  MemoryManager::map_frame_with_attribute accepts IllegalOperation as success,
  while map_frame_strict only publishes Attribute::ALL. Deferred pages need
  strict final-permission publication and explicit partial-failure ownership,
  not default RWX followed by protection while shared-VM peers may run.
- Existing checked copies resolve current Frames, mapping release validates
  physical ownership, and exec resets VmSpace/lazy plans before releasing
  backing. Pending snapshot bytes must participate in all of these lifetimes,
  plus nested fork, PROT_NONE and shared-memory replacement. Read-only aliases
  of mutable parent Frames do not provide an immutable private snapshot.

The relevant sources are alpha/threads.rs, alpha/memory_pages.rs,
alpha/memory_requests.rs, alpha/mapping_{protection,release}.rs, process.rs,
vm_space.rs, Alter common/fault.rs and Linux main.rs; Kernel references are
capability/address_space.cpp and both HAL interrupt dispatchers. This audit
changes no Kernel source or guest authority.

First measure whether capturing current private bytes into Alpha-owned RAM is
plausible. A temporary NANAMI_EXEC_PROFILE hook copies backed pages into a
try-reserved Vec, capped at 8 MiB of requested payload. Empty/large/allocation-
pressure cases keep the old path; the temporary capture is dropped before the
unchanged eager clone. The buffer is initialized by extending from each current
Frame, without a preceding zero pass. It is never mapped into a guest. The
normal build has no such hook.

One normal-device four-vCPU diagnostic boot completes all three real Tiny Core
startup/kill/delete/same-ID reuse sessions. It records 51 private clones:
48 with 1,202 pages and three with 1,837 pages, totaling 63,207 pages and
258,895,872 captured bytes. Aggregate elapsed local counter ticks:

| Phase | Ticks |
| --- | ---: |
| Reserve temporary capture storage | 217,000 |
| Resolve/map source and append current bytes | 328,928,000 |
| Drop capture | 186,000 |
| Subsequent unchanged eager clone | 2,134,336,000 |

Capture warms the existing source-alias cache before that eager interval.
These are instrumented spans including descheduling and counter overhead,
not exclusive CPU time, calibrated nanoseconds, or an application speedup.
They support evaluating a backing-lifetime change but do not establish that
eager staging or later demand faults improve end-to-end startup.

Diagnostic patch: /private/tmp/nanami-opt-cycle35-capture-profile.patch.
Original/capture source copies: /private/tmp/nanami-opt-cycle35.DdKWPo.
Build/startup logs: /private/tmp/nanami-opt-cycle35-capture-profile-{build,startup}.log.
Serial: /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-i1b1hove.
The patch passes git apply --check after both instrumented sources are restored
byte-for-byte. Its terminal temporary image is removed after an empty lsof
check; source archives, logs and normal images remain.

Before adding deferred-fault machinery, evaluate a smaller runtime candidate:
capture each contiguous private-fork run into an optional, request-local Vec,
then initialize ordinary independent child Frames from its immutable slice.
The existing page constructor handles both process-Frame and snapshot-slice
inputs, retaining validation, allocation ownership, initialization-before-MAP,
rollback and suspended-child final permissions. Runs over 8 MiB or failed
optional allocation use the original direct constructor. CLONE_VM, nonfresh
destinations and overlap-aware range copying are unchanged. This deliberately
adds a byte-copy pass, so source-contiguity benefits must exceed that cost;
capture timing alone cannot justify retaining it.

The candidate passes 1,121 native tests (15 opt-in benchmarks ignored), including
two snapshot-constructor content/rollback regressions. Two additional general
fork tests check nested children against current child bytes/permissions,
retained PROT_NONE contents, and parent exec plus same-VA replacement. Those
general invariants must remain covered regardless of the candidate's timing.
Log: /private/tmp/nanami-opt-cycle35-staged-native.log.

Candidate Alpha SHA-256:
75a3e815c323625f904fc19148957d770fc3a786c9a1331cb8dc4ead9a57b07a.
The compared normal baseline is /private/tmp/nanami-opt-cycle34-restored.img
with Alpha 853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b.
Both use Kernel 312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88,
Alter 58596c9f1271e8b5b680ca3e5a676ff07ab3e44355e227bf7cb814259f49cada,
Honoka 7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c,
and the unchanged 2e6eef2c3829db0c3d64606b41138b2de32d4f6e59268600ac051f3702dab8f0
observer. The same cycle-32 overlay/fixtures are used, with all profilers unset.

Normal comparisons run sequentially in before/after/after/before order, with
no concurrent builds, native tests, host benchmarks or other Nanami VMs.
Host load averages are 5.65/6.07/8.07 initially and 5.94/6.05/7.45 after
both comparisons on ten host CPUs. No unrelated process is stopped or tuned.
All eight comparison boots exit successfully; serial markers confirm all
12 actual startup/kill/delete/same-ID reuse sessions and all four complete
process/freshness/protection fixtures. Startup observations, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 5137.141 | 5114.944 | 5089.944 | 5537.001 |
| 2 | 5817.770 | 5509.739 | 5603.889 | 5311.766 |
| 3 | 5394.722 | 6428.635 | 5902.655 | 6247.310 |

The pooled startup median is 5465.8615 -> 5556.814 ms (1.7% higher),
not a 5% improvement. The first-session observations happen to be lower in
both candidate boots; later-session variance and the whole-session result
do not justify presenting that subset as an end-to-end gain.

Per-boot process medians, milliseconds; retain both direct fork fixtures
and controls rather than selecting only favorable results:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| alpha-spawn-reap-8 | 26.564 | 27.395 | 26.469 | 33.714 |
| alpha-breakdown-spawn | 3.305 | 3.045 | 2.852 | 2.767 |
| alpha-breakdown-reap | 0.396 | 0.465 | 0.487 | 0.399 |
| alpha-breakdown-lifecycle | 3.687 | 3.474 | 3.330 | 3.185 |
| negoto-create | 8.771 | 9.518 | 8.925 | 8.459 |
| alpha-xfe-spawn | 5.677 | 5.783 | 5.377 | 6.285 |
| alpha-xfe-reap | 0.845 | 0.611 | 0.564 | 0.845 |
| alpha-xfe-lifecycle | 6.488 | 6.527 | 5.941 | 7.082 |
| alpha-lifecycle-busybox-8 | 30.378 | 29.591 | 28.946 | 35.356 |
| alpha-lifecycle-xfe-3 | 31.005 | 29.800 | 30.393 | 35.194 |
| getpid-2048 | 40.563 | 40.978 | 43.691 | 39.834 |
| getuid-2048 | 159.428 | 162.001 | 158.121 | 156.723 |
| copyout-normal | 285.393 | 268.352 | 303.811 | 325.540 |
| copyout-128-vmas | 282.932 | 267.840 | 298.938 | 310.665 |
| copyout-256-vmas | 283.977 | 271.862 | 300.129 | 316.687 |
| fork-8 | 21.249 | 19.074 | 20.352 | 22.828 |
| fork-exec-8 | 785.240 | 695.365 | 785.340 | 763.616 |
| fork-128-vmas-8 | 22.982 | 19.456 | 18.134 | 25.329 |
| fork-256-vmas-8 | 20.827 | 18.596 | 18.299 | 23.519 |

Pooled ordinary fork is 22.0385 -> 19.713 ms (10.6% lower), and fork/exec
is 782.8005 -> 724.7575 ms (7.4% lower). However, complete native BusyBox/Xfe
lifecycles, which do not execute the new private-fork capture, also fall
9.2%/11.9%; several second-baseline controls are slower. getpid instead
increases 5.7%. The common-constructor refactor also changes generated code
outside private forks. These observations do not identify a single cause
or establish that all favorable differences are caused by the staging pass.
Do not report a universal 10-20% speedup from these heterogeneous controls.

Reject and archive the eager-staging candidate: it adds payload copies and
transient heap demand without demonstrating the intended real-startup gain.
The observed cheap capture does not remove destination allocation, Frame
cap construction, publication or the second full copy. A useful deferred
backing mechanism must eliminate that work for pages the child never needs,
rather than simply inserting another buffer before all the same work.

Comparison logs: /private/tmp/nanami-opt-cycle35-{startup,process}-
{before1,after1,after2,before2}.log.
Startup serial suffixes: 9bq67a70 / 9dm2hcsi / pq34ah08 / m40cdtya.
Process serial suffixes: pnkylqrq / wyiu1fg4 / so9v6qym / zeev9qlq.
All are under the same nanami-negoto-qemu- temporary-log prefix noted above.
Candidate code/tests are saved in /private/tmp/nanami-opt-cycle35.DdKWPo
and /private/tmp/nanami-opt-cycle35-staged-candidate.patch; the patch passes
git apply --check against the restored, formatted sources.

Restore memory_clone.rs, memory_pages.rs and exec_profile.rs byte-for-byte;
remove the candidate-only mock extensions and constructor tests. Keep the
two general nested-fork/parent-exec regressions. Restored validation passes
1,119 native tests (15 ignored), 113 focused AddressSanitizer tests (five
ignored), and the AArch64 Alpha Release check. Normal Alpha returns to
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b.
Logs: /private/tmp/nanami-opt-cycle35-restored-{native,asan,arm,build}.log.
Rebuild the unused terminal candidate image as cycle35-restored.img after
lsof confirms no open handles. No rejected runtime path, Kernel change,
IPC-fastpath change, new external protocol or version change is retained.

Next: implement a generic, bounded immutable private-backing lifetime, with
page-resolution integration rather than live-parent-frame aliases. It needs
reserved Frame-slot accounting, nested-fork current-data semantics, checked
privileged access, exact final permissions, stale queued-fault handling,
partial release/protection, shared-VM ownership and fail-closed mapping-error
cleanup. Admission failure must use ordinary eager cloning, not restrict
process size or change fork into CLONE_VM. Measure both fork/exec and actual
desktop startup against the current accepted runtime; this diagnostic cycle
neither reaches the 5% aim nor completes the optimization goal.

The rebuilt normal image passes three additional startup/kill/delete/reuse
sessions: 6541.637, 6316.395 and 5601.898 ms. These are restoration checks,
not a paired improvement claim; do not attribute their variation to the
removed candidate. Log: /private/tmp/nanami-opt-cycle35-restored-startup.log;
serial suffix ek15xdm1. Both completion markers are present and the VM exits
successfully. After terminal-handle and empty-lsof checks, remove only the
superseded agent-generated cycle34-restored.img (321 MiB). Keep the verified
cycle35-restored.img, cycle31 full-GUI image, both experimental patches,
source snapshots, frozen overlay/fixtures and all logs. This removes no user
source or live image; the temporary baseline can be regenerated from those
inputs. A9N remains clean, and formatting/diff checks pass.

## Cycle 36: immutable deferred private backing (rejected)

The preceding turn makes progress by auditing private-fork ownership, rejecting
eager staging and restoring the accepted runtime. Explore whether user-level
deferred construction can actually omit child Frames/PTEs for unused pages.
The user explicitly chooses to keep the Kernel unchanged on 2026-11-02;
do not change A9N diagnostics, ABI, fault delivery or IPC fastpath.

Implement an experimental, explicit private-fork opt-in rather than changing
legacy requests or replacing ordinary fork with CLONE_VM. Alpha captures current
resident bytes into immutable, reference-counted storage with an 8-MiB aggregate
retained-payload budget. Admission failure uses the old eager constructor.
Pending pages reserve logical Frame slots and preserve individual permissions;
nested forks reuse only immutable pending bytes and capture current materialized
bytes. Resolve on checked supervisor access or a missing-translation fault.
Never share a mutable parent's Frame as a substitute for an independent child.

Audit partial construction and lifetime, not just successful page copies:
initialize bytes and prepare page tables before granting the child's Frame
capability, then publish the exact final permissions. PROT_NONE remains hidden
but can later be promoted. A failed capability/MAP publication retains both
Frame ownership and immutable bytes and cannot retry over potentially modified
guest data. A failure before publication can reuse the already-owned physical
page. Mixed pending/resident protection, sparse release, fixed shared-memory
collisions, privileged reads, canonical shared-VM owners and exec/reap lifetime
are integrated and tested. Queued stale faults cannot fabricate anonymous pages.

The first functional QEMU process test passes, but fault-by-fault construction
is much slower. The current Release Kernel unconditionally prints a full
register dump and page-fault details before delivering each recoverable user
page fault. Read-only inspection identifies these calls in
spencer/A9N/src/hal/x86_64/interrupt/interrupt.cpp, do_irq_from_user; this is
not an authorization to modify them. Honor the user's user-level-only decision.

Try generic user-level mitigation: prefault the already-known child PC, SP and
TLS address, then add bounded aligned locality and 128-byte VM-incarnation-local
address hints. Hints predict addresses only, never cache guest contents or
permissions. Old children cannot warm a new exec VM; dense predicted sets use
eager construction before capture. Every materialization still copies fresh
snapshot bytes into an independent Frame with that page's current permissions.

The candidate passes 1,146 native tests (15 opt-in probes ignored), 140 focused
AddressSanitizer tests (five ignored), x86_64 normal Release image construction
and AArch64 Alpha checking. Earlier AArch64 Alter checking predates the final
hint changes; do not claim that as final-candidate architecture validation.
The simpler PC/SP/TLS-prefault process fixture also passes guest-visible fork,
freshness and protection checks. These checks do not establish speedup.

Run real startup in ABBA order, three start/kill/delete/reuse sessions per boot,
with the same frozen overlay, observer, normal devices, SMP=4 and polling
interval. No other build, test or QEMU job runs during these measurements.
The final comparison process handle expires before it can be resumed; recover
its four complete log files rather than restarting or discarding observations.
Both completion markers are present in every serial log.

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 5726.651 | 7170.928 | 7716.905 | 5816.375 |
| 2 | 6320.807 | 7414.365 | 7623.602 | 6299.701 |
| 3 | 6376.262 | 7403.756 | 7744.882 | 6005.782 |

Times are milliseconds. Pooled startup median is 6152.7415 -> 7518.9835 ms,
22.2% higher, not the intended 5% improvement. Each final candidate boot has
237 handled page-fault diagnostic records; both baseline boots have none.
This identifies work added by the demand path, not a precise causal split among
UART time, extra copies, allocation, metadata and scheduling. Shared UAPI/runtime
rebuilds also change non-Alter binaries; do not claim isolated attribution or
that a Kernel-only logging change would make this candidate worthwhile.

The preliminary no-hint prefault variant took 25519.433 / 24583.290 / 25181.250
ms in one boot. The much faster final candidate is still slower than the
accepted baseline; comparison against the weaker prototype is not acceptance.
Deferred capture also adds a full source copy and retained heap demand. Reject
the implementation rather than keeping substantial unproven VM/protocol/fault
machinery enabled or advertising reduced Frame counts as an application gain.

Logs: /private/tmp/nanami-opt-cycle36-startup-
{before1,after1,after2,before2}.log. Serial suffixes under the standard
nanami-negoto-qemu- temporary directory: 1_7uet5w / 8o9y_tx3 / hgnvq_wx /
ucz5lqnl. Earlier functional/process logs use dja0lwj9 / 3a04khtq and the
preliminary startup log uses bh51vvdn. Candidate native/ASan logs are
/private/tmp/nanami-opt-cycle36-hints-{native2,asan}.log.

Archive all experimental implementation and tests in
/private/tmp/nanami-opt-cycle36-deferred-candidate.patch (28 changed/new files),
with pre-experiment sources in /private/tmp/nanami-opt-cycle36.2nm7iA.
The archive passes git apply --check against restored sources. Restore all 27
saved source/test/doc files exactly, including the existing cycle-35 fork
regressions; remove only the three new candidate-only module/proposal files.
Keep all measurement logs. No deferred request flag, page-resolution opcode,
fault-path branch, payload budget or address predictor remains in production.

Next investigate a user-level path that removes work without requiring new
demand faults. In particular, inspect vfork's actual semantics and exec VM
ownership before considering shared-VM construction: both current Alter exec
and Alpha image replacement reject live shared-VM peers, so merely swapping
the fork call for CLONE_VM would be incorrect. Preserve ordinary fork isolation,
existing Kernel and the accepted runtime while evaluating that design.

Restored validation passes 1,119 native tests (15 ignored), 113 focused ASan
tests (five ignored), and AArch64 Alpha plus Alter/Linux Release checking.
The normal x86_64 Release image rebuild succeeds. Logs are
/private/tmp/nanami-opt-cycle36-restored-
{native,asan,arm-alpha,arm-alter,build}.log. A9N's source worktree remains clean;
Kernel SHA-256 is unchanged:
312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.
Alpha returns exactly to
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b,
and Honoka to
7b788303136f650ea0a257ef7c640bd60ef2753d43c9d6583004b9604aba720c.
The regenerated Alter/Linux and Alter/FreeBSD hashes are respectively
7e6e6009f038b6ec9c1de1e2bdd8b0a646b1a785f240ba04f7ea32cbf8abe5c6
and f65970a9688b6880591ec80bcda52c4f8c40fef5153f4f2061beeb8a57f68a0f;
they differ from the earlier baseline artifacts despite restoring saved sources.
Do not claim whole-image bit identity or infer a new optimization from this
restoration run.

After confirming no QEMU process or open image handle remains, rename/rebuild
the terminal experiment image as /private/tmp/nanami-opt-cycle36-restored.img.
Keep cycle35-restored.img, the cycle31 GUI fallback, frozen inputs and all logs.
The rebuilt image passes three startup/kill/delete/reuse sessions at
5005.073 / 5294.819 / 6105.798 ms, with both completion markers and successful
test-process termination. Log: /private/tmp/nanami-opt-cycle36-restored-startup.log;
serial suffix fvt_ee26. These are functional restoration checks, not a matched
speedup claim. The rejected modules and proposal are recoverable from the patch;
no user-created source, live image, accepted optimization or version is removed.
This cycle does not meet the 5% aim or complete the optimization goal.

## Cycle 37: bounded physical allocation for private-page construction

The preceding goal turn makes progress: a complete ABBA startup comparison
rejects deferred private backing, its implementation is archived, and restored
sources pass native/ASan, architecture and repeated QEMU startup checks. Honor
the user's explicit Kernel-unchanged decision. Inspect the actual accepted
constructor instead of repeating the rejected final-permission, extra-staging
or demand-fault experiments.

map_process_initialized_pages already reserves contiguous virtual runs, but
ordinary fresh backing still invokes the physical allocator for each page.
Try one physical allocation per current Frame-node fragment (at most 512 pages),
then retain the same individual Frame capabilities, bytes, PTE publication,
protections and page-granular ProcessManager references. A fragmented RAM pool
falls back to the previous independent-page allocation; a one-page request does
not attempt the optimization. This neither requires contiguous physical memory
for successful ordinary allocations nor changes an API, IPC fastpath, Kernel,
guest behavior or resource limit. Existing deferred reuse remains unchanged.

Register every page of a successfully allocated batch before any fallible Frame
construction/copy/MAP, so rollback owns its unused tail too. New constructor
tests cover node-aligned and partially occupied Frame nodes, full initialization,
copied prefixes/tail zeroing, contiguous allocation denial with successful
single-page fallback, failure at each Frame copy and at a later batch, exact
ownership of every allocated page, and absence of references for future batches.
The real physical allocator is separately checked for independently releasing
bulk-backed leaves in descending and interleaved order, double-free rejection,
retention of unrelated allocations and reuse of the complete recovered pool.

A small opt-in host allocator diagnostic reports these aligned work counts:
64 / 512 / 1202 / 1837 pages take 64 / 512 / 1202 / 1837 calls individually,
versus 1 / 1 / 3 / 4 with batches. Its allocation samples are lower but its
page-at-a-time release samples are higher. This probe excludes Frame capability
creation, mappings, initialization, guest execution and scheduling; its very
short timings cannot establish a system-wide 5% gain. Keep complete lifecycle
and real startup as acceptance criteria. Log:
/private/tmp/nanami-opt-cycle37-allocator-perf.log.

Freeze the normal baseline at /private/tmp/nanami-opt-cycle36-restored.img and
the candidate at /private/tmp/nanami-opt-cycle37-bulk.img. Both use the same
frozen cycle32 overlay and observer, normal devices, SMP=4, Release configuration
and unchanged Kernel. Candidate Alpha SHA-256:
a716536ab93e180e543ff4aedd7cb7e33b7c0feb1520e6797f44b6d293930c69.
Alter/Linux, Alter/FreeBSD, Honoka and Kernel executable hashes equal the
cycle36 restored artifacts; only Alpha's runtime source changes. Before
measuring, all build/test/VM handles are terminal and a fresh process inventory
finds none active. Host load average is 8.09 / 7.28 / 7.68 on 10 CPUs; do not
stop or retune unrelated host processes. Run independent process and startup
boots sequentially in before/after/after/before order, without concurrent builds,
tests or VMs. Logs:
/private/tmp/nanami-opt-cycle37-{process,startup}-
{before1,after1,after2,before2}.log.

All eight boots complete their fixtures successfully, including guest-visible
fork freshness/protection and three desktop start/kill/delete/same-ID sessions
per startup boot. Serial suffixes, before1/after1/after2/before2:
process isf7j1vr / 92u1y08h / e4kcmavv / g6rerri7;
startup 7qxvl8p1 / 7bk6wajo / lp4k5tkl / lswq0ytv.
Real startup measurements, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| 1 | 5304.874 | 5364.277 | 5108.449 | 5317.374 |
| 2 | 5497.201 | 5541.799 | 5482.884 | 5395.641 |
| 3 | 5702.927 | 5703.895 | 5702.360 | 5499.455 |

Pooled startup median is 5446.421 -> 5512.3415 ms, 1.2% higher, not a 5%
improvement. This difference is smaller than one 100-ms polling interval;
do not overstate it as a precisely identified application regression either.
Process measurements are mixed. Per-boot medians, milliseconds:

| Workload | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| fork-8 | 22.671 | 23.406 | 18.003 | 17.951 |
| fork-exec-8 | 774.108 | 820.434 | 708.650 | 695.119 |
| fork-128-vmas-8 | 20.811 | 21.258 | 19.199 | 18.524 |
| fork-256-vmas-8 | 17.814 | 19.996 | 20.274 | 18.355 |
| alpha-spawn-reap-8 | 29.499 | 24.058 | 26.208 | 25.087 |
| alpha-breakdown-spawn | 2.658 | 2.703 | 2.660 | 2.974 |
| alpha-breakdown-reap | 0.405 | 0.401 | 0.362 | 0.455 |
| alpha-xfe-spawn | 5.785 | 5.942 | 5.865 | 5.338 |
| alpha-xfe-reap | 0.674 | 0.561 | 0.584 | 0.571 |
| alpha-lifecycle-busybox-8 | 58.655 | 32.375 | 32.495 | 28.451 |
| alpha-lifecycle-xfe-3 | 33.728 | 32.241 | 33.283 | 29.790 |
| negoto-create | 11.302 | 7.362 | 8.681 | 8.3165 |
| getpid-2048 | 48.588 | 39.223 | 41.340 | 40.073 |
| getuid-2048 | 238.606 | 158.936 | 155.914 | 154.423 |
| copyout-normal | 396.805 | 271.116 | 274.909 | 275.130 |
| copyout-128-vmas | 344.574 | 268.489 | 272.837 | 274.623 |
| copyout-256-vmas | 296.504 | 247.024 | 275.118 | 279.011 |

Pooled ordinary fork is 21.6695 -> 20.9255 ms (3.4% lower), fork/exec
742.7995 -> 717.0975 ms (3.5% lower), 128-VMA fork is 3.4% higher and
256-VMA fork is 11.0% higher. The first baseline's copyout/credential controls
are much slower despite not executing the changed allocator path; later
baseline controls settle near the candidate. Preserve those observations,
rather than discarding the first boot or attributing all favorable differences
to batching. Physical placement and later partial-free work also change;
fewer allocation calls alone do not establish lower whole-system cost.

Reject the runtime candidate. Archive its constructor and six candidate-specific
regressions at /private/tmp/nanami-opt-cycle37-bulk-candidate.patch; the pre-cycle
files are retained in /private/tmp/nanami-opt-cycle37.RCF1nJ. Restore memory_pages.rs
and alpha-private-pages.rs byte-for-byte. The archive passes git apply --check.
Keep only the independent physical-allocator partial-release regression and
opt-in host diagnostic, which exercise the unchanged allocator and are useful
without the rejected constructor. No bulk-allocation branch, new protocol,
Kernel change or version change remains in production.

Candidate checks before measurement: full native suite passes 1,124 tests
(16 opt-in probes ignored); after adding the final two constructor cases,
that complete constructor target passes 14 tests in both normal and ASan
builds. Focused pre-addition ASan checks and AArch64 Alpha checking also pass.
Do not present the earlier full-suite count as coverage of tests added later.
Logs: /private/tmp/nanami-opt-cycle37-{native,asan,arm,pages-final,pages-final-asan}.log.

Next inspect copy-alias lifetime rather than adding demand faults or another
copy pass. The current process-copy cache is keyed by each child's Frame
capability and is invalidated on teardown; initialization already uses stable
Alpha-owned physical-pool Frame capabilities. Audit whether those stable pool
aliases can be used for private-copy work while still resolving current pages,
permissions, sharing, Device exclusions and revocation ownership correctly.
Do not retain stale child capabilities or drop invalidation on that assumption.

Restored full native validation passes 1,120 tests (16 ignored), focused ASan
validation passes 120 tests (seven ignored), and AArch64 Alpha Release checking
passes. Logs: /private/tmp/nanami-opt-cycle37-restored-{native,asan,arm}.log.
The first normal image rebuild stops before Alpha generation because rootfs
temporary-file creation exhausts available disk space. Do not treat that build
or its still-candidate output image as restored. Confirm the comparison handle
is terminal and lsof shows no users of the old cycle35-restored.img, then remove
only that superseded agent-generated 321-MiB image. Source snapshots, archived
patches, frozen input/observer and all logs remain; the image can be regenerated
from those build inputs. Keep cycle36-restored.img as the exact comparison
baseline and the earlier full-GUI fallback. No source or live image is deleted.

Retry succeeds: /private/tmp/nanami-opt-cycle37-restored-build2.log produces
/private/tmp/nanami-opt-cycle37-restored.img. Alpha returns exactly to
853748621fa5db237035d0875521ccdb3d7943834f71bb0e44d8f054444cd86b;
Alter/Linux remains
7e6e6009f038b6ec9c1de1e2bdd8b0a646b1a785f240ba04f7ea32cbf8abe5c6,
and Kernel remains
312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.
A9N's source worktree is clean. Both restored source/test files compare equal
to their pre-cycle copies; archive applicability and git diff --check pass.

The rebuilt normal image completes three further startup/kill/delete/reuse
sessions and both completion markers, with successful test-process termination.
Log: /private/tmp/nanami-opt-cycle37-restored-startup.log; serial suffix of0lxopn.
Samples are 5840.946 / 7932.454 / 7161.085 ms. Host load recorded afterward is
10.10 / 7.84 / 7.71 on 10 CPUs. These are restoration/behavior checks, not a
paired performance comparison or a new code regression: the accepted Alpha
binary is bit-identical. Preserve the slower observations rather than reporting
only the earlier favorable restoration timings. The 5% per-cycle aim and the
overall optimization goal remain unproven; this cycle retains only additional
allocator verification and the measured rejection evidence.

## Cycle 38: copy aliases backed by Alpha-owned pool Frames

Status: retain the smaller owned/shared-RAM-only change after both measurement
orders, host regressions and two real GUI sessions pass. The initial all-Frame
candidate below was retired, not accepted. Each implementation and its evidence
are recorded separately. Neither variant establishes a repeatable 5%
desktop-startup improvement; the retained gain is in bounded input/create/reap
lifecycles, especially shared input-buffer cleanup.

The preceding goal turn makes progress by implementing/testing bounded physical
allocation, measuring complete process/startup ABBA comparisons, rejecting the
candidate and restoring the accepted runtime. Keep the Kernel unchanged per the
user's explicit constraint. Audit why private-copy aliases require teardown,
rather than adding faults, extra payload copies or another allocator cache.

Current VmSpace values mostly reference Frame capabilities in child roots.
CopyWindow keys them by PID/descriptor and must unmap those aliases before the
child cap is revoked/reused. Alpha already retains permanent pool Frame
capabilities for the same physical objects; InitializationWindow uses them
across process cleanup and physical-page reuse. Its frame-range records are
not removed/retyped by ordinary process release. Recursive Generic delegation
selects untouched physical-source ranges, not an already materialized Frame
range. No new Kernel/direct-map/Device attribute policy is introduced here.

The candidate maps/tracks private pages, DMA, shared mappings and explicit
MMIO/framebuffer mappings through the existing Alpha pool capability. Child
Frame aliases are still installed with the same rights in the same slots;
no pool authority is transferred to clients. Opaque service-session mappings
already used pool capabilities. Inspect every non-bootstrap VmSpace producer,
including map_frame_with_attribute callers, rather than assuming heap pages
are the only source. Shared-map validation now compares against the expected
pool Frame for the registered physical allocation and requires that descriptor
to exist; two missing Option values cannot authorize a share.

Read-only Kernel inspection confirms MAP/UNMAP convert either Frame alias into
the same physical frame description before invoking HAL; capability slot identity
is not the PTE's physical identity. Kernel source, ABI, IPC fastpath and mapping
attributes remain unchanged. Non-bootstrap VM indexing still stores one Word
per Frame, not a second physical-address/alias index.

CopyWindow now accepts only the resolved Alpha pool Frame. Keep its existing
2048-entry, four-way, pinned-source-safe bounded mapping policy; remove PID tags,
owner buckets, repeated owner lookup and per-process invalidation. Failed MAPs
remain non-hits with cleanup metadata; failed eviction UNMAPs still forbid
replacement. No bytes, VA-to-Frame bindings, permission decisions or authority
are cached. Current page lookup and checked-copy permission/supervisor validation
still precede every copy. Pool aliases are Alpha-private, so retaining one after
session withdrawal does not restore a participant's removed mapping or grant.
The obsolete invalidate phase is removed from opt-in reap diagnostics as well.

Update test mocks to distinguish permanent pool caps from revocable child caps
and compare underlying physical objects for UNMAP. Constructor tests explicitly
reject storing a child alias in the VM index. Shared-memory checks keep the
same pool alias across both guest-cap deletions, complete physical release and
twenty fresh/re-zeroed reuses of that page. Guest leaf-UNMAP failures still keep
backing/authority live. Cache checks cover failed publication/cleanup, eviction,
pinning, independent current Frames and repeated stable-Frame use. Existing
checked-copy tests continue to reject current PROT_NONE and foreign supervisors.
These host models are not hardware TLB or Kernel-revocation proofs.

Frozen candidate validation: 1,120 native tests pass (16 opt-in probes ignored),
123 focused ASan tests pass (three ignored), AArch64 Alpha Release checking
passes, and the x86_64 normal Release image builds. Logs:
/private/tmp/nanami-opt-cycle38-{native,asan,arm,build}.log.
No profiler is enabled. Baseline is cycle37-restored.img (Alpha hash recorded
above); candidate is /private/tmp/nanami-opt-cycle38-pool.img with Alpha SHA-256
3b93e71d0c5d4817788899ce68cd2e0e44f9dd4a57250f3e4d97f900c10dadfc.
Alter/Linux, Alter/FreeBSD, Honoka and Kernel hashes match the baseline.
Pre-experiment files are in /private/tmp/nanami-opt-cycle38.UdGdPH.

All build/test handles are terminal and a fresh process inventory finds no
competing build/test/VM before starting measurements. Host load average is
7.40 / 8.18 / 7.90 on 10 CPUs. Use the same frozen cycle32 overlay/observer,
normal devices, SMP=4 and Release configuration. Run separate process and
three-session startup boots in before/after/after/before order, without running
other builds/tests/VMs concurrently. Do not accept on cache-hit counts alone.
Logs: /private/tmp/nanami-opt-cycle38-{process,startup}-
{before1,after1,after2,before2}.log.

### Initial candidate: reverse-order replication and retirement

Repeat both workloads in after/before/before/after order using after3, before3,
before4 and after4. All timing values below are milliseconds, and pooled medians
include all three samples from each corresponding boot; lower is better.

| Workload | Initial ABBA baseline / candidate | Reverse BAAB baseline / candidate | Both orders baseline / candidate |
| --- | ---: | ---: | ---: |
| Real OCI create/start to X11 terminal readiness | 6116.994 / 5799.307 | 5544.176 / 5774.930 | 5826.233 / 5799.307 |
| Linux normal copyout workload | 274.632 / 319.907 | 287.400 / 274.395 | 279.085 / 290.226 |
| Linux getpid, 2048 calls | 40.387 / 42.697 | 40.011 / 39.110 | 40.011 / 40.429 |
| Linux fork, eight children | 23.710 / 23.734 | 19.503 / 18.442 | 21.051 / 21.538 |
| Linux fork/exec, eight children | 731.826 / 759.126 | 706.116 / 693.744 | 709.053 / 730.017 |
| Native BusyBox input/create/kill/release lifecycle, eight children | 32.166 / 29.562 | 31.216 / 30.320 | 31.570 / 30.090 |
| Native Xfe input/create/kill/release lifecycle, three children | 32.297 / 29.517 | 31.486 / 30.076 | 31.534 / 29.517 |
| BusyBox input-buffer release subset | 0.908 / 0.444 | 0.924 / 0.461 | 0.924 / 0.456 |
| Xfe input-buffer release subset | 1.790 / 0.976 | 1.728 / 0.892 | 1.767 / 0.928 |

The apparent initial 5.19% startup reduction reverses to a 4.16% increase in the
opposite order. Both orders together show only a 0.46% reduction, below the 100ms
readiness polling resolution. Do not adopt or advertise the first result as a
whole-system speedup. Native input/create/kill/release rows do not run the Linux
interpreter/application and are not desktop startup measurements. Their stable
input-buffer-release reduction motivates the smaller experiment below.

A separate lifetime review rejects extending all MMIO/framebuffer copy aliases
to permanent pool ownership. Keep the existing child-capability mappings and
PID-scoped teardown for those paths. Preserve the initial experiment at
/private/tmp/nanami-opt-cycle38-unqualified-pool.patch and its Alpha executable
as alpha-unqualified.elf in the pre-cycle snapshot directory. Its old image was
overwritten only after both comparison jobs terminated and no open-image handle
remained. The initial candidate is not the current source/image.

### Lifetime-qualified intermediate candidate: tests only

An intermediate version restores PID-scoped CopyWindow entries and invalidation
for transient child aliases, recognizes permanent Alpha pool descriptors by their
internal namespace, and restores MMIO/framebuffer child-capability producers.
Private RAM/ELF/lazy/DMA producers still use pool descriptors in this intermediate
version. It passes native, focused ASan and AArch64 checking after fixing a
private FrameRange field access through an encapsulated method. It has no QEMU
performance measurements; do not attach either comparison above to it.

Archive: /private/tmp/nanami-opt-cycle38-qualified-all-ram.patch and
alpha-qualified-all-ram.elf in the snapshot directory. Alpha SHA-256:
595c917c33601f542e083036c3d94d3017e247b786e41c5aa3136c546bcdc017.

### Current candidate: owned/shared RAM buffers only

Restore the pre-cycle private-page, ELF/lazy, DMA, staging and Device producers.
Only owned/shared RAM buffer VM records switch to permanent pool Frame aliases;
the same client child capabilities and rights are still installed. CopyWindow
keeps its original per-PID layout, owner counters, eviction and invalidation.
Already-resolved permanent Alpha pool descriptors use reserved owner zero;
ordinary private RAM and Device child aliases keep the previous owner path.
No bytes, guest address translations, permission decisions or client authority
are cached, and no public protocol, Kernel API, mapping attribute or IPC fastpath
changes. This is not the smaller all-global cache layout of the first variant.

The namespace predicate checks encoded descriptor prefixes/depths, not object
identity, capability validity or rights. FrameRange encapsulates its node field.
Real MemoryManager tests check both bootstrap and normal pool descriptors,
adjacent/invalid namespaces and survival of unrelated process-arena reset.
Shared-map validation requires the expected physical pool descriptor to exist
and accepts only that descriptor or the registered source child alias. A foreign
pool Frame is rejected. Guest leaf-UNMAP failures still retain guest authority
and backing. Scoped MMIO tests assert the original child descriptor is passed
to the mapper. CopyWindow tests retain the old PID failure cases and add shared
pool reuse, owner-zero bucket collisions and non-pool descriptor lookalikes.

Frozen validation: 1,126 native tests pass (16 opt-in probes ignored), 144 focused
ASan tests pass (two ignored), AArch64 Alpha Release checking passes, and the
x86_64 normal Release image builds. Logs:
/private/tmp/nanami-opt-cycle38-owned-{native,asan,arm,build}.log.
Candidate image: /private/tmp/nanami-opt-cycle38-owned.img; Alpha SHA-256:
d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af.
Alter/Linux, Honoka and Kernel executables match the cycle37 baseline, including
Kernel SHA-256 312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.

Start an independent process/startup ABBA comparison against cycle37-restored.img
at 2026-11-01 18:20:17 UTC. All builds/tests are terminal; no competing VM/build
appears in the filtered process inventory. Host load is 6.57 / 6.64 / 6.91 on
10 CPUs. Use the same frozen cycle32 overlay/observer, normal devices, SMP=4 and
non-profiled Release binaries. No runtime changes during comparison. Logs:
/private/tmp/nanami-opt-cycle38-owned-{process,startup}-
{before1,after1,after2,before2}.log. Performance acceptance is pending, not implied
by passing correctness tests or the initial variant's timing results.

To provide room for the next atomic rootfs build, remove the redundant, owned
/private/tmp/nanami-opt-cycle36-restored.img (321MiB logical) after its VM is
terminal and an open-handle check is empty. Preserve the cycle37 comparison
image, cycle31 GUI fallback, frozen inputs, source snapshots, patches and logs.
The deleted image can be rebuilt; no source or user-provided image is removed.

### Owned/shared-RAM candidate: both comparison orders

The ABBA job finishes successfully; repeat process/startup boots in inverse
after/before/before/after order at 18:24:58 UTC, after confirming the process
inventory is empty. Host load is then 8.63 / 7.88 / 7.38 on 10 CPUs. Both jobs
finish with exit zero. Audit all 16 serial logs: every process boot contains
PROCESS-CACHE-FRESHNESS-PASS, TINYCORE-PROCESS-PERFORMANCE-PASS and the final
standalone PROCESS-PERFORMANCE-PASS; every startup boot contains the three-sample
TINYCORE-STARTUP-PERFORMANCE-PASS with cleanup=true. None contains the checked
panic/fatal, process-spawn or shared-memory rollback failure markers. The initial
unqualified variant's separate 16 serial logs also have their completion markers.

| Workload (ms, lower is better) | ABBA baseline / candidate | BAAB baseline / candidate | Both orders baseline / candidate |
| --- | ---: | ---: | ---: |
| Real OCI create/start to X11 terminal readiness | 5768.181 / 5975.945 | 5968.574 / 5904.448 | 5856.601 / 5904.448 |
| Linux normal copyout workload | 287.198 / 287.760 | 292.220 / 281.623 | 290.386 / 287.316 |
| Linux getpid, 2048 calls | 43.564 / 39.487 | 40.485 / 39.795 | 41.461 / 39.637 |
| Linux getuid, 2048 calls | 160.938 / 167.183 | 170.151 / 159.250 | 169.344 / 164.714 |
| Linux fork, eight children | 22.821 / 18.859 | 24.792 / 20.177 | 24.210 / 20.030 |
| Linux fork/exec, eight children | 729.581 / 737.647 | 754.409 / 772.844 | 736.402 / 750.465 |
| Linux fork, 128 VMAs / eight children | 19.613 / 20.844 | 23.041 / 20.691 | 22.092 / 20.820 |
| Linux fork, 256 VMAs / eight children | 21.255 / 19.181 | 23.320 / 19.168 | 22.674 / 19.181 |
| Native BusyBox input/create/kill/release lifecycle, eight children | 31.338 / 30.048 | 35.686 / 30.807 | 33.493 / 30.474 |
| Native Xfe input/create/kill/release lifecycle, three children | 34.070 / 32.204 | 34.027 / 30.246 | 34.040 / 30.586 |
| BusyBox input-buffer release subset | 0.919 / 0.525 | 0.921 / 0.619 | 0.919 / 0.552 |
| Xfe input-buffer release subset | 1.887 / 1.093 | 1.850 / 1.068 | 1.885 / 1.070 |

There are 12 samples per side in the combined rows (24 in Negoto-create, not
listed in the table). Startup is 0.82% longer, approximately 48ms, below the
100ms readiness sampling interval; do not claim a desktop-startup improvement.
Fork/exec is 1.91% longer in the combined median. Opposite movements in unrelated
getpid/getuid controls and between boots make small total changes uncertain;
do not relabel them as IPC transport improvements. The complete native input
lifecycles improve 9.01% / 10.15%, and their source-buffer-release subsets improve
39.93% / 43.25%. These are bounded process/input lifecycles, not running Linux
applications, and the whole-system 5% goal is not established by them.

Logs: /private/tmp/nanami-opt-cycle38-owned-{process,startup}-
{before1,after1,after2,before2,after3,before3,before4,after4}.log.
Keep the production freeze through all 16 boots. Alpha still hashes to
d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af afterward;
Alter/Linux, Honoka and Kernel still match their baseline hashes. The initial
owned candidate patch is /private/tmp/nanami-opt-cycle38-owned-candidate.patch.

### Additional correctness checks, not timing comparisons

The unqualified default Negoto smoke command is not applicable to this frozen
Tiny Core image: it stops because /alter/linux/bundles/busybox/config.json is
absent (status 0x3). Log: /private/tmp/nanami-opt-cycle38-owned-reuse.log, serial
suffix 8migfqj5. This is not a successful 20-cycle OCI smoke result, nor a
Kernel/Frame-mapping failure. Do not manufacture a pass or change the frozen
overlay during the comparison. Run its supplied Tiny Core desktop profile
directly, including two real GUI sessions in one boot, instead.

Add a host integration regression combining production checked-copy authorization,
shared mappings, CopyWindow and mapping-protection handling. Warm both permanent
copy aliases, alternate target permissions through PROT_NONE/read-only/writable,
and require forbidden transfers and a foreign supervisor to fail before the
modeled byte-transfer boundary while the aliases remain resident. Guest mappings
and physical ownership still clean up normally. Bytes are modeled, not actually
copied in this test. The first draft correctly fails because its fixture has no
supervisor grant; use an authorized self-copy between two different shared pages
instead of weakening production authorization. The corrected core-process run
passes 90 tests, with two opt-in diagnostics ignored. Logs:
/private/tmp/nanami-opt-cycle38-owned-authority{,2}.log. Runtime binaries are not
changed by this host-only test; full native/ASan and GUI validation follow.

Final host validation, including the new authority/cache integration check:
1,127 native tests pass (16 opt-in probes ignored) and 145 focused ASan tests
pass (two ignored). Both jobs exit zero. After ordering the test modules as
required by rustfmt, rerun both suites against the final file contents, with
the same counts and zero failures. The single-file rustfmt check and scoped
git diff --check pass. Logs:
/private/tmp/nanami-opt-cycle38-owned-final2-{native,asan}.log.
The unchanged runtime also retains its successful AArch64 Alpha Release check.
Final scoped patch, including documentation and the additional regression:
/private/tmp/nanami-opt-cycle38-owned-final.patch (29,635 bytes).

The supplied Tiny Core desktop regression completes successfully with normal
devices and SMP=4. In one boot, both cycles pass real mouse/title dragging,
terminal input and profile/link checks, dock-launched Terminal and Information,
and Exit/session cleanup; the first cycle also visibly opens the stock Editor.
The second session is not a fresh VM. Serial contains each cycle's READY,
LINKS-PASS, INPUT-PASS, LAUNCH-PASS, INFO-PASS and SESSION-PASS markers, followed by
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. No checked fatal, panic,
process-spawn or shared-memory rollback failure marker appears. Inspect the
first cycle's actual Editor/Terminal/dock screenshot and the second cycle's
Information screenshot as well as the markers. The job exits zero.
Log: /private/tmp/nanami-opt-cycle38-owned-desktop.log; serial/screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-uv054s9h.

Decision: retain only the owned/shared-RAM VM descriptors and lifetime-qualified
copy-cache owner selection, plus their small internal namespace helpers and
regressions. Do not migrate private RAM/ELF/lazy/DMA or MMIO/framebuffer producers
to pool ownership. Keep all existing PID-scoped cleanup and failed-UNMAP rules
for transient aliases. No new client authority, public protocol, data cache,
Kernel source/API/IPC fastpath or version change. The Kernel worktree is clean
and its executable still matches the baseline hash above. The normal candidate
image and Alpha executable are the final validated runtime, not a profiler or
the retired broad experiment. The optimization goal remains active: reducing
shared-buffer cleanup does not satisfy the requested whole-system startup gain.

## Cycle 39: eliminate repeated X-readiness subprocesses

Status: retain the profile-owned Xlib helper and builder integration after both
comparison orders, host checks, a real offline assembly and two real GUI
sessions pass. Measured improvement is 6.67% for first startup and 3.04% for later
sessions, not 5% for every startup or emulated workload. Kernel/Alpha/Alter/Honoka
binaries and ordinary private-fork semantics remain unchanged in this cycle.

The preceding turn makes progress by retaining the qualified owned/shared-RAM
copy-cache optimization, measuring both comparison orders and passing host and
real two-session GUI regressions. Keep the Kernel unchanged. Whole-system 5%
startup improvement is not established; do not substitute the narrower buffer
cleanup result for that goal.

Revalidate loader, interpreter, private-fork and desktop startup code. PT_INTERP
single-initialization, final-permission construction, bulk physical allocation,
GPR memcpy and deferred private backing have already been evaluated/rejected.
The native and Linux stacks are the same 64-page range, not duplicate 4-MiB
allocations. Existing traces show ordinary private clones, not VFORK, so a
shared-VM implementation would not address those calls and cannot silently
replace private fork semantics.

The profile currently launches stock xset for every readiness retry and launches
a separate sleep between failures. First count actual retries with a diagnostic
profile-only printf after the existing loop. Put this profile in a clone of the
frozen cycle32 overlay, not in the accepted production script. Add an optional
observer read after the timed Terminal/X11 readiness boundary; its UART output
is after teardown, and normal profiles need not create the diagnostic file.
No guessed saving is accepted before observing retries and a complete comparison.
Pre-cycle snapshots/overlay: /private/tmp/nanami-opt-cycle39.DgxgXE.

After the previous cycle's full GUI pass supersedes its coverage, delete only
the owned /private/tmp/nanami-opt-cycle31-gui.img (321MiB logical), with no live
open handle reported by lsof. Preserve the cycle38 validated normal image and
cycle37 earlier comparison image, all source snapshots and diagnostic logs.
The removed image is reproducible. Available space becomes about 762MiB.

The three diagnostic startup sessions each report exactly one failed X query
before success. Count stdout is read only after the timed readiness boundary.
All three complete same-ID kill/delete/reuse; their elapsed values are
5121.850 / 5295.082 / 5409.950ms. These diagnostic observations select a
candidate, not establish any improvement. Log:
/private/tmp/nanami-opt-cycle39-count-startup.log; serial suffix 0vsca5lz.
Restore the stock script in the comparison baseline: neither normal side
writes the optional count file, and the same observer runs on both sides.

Implement a small profile-owned Xlib readiness helper, not a modified upstream
program, cloned fork semantics or a new Kernel interface. It accepts a display
and positive server PID, uses the existing pinned libX11/libc, and retries
XOpenDisplay up to 30 times with 100ms nanosleep and liveness checks. Resume an
interrupted sleep using its remaining interval. XCloseDisplay completes the
connection with its documented final XSync; checking only a socket pathname
would not establish server readiness. This limits retries, not the duration of
a hung Xlib call, matching the old synchronous probe's limitation. Normal client
startup still waits for that real connection and uses DISPLAY=unix/:0. Xfbdev,
FLWM, wbar, aterm, xset and packaged waitforX remain unchanged.

Use the standard Linux x86-64 libc entry ABI rather than substituting TLS or
allocator initialization. The helper is a 3.6KiB dynamically linked ET_EXEC;
readelf confirms only libX11.so.6/libc.so.6 dependencies, the pinned root's
/lib/ld-linux-x86-64.so.2 interpreter and a non-executable stack. Disassembly
contains no SIMD/FPU instructions in the helper. This says nothing new about
the unchanged upstream libraries. SHA-256:
d7699dbb284339e889c36db3d731289dd421067a8cd40ba71670e8e885ed01e3.

Desktop assembly builds the helper with Clang/LLD and records its checksum in
profile_helpers. Console assembly remains independent. Reject symlinked root
or output ancestors, existing output and libraries resolving outside the root.
The normal staging/rename path preserves an older build if compilation fails.
Mocked host C tests cover immediate success, bounded failure, liveness, invalid
PID/overflow and interrupted/error sleeps; both ordinary and ASan/UBSan runs
pass. The initial builder test expected a noncanonical /var path and fails on
macOS's /private/var alias; compare resolved paths instead. The corrected
builder suite passes 29 tests before the final extra safety cases are added.

Build matched baseline/candidate images, both containing the same helper and
observer; only the candidate startup script invokes it. The baseline does not
include the optional retry-count write. The observer SHA-256 is
d3f2bdb84b137c0db762b67ad6489af9d204f699dc2197aba291c44bda152f72.
Alpha, Alter/Linux, Honoka and Kernel match cycle38's unchanged hashes. No
profiling is enabled. All build/test handles are terminal and filtered process
inventory is empty before starting sequential normal-device SMP=4 startup
boots in before/after/after/before order at 18:47:25 UTC. Host load is
7.93 / 7.54 / 7.26 on 10 CPUs. No other builds/tests/VMs run during timings.
Images: /private/tmp/nanami-opt-cycle39-{before,after}.img. Logs:
/private/tmp/nanami-opt-cycle39-startup-{before1,after1,after2,before2}.log.
Performance and real Alter/Linux correctness acceptance are still pending.

The first ABBA comparison completes with exit zero and all 12 actual Terminal
connection plus kill/delete/same-ID reuse checks. Each baseline session emits
17 private fork-vm creations; each candidate emits 15. Both variants retain
ordinary fork semantics. Raw startup observations, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First session | 5125.585 | 4718.860 | 4725.974 | 5110.444 |
| Second session | 5304.402 | 5090.461 | 5191.508 | 5308.514 |
| Third session | 5447.543 | 5198.893 | 5202.636 | 5416.762 |

Initial first-session medians are 5118.015 / 4722.417ms (7.73% lower); later
session medians are 5362.638 / 5195.201ms (3.12% lower). The pooled six-sample
median is 5306.458 / 5140.985ms (3.12% lower), not a 5% overall claim. Do not
change the summary statistic to whichever makes a larger percentage.

Repeat in after/before/before/after order at 18:51:08 UTC, after the ABBA
handle has exited successfully. All eight comparison boots use the same frozen
inputs and normal-device SMP=4 configuration. Host load is now
9.66 / 8.35 / 7.63 on 10 CPUs. No other builds/tests/VMs run during this timing
job either. Logs:
/private/tmp/nanami-opt-cycle39-startup-{after3,before3,before4,after4}.log.
Final performance/GUI acceptance is still pending this replication.

The inverse comparison also exits zero. All eight boots' serial logs contain
the final three-sample cleanup=true startup marker with no checked panic,
fatal, process-spawn or shared-memory rollback failure. Each of the 12 baseline
sessions emits 17 private fork-vm creations; each of the 12 candidate sessions
emits 15. The count is directly observed, not inferred from script spelling.
Inverse-order raw milliseconds:

| Session | After 3 | Before 3 | Before 4 | After 4 |
| --- | ---: | ---: | ---: | ---: |
| First session | 4715.629 | 5008.924 | 5008.048 | 4741.406 |
| Second session | 5188.263 | 5292.548 | 5298.937 | 5112.968 |
| Third session | 5194.522 | 5399.859 | 5407.761 | 5191.375 |

| Startup summary (ms) | ABBA baseline / candidate | BAAB baseline / candidate | Both orders baseline / candidate |
| --- | ---: | ---: | ---: |
| First session, fresh boot | 5118.015 / 4722.417 | 5008.486 / 4728.518 | 5059.684 / 4722.417 |
| Later sessions, same boot | 5362.638 / 5195.201 | 5349.398 / 5189.819 | 5354.187 / 5191.442 |
| All sessions pooled | 5306.458 / 5140.985 | 5295.743 / 5150.616 | 5301.670 / 5150.616 |

First-session reductions are 7.73% in ABBA, 5.59% in BAAB and 6.67% combined.
Later-session reductions are 3.12%, 2.98% and 3.04%; pooled reductions are
3.12%, 2.74% and 2.85%. The 337ms combined first-start difference exceeds the
100ms observation interval; the roughly 163ms later-session difference is much
smaller. There are only four boots per variant: keep this limited evidence,
raw observations and environment qualifiers visible. Do not claim universal
emulation, IPC, hardware or 5% all-session acceleration. This is specifically
startup-profile work that avoids two unnecessary private process lifecycles.

After the timed comparisons finish, run the normal-device SMP=4 real GUI
regression, host builder tests and native regressions. The final builder suite
passes 31 tests, including library/output alias safety and preservation of the
previous generated build after helper compilation failure. The C retry-policy
model passes ordinary and ASan/UBSan execution with an aligned opaque fake
Display handle. No runtime helper source or image input changes during timing.
Logs: /private/tmp/nanami-opt-cycle39-builder-final2.log and
/private/tmp/nanami-opt-cycle39-native.log. Real offline desktop assembly and
the two-session GUI job are still being checked; host mocks are not substituted
for either command.

Final validation and decision:

- Native regressions pass 1,127 tests (16 opt-in diagnostics ignored).
- Builder tests pass 31 cases. The C helper's mocked retry/liveness policy also
  passes host ASan/UBSan, and the observer's single-file rustfmt check and scoped
  git diff --check pass.
- A real `python3 -B thirdparty/tinycore/build.py --desktop --offline
  --output-dir /private/tmp/nanami-opt-cycle39.DgxgXE/assembled` completes.
  Its manifest records helper SHA-256 d7699dbb...e01e3, byte-identical to the
  measured image helper; the generated script also compares byte-identical.
  Fresh/preserved libc and Xlib hashes match respectively
  ac801523a9b9bdba84b10e5df466d9ee03b9a0e8a363985d7fd8b146b6451684
  and ea17c01e14debce9f0ab6021970a758b436412b1977d39d22414ea715fddeecc.
  This tests the real build path, not just the compiler mock.
- The first assembly attempt fails with host ENOSPC while extracting/copying
  packages, not a helper/runtime failure. Its temporary staging is removed and
  no partially published output remains. After lsof reports no open handle,
  remove the redundant owned cycle37-restored.img (321MiB logical); cycle38's
  validated image and both cycle39 comparison images remain. Retry succeeds;
  logs are /private/tmp/nanami-opt-cycle39-assembly{,2}.log. QEMU's separate
  diagnostics remain empty during this host-space incident, and its GUI test
  continues normally. After checking the successful assembly manifest/binaries,
  remove only that newly generated duplicate assembled/ tree (106MiB allocated).
  Sources, cached inputs, patches, logs and test images remain; it is rebuildable.
- The SMP=4 normal-device GUI job passes two complete sessions in one boot,
  including real USB mouse/title drag, keyboard input, menu/link preservation,
  dock-launched Terminal/Information, first-session Editor, and Exit/restart
  cleanup. Both per-cycle INPUT/LAUNCH/INFO/SESSION markers and final
  TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true are present. The job
  exits zero; no checked fatal/panic/spawn/shared-memory error is observed.
  Log: /private/tmp/nanami-opt-cycle39-desktop.log; serial/screenshots:
  /var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-qtshies7.
- Alpha remains d6af9865...438af, Alter/Linux 7e6e6009...be5c6, Honoka
  7b788303...2070c and Kernel 312d579b...d0c88. A9N's worktree remains clean.
  No Kernel/API/IPC-fastpath, version or release-metadata change is introduced.

Keep the small helper in profile-owned tools with the standard libc entry,
explicit DISPLAY/PID arguments and existing Xlib behavior; do not patch upstream
programs or replace private fork with shared memory. Scoped pre-cycle files are
in /private/tmp/nanami-opt-cycle39.DgxgXE; complete source/test patch:
/private/tmp/nanami-opt-cycle39-readiness-candidate.patch (20,962 bytes).
Keep the optional count read in the performance observer outside the measured
boundary, with no counter-file requirement in normal profiles. The normal
validated image is /private/tmp/nanami-opt-cycle39-after.img. The complete
optimization goal remains active; this cycle does not establish uniform 5%
acceleration of the full system or solve the remaining ordinary-fork copying.

## Cycle 40: admit medium DSOs within the existing snapshot byte budget

Status: reject the 4MiB admission candidate after the matched normal comparisons.
Production is restored byte-for-byte to the 2MiB policy; keep two general
cache-boundary/version/segment regressions and the rejection evidence. Final
normal-image startup verification passes as recorded below. A simulated
read-traffic reduction is not substituted for a measured end-to-end speedup.

The preceding goal turn makes progress: the profile-owned Xlib helper reduces
private forks per desktop startup from 17 to 15, passes eight normal comparison
boots, real offline assembly and two GUI sessions. Keep that accepted profile
and the Kernel unchanged. Investigate Alter's file-backed mmap itself next.

Inspect the production loader, streaming mapper and bounded image cache before
changing capacities. File mmap still obtains a fresh versioned identity from
the same open FD; whole-file snapshots are private immutable Alter buffers and
guests receive private initialized backing. A cold partial mapping does not
prefetch the entire file. Those contracts must remain unchanged.

The pinned root's /usr/lib/libstdc++.so.6.0.34 is 3,171,128 bytes, above the
2MiB per-file admission ceiling. The much larger GTK/FOX/librsvg libraries are
not a basis for claiming a default-startup gain. Revisit the old cycle31 lookup
trace as a candidate selector, not a measurement of the current runtime.
It has 2,593 records; coalescing 396 nested mapping-fill/loader records yields
2,197 logical lookups. A replay with the original eight slots/2MiB ceiling
matches every observed hit/miss (zero disagreements), with 1,736 hits, 461
misses and 180,021,982 estimated read bytes. Model capacities using page-rounded
grants, geometric reuse growth, actual retained-capacity admission, LRU victim
selection and protection of the outstanding main image while loading PT_INTERP.

For 48 slots and the same 16MiB total budget, the old trace predicts:

| Per-file ceiling | Hits / misses | Estimated file bytes read | libstdc++ hits / misses | Final retained capacity |
| --- | ---: | ---: | ---: | ---: |
| 2MiB | 2018 / 179 | 79,050,736 | 0 / 24 | 16,764,928 |
| 4MiB | 2030 / 167 | 72,435,352 | 18 / 6 | 16,773,120 |

This suggests 8.37% fewer read bytes, not 8.37% faster startup. Retaining the
medium DSO also displaces smaller entries, and its first snapshot fill adds a
copy that the uncached streaming mapper avoided. The old trace predates the
cycle39 profile and is not proof of the present working set or speed. Only
normal current-image comparisons can justify retention.

Candidate: change only MAX_IMAGE_BYTES from 2MiB to 4MiB. Keep 48 entries and
MAX_RETAINED_BYTES=16MiB, including capacity/page rounding and failed-fill
ownership. Do not enlarge the memory budget, add a cache layer, prefetch cold
partial mappings, skip fresh FD version checks, share mutable data with guests
or change any Kernel/IPC/public protocol path. Large/non-admitted files keep
the existing reusable/streaming fallback.

Add production-loader tests for a 3MiB+37-byte DSO: a cold partial request does
not read/allocate; a whole-file snapshot serves later segment reads; a new file
version reloads and preserves the old immutable snapshot. An over-ceiling
whole-file mmap bypasses without prefetch/allocation. Existing churn and main/
interpreter-protection tests retain the 16MiB bound with the larger ceiling.
Focused native checking passes 14 cases. Pre-cycle files:
/private/tmp/nanami-opt-cycle40.hf5GZb. Full native, ASan, AArch64 and normal-image
validation follow before timed comparisons against cycle39-after.img.

Frozen validation passes: 1,129 native tests (16 opt-in probes ignored), 14
ASan loader/cache tests, AArch64 Alter/Linux Release checking and the normal
x86_64 image build. The single-file rustfmt check also passes. Logs:
/private/tmp/nanami-opt-cycle40-{native,asan,arm,build}.log.
Only the shared Alter cache admission constant/comment changes in production;
the common component also changes the FreeBSD executable, without changing
its memory/file semantics. Candidate executable SHA-256:
Alter/Linux b9d1f3b3a0fb6bafa9f08b5ad3281d459af2320e11bb5aa0f8ec4395755e1ba6;
Alter/FreeBSD d27608b66abe9fc436b215bf08257df6b6884d005d819943837856947c89a84a.
Alpha, Honoka and Kernel retain cycle39 hashes d6af9865...438af,
7b788303...2070c and 312d579b...d0c88; observer remains d3f2bdb8...2f72.

Use /private/tmp/nanami-opt-cycle39-after.img as the baseline, not cycle39's
before image (which has the old X-readiness loop). Candidate is
/private/tmp/nanami-opt-cycle40-cache.img. Both have the same frozen cycle39
overlay, helper, payloads and observer. Profiling is unset. All build/test
handles terminate and a filtered process inventory is empty before starting
sequential before/after/after/before process-plus-startup comparisons at
19:06:43 UTC, with normal devices and SMP=4. Host load is elevated at
15.54 / 14.11 / 10.91 on 10 CPUs. Do not stop unrelated host processes or
attribute small control movements to this cache constant. No other local
build/test/VM work runs during the timed comparisons. Logs:
/private/tmp/nanami-opt-cycle40-{process,startup}-
{before1,after1,after2,before2}.log. Performance acceptance remains pending.

All eight normal comparison boots finish with exit zero. Every process log has
PROCESS-CACHE-FRESHNESS-PASS, TINYCORE-PROCESS-PERFORMANCE-PASS and the standalone
PROCESS-PERFORMANCE-PASS; every startup log has its three-sample cleanup=true
completion marker. No checked fatal/spawn/shared-memory failure appears.
Raw startup samples, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4889.180 | 4710.230 | 4812.351 | 4899.349 |
| Second | 5094.586 | 5080.860 | 5104.141 | 5192.476 |
| Third | 5102.134 | 5196.864 | 5196.738 | 5299.702 |

Pooled medians include six samples per side, lower is better:

| Workload (ms) | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| All real startup sessions | 5098.360 | 5092.501 | -0.11% |
| First startup only | 4894.265 | 4761.291 | -2.72% |
| Later startup sessions | 5147.305 | 5150.440 | +0.06% |
| getpid, 2048 calls | 38.322 | 38.481 | +0.41% |
| getuid, 2048 calls | 153.224 | 158.339 | +3.34% |
| Normal copyout workload | 274.484 | 269.222 | -1.92% |
| fork, eight children | 17.341 | 18.122 | +4.50% |
| fork/exec, eight children | 669.875 | 680.757 | +1.62% |
| fork, 128 additional VMAs / eight children | 18.835 | 20.626 | +9.51% |
| fork, 256 additional VMAs / eight children | 18.273 | 18.508 | +1.29% |
| Native Alpha construction/reap, eight children | 24.597 | 24.246 | -1.43% |

The all-startup difference is about 6ms, far below the 100ms readiness polling
interval, and later sessions are essentially unchanged. Unrelated controls
also move under the elevated host load. Do not attribute their regressions to
medium-file admission without evidence, or their improvements to it. The
candidate does not establish the requested 5% end-to-end improvement, so reject
it rather than retain a speculative cache policy. This does not prove the
policy is slower for every other application; that was not measured.

Archive: /private/tmp/nanami-opt-cycle40-medium-cache-candidate.patch (4,165
bytes), with raw candidate Alter/Linux and FreeBSD executables under the
pre-cycle snapshot directory. Restore image_cache.rs byte-for-byte from its
pre-cycle snapshot; cmp succeeds. Generalize the new positive DSO test to
MAX_IMAGE_BYTES-37 and retain the over-ceiling no-prefetch/no-allocation check,
so both regressions exercise the accepted policy without granting a new size
limit. No 4MiB production path remains.

Restored verification passes 1,129 native tests (16 ignored), 14 ASan loader/
cache tests and AArch64 Alter/Linux Release checking. Formatting and scoped
git diff --check pass. Logs:
/private/tmp/nanami-opt-cycle40-restored-{native,asan,arm,build}.log.
The rebuilt Linux executable is again byte-identical to the baseline:
7e6e6009f038b6ec9c1de1e2bdd8b0a646b1a785f240ba04f7ea32cbf8abe5c6;
FreeBSD also returns to f65970a9688b6880591ec80bcda52c4f8c40fef5153f4f2061beeb8a57f68a0f.
Remove only the owned rejected cycle40-cache.img (321MiB logical) after the
comparison job exits and lsof reports no open handle. Its patch, raw binaries,
frozen inputs and logs remain reproducible. Build the normal restored image
as /private/tmp/nanami-opt-cycle40-restored.img and verify actual startup next.

The restored normal image completes all three actual X11 terminal startup,
kill/delete and same-ID reuse sessions, with the final cleanup=true marker.
Elapsed startup values are 4694.305 / 5108.784 / 5204.514ms. This is restoration
verification, not a new paired speedup claim. Log:
/private/tmp/nanami-opt-cycle40-restored-startup.log; serial suffix vnz0ds_o.
The preserved runtime is the cycle39 implementation, including the qualified
shared-RAM alias cache and one-process X readiness helper, not the rejected
4MiB experiment. No Kernel, public protocol, IPC-fastpath, version or release
change is retained in this cycle. The broader optimization objective remains
active and the 5% target is not established here. Do not retry the same ceiling
increase or enlarge the total memory budget on the old model's prediction;
obtain current read/fill/copy evidence before choosing another cache policy.

## Cycle 41: measure current file-mmap read and initialization work

Status: diagnostic progress, with normal runtime restored and verified. The
5% improvement target is not established in this cycle. The user explicitly
retains the user-level-only boundary;
do not change A9N, including its recoverable-fault logging. Cycle40 makes
progress through matched rejection, restoration, new general regressions and
normal startup verification. It does not establish a 5% improvement.

Snapshot the four diagnostic source files under
/private/tmp/nanami-opt-cycle41.4thI4U before editing. Add temporary aggregate
counts, payload bytes and local-counter elapsed spans to the existing opt-in
Alter profiler: snapshot reads/copies, streaming reads/partial-page copies,
cached/streamed/zero initialization requests and file-mmap initialization.
Cache hit/miss byte counts have no elapsed span. Snapshot-loader lookups include
nested mmap-triggered fills, so their logical lookup counts must not be added
blindly to mmap misses. File-mmap outer spans start after versioned fstat and
address reservation, and overlap their read/initialization children.

All aggregate output occurs at existing syscall-exit flush points, outside
the measured handler. Totals are global to the single-threaded Alter loop,
not attributed to the PID that triggers the flush. The final unflushed live
tail may be absent. RPC intervals include waits/descheduling/allocation and
initialization, not exclusive IPC or byte-copy CPU time. Diagnostic startup
times cannot be compared with normal images. Restore all temporary changes
before normal comparison or handoff; retain no profiling overhead by default.

Disk initially has about 180MiB available. After lsof finds no open handles,
remove only owned cycle39-before.img and cycle39-after.img (321MiB logical
each). Keep cycle40-restored.img as the byte-identical current normal baseline,
plus source snapshots, candidate patches, frozen inputs and all comparison/GUI
logs. Both old images are reproducible; no user sources or test evidence is
deleted. No build/test/VM process is started before this diagnostic cycle.

The first diagnostic build initially fails on a temporary profiler import in
the Linux modules (crate::syscall_profile is not re-exported); fix it to the
existing crate::common::syscall_profile before rebuilding successfully. This
is instrumentation only. The normal Kernel remains SHA-256
312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.
Profile Alter/Linux is 3bb19e46...009f3, FreeBSD 39b6f3e8...8099a;
save both raw diagnostic executables in the pre-cycle directory.

The diagnostic image completes all three real startup/kill/delete/reuse
sessions with cleanup=true, without a checked fatal/spawn/shared-memory error.
Log /private/tmp/nanami-opt-cycle41-profile-startup.log; serial directory
suffix 9fsf1ywi. Across the collected global exit-flush coverage, snapshot reads
are 557 / 32,711,574 bytes / 256,410,000 local-counter ticks; the matching
snapshot copies are 11,954,000 ticks. Streaming reads are 352 / 22,498,528
bytes / 135,039,000 ticks. Partial-page buffering copies only 3,296 bytes.
Initialization requests account for 395,853,824 bytes: cached 373,293,056,
streamed 22,511,616 and zero-only 49,152. Their aggregate elapsed spans are
2,443,664,000 ticks versus 3,250,194,000 for overlapping outer file-mmap
initialization spans (75.19%); snapshot copy is 0.37% of that outer interval.
Do not add child/outer intervals. Those snapshot reads/copies also include
ordinary executable loads outside file mmap, so their ratios are contextual,
not an exclusive decomposition of the outer span.

Cache mapping lookups report 1,434 hits and 114 misses. Do not count nested
snapshot-fill lookups twice. These fresh observations do not justify a larger
cache: bytes read are much smaller than bytes privately initialized. Exit
flushes cross session boundaries, especially live application work flushed
during the following session, and omit the final unflushed tail. Do not label
the per-flush-boundary totals as per-session ownership or whole-system totals.

Add a second temporary Alpha-only phase aggregation to the existing opt-in
exec profiler: validate source, reserve destination, allocate/copy Frame caps,
map initialization window, initialize contents and publish guest mappings.
Accumulate per successful request and group by source PID, with zero-fill in
bucket zero and >=127 in an explicit overflow bucket. Flush at existing reap
report points, outside sampled phases. Per-page counter reads perturb this
diagnostic, so use only for bottleneck selection, not speedup comparisons.
Snapshot memory_pages.rs and exec_profile.rs before editing. First image is
removed only after its test handle exits zero and lsof finds no open handle;
its logs/raw binaries and reproducible inputs remain, and the current normal
baseline remains. Build the second image as cycle41-phases.img.

The second image builds and completes its three startup/kill/delete/reuse
sessions, with the final cleanup=true marker and no checked fatal/spawn/SHM
error. Log /private/tmp/nanami-opt-cycle41-phases-startup.log; serial suffix
8zwsig9a. Diagnostic init Booter SHA-256 is
88dac1d3170250653251b5181877f114f1da1b78225dfee074c9c26e1b99d172;
save its raw executable as init-phases.elf in the pre-cycle directory. This
Booter embeds the profiled Alpha; it is not the independent Alpha executable.
A9N's hash remains exact.
The source service PID is observed from system-manager's spawn log, not guessed:
Alter/Linux is PID 18. All recorded sources are below 127 (no overflow bucket).

| Successful initialization group | Requests | Pages | Validate | Reserve | Allocate | Destination window | Content path | Publish |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Alter source (PID 18) | 2432 | 115544 | 28115000 | 48490000 | 310593000 | 476630000 | 1937445000 | 220294000 |
| Guest / other source | 225 | 55995 | 10611000 | 3114000 | 146830000 | 222568000 | 1446581000 | 108084000 |
| Zero source | 711 | 35724 | 166000 | 16822000 | 104216000 | 171905000 | 356816000 | 74711000 |

All phase values are elapsed local-counter ticks, not exclusive CPU time.
For the Alter-source group, the content path is 64.12% of the six-phase sum,
destination window 15.77%, allocation 10.28%, publication 7.29%, reservation
1.60% and source validation 0.93%. The content path includes source-Frame
lookup and source-alias lookup/mapping as well as byte copy and suffix zeroing;
do not describe it as memcpy alone. The 115,544 pages also include executable,
stack and other source-18 initialization, not just file mmap. The zero group
can include pre-desktop service initialization before the first reap. Reap
flushes cover a different boundary from Alter exit flushes, so do not subtract
their totals, attribute one PID trigger as ownership, or compare raw counters
across cores. These are two diagnostic runs, not normal performance trials.

This rules out the tempting duplicate source-validation scan as a sufficient
5% lever in this trace: it accounts for less than 1% of the six-phase subtotal
even before considering other startup work. Preserve its early-failure and
authority behavior. It also does not justify repeating the rejected medium-
DSO admission, pooled source hints or custom scalar memcpy experiments.
Next investigation should separate source lookup/alias work from actual copy/
zero inside the content path, retaining current per-page freshness, private-
backing and rollback guarantees, then test a normal user-level candidate.

Archive both stages of temporary instrumentation in
/private/tmp/nanami-opt-cycle41-diagnostics.patch. Restore all six modified
runtime sources with apply_patch from their pre-cycle snapshots; cmp succeeds
for every file. No Kernel, ABI, IPC fastpath, cache policy, permission check,
version or release change is retained. After the second test handle exits zero
and lsof finds no open handle, delete only the reproducible phases image to
make room for normal restoration. Raw diagnostic executables, patches, logs,
frozen inputs and cycle40-restored.img remain. Full native checking and a
fresh normal-image restoration follow, with all profiling flags unset.

Restored native checking passes 1,129 tests, zero failures and 16 opt-in probes
ignored. rustfmt --check and git diff --check pass. Normal x86_64 image build
passes; logs /private/tmp/nanami-opt-cycle41-restored-{native,build}.log. The
independent Alpha at nanami/alpha.elf and out/alpha-target/.../release/alpha
returns exactly to d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af;
Linux 7e6e6009...be5c6, FreeBSD f65970a9...8a0f and Kernel 312d579b...d0c88 also
match the accepted baseline. An initial comparison used the enclosing init
Booter at Spencer's nun_os_target_dir by mistake, which is not the independent
Alpha executable; correct that artifact identity before claiming a mismatch.
The saved diagnostic Booter is accordingly named init-phases.elf. A9N's
worktree is clean throughout.

The profiler-disabled cycle41-restored.img completes three actual X11 terminal
startup, kill/delete and same-ID reuse sessions with cleanup=true. Startup
times are 4707.073 / 5095.565 / 5302.801ms; restoration verification, not a
paired new speedup. No temporary alter.work.profile, init.profile or Alter
syscall-profile output appears, and no checked fatal/spawn/SHM failure occurs.
Log /private/tmp/nanami-opt-cycle41-restored-startup.log; serial suffix knmvp7k_.
The broader objective remains active. Retain only diagnostic evidence and the
README link in this cycle, not a speculative production optimization or a new
release/version. The accepted cycle39 runtime remains the current baseline.

## Cycle 42: separate private-page content work and source-alias reuse

Status: reject the eight-way policy after normal matched comparisons; runtime
is restored and final normal verification passes. The previous turn makes
diagnostic progress: two current
user-level profiles identify the content path as the largest initialization
phase, rule out source validation as a sufficient 5% lever, restore six files
exactly and verify normal runtime hashes, 1,129 native tests and three actual
TinyCore startups. Preserve the explicit no-Kernel-change boundary.

Re-read current sources and verify Alpha d6af9865...438af, Alter/Linux
7e6e6009...be5c6 and Kernel 312d579b...d0c88. Snapshot memory_pages.rs,
exec_profile.rs, memory_copy.rs and the copy-window harness under
/private/tmp/nanami-opt-cycle42.gPf2D0. Temporarily instrument source Frame
resolution, source-alias lookup/mapping, byte copy and zero-fill separately.
Aggregate elapsed local-counter ticks and exact bytes/chunk counts per source
PID at existing reap reports; zero-fill-only work uses PID zero and source
PIDs >=127 share an overflow bucket. Separately count copy-cache hits and
misses by normalized cache owner (zero for permanent pool Frames), not by
source PID or content-path request ownership. The counters include calls
outside this particular initializer, so do not equate their coverage.

No per-page output or Kernel changes are added. Counter reads perturb timings;
intervals include interrupts/descheduling and are not exclusive CPU time.
Report only successful initializer requests; reap-triggered flush boundaries
can cross desktop sessions and omit a final unflushed tail. Restore temporary
instrumentation before any normal comparison. Do not preselect cache sizes or
weaken validation based on earlier traces.

Available disk is about 428MiB. After lsof finds no open handle, remove only the
owned cycle40-restored.img (321MiB logical): cycle41-restored.img has the same
accepted executable hashes and completed three normal sessions. Keep that
current baseline plus all source snapshots, patches, logs and frozen inputs.

The detailed diagnostic builds and completes all three startup/kill/delete/
same-ID reuse sessions. Log /private/tmp/nanami-opt-cycle42-content-startup.log;
serial suffix 4go1k_ze. Actual independent profiled Alpha SHA-256 is
eb55f757c4a3a3a68acca7f403c516073bbe1064712482b416ddf737622167cb;
save alpha-content.elf in the snapshot directory. Kernel/Alter hashes remain
unchanged. Observed source PID 18 is Alter/Linux, confirmed in the spawn log.
Collected content-path totals for that source are: Frame resolution 31,852,000,
source-alias preparation 244,616,000, byte copy 1,686,028,000 and suffix zeroing
36,459,000 elapsed local-counter ticks. Copy traffic is 466,034,436 bytes in
114,141 chunks; suffix zero traffic is 12,349,692 bytes. Byte copy accounts for
84.35% of that four-phase subtotal, alias preparation 12.24%, resolution 1.59%
and zeroing 1.82%. This is a detailed diagnostic, not exclusive CPU time or a
breakdown of the entire startup. It does not equate memcpy elapsed time with
memory bandwidth, nor justify repeating cycle16's rejected scalar primitive.
Current compiler-builtins disassembly still uses REP MOVSB/MOVSQ.

Cache-owner 18 has 88,223 observed hits and 41,280 misses (31.88% misses).
Permanent pool-owner zero has 43,499 hits and 146 misses, consistent with the
accepted pool-alias lifetime improvement. These cache counts include all
copy-window uses, not just this initializer. Keep current owner normalization,
per-page Frame resolution, permission checks, failure cleanup and invalidation.

Archive temporary diagnostics in
/private/tmp/nanami-opt-cycle42-content-diagnostics.patch; restore all three
runtime files byte-for-byte with cmp confirmation. The diagnostic image is
removed only after its test handle exits zero and lsof finds no open handle;
raw Alpha, source snapshots and logs remain. Do not benchmark instrumentation.

Candidate: increase existing associativity from four to eight ways, reducing
sets from 512 to 256 so the alias count remains exactly 2,048 (8MiB virtual
window). No new cache, data buffer, service message or Kernel change is added.
The round-robin array shrinks by 256 bytes; all identity/pinned-source/failure/
owner-count semantics stay intact. The tradeoff is scanning up to eight tags
instead of four in exchange for potentially fewer conflict evictions; current
miss counts do not prove a speedup or distinguish all capacity misses. Require
normal comparisons before retention. Add a generic alias/metadata budget guard
that passes for either policy. Focused production-source tests pass 15 cases.

Full native checking passes 1,130 tests (16 ignored), focused ASan passes 113
(two ignored), AArch64 Alpha Release checking and the x86_64 normal-image build
pass. An initial ASan command incorrectly names a nested module as a Cargo test
target; it runs no tests and exits 101. Correct it to the actual core-process
target, containing the checked-memory integration, before reporting the 113
passing tests. Logs /private/tmp/nanami-opt-cycle42-{native,asan,arm,build}.log.
Formatting and diff checks pass. Candidate Alpha SHA-256 is
e7a101a50642a75985b9b08e08cc6067782454ee3b941171e7807274253b4683;
Alter/Linux 7e6e6009...be5c6, FreeBSD f65970a9...8a0f, Honoka 7b788303...2070c,
Kernel 312d579b...d0c88 and observer d3f2bdb8...2f72 are unchanged. Save raw
Alpha and /private/tmp/nanami-opt-cycle42-eight-way-candidate.patch.

Compare normal-device SMP=4 sequential process-plus-startup runs in
before/after/after/before order. Baseline is cycle41-restored.img, candidate
cycle42-eight-way.img, using the same frozen cycle39 overlay/helper and observer.
All build/test handles finish before comparison; no concurrent owned builds,
tests or VMs run. Host load before comparison is about 10.21 / 9.23 / 9.74 on
10 CPUs; leave unrelated host work untouched. All profiling flags are unset.
Logs /private/tmp/nanami-opt-cycle42-{process,startup}-
{before1,after1,after2,before2}.log. Performance acceptance remains pending.

All eight normal comparison boots finish with exit zero. Every process serial
contains PROCESS-CACHE-FRESHNESS-PASS, TINYCORE-PROCESS-PERFORMANCE-PASS and
the standalone PROCESS-PERFORMANCE-PASS. Every startup serial contains its
three-sample cleanup=true marker. No checked fatal/spawn/shared-memory failure
or temporary profile output appears. Raw startup samples, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4820.706 | 4696.103 | 4810.634 | 4707.467 |
| Second | 5205.693 | 5107.400 | 5095.461 | 5099.891 |
| Third | 5204.175 | 5192.028 | 5199.823 | 5305.463 |

Pooled medians include six samples per side, lower is better:

| Workload (ms) | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| All real startup sessions | 5152.033 | 5101.431 | -0.98% |
| First startup only | 4764.086 | 4753.369 | -0.22% |
| Later startup sessions | 5204.934 | 5149.714 | -1.06% |
| getpid, 2048 calls | 39.232 | 38.873 | -0.92% |
| getuid, 2048 calls | 156.260 | 154.068 | -1.40% |
| Normal copyout | 272.700 | 274.860 | +0.79% |
| fork, eight children | 17.846 | 18.502 | +3.68% |
| fork/exec, eight children | 672.363 | 669.889 | -0.37% |
| fork, 128 additional VMAs / eight children | 18.480 | 18.212 | -1.45% |
| fork, 256 additional VMAs / eight children | 18.522 | 18.795 | +1.47% |
| Native Alpha construction/reap, eight children | 28.407 | 23.192 | -18.36% |
| Full native BusyBox prepare/create/kill/release | 28.827 | 27.992 | -2.90% |
| Full native Xfe prepare/create/kill/release | 28.381 | 28.288 | -0.33% |

The all-startup shift is 51ms, below the 100ms readiness polling interval;
first-start differences pair to only about 11ms despite the reversed order.
Do not count the large pure-construction/reap movement as a Linux application
speedup: that workload does not execute guest Linux code or its changed source-
copy hit/miss path. Other controls move in both directions. The results do not
establish the requested 5% gain or a convincing copy/exec improvement; reject
the new policy rather than retain an unsupported extra hit-path scan. This
does not prove eight ways are slower for every possible workload.

Restore memory_copy.rs byte-for-byte from before cycle42, with cmp success.
Keep the new generic budget guard, which also passes with four ways. Candidate
patch and raw Alpha remain archived. After the comparison cell is terminal
and lsof finds no open handle, remove only the rejected eight-way image and
rebuild normal cycle42-restored.img, retaining cycle41's normal baseline.
Restored full native checking passes 1,130 tests (16 ignored); ASan copy-window
checking passes all 15. Alpha returns exactly to d6af9865...438af, Linux
7e6e6009...be5c6 and Kernel 312d579b...d0c88. Logs:
/private/tmp/nanami-opt-cycle42-restored-{native,asan,build}.log.

The restored normal image completes three real X11 terminal startups, kill/
delete and same-ID reuse with cleanup=true: 4725.266 / 5200.231 / 5193.496ms.
This is restoration verification, not a paired speedup. Log:
/private/tmp/nanami-opt-cycle42-restored-startup.log; serial suffix 1avhfh0w.
No checked fatal/spawn/SHM failure or temporary content/Alter profile output
appears. Formatting and git diff checks pass. A9N's worktree remains clean.
Keep only the generic budget regression and investigation evidence this cycle;
the eight-way policy, profiling code and all runtime behavior changes are
removed. Nanami remains 0.7.1; no commit/tag/release/API change is performed.

Next bottleneck investigation must aim at reducing required private copies,
not repeatedly adjusting this cache's way count. A source-level feasibility
audit of read-only fork sharing finds contracts that must first be satisfied:

- clone_process_vm in alpha/memory_clone.rs copies backed runs and applies
  final permissions page by page. It does not track per-page shared backing.
  Existing process memory_owner/VM peers describe whole-address-space sharing,
  not independent private mappings of a common read-only physical page.
- alpha/mapping_protection.rs can grant write permission via mprotect-style
  requests, and alpha/checked_memory.rs allows an authorized supervisor to
  write through Alpha aliases. Sharing would have to detach private backing
  before either legitimate write path; hardware PTE read-only alone is not
  a complete ownership or immutable-content contract.
- process_requests.rs currently configures the same child_root used for its
  AddressSpace/Frame management as the guest's execution capability root.
  Kernel syscall.cpp handles native CAPABILITY_CALL (-1) before the Linux
  personality fallback, and address_space.cpp's Frame MAP path accepts valid
  requested R/W/X attributes. Do not assume a Linux guest cannot issue native
  capability calls or that the current PTE is the complete authority boundary.
  This is a source-level risk audit, not a newly demonstrated runtime exploit.
  A user-level isolated execution root could be evaluated using the existing
  PCB configure API, without changing Kernel ABI or native-process policy.
- Physical-page reuse must wait for the final independent mapping owner;
  parent exec/reap/unmap and cap revocation must not free or revoke backing
  still used by a child. Existing private-allocation and copy-cache rollback
  guarantees must remain true, including SMP stop/teardown ordering.

No read-only sharing or capability-root change is implemented on speculation.
First measure how many actual fork pages are safely shareable under their
current permissions; then design a bounded backing/lifetime policy and guest
authority separation before constructing a normal comparison candidate.
Ordinary Linux fork must not silently become CLONE_VM or vfork. The broader
optimization goal remains active; this cycle does not establish 5% end-to-end.

## Cycle 43: measure private-fork permission inventory before sharing

Status: permission inventory and contract regressions complete; normal runtime
restored and verified. Sharing/authority policy awaits the user's response to
the explicit opt-in isolation question; no new runtime optimization is claimed.
The preceding turn makes progress: current source content
profiling, a matched eight-boot comparison and rejection, restoration with
1,130 native/15 focused ASan passes and three real startups, plus a source-level
authority/lifetime audit. Do not repeat associativity or scalar-memcpy tuning
without new evidence. The explicit user-level-only boundary remains in force.

Re-read clone traversal, Linux fork and memfd inheritance. Alpha's ordinary
private fork currently copies every backed page and restores final protection;
the later memfd inheritance can replace copied shared pages with aliases. Do
not assume that all pages lacking write permission are immutable or eligible
for sharing. Private MAP_PRIVATE semantics, authorized supervisor writes,
mprotect upgrades, independent resource lifetimes and native capability
authority must all be preserved.

Snapshot memory_clone.rs and exec_profile.rs under
/private/tmp/nanami-opt-cycle43.rtMhSm. Temporary diagnostics record backed-page
counts for permissions 0 through 7, an explicit >=8/other bucket, source/target
PIDs and whether the destination was fresh. Report only completed clone_process_vm
requests, after the copy/protection interval. Sparse unbacked reservations are
absent from this inventory. Current permissions measure only a potential upper
bound, not safe immutable-shareable content or an implemented speedup. The
elapsed local-counter interval includes waits/descheduling and diagnostic
inventory work; do not compare it with normal-build startup times. No page
contents, flags, mappings, permissions, capability roots or Kernel sources are
changed by the diagnostic.

Disk starts at about 422MiB available. After lsof finds no open handle, delete
only owned cycle41-restored.img (321MiB logical); the matching accepted normal
runtime remains in cycle42-restored.img, verified with three startup/reuse
sessions and exact Alpha/Alter/Kernel hashes. Logs, snapshots and reproducible
frozen inputs remain. Build the diagnostic as cycle43-inventory.img.

The diagnostic builds and completes all three real startup, kill/delete and
same-ID reuse sessions, ending with cleanup=true and no checked fatal/spawn/SHM
failure. Log /private/tmp/nanami-opt-cycle43-inventory-startup.log; serial
suffix rmssssd3. Profile Alpha SHA-256:
8e98400087290a18e60e4c63dfe7e963de9aa79c2d91d49fcb4af4ddc74d7573;
save alpha-inventory.elf in the snapshot directory. Alter/Linux remains
7e6e6009...be5c6 and Kernel 312d579b...d0c88.

All 45 completed private clones have fresh destinations. Aggregate pages by
current permission value:

| Permissions | Pages across three startups |
| --- | ---: |
| R (1) | 13533 |
| RW (3) | 16185 |
| RX (5) | 21672 |
| RWX (7) | 4605 |
| None, W, X, WX or other | 0 |

Total 55,995 pages = 229,355,520 bytes. Non-writable backed pages are 35,205 =
144,199,680 bytes (62.87% of private-fork copy volume), not 62.87% of all system
memory traffic or startup time. Each container session has the same counts:
fourteen clones from its initial parent (source PIDs 23/40/57) and one clone
from another parent (37/54/71). The initial parent's fourteen clones cover
16,828 pages with 10,850 non-writable; the other clone covers 1,837 with 885
non-writable. No unbacked reservation is counted. Current permissions are an
opportunity bound, not proof that these pages may immediately be shared.

The source audit requires a policy decision before reducing copies by sharing.
Ask asynchronously whether an explicitly enabled, user-level isolated execution
capability root may be added for Linux guests. Such a mode would keep native
process policy and Linux syscalls, but prevent that guest from issuing native
A9N calls to rewrite its mappings behind the supervisor's tracking. Default/
native capability policy must not silently change. No Kernel changes are
requested or authorized. The question is pending; do not implement an authority
restriction or read-only sharing until its scope is resolved.

Archive temporary code in /private/tmp/nanami-opt-cycle43-fork-inventory.patch;
restore memory_clone.rs and exec_profile.rs exactly with cmp confirmation.
After the diagnostic handle exits zero and lsof finds no open handle, remove
only the reproducible diagnostic image; retain its raw Alpha, patch and logs.

Add two contract regressions using the production clone traversal with modeled
page construction/copy/protection: later mprotect-style write upgrades plus
authorized supervisor copies keep parent/child data independent; source unmap/
replacement followed by a descendant fork preserves the original child's
contents and permissions. These host backing models do not prove future shared-
Frame lifetime, real PTE protection, capability isolation or physical reuse;
those require production integration and normal guest tests when implemented.
Focused core-process checking passes 92 tests (two ignored); full native
checking passes 1,132 (16 ignored). ASan and normal restoration follow.

ASan core-process checking passes all 92 tests (two ignored), including both
new clone contracts. Normal x86_64 image build succeeds with profiling flags
unset. Independent Alpha returns exactly to d6af9865...438af, Alter/Linux
7e6e6009...be5c6, FreeBSD f65970a9...8a0f and Kernel 312d579b...d0c88.
All runtime edits are byte-identical to the pre-cycle snapshots; no authority,
root policy, sharing, protocol, version, commit or release change is retained.
Formatting and git diff --check pass; A9N's worktree remains clean.
Logs /private/tmp/nanami-opt-cycle43-{native,asan,restored-build}.log.

The restored profiler-disabled cycle43-restored.img completes three actual X11
terminal startups, kill/delete and same-ID reuse, with cleanup=true. Values:
4811.685 / 5201.117 / 5301.200ms. This is restoration verification, not a new
paired speedup. No clone.inventory or checked fatal/spawn/shared-memory failure
appears. Log /private/tmp/nanami-opt-cycle43-restored-startup.log; serial suffix
7dvb7nt5. Keep the measured opportunity, two contract regressions and evidence;
the 5% target is not established in this cycle and the broader goal stays active.
The outstanding choice must not be treated as permission to silently remove
existing Linux-guest native capabilities. An explicitly enabled isolated mode,
if approved, should leave the default/native path available and be validated
before any read-only backing is actually shared.

## Cycle 44: eliminate repeated immutable POSIX identity queries

Status: retain the connection-owned identity snapshot after both-order normal
process comparisons, startup controls and two-session GUI verification. This
improves the measured getter/getresid path, not whole desktop startup. The
previous turn makes progress
through a current permission inventory (62.87% non-writable private-fork pages),
two clone contract regressions and normal restoration checks. The optional
Linux-guest capability isolation question remains unanswered; do not alter
authority policy or implement read-only sharing on that basis. Keep Kernel
unchanged and inspect an independent existing syscall path instead.

Current fault handling uses the same inline hardware-context path for getpid
and getuid. The observed 39ms versus 154ms/2048-call control difference is not
proof of expensive register access: Linux getuid/geteuid currently query POSIX
on every call, as do getgid/getegid and getresid. Review POSIX Session identity
assignments, session creation/inheritance, request dispatch and client wrappers.
The current wire API has no credential-changing operation; identities are
initialized at session creation and inherited only when a child session is
created. Linux setresid accepts only current root/no-change operations and
rejects unsupported non-root changes instead of faking a privilege drop.

Candidate: an explicit per-Runtime, per-POSIX-connection four-field identity
snapshot, lazily filled from the exact real/effective user/group getter. Cache
only successful queries, never hardcode root ID or authorization results.
Read-only getters do not need to query unchanged session identity repeatedly.
getresid uses its existing root-only model and the corresponding real-ID
snapshot; guest memory-copy/protection checks stay unchanged. Distinguish
effective getters rather than aliasing their query to real UID/GID. Native
FreeBSD dispatch is not redirected, though its shared Runtime layout gains the
small empty cache. No per-Linux-process table, heap buffer, Kernel change,
capability-right change or new message opcode is added.

The cache is at most six machine words (48 bytes on supported targets), not a
global static or authorization decision cache. A different endpoint invalidates
all fields. Same-descriptor/new-session rebinding must explicitly clear it;
descriptor equality does not prove object/session continuity. No Runtime
POSIX-port rebinding currently exists. Future credential mutators must include
snapshot update/invalidation in their owner contract. Document the current
session-stability contract in message.md, without changing wire layouts.

Snapshots: /private/tmp/nanami-opt-cycle44.SeMQJx. Add production cache/adapter
tests for all four distinct getters, 40,000 warm reads with only four queries,
failure/retry without losing other fields, connection change, explicit same-slot
reset, independent Runtime caches and zero IDs/metadata bounds. An initial
test-module path goes through a nonexistent nested state directory; move its
production-source inclusion to the test root. All six focused tests then pass,
and full native checking passes 1,138 tests (16 ignored); ASan passes all six.
The first AArch64 compile catches a missing IdentityField import in system.rs,
outside the narrow adapter host harness. Fix that import before continuing;
do not claim those six host tests compile or test the whole Linux dispatcher.
ARM checking and normal x86_64 build follow. Logs:
/private/tmp/nanami-opt-cycle44-{identity-tests,native,asan,arm,build}.log.

The unchanged frozen process fixture checks actual getuid return values and all
three getresuid output words while timing; no fixture or observer is rebuilt.
This can measure identity-query elimination, not a faster Alpha byte-copy
primitive, even for the workload labelled copyout-normal. Real desktop timing
is still required; do not extrapolate a microbenchmark gain to system startup.

After lsof finds no open handle, remove only the owned cycle42-restored.img
duplicate (321MiB logical). Keep cycle43-restored.img, with matching accepted
runtime hashes and three verified normal startup/reuse sessions, as the before
image. All frozen inputs, logs and raw diagnostic patches remain reproducible.

After the missing import is corrected, AArch64 Alter/Linux Release checking
and the normal x86_64 image build pass. Full native is 1,138 passed / 16 ignored;
ASan identity adapter/cache is six passed. Formatting and diff checks pass.
Candidate Linux SHA-256:
4f1283cebe5c6641f16703fb6a04f43f098d9ad37d59d2070cceb93107d264b4;
FreeBSD f057e1a54f39a4ad5c2431bfa31cd4c78db49efc2897aaa5e581991c782af3da.
The latter rebuild reflects the shared Runtime layout, not redirected FreeBSD
identity queries. Save both raw executables in the snapshot directory. Alpha
d6af9865...438af, Honoka 7b788303...2070c, Kernel 312d579b...d0c88 and observer
d3f2bdb8...2f72 remain exact. No profile flags or diagnostic code are enabled.

Compare cycle43-restored.img with cycle44-identity.img using sequential normal-
device SMP=4 process-plus-startup boots in before/after/after/before order.
All build/test handles terminate first, with no concurrent owned build/test/VM
work. Leave unrelated host work alone; load is about 6.15 / 7.11 / 7.90 on ten
CPUs. The image overlay is the unchanged frozen cycle39 input and helper. Logs:
/private/tmp/nanami-opt-cycle44-{process,startup}-
{before1,after1,after2,before2}.log. Performance acceptance remains pending;
getuid and the getresuid-based copyout fixture must not be mislabeled as a
changed Kernel IPC or Alpha byte-copy primitive.

All eight initial normal boots finish with exit zero, with cache-freshness,
process-performance and three-startup cleanup=true markers in their respective
serial logs and no checked fatal/spawn/SHM failure or temporary profile output.
Raw startup samples, milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4710.310 | 4804.276 | 4700.896 | 4704.222 |
| Second | 5200.584 | 5200.920 | 5186.771 | 5181.965 |
| Third | 5306.408 | 5189.568 | 5197.631 | 5192.933 |

Initial pooled medians (six per side): getuid-2048 155.995 -> 39.1175ms
(-74.92%); getpid 39.521 -> 39.9325 (+1.04%); getresuid-based normal copyout
272.454 -> 171.228 (-37.15%), 128-VMA copyout 274.560 -> 174.725 (-36.36%) and
256-VMA copyout 277.4535 -> 172.619 (-37.78%). The fixture checks root identity
and all three output values, so a failed/short-circuited query is not a pass.
The copyout gain removes an identity RPC, not guest byte copying/protection.

Startup is 5187.449 -> 5188.1695ms (+0.014%); first 4707.266 -> 4752.586
(+0.96%), later 5196.7585 -> 5193.5995 (-0.061%). These are effectively unchanged
relative to the 100ms observation interval, not a system-wide 75% improvement.
Ordinary fork-8 is 17.899 -> 18.989ms (+6.09%); fork-exec-8 669.661 -> 661.031
(-1.29%), fragmented fork controls -3.18%/-3.92%. Do not ignore the ordinary
fork movement merely because the intended getter improves. Run additional
normal process boots in after/before/before/after order before acceptance,
with no concurrent VM/build/test work. Keep all observations, not only favorable
samples. Extra logs use trial names after3/before3/before4/after4.

All four reverse-order process boots finish with exit zero and every required
freshness/process marker, with no checked fatal/spawn/SHM failure. Combined
process medians now include twelve samples per side (four boots each):

| Workload (ms) | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| getuid, 2048 calls | 155.174 | 39.008 | -74.86% |
| getpid, 2048 calls | 39.242 | 39.6085 | +0.93% |
| getresuid-based normal copyout | 272.904 | 170.4385 | -37.55% |
| getresuid-based copyout, 128 VMAs | 274.560 | 172.068 | -37.33% |
| getresuid-based copyout, 256 VMAs | 276.396 | 173.2705 | -37.31% |
| fork, eight children | 19.105 | 18.750 | -1.86% |
| fork/exec, eight children | 670.5555 | 664.4805 | -0.91% |
| fork, 128 additional VMAs / eight children | 18.9445 | 18.378 | -2.99% |
| fork, 256 additional VMAs / eight children | 18.626 | 18.0145 | -3.28% |
| Native Alpha construction/reap, eight children | 24.4085 | 25.529 | +4.59% |
| Full native BusyBox prepare/create/kill/release | 28.1115 | 28.5125 | +1.43% |
| Full native Xfe prepare/create/kill/release | 28.248 | 28.0355 | -0.75% |

Per-boot getuid medians before are 157.208 / 153.966 / 152.901 / 156.839ms,
after 38.318 / 39.509 / 38.618 / 39.514. The intended gain is stable in both
orders. Ordinary-fork boot medians before range 17.286..19.954ms and after
17.249..19.960ms; the initial +6% movement is not a reproducible regression.
Do not relabel its later -1.86% as a proven fork optimization; neither fork
nor Alpha's copying implementation changed. Other controls also vary. The
large identity/getresid gain is specific to eliminated repeated identity RPCs.

There are no extra startup boots in the reverse-order extension: the six-per-
side startup values above remain the complete startup evidence, essentially
unchanged (+0.014%). Do not present twelve startup samples per side, claim a
5% full-startup improvement or narrow the overall objective to this getter.
Retain the small connection-owned identity cache if final two-session real-GUI
checks pass. No stale authorization decisions, PID/rights shortcuts, Kernel
fastpath changes or capability-root restrictions are introduced. Final GUI
log /private/tmp/nanami-opt-cycle44-desktop.log; verification pending.

Final GUI verification passes two complete sessions in one SMP=4 normal boot:
X11/FLWM/wbar readiness, real keyboard input, launcher application checks,
Information windows and Exit/restart cleanup. Both sessions have READY, LINKS,
INPUT, LAUNCH, INFO and SESSION pass markers, followed by
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. The process exits zero,
with no checked fatal/spawn/shared-memory failure. Serial/screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-bfbx89gx.
Visually inspect the first input and second launched-window screenshots; they
show an actual Tiny Core desktop, launcher and responsive terminal, not just
a framebuffer test surface. Preserve their scope; do not infer testing every
installed GUI app from these screenshots.

Retain the small read-only identity snapshot and its six production-source
cache/adapter tests, without changing POSIX/VFS permission decisions or caching
their results. Normal cache state is per Alter Runtime, not per Linux process
and not shared globally. The message documentation records connection lifetime,
same-slot reset and future-mutator obligations. No diagnostic instrumentation,
new opcode, Kernel/Alpha/IPC-fastpath change, capability restriction, UID privilege
drop, version bump, commit/tag or release is performed. Optional guest-root
isolation still awaits the earlier user decision and is not presumed approved.

Archive the exact candidate runtime/test/message-document delta at
/private/tmp/nanami-opt-cycle44-identity-candidate.patch, plus raw Linux/FreeBSD
executables and pre-cycle sources in /private/tmp/nanami-opt-cycle44.SeMQJx.
Current normal image is cycle44-identity.img, baseline cycle43-restored.img;
both are preserved with the same frozen helper/fixture inputs. Final native
1,138/16 ignored, ASan six, AArch64 and normal x86_64 build/format/diff results
above cover the retained code. The 74.86%/37.55% reductions are specific syscall
workloads; startup remains +0.014%, so the broader system-wide 5% target remains
unproven and the full optimization objective stays active.

## Cycle 45: exact short-product HPET conversion (evaluation)

Keep the Kernel unchanged. Start from accepted cycle44 identity caching:
Alpha d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af,
Alter/Linux 4f1283cebe5c6641f16703fb6a04f43f098d9ad37d59d2070cceb93107d264b4,
HPET b8d0e6532c7badacbc757a09deaa1cca3307aec0d1bf89e4e857a8c6fba463cf.
The normal before image is /private/tmp/nanami-opt-cycle44-identity.img.
Frozen overlay39, observer d3f2bdb8...2f72, fixtures and Kernel remain unchanged.

Timer-service already samples once per event and retains unchanged comparators.
Linux clock/poll paths require fresh observations; do not cache current time,
assume cross-core TSC calibration, remove elapsed-time updates after readiness
queries, alter HPET MMIO consistency reads or change 32-bit wrap maintenance.
Baseline x86_64 Release disassembly has four calls to __udivti3, including clock
counter scaling even when its product fits in 64 bits. Test a checked u64 product
for exact counter-to-nanosecond floor and deadline-to-counter ceiling. Overflow
falls back to the original u128 calculation and saturation. No reciprocal
approximation, epoch reset, frequency assumption, additional metadata or Kernel
API is introduced. The short-product range is finite (about 5.1 hours for the
nanosecond product boundary); later timestamps retain original behavior/cost.

Save pre-cycle sources and raw timer executable in
/private/tmp/nanami-opt-cycle45.qNzF2B. Add production-source conversion tests
against an independent u128 oracle at product overflow/saturation boundaries,
valid/random periods and small/large timestamps. Focused HPET tests: ten passed;
full native tests: 1,141 passed, zero failed, 16 ignored. Performance acceptance
is pending normal-device SMP comparisons; a host arithmetic microbenchmark is
not evidence of faster Tiny Core startup or process creation.

After lsof confirms no open handle, remove only the owned, superseded
cycle43-restored.img (321MiB logical). Current accepted cycle44 image, frozen
inputs, logs, pre-cycle sources and executable archives remain available; the
removed comparison image can be rebuilt. No unrelated file is deleted.

ASan HPET tests pass all ten cases; normal x86_64 image build passes. The
candidate timer hash is fa47dfa0660ee7ab5e30b359a4596b85592d455117598df8b83c40ed37ef778a.
Release disassembly confirms the short product branches on multiply overflow
and performs exact constant division using a multiply/shift, with __udivti3
remaining on fallback paths. This proves code generation, not a desktop gain.
Alpha/Linux/FreeBSD/Kernel hashes remain exactly the baseline values above.

Eight sequential normal-device SMP=4 boots run process and startup workloads
in before/after/after/before order, with no concurrent owned build/test/VM.
Host load before measurement is 7.59 / 7.15 / 7.57 on ten CPUs. All exit zero;
each process serial log has cache-freshness and process pass markers, and each
startup log has three samples with cleanup=true. No checked fatal fault, spawn
failure, shared-memory rollback or temporary profile output occurs. Logs:
/private/tmp/nanami-opt-cycle45-{process,startup}-
{before1,after1,after2,before2}.log; original serial directories are recorded in
each log. Each row below has six observations per side unless noted otherwise.

| Workload (ms) | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| getpid, 2048 calls | 38.537 | 38.295 | -0.63% |
| getuid, 2048 calls | 38.657 | 38.887 | +0.59% |
| getresuid-based normal copyout | 171.2895 | 171.2045 | -0.05% |
| Copyout, 128 VMAs | 173.9105 | 172.5775 | -0.77% |
| Copyout, 256 VMAs | 173.049 | 174.2675 | +0.70% |
| fork, eight children | 19.0725 | 18.5245 | -2.87% |
| fork/exec, eight children | 683.382 | 676.7865 | -0.97% |
| fork, 128 VMAs / eight children | 19.733 | 18.4115 | -6.70% |
| fork, 256 VMAs / eight children | 19.190 | 18.027 | -6.06% |
| Native Alpha construction/reap | 26.755 | 25.400 | -5.06% |
| Full native BusyBox lifecycle | 27.5645 | 27.5045 | -0.22% |
| Full native Xfe lifecycle | 28.336 | 27.931 | -1.43% |
| Startup, all sessions | 5092.463 | 5143.494 | +1.00% |
| Startup, first (two per side) | 4706.6125 | 4704.0805 | -0.05% |
| Startup, later (four per side) | 5145.0095 | 5197.1425 | +1.01% |

Startup raw triplets: before1 4700.087 / 5095.084 / 5293.140;
after1 4707.420 / 5090.055 / 5197.352;
after2 4700.741 / 5200.851 / 5196.933;
before2 4713.138 / 5089.842 / 5194.935. The 100ms observation interval and host
variation do not support a full-startup improvement. Some constructor/fragmented
fork controls move by more than 5%, but no copy/fork implementation changed and
these movements alone are not causal evidence for the timer conversion.

Reject the production candidate: the useful desktop/fork-exec workloads do not
show the targeted gain, while the new conditional path would add runtime code.
Restore hpet_clock.rs byte-for-byte to its pre-cycle source. Keep the three
general conversion/clock-state regressions, not an implementation-specific
fast-path test. Archive the exact experiment at
/private/tmp/nanami-opt-cycle45-clock-candidate.patch and both raw executables in
/private/tmp/nanami-opt-cycle45.qNzF2B. Kernel, ABI, IPC fastpath, capability-root
policy and version remain unchanged. Rebuild and recheck the restored runtime;
final restoration verification follows below. The full optimization goal
remains active; no global 5% gain is claimed for this rejected cycle.

After restoration, all 1,141 native tests pass again (16 ignored), all ten HPET
ASan tests pass, formatting and git diff checks pass. Normal x86_64 rebuilding
produces the exact baseline HPET executable b8d0e653...63cf, Alpha d6af9865...438af,
Linux 4f1283ce...4b4, FreeBSD f057e1a5...3da and Kernel 312d579b...d0c88. No Kernel
source difference is present. Remove only the closed, rejected
cycle45-clock.img after lsof reports no open handle; retain its raw executable,
candidate patch and measurement logs. Current normal rebuilt image is
/private/tmp/nanami-opt-cycle45-restored.img; run the two-session real desktop
workflow on it before final handoff. Log:
/private/tmp/nanami-opt-cycle45-restored-desktop.log.

Final desktop workflow exits zero and passes both complete sessions in one
normal SMP=4 boot, including X11/FLWM/wbar readiness, actual keyboard input,
launcher checks, Information windows and Exit/restart cleanup. Both sessions
have READY/LINKS/INPUT/LAUNCH/INFO/SESSION markers followed by
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. No checked fatal/spawn/
shared-memory failure occurs. Serial/screenshots:
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-n_g123rh.
Visually inspect the first launched Terminal/launcher and second input screen;
these are actual desktop surfaces, not a framebuffer test pattern. Preserve
the test's application/window scope rather than asserting every GUI application
has been exhaustively tested. Retain only conversion regressions and this
measurement record for cycle45; no production change survives the experiment.
The accepted baseline is still cycle44's runtime, now also available in the
verified cycle45-restored.img. No commit, tag, release or version bump is made.

## Cycle 46: avoid an image-rendering client for a solid background

Status: retain the small desktop profile/tool selection change after both
measurement orders, ordinary package assembly and two real GUI sessions pass.
The first-session startup median improves 5.18%; all-session startup improves
3.22%, so this is not a 5% all-system claim. The VMA-search diagnostic below
does not become a production algorithm change.

The preceding goal turn makes progress: evaluate an HPET arithmetic candidate
with eight matched normal boots, reject it after no startup gain, restore exact
accepted runtime hashes and verify two real GUI sessions. Keep Kernel changes
forbidden and the broader optimization goal active. Start from the verified
/private/tmp/nanami-opt-cycle45-restored.img and the same accepted cycle44
Alpha/Linux/FreeBSD/timer binaries. No earlier rejected candidate is reinstated.

Source review finds that private file mmap reserves a PROT_NONE address range
by restarting a full unsorted VMA scan after each conflict. In the worst case
this is quadratic, but that does not prove it is costly in the real desktop.
Before introducing a sorted index, extra per-process state or a changed address
placement policy, add temporary opt-in counters for successful gap searches:
VMA count, maximum count, scan passes, visited entries and elapsed local ticks.
They flush at existing exit boundaries and include other guests since the last
flush. Reported elapsed time includes descheduling and is not exclusive CPU or
IPC cost. This diagnostic does not alter allocation decisions or omit checks.

Save original sources/executable in /private/tmp/nanami-opt-cycle46.rK5T4W.
Use NANAMI_ALTER_PROFILE=1 only for the diagnostic image, with frozen overlay39,
observer/fixtures and normal SMP=4 devices. Remove the instrumentation before
any normal before/after timing comparison. Actual gap-search evidence and a
decision whether to change the algorithm follow below.

After lsof reports no open handle, remove only the superseded owned
cycle44-identity.img (321MiB logical) to make room for the diagnostic image.
The verified cycle45-restored.img contains exactly the same accepted runtime;
keep all frozen inputs, raw executables, patches and measurements. The removed
duplicate is rebuildable; no unrelated file is deleted.

The first diagnostic build catches a temporary import using crate::syscall_profile
instead of the shared crate::common::syscall_profile. Correct it and rebuild.
Profile Linux hash 913677bdfc221482c2832cbf039143bafe2668fac5511778825d3f2a1ee51939;
Alpha/Kernel remain exact. All three diagnostic startup/reuse sessions pass
cleanup=true, without checked fatal/spawn/shared-memory errors. Log:
/private/tmp/nanami-opt-cycle46-gap-profile.log; serial directory
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-c6akneas.
The logger pads tag names: match alter.gap] rather than an unpadded [alter.gap].
An initial exact-bracket search misses existing records; inspect the serial
content and correct the extraction instead of rerunning the VM.

Collected global exit-flush coverage: 432 successful gap searches, summed VMA
counts 22,804, maximum 204, 9,740 full-scan passes, 979,250 visited entries and
7,454,000 elapsed local-counter ticks. Corresponding emitted mmap totals are
1,900 calls / 4,357,142,000 ticks; gap spans are about 0.171% of that interval.
This includes diagnostic overhead/descheduling and is not exclusive CPU time
or complete coverage of every live-tail search. The final unflushed tail may be
absent. Do not extrapolate counts into all system work. The quadratic behavior
exists but does not justify a new index/cursor, changed placement policy or a
5% startup claim for this workload. Do not implement that candidate now.

Archive temporary source deltas at
/private/tmp/nanami-opt-cycle46-gap-diagnostics.patch and the raw diagnostic ELF
in the snapshot directory. Restore both diagnostic source files byte-for-byte
before normal building. After its terminal exit and an empty lsof result, remove
only the owned diagnostic image (321MiB logical); logs/patch/binary stay intact.
Re-inspect current memcpy disassembly: compiler-builtins still uses aligned
REP MOVSQ with byte prefix/tail handling, so there is no evidence for repeating
cycle16's rejected private integer-copy override.

### Candidate: use the upstream solid-root setter, not an image renderer

The desktop profile needs one solid background color, yet hsetroot loads
Imlib2, FreeType, zlib and further dependencies for that job. The already pinned
Xorg-7.7-bin.tcz contains an unchanged xsetroot intended for root-window setup.
Verify the archive SHA-256 remains
6eeb48b1bec47a1f18a81a9f8873ca3ac3d95fe15a94bbec052914471ad935e7.
Extract only xsetroot: 15,096 bytes, SHA-256
435804144aebbf586fa72b2c47d87e55e53919fb93edadc5f9a87225ab86a043.
Its direct dependencies are libXmuu, libX11, libXcursor and libc, all in the
existing pinned package graph. Select the additional binary and record the
explicit dependencies without changing package bytes/hashes or downloading
new versions. Keep hsetroot and its libraries installed for other applications.

Change only the profile's hard-coded solid-color command to xsetroot -solid
'#3d6f8e'. No dedicated replacement program, Kernel change, guest authority
restriction or main emulator optimization is added. Preserve readiness,
window-manager/launcher commands, cleanup and environment. Verify background,
dock appearance, actual apps/input and restart before acceptance; successful
readiness alone does not prove the color command succeeded.

Builder tests pass 32 cases including selected upstream-byte preservation,
lock/selection agreement and the solid-color command. Derive a separate cloned
overlay under /private/tmp/nanami-opt-cycle46.rK5T4W from frozen overlay39, adding
only the unchanged xsetroot binary and changing that one script line. Do not
modify the frozen before overlay. Normal candidate image:
/private/tmp/nanami-opt-cycle46-xsetroot.img; before cycle45-restored.img. Normal
comparison and final visual/regression acceptance are pending below.

Normal candidate building passes. Restored full native checking passes 1,141
tests (16 ignored); builder tests pass 32. The candidate has exactly unchanged
Alpha d6af9865...438af, Linux 4f1283ce...4b4, FreeBSD f057e1a5...3da, timer
b8d0e653...63cf and Kernel 312d579b...d0c88. No diagnostic counters remain. All
build/test handles terminate before normal timing. Host load before comparison
is 6.44 / 8.33 / 8.38 on ten CPUs, with no concurrent owned build/test/VM.

Run four normal-device SMP=4 startup boots in before/after/after/before order.
All exit zero with three-session cleanup=true markers and no checked fatal,
spawn/shared-memory failure or temporary profile output. Raw milliseconds:

| Session | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4816.012 | 4605.699 | 4512.571 | 4708.477 |
| Second | 5214.191 | 5086.598 | 5106.817 | 5184.936 |
| Third | 5295.336 | 4991.002 | 5096.795 | 5297.531 |

Pooled median is 5199.5635 -> 5038.800ms (-3.09%). This is preliminary, not a
5% claim or acceptance based on the favorable first pair. Logs:
/private/tmp/nanami-opt-cycle46-startup-
{before1,after1,after2,before2}.log. Each log records its original serial path.
Run two actual desktop sessions in one normal candidate boot before reverse-
order timing. Log /private/tmp/nanami-opt-cycle46-xsetroot-desktop.log; serial
directory suffix rz33c8ja. Visually inspect the first ready screenshot: the
expected blue background, rounded dock and actual Terminal are present. This
does not replace input/launcher/Information/restart checks, still running.

The full desktop workflow subsequently passes both sessions and exits zero:
READY/LINKS/INPUT/LAUNCH/INFO/SESSION for each, then
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. No checked fatal,
spawn/shared-memory failure occurs. The first ready screenshot shows the
expected blue root background, dock and actual Terminal. Preserve these
specific visible checks, not an exhaustive claim about all installed apps.

Also run the ordinary pinned-package assembly, without --image, offline:
CC=/opt/homebrew/opt/llvm/bin/clang python3 -B thirdparty/tinycore/build.py
--desktop --offline --output-dir /private/tmp/nanami-opt-cycle46.rK5T4W/assembled.
It exits zero. Its xsetroot matches the extracted/candidate binary exactly
(43580414...a043), and nanami-desktop matches the compared script
c71028258c3158f5ef8e3d6b6052906867934b674f5db0ae686e741993a33651.
The ordinary rebuilt X-readiness helper is also exact d7699dbb...e3, so this is
not solely a manually patched test overlay. No upstream package hash changes.

After GUI and assembly handles terminate, run four more normal startup boots
in after/before/before/after order. No owned build/test/VM is concurrent. Host
load is 10.96 / 9.22 / 8.69 before the extension (a trailing host average, not
proof of current competing work). All four exit zero, with three-session
cleanup=true markers, no checked fatal/spawn/shared-memory failure and no
temporary profile output. Extra logs use after3/before3/before4/after4.

| Session (ms) | After 3 | Before 3 | Before 4 | After 4 |
| --- | ---: | ---: | ---: | ---: |
| First | 4511.522 | 4811.418 | 4805.356 | 4608.694 |
| Second | 5082.261 | 5296.640 | 5194.409 | 5104.586 |
| Third | 4898.767 | 5296.452 | 5299.409 | 5104.087 |

Both orders combined, four boots / twelve startup observations per side:

| Startup scope | Before (ms) | Candidate (ms) | Change |
| --- | ---: | ---: | ---: |
| First session, four per side | 4808.387 | 4559.135 | -5.18% |
| Later sessions, eight per side | 5295.894 | 5091.6965 | -3.86% |
| All sessions, twelve per side | 5204.300 | 5036.6315 | -3.22% |

First-session boot values are 4708.477..4816.012 before and
4511.522..4608.694 after, improving in both orders. Observation resolution is
100ms, and sessions within one boot are correlated. Report the measured scope;
do not present a 5% all-session/system-wide gain or independent CPU/IPC speedup.
No process-only performance fixture is rerun for this profile command because
that fixture never executes the changed desktop script, and all emulator/Alpha
binaries are byte-identical. Do not attribute this to a changed memcpy or IPC
fastpath.

Retain the small profile/tool selection change: it removes unnecessary work
without a new helper, new caching policy, altered guest memory authority or
Kernel modification. Keep hsetroot and its libraries for other applications.
Record the exact candidate source/test/document delta in
/private/tmp/nanami-opt-cycle46-xsetroot-candidate.patch. General source/build
checks remain 1,141 native passed (16 ignored), 32 builder tests and successful
normal x86_64 build plus real desktop checks. No version bump, commit, tag or
release is performed. The broader optimization objective remains active.

Finally inspect the second Information-window screenshot as well: the actual
information contents, blue desktop and dock are present after restart. Repeat
the 32 builder tests successfully; git diff/format checks pass and both temporary
diagnostic source files still match their pre-cycle snapshots byte-for-byte.
Preserve cycle46-xsetroot.img as the current accepted tested normal image and
the separate candidate overlay/ordinary assembled tree. After lsof reports no
open handle, remove only the owned cycle45-restored.img comparison duplicate
(321MiB logical) to leave build headroom. Its exact frozen inputs, runtime
hashes, diagnostic archives and all timing/GUI logs remain available; the image
is rebuildable. Default out/ images are not silently updated by this private
comparison build. No unrelated file is deleted.

## Cycle 47: coalesce adjacent UNIX recvmsg result fields

Status: retain the small result-copy consolidation after complete corrected-
runner measurements and two real GUI sessions pass. Target socket loops improve
13.17% / 19.24%; whole startup remains effectively unchanged. Kernel, service
protocols, capability rights/root policy and version remain unchanged.

The preceding turn makes progress by retaining a measured, visually validated
desktop-profile improvement. Preserve that accepted xsetroot profile and all
earlier runtime work. Keep Kernel changes forbidden; no guest-root restriction
is presumed approved. Review actual UNIX socket marshaling rather than changing
Kernel IPC, increasing caches or reinstating a rejected copy primitive.

UNIX recvmsg currently writes msg_namelen, msg_controllen and msg_flags with
three checked Alpha copies. The final two fields occupy twelve adjacent bytes
in the supported LP64 ABI. Test a shared success/EOF result writer that copies
only those twelve output bytes together, never input pointers or ABI padding.
Keep msg_namelen first. On combined-copy failure, retry an entire eight-byte
control length followed by four-byte flags, preserving the original field and
failure order even when the control word straddles a protected page. Existing
payload/ancillary transaction, descriptor rollback, PEEK, queue consumption,
readiness and permissions remain unchanged. No new RPC, persistent state,
allocation, privilege or Kernel/ABI change is introduced.

Snapshot the two source/test files and the benchmark source in
/private/tmp/nanami-opt-cycle47.3Fn7Br; save raw before Linux 4f1283ce...4b4.
Before changing the production handler, extend the integer-only desktop socket
fixture with actual send/recvmsg and EOF loops (2048 calls, three samples each),
checking every output value, unchanged input pointers and padding. Untimed
protected-page cases cover an unaligned header with its u64 control length
straddling a read-only page, and a separately inaccessible flags word. Verify
original partial metadata, payload and packet retry behavior before measuring.
Use byte copies in the fixture for unaligned headers, not misaligned C struct
access. Keep the old stat/missing-stat/plain read-write controls intact.

Derive a separate overlay from accepted cycle46 inputs, adding only the new
fixture binary and its console bundle. Fixture SHA-256:
6fde133713a9a67f3433919e48c610eb2ff08fd5ecbfb166eb374829f96bff95.
Build /private/tmp/nanami-opt-cycle47-before.img with byte-identical accepted
runtime, unchanged observer/helper, xsetroot profile and Kernel. That image
passes the expanded socket fixture, including both protected-page cases and
the old controls. Initial diagnostic validation medians are approximately
961.644ms for send/recvmsg and 637.505ms for EOF; these are not paired before/
after results. Log /private/tmp/nanami-opt-cycle47-unix-before-validation.log.
Normal startup/reuse validation is running separately before freeing the old
comparison image; full performance acceptance remains pending.

Production-source native UNIX tests add exact fast-case copy counts and zero
allocation, unchanged pointers/padding, faults at each metadata field/interior
byte, whole-u64 store boundaries, unchanged queued bytes after EFAULT and
successful retry. Improve the fake guest-memory writer to reject an inaccessible
byte anywhere in the requested range, not only at its first address; otherwise
the combined write could hide a bad flags pointer in the existing SCM_RIGHTS
rollback test. Preserve the existing queued-FD rollback coverage.

The production-source UNIX target passes all 22 tests, including the strengthened
inaccessible-byte model and existing SCM_RIGHTS rollback. Full native checking
passes 1,143 tests (16 ignored), UNIX ASan passes 22, and AArch64 Alter/Linux
Release checking passes. No FPU/SIMD or Kernel work is added. Normal x86_64
candidate build is next; keep both images' fixture/observer/profile inputs exact.

The unchanged-runtime before image also passes three real startup/reuse sessions:
4517.315 / 4993.705 / 4991.318ms, with cleanup=true and no checked fatal/spawn/
shared-memory failure. These are preliminary validation observations, not a
paired performance comparison. Log
/private/tmp/nanami-opt-cycle47-startup-before-validation.log; serial suffix
ll6az9j2. After lsof confirms no open handle, remove only the superseded owned
cycle46-xsetroot.img (321MiB logical). The verified cycle47-before.img contains
the same accepted runtime/profile plus the new test fixture. Keep old frozen
inputs, archived source deltas and timing/GUI logs; the removed image remains
rebuildable. No unrelated file is deleted.

Normal candidate build passes with Linux SHA-256
4a40dc21620c60cb9978415df5b506b1eb27d946553c5ddf75ef1df07db72df6;
FreeBSD 64665ade0ffd21b7ba50542f1563497373d8c3c55a93e51fefce5b1a635990d6.
The latter shares the Linux personality sources. Alpha d6af9865...438af,
timer b8d0e653...63cf, Kernel 312d579b...d0c88 and frozen observer/helper/fixture
remain exact. No profiler is enabled. All owned build/test/VM handles terminate
before normal timing; host load before the initial comparison is
9.56 / 8.92 / 8.63 on ten CPUs.

Run process-independent socket and real-startup workloads sequentially in
before/after/after/before order (eight normal SMP=4 boots), without concurrent
owned work. All processes exit zero; every startup has three cleanup=true
sessions, without checked fatal/spawn/shared-memory errors. One socket report,
after1, is incomplete despite the host exit code: stdout stops mid-stat value,
and serial ends during the following result rows. Do not include any of its
partial numbers in comparisons or infer missing measurements.

Inspect the runner: --tinycore-perf stops at TINYCORE-PERFORMANCE-PASS, which
the observer prints BEFORE its captured timing text. It can terminate QEMU
before UART emission of the report finishes. Fix that branch to wait for the
final DESKTOP-PERFORMANCE-PASS after the prefix, matching the process-runner's
existing completion policy. This changes capture after the timed work, not
the workload or either guest image. Add three host tests of the actual AST-
extracted predicate without importing/booting the top-level runner: prefix/
partial output is incomplete, a footer before the prefix is insufficient,
and complete LF/CRLF reports pass. All three tests pass. Save the old runner
in the snapshot directory; preserve the truncated log/serial unchanged.

Initial complete reports before1/after2/before2 are supplementary only. Their
unequal observations give send/recvmsg 957.3035 -> 845.239ms (-11.71%) and EOF
648.8615 -> 530.760ms (-18.20%). Use a new balanced primary comparison after
fixing the runner, in after/before/before/after order, with after3/before3/
before4/after4. All four reports exit zero and contain the final marker and
exactly three observations of each of the five fixed workloads. No checked
fatal/spawn/shared-memory error or temporary profiler output occurs.

Primary corrected-runner comparison, milliseconds, six observations per side:

| Workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Successful stat, 512 calls | 144.8045 | 140.7905 | -2.77% |
| Missing stat, 512 calls | 109.068 | 108.208 | -0.79% |
| Plain UNIX write/read, 2048 exchanges | 373.9295 | 371.237 | -0.72% |
| UNIX write/recvmsg, 2048 exchanges | 972.0285 | 844.0535 | -13.17% |
| UNIX recvmsg EOF, 2048 calls | 647.4445 | 522.881 | -19.24% |

Per-boot write/recvmsg medians before3/before4 are 959.163 / 972.369ms, after3/
after4 824.223 / 844.177ms. EOF before 648.571 / 646.922ms, after 522.255 /
524.738ms. All primary after samples are lower than all corresponding before
samples for those two target loops; unchanged controls move much less. This
is removal of repeated copy/validation/Frame-lookup requests, not proof that
Kernel IPC transport itself was inefficient or changed.

All seven complete socket reports, INCLUDING the primary four, additionally
give twelve before and nine after observations: write/recvmsg 964.9095 ->
844.177ms (-12.51%), EOF 647.7275 -> 524.738ms (-18.99%). Do not present these
as a second independent balanced experiment or count primary samples twice.
Keep after1 excluded because of its missing footer/data, not its outcome.
Logs /private/tmp/nanami-opt-cycle47-unix-
{before1,after1,after2,before2,after3,before3,before4,after4}.log.

Startup remains the four original complete boots, three sessions each:

| Session (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4510.532 | 4492.654 | 4500.086 | 4512.745 |
| Second | 4995.406 | 4989.410 | 5097.681 | 5095.928 |
| Third | 5000.303 | 5086.728 | 5093.142 | 5092.139 |

Pooled startup 4997.8545 -> 5038.069ms (+0.80%). The 40.2ms difference is below
the 100ms readiness polling interval; no established whole-startup gain or
regression is claimed. Both sides already include cycle46's xsetroot profile;
do not attribute that earlier gain to this recvmsg change. No extra startup
observations are added by the corrected-runner socket-only extension. Logs:
/private/tmp/nanami-opt-cycle47-startup-{before1,after1,after2,before2}.log.

Run two full normal SMP=4 GUI sessions before final acceptance, including real
input/launcher/Information and Exit/restart cleanup. Log
/private/tmp/nanami-opt-cycle47-desktop.log; serial directory suffix j_f0euib.
The workflow is currently running; a ready surface alone is not final success.

Final GUI verification exits zero with both complete sessions: READY/LINKS/
INPUT/LAUNCH/INFO/SESSION each, followed by
TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. No checked fatal,
spawn/shared-memory failure occurs. Inspect both Information screenshots:
actual OS information, responsive terminal content, the blue background and
dock are present before and after restart. This preserves the named workflow,
not a claim that all installed GUI applications have exhaustive coverage.

Retain the small output helper, stronger native/fault guest checks, expanded
fixed-work fixture and the report-completion fix with its three host tests.
Final source checks remain 1,143 native passed / 16 ignored, UNIX ASan 22,
AArch64 Alter/Linux check, normal x86_64 build and clean formatting/diff checks.
No temporary profiling is present. The exact runtime/test/runner experiment is
archived at /private/tmp/nanami-opt-cycle47-recvmsg-candidate.patch; a non-mutating
reverse-apply check succeeds against the retained files. Save raw before and
candidate Linux plus candidate FreeBSD in /private/tmp/nanami-opt-cycle47.3Fn7Br.
Current normal image is /private/tmp/nanami-opt-cycle47-recvmsg.img, with frozen
inputs /private/tmp/nanami-opt-cycle47.3Fn7Br/overlay.

After lsof confirms no open handle, remove only the owned cycle47-before.img
comparison image (321MiB logical) to preserve build headroom. Keep all snapshots,
raw executables, exact fixture, source archive and timing/GUI logs, including
the truncated report; the removed image is reproducible. No unrelated file is
deleted, and default out/ images are not silently updated. No version bump,
commit, tag or release is performed. The broader optimization goal remains
active; this is not a 5% system-wide startup improvement.

## Cycle 48: decode UNIX I/O descriptions in place

The user-level-only policy remains in force. Inspect the accepted cycle47
normal x86_64 Alter/Linux, not a diagnostic build: UNIX message::io initializes
its bounded description and then copies 300 bytes twice while transporting
the single-caller decoder's Result<Message>. Each description contains two
16-word iovec arrays (256 bytes together), not 64-element arrays. Scalar I/O
also traverses these copies. This is generated-code evidence, not a timing
attribution to IPC or an assumption that structure size dominates X11 startup.

Fold the private decoder into its sole io caller, retaining one ordinary,
fully initialized local Message. Do not add output-parameter APIs, callbacks,
new message variants, cached state, MaybeUninit or unsafe initialization.
Header snapshot, vector count/null checks, address/flag decoding, send/receive
selection and queue/rights commit behavior retain their order. The previous
recvmsg result-tail helper is unchanged. Production source is slightly shorter.

Normal x86_64 disassembly confirms both 0x12c-byte memcpy calls are gone from
description decoding. The io stack reservation changes 0x418 -> 0x398 (1048 ->
920 bytes, excluding saved registers); a separate 176-byte address/packet copy
elsewhere in io remains. Inlining decisions shift functions, so do not compare
only io symbol size. Total .text changes 0x7cff2 -> 0x7cf22 (208 bytes smaller).
The compiler's existing vector instructions are not newly introduced FPU/SIMD
support and are not described as absent. No compiler-specific inline hints
are added.

Add general production-handler native tests for zero, maximum 16 and rejected
17-element readv/writev/sendmsg/recvmsg descriptors. A late copyin failure in
writev/sendmsg must publish no partial packet, then succeed on retry. Existing
partial copyout, padding, EOF, SCM_RIGHTS and error-order checks remain intact.
Full native release tests: 1,145 passed, 16 ignored; focused UNIX and its ASan
variant: 24 passed each. AArch64 Alter/Linux compile check and normal x86_64
image generation pass. Logs /private/tmp/nanami-opt-cycle48-
{native,unix-native,unix-asan,arm,build}.log. No build/test runs concurrently
with timed QEMU.

Baseline image /private/tmp/nanami-opt-cycle47-recvmsg.img uses accepted Linux
4a40dc21620c60cb9978415df5b506b1eb27d946553c5ddf75ef1df07db72df6.
Candidate /private/tmp/nanami-opt-cycle48-message.img uses Linux
9ef6b62c3ae1b43ba3c296d7e8884f416bda5d58f74e704f0f377204d1739267,
FreeBSD cda8edd4fd68017650e9618d27194f95ba4dfc33a873e3a69b7e3e986ecc7e8e.
Alpha remains d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af,
Kernel 312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88,
observer d3f2bdb84b137c0db762b67ad6489af9d204f699dc2197aba291c44bda152f72.
Use the identical frozen /private/tmp/nanami-opt-cycle47.3Fn7Br/overlay in both
images. The cycle47 expanded desktop-performance ELF, stock Xsetroot, wait
helper and desktop script are unchanged. No profiler is enabled.

Run sequential normal-device SMP=4 QEMU, before1/after1/after2/before2. For each
trial run socket performance and real X11-ready startup in separate boots.
All eight runs exit zero; every socket report has its full final footer and
exactly three observations for each of five workloads, all startup reports
three actual create/start/kill/delete/reused-ID sessions. No incomplete report
is accepted or omitted by outcome. Generic observer init/interactive words in
the performance footer are not independent console/GUI test evidence.

| Workload (ms) | Before median (six samples) | After median (six samples) | Change |
| --- | ---: | ---: | ---: |
| Existing stat, 512 calls | 143.735 | 140.9915 | -1.91% |
| Missing stat, 512 calls | 107.896 | 108.9945 | +1.02% |
| Plain UNIX write/read, 2048 pairs | 376.375 | 370.3735 | -1.59% |
| UNIX write/recvmsg, 2048 pairs | 840.7195 | 844.9865 | +0.51% |
| UNIX recvmsg EOF, 2048 calls | 522.8855 | 523.5575 | +0.13% |

Raw socket samples in milliseconds, by boot:

| Boot | stat | missing stat | UNIX plain | UNIX recvmsg | UNIX EOF |
| --- | --- | --- | --- | --- | --- |
| before1 | 146.530,142.057,139.920 | 100.351,107.529,108.974 | 376.644,376.935,376.106 | 845.638,850.816,838.072 | 525.137,516.906,513.453 |
| after1 | 143.539,141.212,140.771 | 108.909,111.653,109.080 | 372.768,369.288,372.429 | 833.067,937.492,841.285 | 524.896,521.827,507.272 |
| after2 | 154.380,140.694,139.505 | 109.380,107.528,106.232 | 371.459,358.598,368.483 | 848.688,857.071,840.232 | 522.812,531.443,524.303 |
| before2 | 144.664,143.560,143.910 | 108.263,107.401,109.509 | 377.833,374.301,372.045 | 843.367,827.925,805.623 | 523.080,528.288,522.691 |

Startup, separately:

| Session (ms) | Before 1 | After 1 | After 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4499.880 | 4500.539 | 4595.104 | 4500.426 |
| Second | 5096.328 | 4982.650 | 4986.413 | 5091.993 |
| Third | 4988.498 | 5096.745 | 4988.746 | 5090.259 |

Pooled startup 5039.3785 -> 4984.5315ms (-1.09%); the 54.8ms difference is
below the 100ms readiness observer interval. First and later startup sessions
are not interchangeable independent observations. Plain socket medians improve
slightly in both orders, while recvmsg and EOF are effectively unchanged and
controls move too. This is not an established 5% target/workload/system gain.
Keep the no-new-state, smaller-copy cleanup, but do not claim cycle47's already
accepted gain again or call generated-copy deletion a measured IPC speedup.
Logs /private/tmp/nanami-opt-cycle48-{unix,startup}-
{before1,after1,after2,before2}.log preserve all samples including outliers.

The final two-session normal GUI check is separate from timing; require real
input, launcher, Information, Exit/restart and final cleanup before acceptance.
Its log is /private/tmp/nanami-opt-cycle48-desktop.log.

Final GUI run exits zero: both sessions pass READY/LINKS/INPUT/LAUNCH/INFO/
SESSION and TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true. Serial and
screenshots are in the nanami-negoto-qemu-m6em02p2 temporary log directory.
Inspect both Information screenshots: actual OS information and a live shell,
blue background and dock remain present before and after restart. No checked
fatal/spawn/shared-memory failure is reported. This is the named GUI workflow,
not exhaustive coverage of every installed application.

Retain the source cleanup and two native regressions. Snapshot raw before and
candidate Linux/FreeBSD executables plus the two exact before source files in
/private/tmp/nanami-opt-cycle48.BKzScE. The exact source/test delta is archived
at /private/tmp/nanami-opt-cycle48-message-candidate.patch; reverse-apply checking
and formatting/diff checks pass without modifying the accepted worktree. The
normal before/candidate images and frozen cycle47 overlay remain available.

To preserve build headroom, lsof confirms no open handle before removing only
the owned, superseded /private/tmp/nanami-opt-cycle38-owned.img. Keep its older
logs and current before/candidate images, raw executables and frozen inputs.
No unrelated artifact or source is removed. No version bump, commit, release,
Kernel change or whole-goal completion is implied.

## Cycle 49: classify UNIX requests before descriptor lookup (rejected)

The preceding turn makes progress by removing two generated description copies,
verifying normal-image behavior, and documenting the small/neutral timings.
Keep Kernel unchanged. Inspect the actual accepted cycle48 source and generated
x86_64 code: Linux dispatch calls unix::dispatch first for every syscall, and
unix::attempt resolves/copies arg0's LinuxFile before classifying the number.
Getpid/getuid with arg0 zero therefore consult the terminal FD unnecessarily.
This is Alter-local work, not evidence that A9N IPC transport is inefficient.

Candidate: a private, request-local eight-variant Operation decoding the eighteen
existing UNIX FD operations before linux_file. Keep number-to-operation mapping
in one place, with an exhaustive operation handler match; no duplicated opcode
whitelist, runtime state, permission/FD cache, wire change or Kernel change.
Socket/socketpair creation stays separate, EAGAIN parking and SIGPIPE/retry
semantics remain intact. Candidate native lookup counters prove zero descriptor
lookups for unrelated syscalls and one live lookup for supported operations on
foreign/invalid descriptors. Reuse must observe the current descriptor kind.

Normal x86_64 disassembly confirms the syscall-number branch/jump precedes
linux_file; baseline calls linux_file before its number dispatch. Candidate
total .text shrinks 0x7cf22 -> 0x7ce92 (144 bytes), but retry's stack reservation
increases 0x208 -> 0x2a8 (520 -> 680 bytes, excluding saved registers). Do not
claim that reordering source guarantees an unchanged or faster socket path.

Candidate Linux 629842d722ba50a156d79cde71494e45a8d4c2e2a478667e2b59cba52fcdccf6,
FreeBSD 661fc52d3ead2eaed2a59779891452752b82271a5b6bf3e1be3c536bff79a2f3.
Before uses accepted cycle48 Linux
9ef6b62c3ae1b43ba3c296d7e8884f416bda5d58f74e704f0f377204d1739267,
FreeBSD cda8edd4fd68017650e9618d27194f95ba4dfc33a873e3a69b7e3e986ecc7e8e.
Both use exactly /private/tmp/nanami-opt-cycle47.3Fn7Br/overlay and observer
d3f2bdb84b137c0db762b67ad6489af9d204f699dc2197aba291c44bda152f72.
All fixture ELFs, Xsetroot/profile/helper, Alpha, HPET, Honoka and Kernel are
unchanged. No diagnostics are compiled into normal timed images.

Candidate native release suite 1,147 passed / 16 ignored; UNIX 26 passed both
normally and with ASan; AArch64 Alter/Linux checking and normal x86_64 image
build pass. Run no builds/tests concurrently with timed VMs. Four trials in
before1/after1/after2/before2 order each run process, socket and real startup
in separate normal-device SMP=4 QEMU boots, twelve boots total. All exit zero.
Socket reports each contain exactly three samples for all five workloads and
the final footer. Process stdout intentionally filters only PROCESS-PERF lines:
verify completion in each actual serial.log, not by requiring an unprinted
footer in filtered stdout. All four serial logs contain the final
PROCESS-PERFORMANCE-PASS and observer completion. Linux loop rows each have
three samples per boot. Negoto create reports six, not three, with distinct
first/cold and later work; do not pool it as six equivalent startup samples.
Startup contains three real create/start/kill/delete/reused-ID sessions per boot.

Pooled Linux-loop medians, six observations per side:

| Workload (ms) | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| getpid, 2048 calls | 39.3565 | 39.7310 | +0.95% |
| getuid, 2048 calls | 38.8285 | 39.1370 | +0.79% |
| checked getresuid, normal VMAs | 171.8240 | 172.4570 | +0.37% |
| checked getresuid, 128 VMAs | 172.0695 | 174.1185 | +1.19% |
| checked getresuid, 256 VMAs | 174.4495 | 174.3955 | -0.03% |
| fork/wait, eight children | 18.5975 | 17.8945 | -3.78% |
| fork/exec/wait, eight children | 680.4880 | 679.7900 | -0.10% |
| fork, 128 extra VMAs, eight children | 17.5885 | 18.5165 | +5.28% |
| fork, 256 extra VMAs, eight children | 17.7180 | 18.7165 | +5.64% |
| Existing stat, 512 calls | 143.0705 | 143.9705 | +0.63% |
| Missing stat, 512 calls | 109.8900 | 109.2545 | -0.58% |
| Plain UNIX write/read, 2048 pairs | 371.9770 | 371.3515 | -0.17% |
| UNIX write/recvmsg, 2048 pairs | 839.6495 | 844.6940 | +0.60% |
| UNIX recvmsg EOF, 2048 calls | 529.4745 | 529.6065 | +0.02% |

Native constructor controls (never resume/execute the Linux ELF) also move:
Alpha spawn/reap 25.1415 -> 24.7475ms (-1.57%), whole busybox lifecycle
29.4535 -> 28.2005ms (-4.25%), whole Xfe lifecycle 27.915 -> 28.107ms (+0.69%).
The isolated warm/cold-containing Xfe spawn row changes -6.69% and its reap row
-14.09% without Alter running that child. These are controls, not causal gains
from this decoder or proof that all measured fork increases are reproducible
regressions. Retain all raw samples/outliers rather than selecting these rows.

Separate actual desktop startup:

| Session (ms) | Before 1 | Candidate 1 | Candidate 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4499.134 | 4493.669 | 4506.543 | 4503.568 |
| Second | 4988.480 | 5085.222 | 4984.339 | 4992.101 |
| Third | 4997.442 | 5098.274 | 4993.066 | 4998.594 |

Pooled 4990.2905 -> 4988.7025ms (-0.03%), much smaller than the 100ms observer
interval. First and repeated sessions remain distinct. The intended getter
controls do not improve, socket loops are neutral, fork/exec is neutral, and
fragmented-fork controls are slower. This candidate does not meet the 5% target
or justify more production source and the larger socket stack. Reject it,
rather than retaining an operation-count win as a claimed measured speedup.

All raw logs: /private/tmp/nanami-opt-cycle49-{process,unix,startup}-
{before1,after1,after2,before2}.log. Process serial directories suffixes
ylbea327, xa2xx5ry, vtva8ag5, ksbwyv38 preserve the complete reports. The exact
candidate runtime/test patch is /private/tmp/nanami-opt-cycle49-dispatch-candidate.patch;
its hunk formatting is repaired and git apply --check succeeds against the
untouched minimal replay tree in /private/tmp/nanami-opt-cycle49.I7PaX1/replay.
No accepted source is overwritten by this replay check. Raw before/candidate
executables and original sources remain in the enclosing snapshot directory.

Restore production unix.rs byte-for-byte from the recorded baseline through
apply_patch; no reset/checkout or unrelated edits. Remove the rejected lookup
instrumentation/zero-lookup requirement from tests. Keep two general regressions:
unrelated syscall fallthrough without guest-memory access/signals, and all
eighteen UNIX operations leaving foreign/invalid/reused FDs to their own
handlers. These do not require the rejected private enum or enshrine an
unnecessary lookup as API behavior. Final native 1,147 passed / 16 ignored and
UNIX ASan 26 passed before the final naming-only test refinement; rerun the
focused tests after that refinement. Logs cycle49-final-native and
cycle49-final-unix-asan. Normal restored image build succeeds, and both restored
Linux and FreeBSD compare byte-for-byte with baseline raw executables.

The restored image is /private/tmp/nanami-opt-cycle49-restored.img. Verify its
real two-session GUI input/launcher/Information/Exit/restart workflow separately
in /private/tmp/nanami-opt-cycle49-desktop-restored.log before final handoff.

Final verification passes both complete GUI sessions and the final cleanup
marker with zero exit. Serial/screenshots: nanami-negoto-qemu-uegnio7e temporary
log directory. Inspect the second Information screenshot: actual OS information,
blue desktop and dock are present after restart. Re-run native release and UNIX
ASan after the naming-only refinement: 1,147 passed / 16 ignored and 26 passed,
respectively. Final source cmp, formatting and diff checks pass. Final Alpha
and Kernel hashes remain d6af9865... and 312d579b..., with the exact full values
already recorded in cycle48; Linux/FreeBSD hashes match the full baseline values
above. No candidate runtime remains. Archive retained general tests separately
at /private/tmp/nanami-opt-cycle49-routing-regressions.patch; checking it against
the untouched before test file in the replay tree succeeds.

For build headroom, remove only superseded owned cycle47-recvmsg.img after
confirming no open handle; retain cycle48 baseline and all frozen inputs/logs.
After restoration and another empty lsof check, remove the rejected owned
cycle49-dispatch.img, retaining its raw executables and reproducible patch.
No unrelated image/source is removed. No Kernel/IPC fastpath change, version
bump, release or whole-goal completion is implied.

## Cycle 50: measure socket state searches in real startup

The preceding turn makes progress by rejecting an unhelpful dispatch-order
candidate, restoring exact runtime binaries and retaining routing/FD-reuse
regressions. Keep the user-level-only policy. Read current Sockets::get and
get_mut: both linearly search a Vec<Socket>, whose documented limit is 128.
Do not infer that the configured maximum is the real working set or add a
cache solely because O(n) appears in source.

Temporarily instrument both searches with local before/after stamps, visited
entry counts, maximum live/visited entries and misses. A diagnostic active-
handler flag separates searches inside Linux syscall dispatch from event-side
readiness/cleanup work. Existing syscall spans still include service waits and
descheduling, not exclusive CPU time. Counter updates occur after each lookup's
end stamp; their overhead is still included in the parent handler interval.
This is a diagnostic estimate, not a calibrated CPU percentage or a rigorous
upper bound on whole-application speedup. Existing remove/name/reference/mark
scans are not all measured by get/get_mut and are not declared free.

Flush counters outside measured handlers on ordinary exit and outside-handler
process removal, so forced context teardown also exports pending event work.
The trigger PID identifies the flush, not ownership of all accumulated work.
Do not attribute global interleaved totals to that one Linux process. No Kernel,
timer policy, scheduling, permission or capability contract is changed.

Diagnostic normal-device SMP=4 image uses the same frozen cycle47 overlay,
observer and Linux fixtures as accepted cycle49. Set NANAMI_ALTER_PROFILE=1 only
for this build. Linux diagnostic SHA-256
739415214de158196ce7969f5f1baecb9c56b7e254007290f34e14fb7bf681f8,
FreeBSD d815731584753573882838537a91559a15aedc66c2305905969c3b9bf7d2e1e2.
Kernel remains 312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88
and Alpha d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af.
Run real create/start/X11-ready/kill/delete/reused-ID startup three times. All
sessions complete and the runner exits zero. The diagnostic startup durations
4833.007 / 5178.969 / 5204.856ms include logging/instrumentation; never compare
them to normal images as a measured runtime regression or optimization gain.

Log /private/tmp/nanami-opt-cycle50-profile-startup.log, actual serial directory
nanami-negoto-qemu-bwnricld. Parse padded labels such as [       alter.profile],
not only unpadded strings. An initial unpadded-label query found no rows; this
was a parser mismatch, not zero lookup activity. The corrected complete scan
has no malformed rows or truncated output. Full summary is retained at
/private/tmp/nanami-opt-cycle50.ryfsfl/profile-summary.json.

| Search scope | Calls | Comparisons | Average | Maximum live/visited | Misses | Elapsed ticks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Inside syscall handlers | 34,161 | 166,332 | 4.8691 | 8 / 8 | 5 | 10,020,000 |
| Event/readiness/cleanup | 14,924 | 77,512 | 5.1938 | 8 / 8 | 0 | 1,332,000 |

Total instrumented syscall-handler ticks are 11,923,939,000. The syscall-side
search spans are 0.08403% of that total. Do not divide event-side work by this
handler-only denominator or call the ratio exclusive CPU time. Ticks are raw
local counter units; no cross-core subtraction or conversion to ns is used.
The five misses are failed weak-ID lookups, not five failed container starts.

Largest handler elapsed totals in this diagnostic:

| x86_64 syscall | Calls | Elapsed ticks | Share of handler ticks |
| --- | ---: | ---: | ---: |
| mmap (9) | 2,193 | 5,317,877,000 | 44.60% |
| clone/fork (56) | 45 | 2,048,148,000 | 17.18% |
| execve (59) | 51 | 738,050,000 | 6.19% |
| openat (257) | 1,390 | 616,042,000 | 5.17% |
| recvmsg (47) | 2,559 | 491,853,000 | 4.12% |
| ioctl (16) | 3,078 | 366,730,000 | 3.08% |
| read (0) | 2,829 | 360,350,000 | 3.02% |

Do not identify mmap's full elapsed span as memcpy CPU time or revive earlier
rejected fork-sharing/initialization/cache variants without new evidence. This
inventory does not justify a socket lookup cache as the next 5% startup target.
The remaining useful inspection is the larger construction paths and repeated
checked guest-memory transfers, preserving the same permission preflight and
ordered partial-error semantics. IPC fastpath itself is not implicated by
counting requests or this elapsed-time profile. No new batching protocol is
implemented by this diagnostic turn.

Archive the exact temporary four-file probe patch at
/private/tmp/nanami-opt-cycle50-socket-profile.patch. Its reverse check succeeds
before restoration and forward check succeeds after restoration. Restore all
four files byte-for-byte with apply_patch, without resetting any prior work.
Original sources plus raw before/diagnostic Linux/FreeBSD binaries remain in
/private/tmp/nanami-opt-cycle50.ryfsfl. Full native release suite after restoration
passes 1,147 tests / 16 ignored. No new production probe/cache/state remains.

Normal rebuild with the profile environment unset restores Linux
9ef6b62c3ae1b43ba3c296d7e8884f416bda5d58f74e704f0f377204d1739267 and
FreeBSD cda8edd4fd68017650e9618d27194f95ba4dfc33a873e3a69b7e3e986ecc7e8e;
both compare byte-for-byte with recorded accepted executables. Alpha and Kernel
remain unchanged. Current normal image /private/tmp/nanami-opt-cycle50-restored.img
uses the frozen inputs. Its separate normal startup check exits zero with all
three real sessions and final cleanup: 4501.889 / 4987.797 / 4890.729ms, serial
directory nanami-negoto-qemu-t7xatmt2. These are restoration checks, not a new
balanced before/after improvement claim. The complete real-input, launcher,
Information and two-session GUI regression from cycle49 covers the identical
restored runtime; no new full GUI run is claimed here.

After empty lsof checks, remove only owned superseded cycle48-message.img and
temporary cycle50-socket-profile.img for build headroom. Keep reproducible
probe sources, raw binaries, inputs and all logs. No unrelated data is removed,
and no version bump, commit/release, Kernel change or goal completion is implied.

After the normal restoration check, another empty lsof check permits removing
only the owned cycle49-restored.img duplicate. Cycle50-restored.img is the
current normal baseline; its frozen inputs and runtime are the same. Preserve
the prior GUI/measurement logs and raw binaries rather than retaining multiple
byte-identical runtime image generations on the nearly full disk.

## Cycle 51: ordered checked memory-transfer vectors

The preceding profile turn makes progress by ruling out socket-state searches
as the next large startup target and restoring all temporary probes. Keep
Kernel unchanged. Inspect current Alpha copy authorization/permission/copy
handlers, Runtime wrappers and the recvmsg output path. Three output fields
currently use two Alpha requests (name length, then coalesced tail), including
repeated dispatch/authorization and checked memory work. The measured request
reduction is not a claim that Kernel's IPC transport itself is inefficient.

Add generic PROCESS_MEMORY_READ_VECTOR=0x102e and WRITE_VECTOR=0x102f to Nanami's
user-level protocol. Args are target PID, caller descriptor VA, count 1..16,
reserved zero. Each repr(C) record is three native Words: target VA, caller-local
VA, bytes. The server permits only self/reaper authority, snapshots all records,
then executes them in order. Every record is permission-checked over its entire
target range before copying that record. A later argument/permission/copy failure
preserves the completed prefix and stops later records. Descriptor-read failure
does not start any payload copy. This is not an all-or-nothing transaction.
No completion index or automatic retry is added. Same-PID overlap retains the
existing memmove path; no new guarantee about arbitrary cross-PID physical aliases
is claimed. The full wire/lifetime/error contract is in nanami/doc/message.md.

Reuse current caller-frame staging and copy windows; allocate no persistent
buffer/cache or new Kernel object. The local 16-record snapshot is at most 384
bytes on the two 64-bit targets. Decode into it directly, not a large returned
Result/array. Factor existing authorization/range logic so scalar and vector
checks retain the same policy; existing scalar opcodes and privileged debugger/
image copy remain supported. Runtime provides checked readv/writev wrappers.
The caller slice/buffers remain borrowed through the synchronous reply.

Alter's generic copyout_many batches ranges only when its personality preflight
succeeds; otherwise it follows ordered scalar copies so a later VMA error does
not cancel earlier writes. Use three scalar-width entries for recvmsg fields,
not one wide header write or a padding/input-pointer overwrite. Keep u64 control
length storage explicit, independent of native Word width. Queue/rights commit
still follows successful payload and metadata output. Empty receive also uses
the same one-request result path. No Kernel, capability transfer, Notification,
permission cache, terminal/parser policy or request retry changes.

Production-handler tests cover descriptor bounds/authority before input read,
snapshot read failure, both copy directions, maximum count, descriptor address
overflow, invalid later records, whole-range permission failure after a completed
prefix, input storage changed during transfer, and shared-VM protection changes.
Guest-copy tests cover one batch, untouched SHM scratch, ordered prefix for
personality/Alpha failures, and empty/zero-length scalar behavior. UNIX tests
keep padding, SCM_RIGHTS and queue rollback checks; successful payload+metadata
uses two modeled Alpha requests, EOF metadata one. The lifecycle fixture rejects
unexpected vector calls instead of pretending to implement descriptor memory.
Full native release suite: 1,158 passed / 16 ignored. Related core-process,
linux-guest-copy and linux-unix ASan: 139 passed / two ignored. AArch64 Alpha
and Alter/Linux compile checks pass; this is not an AArch64 boot claim.

Normal x86_64 candidate build succeeds without diagnostic variables. An initial
real QEMU socket validation completes all protected-page assertions and timing
footer, but is NOT included in the balanced comparison. The protected-page guest
may take Alter's ordered scalar fallback when its VMA preflight already rejects
the range; do not describe that alone as proof of Alpha's late-denial vector
path. Production Alpha authorization/permission logic is separately unit/ASan
tested, with byte transfer mocked there. Actual successful descriptor/payload
handling is exercised through the real client/server in QEMU.

Before normal image: /private/tmp/nanami-opt-cycle50-restored.img, Linux
9ef6b62c3ae1b43ba3c296d7e8884f416bda5d58f74e704f0f377204d1739267,
FreeBSD cda8edd4fd68017650e9618d27194f95ba4dfc33a873e3a69b7e3e986ecc7e8e,
Alpha d6af986511d40758217e1c2e56f26bec755075e98dcc37869bfb0499c14438af.
Candidate /private/tmp/nanami-opt-cycle51-vector.img, Linux
d6d0ef28f652af101c9148868ad00a14e439b8fb859ba40448a9b58ef3206e07,
FreeBSD 89420d9f22ab5c222a5e9541631ef2c0e3f904fb14a9a20dc07a114ca4bf7b9a,
Alpha 351f57bea0b165d62eaccb9631ad2eae44b8396fff5db9268ce89925287d8452.
Kernel remains 312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.
SDK dependency changes also relink otherwise logically unchanged services:
HPET now 612612c27d5bddb51205dde5fb1eaed07497f1dd7948c0d6116a140b2b83daf1,
Honoka 58c45cc4ba93ec1ba467d1de824f53646dbc28a183d541caa8cf5165dc1e9fbf.
Do not claim every unrelated binary is byte-identical or exclude code-layout
effects by assertion. Observer is frozen d3f2bdb8...152f72, with identical
cycle47 overlay/fixture/helper/profile. No builds/tests run with timed VMs.

Initial before1/after1/after2/before2 trials each run process, socket and actual
startup in separate normal-device SMP=4 boots: twelve complete boots. Socket
reports each have all five rows with three samples and their final footer;
process completion is checked in actual serial, not filtered stdout. Startup
contains three actual create/start/kill/delete/reused-ID sessions per boot.
No outcome-based exclusions or missing reports occur.

| Socket/control workload (ms, six per side) | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Existing stat, 512 calls | 143.1850 | 143.5845 | +0.28% |
| Missing stat, 512 calls | 109.6800 | 108.1250 | -1.42% |
| Plain UNIX write/read, 2048 pairs | 371.0395 | 371.9875 | +0.26% |
| UNIX write/recvmsg, 2048 pairs | 836.7795 | 739.9225 | -11.57% |
| UNIX recvmsg EOF, 2048 calls | 524.1200 | 425.1995 | -18.87% |

Whole real startup remains separate:

| Session (ms) | Before 1 | Candidate 1 | Candidate 2 | Before 2 |
| --- | ---: | ---: | ---: | ---: |
| First | 4506.815 | 4509.548 | 4406.017 | 4608.166 |
| Second | 4986.400 | 5008.154 | 5098.874 | 4993.824 |
| Third | 5099.796 | 5093.324 | 5005.927 | 5095.757 |

Pooled 4990.1120 -> 5007.0405ms (+0.34%), below the 100ms observation step.
Do not combine first and repeated sessions as independent equivalent startup
trials, or call the targeted socket result a 5% whole-system improvement.

Initial ordinary fork/wait is 18.5505 -> 20.0055ms (+7.84%). Do not hide this
behind socket gains or assume functional tests establish performance parity.
After GUI, add four PROCESS-ONLY boots in after3/before3/before4/after4 order.
All complete, with the same fixture and unchanged candidate code. In that
reverse extension fork is 17.729 -> 17.542ms (-1.05%); native Alpha constructor
control 24.458 -> 23.258ms (-4.91%). The extension is not a new socket/startup
experiment, and does not erase the initial fork observations.

| Process workload, all eight boots (12 samples per side), ms | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Native Alpha spawn/reap, eight children | 24.3490 | 24.2335 | -0.47% |
| Whole native BusyBox lifecycle | 28.5725 | 28.1035 | -1.64% |
| Linux getpid, 2048 calls | 39.0685 | 38.8345 | -0.60% |
| Linux getuid, 2048 calls | 39.3005 | 39.5725 | +0.69% |
| Checked copyout, ordinary VMAs | 171.3465 | 171.9400 | +0.35% |
| Checked copyout, 128 extra VMAs | 171.9580 | 172.9805 | +0.59% |
| Checked copyout, 256 extra VMAs | 172.6740 | 174.0390 | +0.79% |
| Ordinary fork/wait, eight children | 18.2500 | 19.5365 | +7.05% |
| fork/exec/wait, eight children | 677.0745 | 672.6755 | -0.65% |
| fork, 128 extra VMAs | 18.7005 | 18.1015 | -3.20% |
| fork, 256 extra VMAs | 18.1155 | 17.8305 | -1.57% |

Ordinary-fork raw samples (cold first sample remains included):

| Boot | Eight-fork samples, ms |
| --- | --- |
| before1 | 22.829,18.077,19.024 |
| after1 | 23.751,19.678,20.333 |
| after2 | 22.752,19.395,18.275 |
| before2 | 22.552,17.896,16.749 |
| after3 | 25.821,16.422,17.147 |
| before3 | 29.617,17.013,18.423 |
| before4 | 24.256,16.444,17.035 |
| after4 | 24.773,17.357,17.727 |

Warm-only fork observations, explicitly not replacements for the complete pool,
give 17.4655 -> 18.0010ms (+3.07%). The reverse order does not reproduce a fixed
7-8% penalty, but the complete pool remains slower; ordinary fork equivalence
is UNPROVEN and investigation remains required. Native constructors do not run
the Linux ELF, and mixed cold/warm Negoto create rows are not desktop timings.

No claim of a new algorithmic fork change is made from elapsed samples alone.
Inspect actual Alpha before/after dispatcher prologues: both reserve 0x2608
stack bytes and immediately jump by opcode, with no new unconditional array
initialization visible there. This rules out that particular proposed prologue
explanation, not all code-generation/layout or fork-path differences. Retain
the additive API/recvmsg work provisionally while investigating this control;
do not treat the broad optimization goal or every performance gate as complete.

All primary logs: /private/tmp/nanami-opt-cycle51-{process,unix,startup}-
{before1,after1,after2,before2}.log. Process-only extension logs use after3,
before3,before4,after4. Validation log is cycle51-unix-validation.log, excluded
from primary medians. All reports and outliers are preserved.

Real GUI run separately passes two complete normal SMP=4 sessions: READY/LINKS/
INPUT/LAUNCH/INFO/SESSION and final TINYCORE-DESKTOP-PASS with zero exit. Log
/private/tmp/nanami-opt-cycle51-desktop.log, serial/screenshots directory suffix
mp7sdohr. Inspect the actual Information output before final handoff. This is
named-workflow coverage, not exhaustive testing of all rich desktop applications.

Formatting-only dispatch refinement is rebuilt through ordinary xtask; Alpha,
Linux and FreeBSD remain byte-identical to the tested candidate raw executables.
No unmeasured runtime change is silently substituted after the comparisons.
Snapshots before/candidate binaries and original touched files reside in
/private/tmp/nanami-opt-cycle51.ScuKJ5. Preserve exact candidate source/test/wire
patches and the frozen inputs. No Kernel change, version bump, commit, tag,
release or whole-goal completion is implied.

Final source/test/wire archive is /private/tmp/nanami-opt-cycle51-vector-final.patch;
its reverse-apply check succeeds against the retained files. Earlier pre-format
candidate patch is kept separately, not mistaken for the final source. Detailed
raw rows/summary tables are also preserved in the snapshot's measurements.json.
Formatting and diff checks pass. Inspect the second Information screenshot:
actual OS information, live shell, blue background and launcher are present
after restart. Keep both cycle50 baseline and cycle51 images for the unresolved
fork comparison; no image is deleted in this cycle. No active VM/test/build
handle remains at handoff.

## Cycle 52: isolate fork controls without Kernel changes

Honor the explicit user decision to continue entirely at user level. Cycle 51
retains its ordered checked-copy API and measured recvmsg benefit, but its
pooled ordinary-fork result remains slower. Investigate that control before
expanding vector adoption. No Kernel/FPU change, version bump, commit or release
is part of this investigation.

Compare actual before/candidate Alpha code with llvm-nm and llvm-objdump,
normalizing symbol relocations, RIP-relative displacements and self-relative
branch destinations, not arbitrary ordinary immediates:

| Alpha method | Before/after bytes | Before/after instructions |
| --- | ---: | ---: |
| clone_process_vm | 1598 / 1598 | 374 / 374 |
| map_process_initialized_pages | 4021 / 4021 | 890 / 890 |
| copy_process_memory | 771 / 771 | 205 / 205 |
| handle_raw_task_create | 776 / 776 | 175 / 175 |

The last three normalized instruction sequences match exactly. Clone differs
only in two rodata pointers in its cold panic_fmt block. This does not establish
that all callees or complete executables match. Alter's fork source is unchanged,
but LLVM inlines clone_registers_for_fork in the new binary: sys_fork grows from
3394 to 3589 bytes while the old 237-byte helper disappears (combined code
decreases 42 bytes); stack storage grows 16 bytes. These facts alone do not
explain the earlier measurement.

First hold one profiled Alter/Linux binary fixed and pin only the exact old
or candidate raw Alpha executable. Add temporary local-counter boundaries
around fork preparation, the Alpha request, register cloning, child installation,
FD inheritance, VMA/memfd inheritance, metadata and resume. Emit only after the
last sample. Use the process fixture, not recvmsg or GUI: the old Alpha does not
implement vector operations. Build overrides and probes are archived, not
retained production options.

Run old/new/new/old as four sequential four-vCPU QEMU boots, normal devices and
frozen observer, Linux fixtures and overlay. All complete the full process
footer, with 96 process-fixture forks plus seven exec-cache-fixture forks per
boot and no checked fatal/spawn/SHM/fork failure. There are 48 ordinary-fork
observations per variant. Median raw local ticks:

| Ordinary fork interval | Old Alpha | New Alpha |
| --- | ---: | ---: |
| Alpha request | 1564500 | 1521500 |
| Register clone | 8000 | 7000 |
| Child installation | 69000 | 69000 |
| FD inheritance | 8000 | 7500 |
| VMA/memfd inheritance | 7000 | 7000 |
| Complete sampled fork handler | 1699500 | 1665500 |

The Alpha request accounts for about 88.9% of summed ordinary-fork handler ticks
on both sides. This includes private-page construction/copy, capability work,
services and descheduling, not isolated IPC transport cost. Diagnostics perturb
execution: the roughly 2% handler reduction is not a normal-build speedup claim.
They do not reproduce a consistent Alpha-only ordinary-fork penalty.

Remove the profiler, then fix Alpha, HPET, Honoka, FreeBSD, Kernel and fixtures
to the current cycle 51 binaries. Build an image differing only in the pinned
old Alter/Linux executable and run old/new/new/old without profilers. Each Linux
loop has six samples per variant, three per boot including the cold first one.
Pooled medians in milliseconds:

| Normal process control | Old Alter | New Alter | Change |
| --- | ---: | ---: | ---: |
| Native Alpha construction/reap, eight images | 26.2745 | 24.4785 | -6.84% |
| getpid, 2048 calls | 39.2295 | 39.5255 | +0.75% |
| getuid, 2048 calls | 39.3565 | 39.3380 | -0.05% |
| Ordinary checked copyout | 173.4005 | 170.9335 | -1.42% |
| fork/wait, eight children | 18.4570 | 18.1650 | -1.58% |
| fork/exec/wait, eight children | 688.6085 | 691.1265 | +0.37% |
| fork with 128 extra VMAs | 20.9515 | 18.2800 | -12.75% |
| fork with 256 extra VMAs | 18.2550 | 19.6200 | +7.48% |

Ordinary-fork samples by boot, old/new/new/old order:
25.749/18.728/17.891; 24.002/18.837/16.573;
22.681/16.706/17.493; 23.855/16.941/18.186ms.
All four boots complete freshness/protection checks and both process markers.
The native construction control does not execute Alter/Linux; do not attribute
its movement to this binary replacement. Fragmented-fork controls move in
opposite directions. The original cycle 51 +7.05% pooled ordinary-fork result
remains in the record: these isolated comparisons neither erase it nor prove
a fixed causal regression, performance parity or a whole-system 5% gain.
No additional runtime optimization is retained in this cycle.

Before/candidate Alpha hashes remain d6af9865...38af and 351f57be...452;
before/candidate Linux hashes are 9ef6b62c...9267 and d6d0ef28...6e07.
Fixed normal HPET is 612612c2...daf1, Honoka 58c45cc4...9fbf,
FreeBSD 89420d9f...7b9a and Kernel 312d579b...0c88. The profiled Linux binary is
0a7982a0...638f. Full hashes, raw rows, summaries and serial directories are in
/private/tmp/nanami-opt-cycle52.uT5nqM/{profile,normal}-measurements.json.
Logs: /private/tmp/nanami-opt-cycle52-process-alpha-{old1,new1,new2,old2}.log
and /private/tmp/nanami-opt-cycle52-process-alter-{before1,after1,after2,before2}.log.
Probe archives: /private/tmp/nanami-opt-cycle52-fork-profile.patch and
/private/tmp/nanami-opt-cycle52-alter-pin.patch.

Restore fork.rs and xtask/src/build.rs byte-for-byte to their pre-cycle
snapshots. Explicitly restore the normal Linux raw executable after pinning:
Cargo's up-to-date artifact must not be assumed to undo a manually pinned ELF.
Remove only the two owned diagnostic image paths after their VM handles close;
the second image uses the first image's freed space. These generated images
are rebuildable from preserved raw executables, frozen inputs and archives.
Keep both cycle50 baseline and cycle51 normal images for further comparisons,
along with all logs and snapshots.

Final normal xtask build regenerates the rootfs and cycle51-vector.img without
any probe variables. Alpha, Linux and FreeBSD match all three saved cycle51
candidate executables exactly; Kernel, HPET and Honoka hashes above also match.
Full native tests pass 1158 / 16 ignored, and focused core-process,
linux-guest-copy and linux-unix ASan passes 139 / two ignored. Restoration logs:
/private/tmp/nanami-opt-cycle52-restored-{build,native,asan}.log. Both probe
archives pass forward-apply checks after restoration; cycle51's final API patch
passes reverse-apply checks. Diff whitespace checks pass, the A9N worktree is
clean, and no probe gate remains in fork.rs or xtask. No new GUI/startup gain
is claimed from these process-only isolation runs. The broader optimization
goal remains active, with ordinary-fork causality/performance parity unresolved.

## Cycle 53: request-local path-walk buffer reuse (rejected)

The preceding turn makes progress by isolating fork controls with exact Alpha
and Alter substitutions, restoring the normal runtime and preserving contrary
measurements. Continue entirely at user level. Do not repeat the rejected
final-permission, GPR memcpy, duplicate source-validation or copy-cache variants.
Source review finds no large new socket chunking opportunity: the direct-copy
loop still uses the SHM-size bound, but both the normal SHM and socket queue
limit are 64KiB. Merely removing that loop is not an established large win.

Inspect ext2's actual rooted path walker instead. Each ordinary component
replaces last_name with a fresh Vec, clones it into the canonical component
list, then final path assembly grows another Vec geometrically. Evaluate two
small internal changes: clear/extend the existing last_name buffer, and compute
the checked canonical output length before reserving its final capacity.
Names, parent discovery, symlink expansion, dot/dot-dot order, missing-target
handling, lookup counts and external path protocol are unchanged. No cache or
new Kernel operation is introduced. The scratch Vec can retain the largest
component's capacity for this request; fewer allocations do not mean every
returned buffer has smaller capacity.

Add a temporary thread-local host allocation counter and an allocation-free
Chain filesystem fixture, then run the production walker on a 64-directory
path. Before the edit it uses 151 allocation/reallocation calls per resolution;
the candidate uses 80 (-47.02%). One initial host timing is contrary
(38.693 -> 49.275ms per 10000 walks). A later matched eight-process host probe,
before/after/after/before/after/before/before/after, gives medians
38.260604 -> 28.340208ms (-25.93%). Preserve both observations. The host counter
and System allocator differ from Nanami's TLSF; neither proves a system gain.
Raw wrappers, binaries and all rows are under /private/tmp/nanami-opt-cycle53.8kxRDu.

Candidate validation passes 1161 native tests / 17 ignored, including an
allocation-work bound. Seven namespace ASan tests pass / one probe ignored,
and ext2's AArch64 Release compile check passes (not an ARM boot claim).
Regressions cover shrinking leaf names after long components, symlink/dot-dot
resolution, missing final leaves, deep file-parent results and exactly 4096-byte
canonical paths versus longer results reconstructed from a relative dirfd.

Build a normal x86_64 candidate with the frozen cycle47 overlay, observer and
Linux fixtures. Only ext2's code and the boot payload containing it change.
Before ext2 SHA-256 is
28dcd19943911a8b96ab5a1803454bf2008e24fd48f0cd8a183ef8948fe29710;
candidate is 0e527185d3200b6a08d82ea9c849f358c7537ad4d95639a9716bc677257fe16b.
Alpha 351f57be...452, Linux d6d0ef28...6e07, FreeBSD 89420d9f...7b9a,
HPET 612612c2...daf1, Honoka 58c45cc4...9fbf and Kernel 312d579b...0c88
remain byte-identical to the previous normal runtime.

Run Unix/stat, process and three-session startup trials sequentially for each
before/after/after/before variant: 12 QEMU boots, four CPUs, normal devices,
no simultaneous build, native benchmark or other VM. All twelve full workload
footers pass without checked fatal, spawn, SHM or fork errors. All twelve real
X11-ready sessions complete kill/delete/same-ID reuse with cleanup=true.
Each Linux timing loop has six samples per variant. Pooled milliseconds:

| Normal workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 141.3540 | 145.9075 | +3.22% |
| missing stat, 512 calls | 110.0130 | 107.9955 | -1.83% |
| UNIX write/read, 2048 iterations | 368.9455 | 369.9355 | +0.27% |
| UNIX write/recvmsg | 740.2995 | 734.6215 | -0.77% |
| EOF recvmsg | 421.7230 | 421.9965 | +0.06% |
| fork/wait, eight children | 21.0915 | 18.0210 | -14.56% |
| fork/exec/wait, eight children | 675.5680 | 683.3700 | +1.15% |
| Xfe whole input/construction/cleanup | 28.4895 | 28.3805 | -0.38% |
| Native eight-image construction/reap | 24.8515 | 24.0760 | -3.12% |
| OCI start to X11-ready | 5036.5315 | 5033.8500 | -0.05% |

Do not credit the ordinary-fork movement to this filesystem path change.
Negoto create moves +8.32%, Xfe construction-only +9.80%, and the complete
BusyBox input/construction/cleanup control +3.46%; retain those contrary rows.
Do not substitute the improved deep-path host probe for actual stat or startup.
Both candidate stat boot medians exceed both baseline medians. Startup changes
far less than its 100ms polling step and does not establish a 5% gain.
Startup session times, milliseconds in before/after/after/before order:
4501.495/4982.025/5091.038; 4507.651/4982.232/5090.416;
4615.371/5085.468/5102.446; 4501.308/5099.025/5093.458.

Decision: reject the runtime candidate and its allocation-bound test/counter.
Restore path_walk.rs byte-for-byte to its pre-cycle snapshot. Retain only the
three semantic regressions and their small filesystem fixture. The full rejected
patch is /private/tmp/nanami-opt-cycle53-path-candidate.patch; its production-only
portion is /private/tmp/nanami-opt-cycle53-path-production-only.patch and passes
forward-apply checking against the restored source. Exact pre-cycle files,
candidate source/binary and raw measurements are retained in the snapshot.
Normal logs are /private/tmp/nanami-opt-cycle53-{unix,process,startup}-
{before1,after1,after2,before2}.log; measurements.json records all rows and
serial-directory locations. The temporary host counter is not left installed
in the retained tests.

After lsof confirms no open handle, remove the owned rejected cycle53-path.img.
The first normal-rootfs restoration attempt then fails for lack of temporary
disk space, before replacing the normal cycle51 image. Preserve that failed
log. Close-handle-check and remove only the generated cycle51-vector.img so
the ordinary build can regenerate it without retaining both full outputs at
once. Retry succeeds, restoring all normal runtime hashes listed above,
including the exact old ext2 executable. Keep the earlier cycle50 baseline and
all source/raw-binary/log evidence. Generated images are rebuildable; no guest
user data is removed (comparison VMs use private snapshots).

Restored validation passes 1161 native tests / 16 ignored and seven namespace
ASan tests. The rebuilt normal image completes three further real X11-ready,
kill/delete/same-ID-reuse sessions: 4507.460/5084.300/5095.359ms. These are
restoration checks, not pooled candidate measurements. Logs:
/private/tmp/nanami-opt-cycle53-restored-{build,build-retry,native,asan,startup}.log;
restored serial directory suffix 6j7nf3so. No Kernel source, ABI, IPC fastpath,
version, commit or release changes. No new runtime optimization or whole-goal
completion is claimed; this measurement rules out allocator-call reduction in
the existing path walker as a demonstrated next large Tiny Core optimization.

## Cycle 54: fixed-slot indexed dentry lookup (rejected)

Cycle 53 makes progress by implementing, testing and measuring a genuinely new
path-allocation candidate, rejecting it on real stat/startup results, retaining
three semantic regressions and restoring exact normal binaries. Keep Kernel
unchanged. Re-read private-fork and memfd inheritance: shared mappings are copied
then rebound, but cycle17 already finds no inherited memfd mapping in its actual
Tiny Core startup. Do not introduce a selective-fork API without new evidence
that eligible shared mappings matter; do not share private read-only pages.

Inspect ext2's 128-slot positive/negative dentry cache. Its current get/update
walks the slots linearly; invalidation is centralized. First add a temporary
comparison counter to the real lookup, reporting every 2048 valid-name queries.
One instrumented four-vCPU normal-device boot completes three real X11-ready,
kill/delete/same-ID reuse sessions. Fourteen cumulative reports cover a prefix
of 28672 queries, 1340209 visited slots, 27547 hits and 1268 negative hits:
46.7428 visits per query and 96.08% hit rate. Counts include native startup and
Linux filesystem work; the unreported final partial block is not included.
They are not elapsed/CPU percentages or proof that dentry lookup dominates.
Log: /private/tmp/nanami-opt-cycle54-profile-startup.log; serial suffix 1839lhc6.
Profile binary: 4b8a0354...5762d. Archive its source probe, then restore main.rs
exactly before implementing or timing the candidate.

Evaluate an internal Cache module indexing the same 128 payload slots with
128 u16 hash heads and 128 u16 links. It adds exactly 512 bytes; total cache
storage stays below 35KiB. There is no heap allocation, capacity increase,
credential cache, external protocol or Kernel change. Hits and in-place updates
retain FIFO replacement order, including invalid slots. FNV-1a mixes every inode
byte/name byte and folds upper bits for bucket selection. Always compare the
complete parent/name, never just a hash. Both positive and negative entries
retain existing named/inode/parent invalidation. Whole-inode invalidation visits
each indexed slot at most once, even with complete hash collisions.

Six candidate tests compare 20000 mixed mutations with an independent linear
oracle; cover FIFO eviction despite hits/updates/holes, positive/negative and
case/parent distinctions, full-word inode values, invalid/255-byte names, exact
memory overhead, and a 128-entry single-bucket collision chain through head,
middle and tail removal, eviction, whole-parent invalidation and refill. Full
native: 1167 passed / 16 ignored. Cache/namespace ASan: 13 passed. AArch64 ext2
Release compile check passes (not an ARM boot claim). Full source, test and
manifest candidate: /private/tmp/nanami-opt-cycle54-index-candidate.patch.

Normal candidate ext2 SHA-256:
2b005739189cfd08a65d0134ec823a3c59d21b932f53292f16ac14949a599c00;
baseline: 28dcd19943911a8b96ab5a1803454bf2008e24fd48f0cd8a183ef8948fe29710.
Alpha, Linux, FreeBSD, HPET, Honoka and Kernel retain the exact cycle51 normal
hashes. Reuse the frozen cycle47 rootfs: xtask excludes core apps from rootfs,
and only the ext2 core executable/boot payload changes. No stale non-core
application is silently packaged, no new Linux fixture or observer is deployed.

Disk space intermittently falls below the filesystem's write reserve during
candidate-binary/archive creation. Preserve the failure, inspect authoritative
space/handles and remove only the untested, rebuildable cycle54-index.img.
Compress the older owned cycle50-restored.img with gzip -1 -k; gzip -t succeeds
and streamed decompression matches original SHA-256
c03acb9ef09582f815771181d583ab94fa7804b1ab62e2e70633d085c45813dd.
After a closed-handle check, remove only its uncompressed generated file. Its
exact contents remain in /private/tmp/nanami-opt-cycle50-restored.img.gz (about
43MiB), recoverable by decompression; no user data/source/log is discarded.
This is the historical pre-vector baseline, not this cycle's normal baseline.
Rebuild the candidate and confirm it matches the saved candidate ELF exactly.
Do all this before normal timings, not during a comparison VM.

Run Unix/stat, process and three-session startup trials sequentially for each
before/after/after/before variant: twelve QEMU boots, four CPUs, normal devices,
frozen inputs, no overlapping own build/test/VM. All full workload footers and
all twelve X11-ready cleanup/reuse sessions pass with no checked fatal, spawn,
SHM or fork errors. Six samples per Linux loop/variant, pooled milliseconds:

| Normal workload | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 151.7880 | 149.0310 | -1.82% |
| missing stat, 512 calls | 109.5635 | 112.0850 | +2.30% |
| UNIX write/read, 2048 iterations | 369.6865 | 370.7090 | +0.28% |
| UNIX write/recvmsg | 739.9230 | 741.5690 | +0.22% |
| EOF recvmsg | 415.8510 | 425.7370 | +2.38% |
| fork/wait, eight children | 19.6670 | 17.9025 | -8.97% |
| fork/exec/wait, eight children | 692.1680 | 697.6205 | +0.79% |
| Xfe whole input/construction/cleanup | 28.8105 | 29.7915 | +3.41% |
| Native eight-image construction/reap | 24.5765 | 24.4000 | -0.72% |
| OCI start to X11-ready | 5093.4770 | 5095.2095 | +0.03% |

Do not credit the ordinary-fork movement to a dentry index. Negoto create is
+2.37%; fragmented-fork controls are +0.99% (128 VMAs) / -6.97% (256 VMAs).
Keep all samples: stat boot medians are 157.442/149.832/148.925/144.603ms in
before/after/after/before order, so the favorable first pairing does not repeat
against the last baseline. The startup difference is below the 100ms polling resolution
and does not establish any gain. Actual session times, milliseconds:
4609.644/5098.468/5088.486; 4505.323/5095.584/5095.800;
4502.346/5099.725/5094.835; 4604.336/5189.188/5188.548.

Decision: reject the entire runtime/test-module candidate. Its 512 bytes and
index maintenance do not earn adoption from these mixed small effects and
neutral actual startup. Restore main.rs and the native manifest, remove the
two candidate-only source/test modules, and leave no profile counter/gate.
The full candidate remains archived and passes forward-apply checking against
the restored files; do not keep an unmeasured linear-cache refactoring just to
retain the test harness. No production code or Kernel change is retained.

Snapshot /private/tmp/nanami-opt-cycle54.KpwUNT contains before/profile/candidate
raw ext2 binaries, original files and measurements.json with all raw/summary
records, verified footers and directory paths. Probe archive:
/private/tmp/nanami-opt-cycle54-dentry-profile.patch. Normal logs:
/private/tmp/nanami-opt-cycle54-{unix,process,startup}-{before1,after1,after2,before2}.log.
Final candidate gates use *-native-final, *-asan-final, *-arm-final and
*-build-final logs; *-build-retry follows disk recovery. Candidate images are
removed only after confirmed termination/closed handles and are rebuildable.

Restore the normal image at /private/tmp/nanami-opt-cycle51-vector.img. All seven
runtime hashes match the baseline, including ext2 28dcd199...710. Restored native
tests pass 1161 / 16 ignored; namespace/file-version/directory ASan passes 14.
The rebuilt normal image completes three further startup/cleanup/same-ID reuse
sessions: 4498.456/5087.465/5196.567ms, not pooled candidate measurements.
Restoration logs: /private/tmp/nanami-opt-cycle54-restored-{build,native,asan,startup}.log;
serial suffix 2w72wrxe. The A9N worktree remains clean, no version/commit/release
is changed, and the broader optimization goal remains active.

## Cycle 55: stat latency attribution and borrowed owner lookup (rejected)

Keep Kernel unchanged, as explicitly requested. Measure the existing stat path
before adding another cache. Separate temporary probes timestamp local stages
in Alter newfstatat, POSIX namespace forwarding and ext2 namespace resolution.
Each actor reports success/failure aggregates every 128 completed operations.
Use existing local counters, not extra clock IPC, and never subtract timestamps
from different CPUs. These intervals include waits and descheduling; child
logging can enter a parent's wait. They are diagnostic attribution, not exclusive
CPU percentages, additive actor costs or normal-image benchmark results.

The fixed UNIX/stat run reports 3072 successful ext2/POSIX operations. Ext2
splits time into root input validation (9.05%), root lookup/inode (15.30%),
actual namespace preparation/walk (68.64%) and output work (7.01%). POSIX spends
96.78% of its measured intervals in the nested VFS request. Alter's 1536
successful measured newfstatat calls split into input (21.26%), namespace request
(57.81%) and checked output (20.93%). Its 1536 missing-path errors split into
28.57%/71.41%/0.02%. The services' status groups are not equivalent: a permitted
missing final component can return successful inode-zero metadata to Alter,
which then returns ENOENT. Early invalid requests and final partial blocks are
not included. In actual three-session startup, the corresponding ext2 walk
share is 85.81% for 1792 successful requests, POSIX's VFS interval is 97.65%,
and Alter's successful-call split is 21.71%/57.55%/20.73% (128 calls).
Do not label the nested wait intervals pure IPC transport overhead.

A second, separately built diagnostic configuration disables those service
probes and measures Alpha's scalar checked copies of exactly 128-byte inputs
and 144-byte outputs. Among 3072 reads, authorization is 13.97% and range/copy
work 86.03%; among 1536 writes, 15.54%/84.46%. This excludes transport and early
authorization failures. Raw ticks are retained without assuming nanoseconds.
No Kernel logging, fault delivery or fastpath change is made. The first and
second diagnostic configurations must not be combined as a single timing run.

Source inspection suggests borrowing the ProcessEntry for memory_owner_pid,
while preserving the public owned find_entry_by_pid API. Check optimized x86
code before claiming a removed copy: the standalone owned getter uses a
528-byte memcpy, but the compiler already eliminates that whole-entry copy in
vm_space_mut and the scalar checked-copy handler. The latter is 976 bytes with
no calls. The small private borrowed getter candidate removes residual Option
payload checks; vm_space_mut changes from 282 to 250 bytes, still without calls.
It does NOT save 528 copied bytes per checked request. No cache, allocation,
authority relaxation or external API is added. A new regression checks current
owner lookup across shared-address-space changes and PID replacement while
previously returned owned entries remain independent snapshots.

Candidate native tests: 1162 passed / 16 ignored; focused core-process,
guest-copy and UNIX ASan: 140 passed / 2 ignored. AArch64 Alpha Release compile
check passes, not an ARM boot claim. Candidate Alpha SHA-256:
dc76ea77d378dcf39521496107b96759f78f0137aba93831c2ea0c6e0ed65d94.
All other normal services, fixtures and Kernel match the normal baseline.

Run UNIX/stat, process and actual three-session startup sequentially in balanced
before/after/after/before order: twelve normal-device four-vCPU QEMU boots, no
overlapping own build/test/VM. All complete workload footers pass, including
cleanup and same-ID reuse; no checked fatal/proc/shm/fork errors. Six samples
per Linux loop/variant, pooled milliseconds (cold samples retained):

| Normal workload | Baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 146.5320 | 146.2245 | -0.21% |
| missing stat, 512 calls | 108.9040 | 111.9955 | +2.84% |
| UNIX write/read, 2048 iterations | 374.3970 | 371.4095 | -0.80% |
| UNIX write/recvmsg | 744.6200 | 738.5465 | -0.82% |
| EOF recvmsg | 426.8240 | 419.7220 | -1.66% |
| fork/wait, eight children | 20.3545 | 18.2775 | -10.20% |
| fork/exec/wait, eight children | 705.0100 | 700.2070 | -0.68% |
| Xfe whole input/construction/cleanup | 29.4935 | 29.4805 | -0.04% |
| Native eight-image construction/reap | 25.8405 | 25.8550 | +0.06% |
| OCI start to X11-ready | 5090.3030 | 5197.3190 | +2.10% |

Other controls remain mixed: getpid +3.42%, getuid -0.42%, normal copyout -0.77%,
128/256-VMA copyout +0.15%/-1.95%, fragmented fork -3.61%/-1.88%, and Negoto
create +3.23%. The ordinary-fork boot medians are
20.813/18.247/18.308/19.896ms; do not erase them or attribute every movement to
one removed check. Actual startup samples in milliseconds are
4608.365/5090.329/5097.888; 4612.628/5195.231/5202.268;
4708.600/5199.407/5294.785; 4505.826/5090.277/5208.135.
The approximately 100ms pooled startup increase is one polling step, not proof
of a precise CPU regression; it also provides no evidence of a startup gain.

Decision: reject the production candidate. Stat/startup do not earn its
adoption, and its motivating large-copy premise was disproved for the relevant
optimized paths. Retain only the owner/snapshot semantic regression. Restore
all seven touched production/probe files exactly to their pre-cycle snapshots;
normal Alpha is again 351f57be...452. Linux, FreeBSD, POSIX, ext2, HPET, Honoka
and Kernel also match the saved normal hashes. A9N remains clean. Rebuild the
normal image at /private/tmp/nanami-opt-cycle51-vector.img, with no diagnostics.
Restored native tests pass 1162 / 16 ignored and focused ASan passes 140 / 2
ignored. The restored image completes three further actual startup, cleanup
and same-ID reuse sessions: 4503.049/5188.668/5194.134ms (serial suffix
itc82ebi). These restoration samples are not pooled into the comparison.

Snapshot /private/tmp/nanami-opt-cycle55.gxEWIi contains original sources,
before/profile/candidate raw ELFs and measurements.json with all diagnostic
aggregates, normal records, codegen and complete-footer checks. Source archives:
/private/tmp/nanami-opt-cycle55-stat-profile.patch,
/private/tmp/nanami-opt-cycle55-alpha-profile.patch,
/private/tmp/nanami-opt-cycle55-borrow-candidate.patch and its production-only
variant (forward-apply check passes after restoration). The combined candidate
archive includes the retained test; do not blindly reapply that test twice.
Normal logs: /private/tmp/nanami-opt-cycle55-{unix,process,startup}-{before1,after1,after2,before2}.log.
Diagnostic logs use profile-unix, profile-startup and alpha-unix suffixes;
restoration logs use restored-{build,native,asan,startup}. Generated diagnostic
and candidate images are removed only after termination/closed-handle checks;
source, exact ELFs and logs remain available to rebuild them.

Next lead, not an implemented optimization: read-only Kernel inspection finds
that the direct Reply-Receive switch requires matching client/server affinity
(ipc_port.cpp, operation_reply_receive). A measured boot places Alpha on CPU0,
Alter/Linux on CPU2, POSIX on CPU1 and ext2 on CPU3. These specific edges cannot
use that same-core return branch. The runtime already uses combined
Reply-Receive; there is no accidental split or known four-word payload cutoff
to fix. Investigate configurable user-level broker placement, preserving guest
and device parallelism, before considering any policy change. This observation
is neither a pure IPC-cost attribution nor a measured speedup. Kernel remains
unchanged; no version, commit or release is created and the goal stays active.

## Cycle 56: isolate IPC locality without changing Kernel

Keep Kernel unchanged. Follow cycle55's concrete placement lead rather than
adding another cache or changing the IPC protocol. The existing direct
Reply-Receive branch requires same-core affinity. Alpha0, Alter/Linux2,
POSIX1 and ext2 3 are confirmed in the actual boot log. Runtime already uses
combined Reply-Receive. Eligibility for this branch is not a claim that every
fault/syscall uses it, nor a measurement of pure transport cost.

First use the pre-existing NANAMI_SAME_CORE_DIAGNOSTIC=1 switch. Kernel still
reports four CPUs, with no source or binary change; all user processes run on
CPU0. One fixed UNIX/stat boot completes its full footer. Medians, milliseconds:
stat 67.424, missing stat 51.244, UNIX read/write 199.184, recvmsg 333.917 and
EOF recvmsg 185.255. A subsequent normal baseline gives
142.004/112.414/372.231/727.007/428.063ms respectively. This large movement is
not by itself a suitable architecture: real startup samples are
5614.958/5382.887/5360.394ms versus baseline
4603.066/5082.294/5194.864ms. Both three-session cleanup/reuse footers pass.
Do not adopt global user-process pinning or call the loop result a whole-system
gain. This is a one-boot isolation comparison, not a balanced acceptance test.

Then evaluate a temporary Alpha-only broker-placement probe. It selects ext2
and the four priority-21 rootfs services (terminal, POSIX, Alter/Linux and
Alter/FreeBSD) for CPU0. Those labels/priorities are strictly diagnostic, not
new privileged identity or acceptable production role-selection rules. Preserve
the old round-robin cursor for every selected process, so all non-selected
processes retain their exact baseline placement. Verify boot logs: Honoka1,
input3, HPET1, USB2, framebuffer2 and Linux applications across CPUs1/2/3 remain
unchanged. No runtime copy/auth/cache/Kernel protocol or fastpath changes.

Probe Alpha SHA-256:
d90e91284d09bb2e1a7cb2b3f82be6bdf17fcf2300559e114fda1fe755157f5c.
The global-core diagnostic Alpha is a2866098...ad83; normal Alpha is
351f57be...452. Other services and Kernel retain exact normal hashes. Build
only the core payload and reuse the frozen cycle47 rootfs correctly. No Linux
fixture or observer is changed, no per-request profiler is enabled. All tests
use four-vCPU QEMU TCG on the same host with normal devices; this is not a real
hardware performance claim.

Run normal/broker/broker/normal UNIX and actual-startup boots sequentially.
Add one process-control boot per side, broker first then normal. No overlapping
own build/test/VM. All ten complete workload footers pass, with no checked
fatal/proc/shm/fork errors. Six loop/startup samples per side, cold retained,
pooled median milliseconds:

| Workload | Normal placement | Broker probe | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 144.5270 | 90.5880 | -37.32% |
| missing stat, 512 calls | 112.1080 | 72.5615 | -35.28% |
| UNIX write/read, 2048 iterations | 372.5905 | 356.4515 | -4.33% |
| UNIX write/recvmsg | 746.5280 | 501.6190 | -32.81% |
| EOF recvmsg | 427.5245 | 272.3850 | -36.29% |
| OCI start to X11-ready | 5132.5355 | 4393.7045 | -14.40% |

Startup samples, in normal/broker/broker/normal order:
4603.066/5082.294/5194.864; 3795.170/4395.175/4392.234;
3901.642/4497.396/4412.838; 4701.462/5182.777/5195.750ms.
The favorable movement repeats against the last normal boot and is several
100ms polling steps, unlike the preceding tiny candidates.

Do not cherry-pick these favorable results. The separate process control has
only three samples per side and is not order-balanced; nevertheless it exposes
a large regression that prevents adoption. Getpid is 40.162 -> 110.787ms
(+175.85%), getuid 39.288 -> 109.869ms (+179.65%). The main probe's Linux task
remains on CPU2 while Alter moves from CPU2 to CPU0. Losing that local
guest/supervisor relationship is a specific hypothesis to test, not proof that
the Kernel fastpath code slowed down. Other controls are mixed: ordinary fork
19.018 -> 18.488ms (-2.79%), fork/exec 699.514 -> 663.598ms (-5.13%), normal
copyout -2.86%, fragmented copyout -2.20%/-2.00%, native eight-image construction
+10.75%, and Xfe whole input/construction/cleanup +14.01%. Negoto create is
-17.48%. All observations and raw samples remain in the snapshot.

Decision: do not adopt the probe or hard-code applications/priorities in Alpha.
Restore process_requests.rs exactly and verify its archived patch still
forward-applies. A subsequent generic placement design must preserve both
synchronous broker locality and guest/supervisor locality, while keeping
compute workers and devices parallel. First isolate the smaller change of
leaving Alter on its normal CPU and moving only its surrounding brokers there.
Alternatively test initially supervised Linux tasks near their supervisor,
with fork/thread placement kept as an explicit separate control. Neither is
implemented or assumed to solve the regression. If configuration is needed,
use caller-owned/configured
scheduling policy, not image names as authority or a Kernel change. Re-run the
small-syscall, process, GUI/restart and startup controls before adoption.

Snapshot /private/tmp/nanami-opt-cycle56.MjpF3d preserves original source and
normal/global-core/broker raw Alpha ELFs, plus measurements.json with every
record and full-footer check. The source probe is archived at
/private/tmp/nanami-opt-cycle56-broker-diagnostic.patch. Initial isolation logs
use cycle56-{same-core,baseline,brokers}-{unix,startup}; reverse trials use
cycle56-{unix,startup}-{after2,before2}; process controls use
cycle56-process-{after1,before1}. All are under /private/tmp. The generated
global-core image is removed after termination and closed-handle checks; its
saved ELF, source and build flags permit reconstruction. No version/commit/
release is created. No Kernel change, FPU work or new production API is retained.

The broker probe also passes the full two-session real GUI regression: X11,
FLWM/wbar, title drag, actual USB input, links, Terminal/Editor/Information dock
launches, independent command markers, Exit and same-ID restart cleanup. Log:
/private/tmp/nanami-opt-cycle56-brokers-gui.log, serial suffix he8pjt93. This
functional result does not override the simple-syscall regression. Remove the
generated broker image only after the runner exits and its handle check is
clear; raw ELF, patch and logs remain rebuildable.

Rebuild normal /private/tmp/nanami-opt-cycle51-vector.img with both locality
switches explicitly unset. Alpha, Linux, FreeBSD, POSIX, ext2, HPET, Honoka
and Kernel all match their pre-cycle normal hashes; A9N's worktree remains
clean. Restored native tests pass 1162 / 16 ignored. Three further actual
startup/cleanup/same-ID reuse sessions pass at
4509.607/5110.158/5105.999ms, serial suffix 68o8ah3j. These are restoration
checks, not extra pooled baseline samples. Logs use
/private/tmp/nanami-opt-cycle56-restored-{build,native,startup}.log. The snapshot
also stores gui.json and restoration.json. Both generated diagnostic images
are removed after terminal runner results and clear handle checks; source,
raw ELFs, measurements, logs and normal runnable image remain. No temporary
role-selection rule or new production optimization is retained in this cycle.
All own build/test/VM sessions are terminal; the broader goal stays active.

## Cycle 57: configurable broker locality without Kernel changes

Cycle56 is progress, not a blocker: its CPU0 cluster improves startup but
severely regresses simple syscalls. Test the smaller change of keeping Alter
on its original CPU2 and moving the surrounding synchronous brokers there.
Keep Alpha0, guest/worker placement and the round-robin sequence unchanged.
The temporary image/priority selectors are diagnostic only and are archived,
not retained in production. First paired process/startup/stat observations
show no 2.8x syscall regression and motivate a configurable implementation.

Add an optional trusted boot declaration, `@placement <priority> <preferred-cpu>`.
It selects initial scheduling only, including buffer spawns and raw fork/thread
creation. No built-in application names or priority-class meanings are added
to Alpha. Exact `@process ... affinity=N` wins; otherwise consume the original
automatic slot before selecting an available preference. An absent preferred
CPU retains that slot. Numeric bounds remain priority 0..31 and CPU0..63;
duplicate/malformed preferences fail closed even when the CPU is unavailable.
The table has at most 32 entries and is consulted only during creation, never
per syscall/RPC. No new RPC, wire word, Kernel ABI, permission cache, running
task migration or Kernel/FPU change is introduced. Preferences do not grant
platform or Service authority and cannot change the requested priority.

Five new native regressions cover exact-placement precedence and automatic
slot consumption, unavailable CPUs and strict-policy errors, independent
image grants, malformed/duplicate/inactive rules, all 32 priority classes and
legacy defaults. Shipped-config assertions also preserve priority22 storage
placement. Full native: 1167 passed / 16 ignored. Focused policy/core-process/
guest-copy/UNIX ASan: 149 passed / 2 ignored. Final AArch64 Alpha Release
compile check passes; this is not an ARM boot/performance claim.

Evaluate two separate normal configurations, without per-call profiler output.
Both use the same new Alpha ELF, SHA-256
d909436b8281f2774a7bff2673340bf2d6be80eb92550747e78caef99adcd8a5;
only packaged boot configuration changes. The six other runtime hashes remain
the exact cycle51 normal values, including POSIX 864ff18c...b54 and ext2
28dcd199...710. Kernel remains byte-identical 312d579b...c88 and A9N's worktree
is clean. Reuse frozen cycle47 fixtures/observer/rootfs, rebuilding the core
payload rather than silently changing non-core applications. Tests use QEMU
TCG, four CPUs and normal devices on the same host, not physical hardware.

First configure priority21 and priority22 on CPU2. Twelve sequential
normal/changed/changed/normal comparison boots all pass complete footers.
Pooled stat improves 23.70%, missing stat 33.95% and startup 4.84%, but Xfe's
whole input/construction/reap interval regresses 8.85%; its input preparation
regresses 24.39%. Source audit confirms that read_image includes actual VFS
open/stat/read, buffer creation/copy and close before construction. This
configuration moves VFS away from both its bulk-I/O caller and AHCI on CPU3.
That is a locality hypothesis, not exclusive CPU-cost attribution. The full
two-session GUI also passes, but functionality does not override the contrary
performance evidence. Reject the priority22 placement, preserving all samples
in two-class-measurements.json. Do not mix its samples with the next phase.

Retain only `@placement 21 2`. Actual logs confirm terminal/POSIX/Alter Linux/
Alter FreeBSD on CPU2, while VFS and AHCI remain on CPU3; RTC/network already
stay CPU2. Alpha0, Honoka1, input3, USB2, framebuffer2 and all guest/worker
round-robin assignments retain their baseline placement. The process fixture's
PID57 and Alter are both CPU2. This is an editable policy, not a claim that CPU2
is optimal on every topology/workload. Removing the declaration restores the
old policy, with no protocol or binary interface migration.

Measure the final configuration in changed/normal/normal/changed order, each
with process, actual startup and UNIX/stat workloads. Twelve sequential QEMU
boots, no overlapping own build/test/VM, fixed inputs, cold samples retained.
All full footers pass with no checked fatal/proc/shm/fork errors. Six Linux
loop/startup samples per side; Negoto create has twelve. Pooled milliseconds:

| Workload | Baseline | Final preference | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 150.4590 | 131.0685 | -12.89% |
| missing stat, 512 calls | 111.4990 | 94.7340 | -15.04% |
| UNIX write/read, 2048 iterations | 376.1200 | 372.0910 | -1.07% |
| UNIX write/recvmsg | 748.9935 | 749.9485 | +0.13% |
| EOF recvmsg | 430.0945 | 427.6470 | -0.57% |
| getpid, 2048 calls | 39.9430 | 40.1305 | +0.47% |
| getuid, 2048 calls | 39.7835 | 39.8875 | +0.26% |
| fork/wait, eight children | 19.9195 | 19.2745 | -3.24% |
| fork/exec/wait, eight children | 708.1280 | 689.1240 | -2.68% |
| Native eight-image construction/reap | 25.4310 | 25.0605 | -1.46% |
| BusyBox input/construction/reap | 29.5615 | 27.7955 | -5.97% |
| Xfe input/construction/reap | 29.3500 | 29.0210 | -1.12% |
| Xfe input preparation | 10.7815 | 10.6870 | -0.88% |
| OCI start to X11-ready | 5201.9090 | 4991.4025 | -4.05% |

Other controls remain recorded: Negoto create -8.99%, normal/128/256-VMA
copyout +1.25%/+0.52%/-1.44%, fragmented fork -2.25%/-6.11%. Small contrary
substages are not erased: native kill/reap is 0.3980 -> 0.4235ms (+6.41%),
and Xfe source release 0.9160 -> 0.9755ms (+6.50%). Whole lifecycle controls
improve, but do not credit every constructor movement to placement or assert
precise timing equivalence. Native construction tests never resume their ELF;
they do not stand in for Linux execution. Actual startup samples in final/
baseline/baseline/final order, milliseconds:
4510.685/4993.307/5102.398; 4723.097/5208.478/5207.233;
4600.784/5196.773/5207.045; 4521.987/4989.498/5199.379.
Some sessions are near baseline. The pooled startup gain is about two 100ms
polling steps and remains below the whole-startup 5% aim; do not round it up.

Decision: retain the generic preference mechanism, one shipped priority21
declaration, five tests and documentation. The two-class storage regression
and cycle56 syscall regression are not adopted. The unchanged Kernel hash
proves no Kernel fastpath modification, not universal performance equivalence
on untested hardware. The broader optimization goal remains active.

Final functional checks pass: one-CPU process/fork/exec/cache-freshness full
footer (serial suffix _434dxta) and two four-CPU GUI sessions (x82749yn),
including X11/FLWM/wbar, actual USB input, links, Terminal/Editor/Information
launch, independent markers, Exit and same-ID restart cleanup. The prior
two-class GUI log is separate (ko2ubkmo). Gates and paths are retained in
/private/tmp/nanami-opt-cycle57.BhxKMC/single-class-measurements.json;
two-class-measurements.json contains the rejected configuration and all
earlier probe records. Source snapshots and raw before/probe/policy ELFs are
in that directory. Archives: cycle57-broker-diagnostic.patch (probe),
cycle57-placement-candidate.patch (two-class candidate) and
cycle57-single-class-candidate.patch (final source/tests/boot docs), under
/private/tmp. Temporary source selectors are removed, not merely disabled.

Final performance image: /private/tmp/nanami-opt-cycle57-normal.img, renamed
only after all users close it; SHA-256
a82a5937cf0639cf52006a384e95291589fb47ed9fa29bb626c83cfbb4a28fa4.
The filename in earlier comparison logs remains cycle57-brokers.img. This is
the frozen regression/performance fixture image, not a claim that standard
out/ user images were rebuilt. Preserve the exact older cycle51 image via
gzip -1 -k, gzip -t and streamed decompression SHA-256 verification:
cf242fffc391bd874a8ac42e8b8085287c188f9306e55aea33d68e69ab52d16b.
After a closed-handle check remove only its uncompressed generated file;
/private/tmp/nanami-opt-cycle51-vector.img.gz remains lossless and recoverable.
This recovers working space without removing source, fixtures or measurements.
No version, commit, tag, release, Kernel change or FPU work is made.

## Cycle 58: bounded namespace-root binding hints (rejected)

Keep the accepted cycle57 placement and all Kernel sources unchanged. Inspect
the repeated absolute root lookup in ext2 namespace requests, following the
measured cycle55 root-lookup/inode interval. This interval includes waits and
does not predict the benefit of removing only the binding walk.

Implement and test four exact positive root-path/inode hints, with 256-byte keys
and 1,072 bytes total storage on the tested 64-bit targets. No permissions,
negative lookup, inode pin, new RPC or protocol limit is cached. Slash-only
roots already resolve immediately; longer valid roots bypass the cache. Every
hit still checks current inode kind and I/O errors. Use the existing owned
root-input snapshot for both key and lookup, rather than rereading peer SHM.
Move the legacy non-following absolute walker into a shared internal module
and check offset/span/address addition before dereferencing its SHM input.

Invalidate all hints before metadata write attempts, including rejected writes
and partial failures. Existing name-cache stores and named/inode invalidations
also clear them, so a root hint cannot outlive an underlying refreshed binding.
Data-only regular-file writes do not invalidate roots. Audit regular-file-only
write/truncate/mapping entry points and directory, inode, allocation, rename,
delete and symlink metadata updates before relying on this distinction.

Seven candidate tests exercise the actual hint/resolver modules: exact/FIFO
keys and bounded storage, hits with fresh kind/I/O checks, owned-input versus
peer rewrite, missing/relative/slash/long roots, final/intermediate symlinks,
SHM arithmetic overflow and 12,000 deterministic mutation/type/inode-reuse
operations against an uncached oracle. Extend actual block-I/O tests with hint
invalidation and add one rejected-metadata-write regression. Candidate gates:
1,175 native tests passed / 16 ignored; root/storage/directory/namespace ASan
56 passed; AArch64 ext2 Release compile check passed, not an ARM boot claim.

First normal candidate/baseline comparisons give stat about -3.2% and startup
about -0.1%, insufficient to justify adoption. Separately insert a temporary
usage counter and run three actual startup/cleanup/same-ID-reuse sessions.
The last complete diagnostic prefix covers 2,048 requests with 1,478 hits
(72.17%); no reported slash-only or over-256-byte root occurs. Exclude the
unreported tail and do not treat hit rate as elapsed/CPU savings. All three
sessions pass. Probe ELF SHA-256: 99b806eb...bf41a; its timing is not pooled
with normal measurements. Remove the entire source probe and verify exact
normal candidate ELF restoration before the second comparison order.

Eight normal QEMU boots use candidate/baseline/baseline/candidate order, each
with sequential UNIX/stat and three-session startup runs. Four CPUs, normal
devices, frozen cycle47 guest/observer/rootfs, no overlapping own build/test/VM
during timings. Only the ext2 core payload changes; xtask excludes core apps
from rootfs, so reuse does not package stale non-core changes. Candidate ext2
SHA-256: 0fb69ec5a44cfd5bb3126af3f2fdb82659b120051e55392bcf19757c9b37fd5f.
All eight full workload footers pass; no checked fatal/proc/SHM/fork error or
profiling marker appears. Six samples per variant, medians in milliseconds:

| Normal workload | Cycle57 baseline | Root hints | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 134.1635 | 129.7170 | -3.31% |
| missing stat, 512 calls | 96.5010 | 94.0855 | -2.50% |
| UNIX write/read, 2048 iterations | 372.1035 | 372.2470 | +0.04% |
| UNIX write/recvmsg | 752.1625 | 747.6475 | -0.60% |
| EOF recvmsg | 429.1510 | 431.4380 | +0.53% |
| OCI start to X11-ready | 4982.6235 | 5035.3885 | +1.06% |

Keep every sample, including candidate stat's 194.444ms first sample in its
second boot. Actual startup samples in comparison order, milliseconds:
4628.206/4979.001/5091.776; 4499.565/4986.064/5092.937;
4604.310/4979.183/5191.814; 4627.008/5101.342/5098.644.
The 100ms observer resolution and session variation prevent interpreting the
52.765ms pooled startup increase as a precise CPU regression; it also provides
no evidence of a startup improvement. This is a rejection screen, not a full
process-performance or physical-hardware acceptance claim.

Decision: reject the entire production/test-module candidate. The small stat
benefit and neutral-to-slower startup do not earn its persistent state and
invalidation work. Restore all six original files byte-for-byte and remove
the three candidate-only modules/target. The full source/test archive at
/private/tmp/nanami-opt-cycle58-root-cache-candidate.patch passes forward-apply
checking against the restored source. Do not leave an unmeasured walker
refactor, cache gate or counter. Candidate source remains available for replay;
its snapshot-safety change is not claimed as a separately deployed fix.

Snapshot /private/tmp/nanami-opt-cycle58.zGWxFB contains originals, exact
before/candidate/profile ext2 ELFs, measurements.json and namespace-call-sites.txt.
The separate root-profile.patch archive under /private/tmp applies after the
candidate, not directly to restored mainline. Normal logs use
/private/tmp/nanami-opt-cycle58-{unix,startup}-{after1,before1,before2,after2}.log;
the profile uses root-profile-{build,startup}.log. All runner directories,
samples, validation and error checks remain in measurements.json. An initial
restoration-patch formatting attempt is rejected before any source change;
the corrected application and per-file byte comparisons all pass.

Rebuild the restored image and confirm all eight runtime/Kernel hashes match
cycle57, including ext2 28dcd199...710, Alpha d909436b...8a5 and Kernel
312d579b...c88. A9N's worktree remains clean. Restored native tests pass
1,167 / 16 ignored; storage/directory/namespace ASan passes 48. Three additional
actual startup/cleanup/same-ID-reuse sessions pass at
4506.008/5098.648/5214.893ms, serial suffix pdiwiujr. These restoration samples
are not pooled into the comparison. Keep the authoritative cycle57-normal.img
unchanged; temporary profile/candidate/restoration images are removed only
after termination and closed-handle checks. They are rebuildable from retained
sources, exact ELFs and frozen inputs; no guest user data is removed.

Next source-evidenced lead, not an implemented speedup: Disk::lookup reads the
parent inode to check its kind, then reads the child's inode to return its kind.
On the next ordinary component, that same child is read again as the parent.
Read-only optimized-code inspection confirms both read_inode calls remain in
path_walk::resolve (normal addresses 0x100f522 and 0x100f598); read_inode searches
the 64-entry cache. Investigate request-scoped metadata reuse while retaining
dirfd/root containment, inode/type/error checks and symlink/dot-dot semantics.
Do not simply remove parent validation or introduce another persistent cache.
Also reject a naive removal of canonical stat paths: missing final inodes use
that path for Alter's resolver/hosts fallback. No Kernel/FPU/ABI/IPC-fastpath,
version, commit or release change is made, and the optimization goal stays active.

## Cycle 59: request-scoped inode-kind reuse (rejected)

Cycle58 is progress: it rules out persistent root-binding hints as a demonstrated
next large improvement and restores exact cycle57 runtime. Keep Kernel/FPU
unchanged. Follow the actual remaining parent/child read_inode call sites in
ext2's optimized path walker, rather than repeating a dentry index or a
persistent root cache.

Prototype a read-only Disk adapter scoped to one resolution. Its last verified
Node may supply the next ordinary component's parent kind. Every unknown
parent and every newly found child is still checked; no permissions, negative
lookup, binding, cross-request state, allocation or invalidation policy is
cached. The event loop dispatches one VFS request at a time, and resolution's
inode/link/block reads do not dispatch another request or mutate filesystem
inodes. Existing fresh root validation, dirfd ownership/ancestry and final
metadata reads stay in place. Extract the adapter into namespace/disk.rs for
production-source tests, not a new external interface.

Six candidate tests compile the real adapter and walker with modeled inode,
name, link and block I/O. Cover exact work counts, bounded stack state, fresh
type/error checks after request boundaries and inode reuse, validation/error
order, non-directory parents even with positive cached names, uncached missing
queries, dirfd rename/ancestry/outside-root denial, malformed directory and
block errors, symlinks/dot-dot/trailing slashes/40-link loops, and 10,000 mixed
rename/link/deletion/reuse operations against a separately implemented
uncached oracle. The first focused run finds a test-model mismatch: a physical
root without a non-dot parent/name fails with Missing in the existing ext2
adapter, before the generic walker's self-parent Invalid check. Correct the
expectation, not the production denial behavior; preserve the failed log.

The first prototype starts with no known Node, reuses parent and child kind
lookups through one helper, and needs at most 24 stack bytes. A three-component
walk reads inodes 2,3,4,5 rather than 2,3,3,4,4,5 (separate root guard excluded).
Native 1,173 / 16 ignored, focused ASan 54 and AArch64 ext2 Release compile
checks pass. Actual first candidate/baseline UNIX/startup screens pass all four
complete footers but give stat 131.328 versus 132.563ms (-0.93%), missing stat
96.818 versus 94.298ms (+2.67%), and startup 4981.565 versus 5000.425ms (-0.38%).
Plain UNIX is +3.12%, not credited to the filesystem change. Do not adopt or
pool this provisional configuration with the later one. ELF SHA-256:
1fb0716269b749e99ede6af0a7196337640e2857ff4b8f86b135cd66eca84fe0.

Simplify the candidate: carry the handler's just-validated root Node into the
adapter, retain only one parent-kind branch, and always read a found child's
inode. No root check is moved or removed. The read-only adapter is now 16 bytes
on tested 64-bit targets; fresh root guard plus a three-component walk needs
four inode reads rather than seven. Both gate sets pass again (native 1,173 /
16 ignored, ASan 54, ARM compile). Codegen confirms the conditional parent read
and unconditional child read; lookup is outlined, 236 bytes, versus the old
inlined lookup. A smaller walker symbol or fewer reads alone cannot establish
a speedup. No compiler hints/extensions are added. Seeded ELF SHA-256:
28de793343143235cd4efba527a428beb334f6aae01e7ecf7df7bce4c0ca7143.

Run eight separate normal four-CPU, normal-device QEMU boots in
baseline/candidate/candidate/baseline order, UNIX/stat then three-session startup
per variant. Freeze existing cycle47 guests/observer/rootfs; only the ext2 core
payload changes. No own build/test/VM overlaps a timing run. All eight full
footers and twelve startup/cleanup/same-ID-reuse sessions pass; no checked
fatal/proc/SHM/fork error. All samples retained, medians in milliseconds:

| Normal workload | Cycle57 baseline | Seeded candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 134.6535 | 131.7155 | -2.18% |
| missing stat, 512 calls | 95.8060 | 95.5850 | -0.23% |
| UNIX write/read, 2048 iterations | 382.0340 | 374.6430 | -1.93% |
| UNIX write/recvmsg | 753.8070 | 746.7260 | -0.94% |
| EOF recvmsg | 447.1710 | 431.9400 | -3.41% |
| OCI start to X11-ready | 4984.1075 | 4984.9495 | +0.02% |

Do not attribute the UNIX control movements to this inode change. Last-baseline
EOF samples rise to 464.069/482.729/496.738ms; they are not excluded. The low
78.716ms missing-stat baseline is also retained. Startup samples, comparison
order: 4501.138/4983.340/5099.045; 4538.247/4980.747/5093.484;
4509.448/4989.152/5090.782; 4403.181/5083.169/4984.875ms. The 0.842ms pooled
startup difference is far below the 100ms polling interval. This screen does
not establish a startup, 5%, process-performance or hardware improvement.

Decision: reject both complete runtime/test-module candidates. Restore the
two original files byte-for-byte and remove both candidate-only modules and
the test target. No unmeasured adapter extraction or forced inlining remains.
Both archived patches pass forward-apply checking against restored source:
/private/tmp/nanami-opt-cycle59-inode-reuse-candidate.patch (unseeded) and
/private/tmp/nanami-opt-cycle59-seeded-candidate.patch (final candidate).
Snapshot /private/tmp/nanami-opt-cycle59.C2jbl0 contains original files,
before/unseeded/seeded ELFs, both measurements JSONs, codegen and final checks.
The prototype logs are cycle59-{unix,startup}-{after1,before1}.log; final normal
logs are cycle59-seeded-{unix,startup}-{before1,after1,after2,before2}.log, under
/private/tmp. Do not silently mix their variants or small contrary results.

Restore all eight cycle57 runtime/Kernel hashes, including ext2 28dcd199...710,
Alpha d909436b...8a5 and Kernel 312d579b...c88; A9N's worktree stays clean.
Restored native passes 1,167 / 16 ignored and storage/directory/namespace ASan
48. Three additional startup/cleanup/reuse sessions pass at
4823.156/5315.852/5195.089ms, serial suffix t8ifpq5s; do not pool these restoration
checks into comparison data. Preserve authoritative cycle57-normal.img and its
a82a5937...4a28fa4 hash. Remove only the terminated, closed-handle-checked owned
candidate/restoration images; exact sources, ELFs and logs remain to rebuild.
No guest user data, versions, commits, tags or releases are changed.

Next source review rules out two tempting but false premises: read_c_string
already copies page-bounded chunks directly into shared memory (starting at
128 bytes), and guest_root_for_process already borrows its slice, with no root
Vec allocation. LINUX_CWD_MAX is 128, not a 4KiB per-stat stack buffer. Do not
claim those copies/allocations can newly be removed.

A distinct concrete lead is result placement: namespace::request_path asks
POSIX to return data at OUTPUT (0xe000), then moves it to SHM offset zero.
resolve_at_with_metadata moves the pathname again over its metadata prefix,
even though successful stat uses only decoded metadata. POSIX already honors
PathRequest.output_offset. Investigate a direct existing-protocol destination
and a metadata-aware result view before considering a new direct-VFS channel.
Preserve descriptor/root/path/target/output capacity checks, aliased-input
lifetime, response-length/type/canonical validation and missing-leaf hosts/
resolver fallback. No such candidate is implemented or timed yet. This affects
non-core Alter binaries: future normal comparisons must rebuild rootfs, not
reuse stale Linux/FreeBSD ELFs. The broader optimization goal remains active.

## Cycle 60: direct namespace reply placement (rejected)

Treat cycle59 as progress: both inode-kind candidates were implemented,
tested, measured and rejected, with exact normal restoration. Follow its
distinct result-copy lead, keeping Kernel, FPU, wire opcodes and service graph
unchanged. Inspect all four request_path call sites: resolution snapshots input
to its stack, while readlink/symlink/chmod supply owned Vec/stack input before
the RPC. Do not add a direct VFS connection or bypass POSIX-owned dirfd checks.

The candidate uses the existing PathRequest.output_offset to receive at offset
zero, removing the OUTPUT(0xe000)-to-zero bounce. A private ResolvedPath view
keeps the canonical path after optional metadata rather than moving it over
that prefix. Namespace stat consumes its own resolved view; the former
metadata::stat_resolved_path body moves there, leaving virtual metadata and
serialization in metadata.rs. Missing-leaf hosts/resolver fallback uses the
retained path offset. No cache, extra RPC, retained buffer or new module is added.

Do not weaken the old OUTPUT capacity check into just a zero-offset output
check: it also protected descriptor/root/path/target workspace. The candidate
requires the full fixed input workspace through TARGET+4096, rejects null/
overflowing SHM spans, retains root/path/target/output length limits, and
asserts the descriptor fits before ROOT. Preserve reply count/type/canonical
checks, path-only NUL termination, readlink's no-NUL contract, virtual paths,
nofollow/dirfd behavior, real-file precedence and errors. The initial five
existing native tests pass. Six additional tests exercise actual namespace
stat consumption (rather than its old mocked callback), unchanged metadata
bytes, fallback after the prefix, workspace/absolute pointer bounds, exact
capacity/maximum targets, nonoutput replies, 4096-byte readlink and the
metadata+127-byte canonical path boundary. Fix a test-only Debug assertion
without changing VirtualNode. Candidate native: 1,173 passed / 16 ignored;
path/namespace ASan 18 passed; AArch64 Alter/Linux Release compile check passes.

Before timing, rebuild the non-core rootfs, not merely the boot payload.
Save the old generated rootfs with gzip -1, gzip -t and streamed hash checking:
d8688aa9677bd68deae66da868a64f8f2cfb602c3ed052675febf69697f6abdd.
Only then remove that closed generated file and explicitly rebuild. A read-only
binary ext2 comparison of all 3,890 paths finds exactly two changed ELF contents/
sizes, with names, inode numbers, mode/owner/link counts and timestamps unchanged:

- Linux: d6d0ef28...6e07 -> e304e4dbba6cdc154e56ce0e13c196fd78b6acc4422bb95e582f21e4df103be5.
- FreeBSD: 89420d9f...7b9a -> bcf758f3a1dae8f249a3f555a12d16e614ba609620bbc018e918fe8fdea1498b.

Debugfs also confirms both installed ELFs match their built files. New rootfs
SHA-256 is 04b71fac0d701912447e7d504fd4257fcf7f4091b72fbbc0d538f6488480ea76.
Alpha, POSIX, ext2, HPET, Honoka and Kernel remain exact normal bytes. Candidate
image /private/tmp/nanami-opt-cycle60-direct-reply.img has SHA-256
dafcdfa68f8f679429372c293e5580257d6fc19668607ffa5e3e23d0f391f1e0.

An attempted candidate-rootfs gzip fails after 12MiB because the host filesystem
will not allocate more space. Preserve that failure. Inspect the image builder
and actual GPT entry: the complete 256MiB root partition starts at 65MiB.
Stream-hash that embedded partition and confirm it matches the new rootfs
before deleting only the failed partial gzip and duplicate generated raw
rootfs. Compress the verified image partition directly; gzip -t and decompressed
hash now both pass. Old/new rootfs archives remain in the cycle snapshot. No
guest user data or source is discarded, and the authoritative normal image is
untouched. This is disk recovery before comparisons, not during a timing run.

Run twelve sequential normal QEMU boots in candidate/baseline/baseline/candidate
order, UNIX/stat, process and three-session startup per variant. Four CPUs,
normal devices, frozen cycle47 observer/guest/overlay inputs, no overlapping
own build/test/VM. All twelve complete workload footers and twelve startup/
cleanup/same-ID-reuse sessions pass, without checked fatal/proc/SHM/fork errors.
Keep all cold samples. Medians in milliseconds, six samples per loop/variant:

| Normal workload | Cycle57 baseline | Candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 128.9615 | 128.0090 | -0.74% |
| missing stat, 512 calls | 93.4380 | 94.0250 | +0.63% |
| UNIX write/read, 2048 iterations | 369.7410 | 366.5640 | -0.86% |
| UNIX write/recvmsg | 731.4535 | 733.8370 | +0.33% |
| EOF recvmsg | 423.8570 | 420.0860 | -0.89% |
| fork/wait, eight children | 18.5975 | 18.9905 | +2.11% |
| fork/exec/wait, eight children | 661.0300 | 665.1625 | +0.63% |
| Native eight-image construction/reap | 25.4670 | 23.3075 | -8.48% |
| BusyBox input/construction/source cleanup | 26.9085 | 28.2425 | +4.96% |
| Xfe input/construction/source cleanup | 27.7470 | 27.7070 | -0.14% |
| OCI start to X11-ready | 4834.3390 | 4848.0295 | +0.28% |

Do not credit the native suspended-ELF constructor movement to this stat-copy
change or call it Linux application startup. Negoto create is +5.43%; BusyBox
input preparation +9.89%, getpid/getuid +0.76%/+1.05%, normal copyout -0.05%,
128/256-VMA copyout +0.29%/+0.35%, fragmented fork -4.05%/+1.08%. Preserve the
contrary observations and do not infer exclusive CPU or IPC overhead. Actual
startup samples in comparison order are
4411.440/4795.472/4900.587; 4410.903/4783.620/4893.390;
4417.382/4885.058/4897.806; 4476.963/4910.098/4988.230ms. The 13.6905ms pooled
startup difference is below the 100ms observer interval, not a useful speedup.

Read-only optimized-code inspection proves both old memmove call sites exist:
request_path 0x106af2e and resolve_at_with_metadata 0x106b901. Neither corresponding
candidate function calls memmove; required input memcpy calls remain. This is
real copy elimination, but not proof of a 5% or startup improvement. Decision:
reject the runtime candidate, restore both production files byte-for-byte and
restore old Linux/FreeBSD executables and rootfs exactly. Retain only three
baseline-compatible native regressions for requested readlink output length/
following-byte preservation, invalid request sizes before writes/nonoutput
reply semantics, and canonical limit/malformed metadata type checks. Improve
the fixture's nonoutput reply and borrowed-root behavior to match production;
the restored metadata callback remains mocked, so these retained tests are
not claimed as complete fallback-consumer coverage.

Full source/test candidate: /private/tmp/nanami-opt-cycle60-direct-reply-candidate.patch.
The production-only archive, direct-reply-production.patch with the same prefix,
passes forward-apply checking after test evolution. The full candidate needs
the saved before-test snapshot, not blind application over retained tests.
The separate retained-tests.patch reverse check passes. Snapshot
/private/tmp/nanami-opt-cycle60.hhbuqR contains originals, before/candidate
Linux/FreeBSD ELFs, both verified rootfs archives, read-only rootfs comparator
and result, actual copy call sites, measurements.json and final checks. Normal
logs: /private/tmp/nanami-opt-cycle60-{unix,process,startup}-{after1,before1,before2,after2}.log.

Restore the rootfs's original host mode0600 as well as its exact d8688aa9...bdd
contents. Rebuild normal services with xtask servers, without making another
large boot image: the unchanged authoritative cycle57-normal.img already
contains these exact normal binaries/rootfs. Its a82a5937...4a28fa4 hash still
matches. Restored native passes 1,170 / 16 ignored; path/namespace ASan 15.
Three further startup/cleanup/reuse sessions pass at
4302.462/4776.657/4882.527ms, serial suffix 7lqyuz1p, not pooled with comparisons.
No new runtime layout, cache, probe, Kernel/FPU/ABI/fastpath/version/commit or
release change is retained. The broader optimization goal stays active.

Next target selection should leave the small stat-only experiments behind.
The earlier cycle50 diagnostic, not a newly measured share, attributes 44.60%
of handler elapsed ticks to mmap and 17.18% to clone. Current source inspection
confirms loader::file_mapping_source bypasses cache admission above 2MiB and
for non-whole-file misses; file_mapping::initialize then streams POSIX reads
and Alpha initialization requests. Measure bypass reasons, mapped/read bytes,
chunk counts and actual initialization work before proposing a region cache or
larger read window. Keep the 16MiB snapshot budget, main/interpreter protection,
version freshness and private guest copies; do not revive the rejected simple
4MiB ceiling, read-only fork sharing or gap-search index without new evidence.
The streaming helper also declares a zero-filled 4096-byte tail buffer even
for full-page reads. Check actual emitted initialization and source-prefix/
destination-zero invariants before considering lazy raw tail storage; first
measure how often this fallback executes. No tail-buffer candidate is deployed.

## Cycle 61: file-mmap source and initialization diagnostic

Keep the user's Kernel-not-changed boundary. Follow cycle60's mmap lead with
measurement before adding state or changing private-memory ownership. Inspect
the actual loader, file-mmap and streaming helper; preserve the 16-MiB snapshot
budget, 2-MiB per-file ceiling, versioned identity checks, fresh fstat and
outstanding main/interpreter protection. Anonymous, shared and virtual-device
mmap paths are not this source-selection workload.

Temporarily extend NANAMI_ALTER_PROFILE counters, without per-page UART:
source hit/fill/bypass reasons and requested bytes; snapshot versus streaming
initialization requests and mapped/source/zero-only bytes; stream reads,
returned bytes, tail staging and completion. Wrap old operations with the same
arguments and errors. Same-actor local elapsed ranges include service waits,
not exclusive CPU or pure IPC time. Teardown flushes reset atomic aggregates;
trigger PID is not counter ownership. Teardown UART can affect handler timing.

Build x86_64 Release with probes and explicitly rebuild the non-core rootfs.
Compare all 3,890 paths against normal: only Linux/FreeBSD ELF contents/sizes
change, with inode IDs, names, modes, owners, links and timestamps preserved.
Linux profile ELF is
3e55bc263b398825669cc3c4bbf9f4d74a7f691044c52eac8d0ea3af83cc8208;
FreeBSD is 5e0c920e67dadbda935bdd74b56725d4968057801f6c4bc67da7f8b3cb949fce.
Debugfs confirms installed Linux matches. Profile rootfs SHA-256 is
8deab49fb20e64f72936afa909c775d5c3af195e2be0c8e5d4cb2f5af3bd9fa3.
Alpha/Kernel stay exact normal bytes. Preserve the complete probe at
/private/tmp/nanami-opt-cycle61-mmap-profile.patch; forward apply checking
passes after all five runtime files are restored byte-for-byte.

Run the frozen cycle47 fixture on four CPUs/normal devices: three complete
startup/kill/delete/same-ID-reuse sessions, runner exit0, full samples=3
cleanup=true footer, no checked fatal/proc/SHM/fork/panic errors. Instrumented
startup is 4730.588 / 5417.058 / 5397.820ms, not paired with normal samples and
not a speedup. Independently rescan the entire 146,267-byte raw serial log to
exclude truncated-output aggregation. Every source/init call has a timestamp;
all operations succeed.

| Source selection | Calls | Requested source bytes | Local elapsed ticks |
| --- | ---: | ---: | ---: |
| Snapshot hit | 1,624 | 354,500,568 | 2,965,000 |
| Whole-file snapshot load attempt | 135 | 39,693,276 | 404,477,000 |
| Above-ceiling bypass | 24 | 33,747,792 | 90,000 |

No unversioned, partial-file, empty-file or invalid-span source misses occur.
Cache hit rate is 91.08%; fill attempts are not necessarily retained admission.
Summed whole-file sizes are not storage-read bytes. The 1,759 hit/fill source
requests exactly match 1,759 snapshot initialization requests.

| Initialization requests | Calls | Mapped bytes | Source bytes | Local elapsed ticks |
| --- | ---: | ---: | ---: | ---: |
| Snapshot-backed | 1,759 | 405,905,408 | 394,193,844 | 2,871,266,000 |
| Streaming chunks | 540 | 33,841,152 | 33,747,792 | 274,163,000 |

All 24 streams complete, none incomplete. Their 528 POSIX reads return exactly
33,747,792 requested bytes without errors in 193,133,000 local elapsed ticks.
Enclosing stream ranges total 467,990,000 ticks and include those read/init
ranges: do not double-count. Zero-source-only requests map 73,728 bytes.
Partial-page staging copies total just 4,944 bytes, 0.01465% of streamed bytes.
Even removing the 24 upfront 4-KiB tail zeroes saves only 98,304 bytes, not the
hundreds of MiB of destination copying. Do not add unsafe/lazy tail storage or
a region cache from this trace. No optimization candidate is deployed.

Snapshot-backed initialization is the largest measured request range here,
including Alpha waits, not proof of CPU time or total syscall share. Next
inspect current Alpha initialization/checked-copy and allocation substages
before another source-cache experiment. Preserve private bytes/raw-capability
authority; do not revive rejected read-only fork sharing, eight-way alias
caching, a larger snapshot ceiling or VM-gap indexing without new evidence.

Snapshot /private/tmp/nanami-opt-cycle61.c8dbxY preserves original sources,
before/profile ELFs, verified compressed profile rootfs, measurements.json and
rootfs-comparison.json. Raw serial is under
/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-71ehijg3/.
Build/native/ASan/restored-startup logs use /private/tmp/nanami-opt-cycle61-.

Disk pressure requires lossless image/rootfs archival, not unrelated cleanup.
gzip-test and streamed hashes precede removal of closed duplicate raw files.
After diagnostic VM exit, remove only its owned image; rootfs/probe/ELFs/logs
remain recoverable. Restore normal57 image a82a5937...4a28fa4 and normal rootfs
d8688aa9...bdd, including host mode0600. Verify complete raw hashes, then remove
only redundant normal-image gzip; exact raw image remains. Rootfs archives
from cycle60/61 remain. Rebuild normal services without probes. Linux
d6d0ef28...6e07, FreeBSD 89420d9f...7b9a and Kernel 312d579b...0c88 match originals;
debugfs verifies installed Linux too. Neither remaining image path is a gzip.

Restored native: 1,170 passed / 16 ignored. ASan: 33 passed across loader/cache,
streaming initializer and namespace/path targets. Three further normal
startup/cleanup/reuse sessions pass at 4606.310 / 5199.467 / 5085.627ms, serial
suffix mn1jpn62, without profiling rows or checked errors. These restoration
checks are not pooled with earlier timings. No new runtime/cache/ABI,
Kernel/FPU/fastpath/version/commit or release change remains. The broader
optimization goal stays active.


After confirming all VMs/builds are terminal, extract and compare the restored
image's init payload against the exact normal executable, then delete only
the owned `restored.img` test disk to recover space. Keep its packed init,
logs, accepted cycle93 image/rootfs and both immutable timing inputs. This is
regenerable test-artifact cleanup, not removal of a user disk or measured input.

## Cycle 62: bounded bootstrap page-table readiness history

Cycle61 is diagnostic progress, not a speedup: it confirms snapshot-backed
initialization dominates the measured mmap request ranges and rules out a
new region cache or lazy tail buffer from that trace. Re-read actual Alpha
construction, temporary mapping, VmTracker, allocation, prior rejected
experiments and both HAL unmap implementations before proposing this change.
Do not repeat capacity tuning of the Frame alias/data caches or remove source
validation. Kernel remains untouched by explicit user instruction.

A distinct source lead is BootstrapVmSpace's one-region table-readiness hint.
Alpha alternates between initialization and process-copy windows, so a miss
can issue get_unset_depth even though that region's table already exists.
Frame UNMAP in both x86_64 and AArch64 clears the leaf without removing its
ancestor tables. Alpha's tables are not reclaimed while Alpha remains live.
Ordinary VmSpace is separate and retains its exact one-region policy.

Temporarily replay an eight-distinct-region FIFO beside the real one-region
bootstrap policy, without changing actual mappings. Count lookups and hits,
update the model only after successful frame metadata insertion, and flush at
existing reap-profile points. No per-page UART/timestamps or Kernel changes.
Counters include bootstrap work before the first reap; trigger PID is not
ownership. The single-Alpha diagnostic model is not a normal latency result
or a claim of actual candidate query counts.

All three diagnostic startup/kill/delete/reuse sessions complete, cleanup=true,
without checked errors. Raw serial suffix wicco9m3; instrumented startup is
4707.321 / 5185.246 / 5292.049ms. Across 45 flush records, 257,735 lookups yield
135,491 actual last-region hits and 257,633 modeled eight-region hits. The
same-history misses would fall 122,244 -> 102, avoiding 122,142 depth checks.
This supports a normal candidate, not a claim of equivalent CPU time saved.
Profile Alpha SHA-256 is aed182b9bbf2b9c0171cee7f9070ce79d449b817738ac01a3c2742c57e1ebe23.
Archive /private/tmp/nanami-opt-cycle62-pt-diagnostic.patch and restore both
probe sources exactly before normal builds. Exec-profile source cmp passes;
no temporary diagnostic code/string remains in the accepted Alpha.

Candidate: retain eight distinct completed 2-MiB table regions in BootstrapVmSpace
with bounded FIFO replacement. Resident repeats do not advance eviction.
A miss follows the old kernel depth-query path. Only successful record_frame
can seed readiness: intermediate tables and failed inserts cannot. Forgetting
a Frame retains readiness because the table survives; tracker replacement
clears all hints. Descriptor/PTE/permission/copy/rollback rules remain intact.
The old one-Word hint becomes eight Words plus a cursor: +64 bytes per 64-bit
Alpha, allocation-free. No additional aliases, physical pages, guest metadata,
resource limit, wire operation, ABI or IPC fastpath is introduced.

Four new production-source regressions check exact metadata footprint, eight
interleaved regions with zero heap allocation, conservative eviction/repeats,
2-MiB boundaries, intermediate-table rejection, capacity-failed record_frame,
full-Word values, Frame-unmap retention, fresh-tracker reset and unchanged
guest policy. Bootstrap fixtures use a large explicit host stack. The first
full suite correctly finds a shared old test requiring Bootstrap to forget
all but one region; split that policy assertion from the unchanged common
lifetime/bounds tests, keeping ordinary-VM expectations exact. Preserve the
initial failure log rather than count that attempt as passing validation.

Final candidate checks: 1,174 native tests pass / 16 ignored; ASan 55 pass /
three ignored across vm-index, alpha-alter, physical-memory and private-page
construction. AArch64 Alpha Release compile check and x86_64 Release image
build pass. Normal Alpha is
e025b8a5161f6e4570f932e52d07b017fabd6d02d2b520ba984e0b8cf8d97a8d;
normal image /private/tmp/nanami-opt-cycle62-eight-regions.img is
7dc7cde28bda2f0e0808afcdcfcd2f8fe0f50dc602ef575d9b45c3d4fff572af.
Alter/Linux d6d0ef28...6e07, FreeBSD 89420d9f...7b9a, POSIX 864ff18c...5b54,
ext2 28dcd199...9710 and Kernel 312d579b...0c88 remain exact normal bytes.
The existing rootfs is not rebuilt for this Alpha-only change; its complete
and embedded partition hashes both equal d8688aa9...bdd. Old normal57 image
remains a82a5937...4a28fa4. No rootfs-content comparison is inferred merely
from source names or timestamps.

Run twelve sequential normal QEMU boots, before/after/after/before, with
UNIX/stat, process and three-session startup per variant; four CPUs, normal
devices and frozen cycle47 inputs. The first screen is mixed: native
construction/reap -9.33% and full BusyBox lifecycle -11.59%, but startup +6.31%,
fork +8.81% and UNIX write/read +11.76%. After2 recvmsg has a 2576.551ms boot
median versus usual roughly 740ms. Do not drop this boot or adopt on the good
numbers. Predeclare one additional before/after set of all three workloads,
not repeated sampling until a favorable answer. All eighteen complete full
workload footers without checked fatal/proc/SHM/fork/panic errors or probes.
Own builds, tests and other VMs do not overlap timed runs. Host load is
5.34/5.99/6.71 before comparison and 6.40/7.01/6.94 before the extra pair;
unrelated host processes are not changed.

Pooled medians in milliseconds, nine samples per variant unless noted:

| Normal workload | Cycle57 | Eight-region history | Change |
| --- | ---: | ---: | ---: |
| Native Alpha: eight suspended constructions/reaps | 27.391 | 25.207 | -7.97% |
| BusyBox input/construction/source cleanup | 31.807 | 28.041 | -11.84% |
| Xfe input/construction/source cleanup | 32.092 | 28.952 | -9.78% |
| Linux fork/wait, eight children | 17.603 | 18.351 | +4.25% |
| Linux fork/exec/wait, eight children | 701.310 | 694.582 | -0.96% |
| Fork, 128 additional VMAs | 19.195 | 18.517 | -3.53% |
| Fork, 256 additional VMAs | 17.816 | 18.108 | +1.64% |
| Normal checked copyout | 173.648 | 169.133 | -2.60% |
| getpid, 2048 calls | 39.738 | 39.253 | -1.22% |
| getuid, 2048 calls | 39.538 | 39.368 | -0.43% |
| stat, 512 calls | 135.532 | 136.653 | +0.83% |
| Missing stat, 512 calls | 93.966 | 94.625 | +0.70% |
| UNIX write/read, 2048 iterations | 369.457 | 384.974 | +4.20% |
| UNIX write/recvmsg | 740.352 | 745.979 | +0.76% |
| EOF recvmsg | 427.499 | 421.279 | -1.45% |
| Negoto create | 7.847 | 7.137 | -9.05% |
| OCI create/start to X11-ready | 4790.662 | 4880.129 | +1.87% |

Negoto-create has eighteen samples per variant; frame workloads have twelve
per variant/configuration. All raw samples, substages, per-boot medians and
capture controls remain in measurements-final.json. No weighted score or
outlier/cold-sample removal is used. Nine raw startup samples per variant,
in actual per-boot order:

- Before1: 4599.298 / 5179.708 / 5093.096ms.
- After1: 4606.020 / 5086.590 / 5086.968ms.
- After2: 4732.307 / 5103.307 / 5092.458ms.
- Before2: 4414.078 / 4779.272 / 4790.662ms.
- Before3: 4511.121 / 4986.670 / 4884.806ms.
- After3: 4403.021 / 4880.129 / 4780.945ms.

Native suspended construction/reap boot medians are before
27.806 / 27.044 / 26.015 versus after 25.576 / 24.470 / 25.207ms: all three
candidate medians are below every baseline median. This workload actually
uses the changed bootstrap mapping path; unlike cycle42's source-alias-only
candidate, its favorable result has a relevant changed path. Verify the
fixture: it never resumes the ELF and does not execute Alter/POSIX in that
timed region. Full BusyBox/Xfe cases include source input and cleanup but
also do not resume the programs. These are native construction improvements,
not Linux application startup or desktop FPS. Extra-pair BusyBox medians
27.799 -> 27.607 overlap the earlier candidate range; do not promise a
repeatable 12% improvement for every native full-input workload.

Tradeoffs remain: ordinary fork +4.25%, UNIX write/read +4.20%, 256-VMA fork
+1.64%, stat +0.83% and recvmsg +0.76%, including the complete slow after2 boot.
After3 recvmsg is 723.902ms, so the multi-second shift is not repeated there;
this neither identifies its cause nor erases it. Startup's +89.467ms pooled
shift is below the 100ms observer interval: no precise startup regression
or improvement is established. Do not label reduced depth queries as pure
IPC savings or claim the whole-system 5% target.

Retain this small user-level candidate for the repeated >5% native suspended
construction/reap improvement, with all contrary controls above, not as a
universal speedup. Four-core real GUI checking then passes two sessions with
fresh Terminal/Editor/Information launch, keyboard input, title dragging,
Exit and restart cleanup (serial ot7h4j71). One-core process/cache-freshness
checking also completes its final PROCESS-PERFORMANCE-PASS (serial 4sb1jkf1).
No checked errors or temporary profile rows appear in either check. This is
functional validation, not a new paired one-core performance comparison.

Snapshot /private/tmp/nanami-opt-cycle62.MJjEWi preserves originals,
before/profile/accepted Alpha ELFs, diagnostic.json, measurements.json for
the first twelve boots and measurements-final.json for all eighteen. The
full source/test patch is /private/tmp/nanami-opt-cycle62-eight-regions-candidate.patch;
reverse-apply checking passes against the retained sources. Logs use
/private/tmp/nanami-opt-cycle62-{unix,process,startup}-{before1,after1,after2,before2,before3,after3}.log.

Initial diagnostic image generation fails for disk space; preserve its log,
remove only the terminal/closed incomplete output, and gzip/hash-test the
normal57 baseline before removing its duplicate raw file. An early gzip-test
runs before compression completes and reports EOF; the completed archive
subsequently passes gzip -t and exact decompressed hash. Restore and verify
raw normal57 before all normal comparisons. Remove only the completed,
closed diagnostic image after its logs/Alpha/probe are saved. The authoritative
accepted image is now cycle62-eight-regions.img, not a profiled image.
After final checks, verify gzip -t, decompressed SHA-256 and closed handles
again, then remove only the superseded raw normal57 comparison image. Its
exact a82a5937...4a28fa4 bytes remain recoverable from
/private/tmp/nanami-opt-cycle57-normal.img.gz. Accepted raw62 and rootfs remain.
No Kernel/FPU/version/commit/tag/release change is made. The broader goal stays
active: native construction gains do not complete Tiny Core whole-system
optimization. Next isolate the remaining private-copy content work or explain
the fork/UNIX control shifts before extending readiness policy to ordinary
processes. Do not enlarge other caches or weaken private ownership on this result.

## Cycle 63: bounded UNIX payload reuse (rejected)

Cycle62 makes progress through a retained, measured Alpha construction change,
with contrary Linux fork/UNIX controls visible. Re-read current sources and
AGENTS.md; keep accepted Alpha e025b8a5...7a8d and Kernel 312d579b...0c88.
Do not infer that fast IPC itself causes the remaining cost. Inspect actual
guest-memory marshaling, fault dispatch, UNIX send/receive, socket ownership,
native mocks and prior experiments. Small headers already use fixed arrays;
the distinct candidate is fresh owned payload Vec allocation on every send.

Temporarily count sent and fully received packets in four length buckets:
<=64, <=512, <=4096 and larger. Aggregate count, bytes and capacity at existing
exit-profile flushes, without per-packet output or timestamps. Trigger PID is
not ownership; unflushed final work and packets discarded instead of received
are not represented as completed receives. Build actual non-core Alter ELFs
into a rebuilt rootfs, compare all 3,890 paths, and verify that only Linux/
FreeBSD contents/sizes change, not names, inode IDs, modes, owners or timestamps.

Three diagnostic startup/kill/delete/reuse sessions complete without checked
errors, with the final cleanup=true footer. Serial suffix 1wskh_z2. Sent bins:
1,238 / 38 / 25 / 46 packets, carrying 31,856 / 6,176 / 65,584 / 1,349,952 bytes.
Received bins are identical except one fewer 32-byte small packet. Thus 1,301
of 1,347 observed sends (96.59%) are <=4KiB by count, not by byte volume.
Profile Alpha remains exact e025b8a5...7a8d; Linux is 3986538f...72d7b,
FreeBSD 245edae6...22f9, rootfs fa60d0a2...a26. Archive the complete temporary
probe at /private/tmp/nanami-opt-cycle63-payload-diagnostic.patch and restore
both probe sources exactly before testing normal code. Diagnostic elapsed
startup 4616.617 / 4997.829 / 5290.942ms is not normal speed evidence.

Candidate: one spare Vec for the entire socket table, never one per socket.
Only fully delivered/consumed packets may donate storage, with reported Vec
capacity <=4096. Clear its visible length; take ownership for a fitting send,
otherwise retain the spare and use normal allocation. Keep only the larger
eligible spare. New payloads are still owned snapshots, populated through the
old checked copyin and published only after complete success. PEEK, partial
receive, output faults and rights-delivery failures never recycle live data.
Limits, queue byte accounting, FD ownership/GC, IPC and Kernel remain unchanged.
The cost would be one Vec field plus retained data capacity up to4KiB across
contexts; allocator overhead is not included in that capacity bound.

The same production-path 256-round native test first fails as expected on old
code: 256 allocations, largest8 bytes. With the candidate it reports zero
allocations, while retaining exact data snapshots, following-byte preservation
and copy-request counts. Four candidate tests check the bounded single spare,
in-flight independence, PEEK/fault/partial receive, stale-length rejection and
the no-allocation warm loop. Candidate native1,178/16ignored, UNIX ASan30,
AArch64 Alter/Linux Release check and normal x86_64 image build pass. The
candidate-only warm input-fault extension is not counted as retained cold-path
coverage: restore the original test when withdrawing the candidate below.

Normal candidate Linux SHA-256 is
c11267c9a4ea1ee678cd7048e9302ed7e0d2044b5c1e76f4f23ebbf20a5362cc;
FreeBSD c068649ef0f95eb479be5fcee96b77b7aca6b51d51cfc9112bacad9f75c6ae3d.
Rootfs is 40e86e885aaf125b832b334db62376babd53fab4479e2a882ddbb9578e0374e5.
Again compare all 3,890 paths: only these two ELF contents/sizes change. Alpha,
Kernel and the frozen cycle47 inputs remain identical to accepted cycle62.

Run twelve sequential normal boots, UNIX/process/startup in before/after/
after/before order, SMP4/normal devices, with no own concurrent build/test/VM.
All runners exit0, but elapsed results are badly contaminated: startup ranges
from 4324.742 to 912437.511ms. Native construction/getpid controls also shift
by roughly50-70%. Inspect macOS power events, not just wall-time guesses:
Sleep/DarkWake cycles overlap the batch, including 15:15:25 Sleep for914s,
15:30:41 Sleep for539s and lid-open Wake at15:39:40 +0900. Preserve all original
logs and initial-measurements.json, but classify this entire batch as functional
checking, not comparative speed evidence. Do not cherry-pick the apparent
29.52% startup improvement or call its slower controls candidate regressions.
No host power setting, sleep assertion or unrelated process is changed.

After actual wake, run a short, separately labeled UNIX before/after/after/
before comparison from06:42:04 to06:44:34 UTC. The macOS event query finds no
Sleep/DarkWake/Wake transition after15:42:00 +0900 through the completed audit.
All four full DESKTOP-PERFORMANCE-PASS footers and error checks pass. Preserve
all six cold/warm samples per variant. Pooled medians, milliseconds:

| Normal awake workload | Cycle62 | Payload reuse | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 132.2220 | 131.3825 | -0.63% |
| Missing stat, 512 calls | 97.0100 | 96.1030 | -0.93% |
| UNIX write/read, 2048 iterations | 385.6670 | 372.8055 | -3.33% |
| UNIX write/recvmsg | 754.3870 | 755.0630 | +0.09% |
| EOF recvmsg | 435.9245 | 419.8295 | -3.69% |

The first pair alone suggests write/read -6.06%; retain it, but do not select
that pair over the reversed-order aggregate. EOF also moves despite not
allocating/consuming a payload in the changed path. Work-count elimination is
real, but these observations do not establish a 5% communication or startup
gain. Reject the new retained state/data budget and withdraw both production
files. Preserve capacity/source ownership and current accepted62 runtime.

Reverse the archived candidate hunks with apply_patch, preserving unrelated
work. Both runtime files and the original syscall profiler compare byte-for-
byte with the before snapshots. Keep only two baseline-compatible semantic
regressions: independent queued bytes across PEEK/output failure/partial reads
and another endpoint, and exact short-packet length/following bytes after prior
transfers. Restore the original cold input-fault test and allocator fixture;
no candidate-specific allocation assertions, spare methods or state remain.

Snapshot /private/tmp/nanami-opt-cycle63.Yinsz4 preserves originals, before/
profile/candidate ELFs, both verified profile/candidate rootfs gzip archives,
all-path comparison JSON, histogram, initial and awake measurements, and
host-power-events.txt. Raw normal comparison logs use cycle63-{unix,process,
startup}-{before1,after1,after2,before2}; awake logs use unix-awake-{before,
after,after2,before2}. Full source/test archive is
/private/tmp/nanami-opt-cycle63-payload-reuse-candidate.patch and requires the
saved before-test snapshot. The separate production-only patch passes forward
checking after retained-test evolution; retained-tests.patch reverse checking
passes. Both have the same cycle63 prefix.

Remove only closed diagnostic/rejected generated images, retaining reproducible
ELFs/rootfs archives/probes/logs. Normal62 raw image stays intact. Restore the
exact normal rootfs d8688aa9...bdd from cycle60's verified archive and host
mode0600, then rebuild normal services. Linux d6d0ef28...6e07, FreeBSD
89420d9f...7b9a and Alpha e025b8a5...7a8d match their accepted hashes. Restored
native1,176/16ignored and UNIX ASan28 pass. Fresh startup/cleanup/reuse validation
completes three sessions at5219.045 / 5407.163 / 5705.237ms, serial iwdvijla;
restoration checks are not pooled as speed evidence. Kernel/FPU/ABI/fastpath,
versions, commits and release state are unchanged. The broader goal stays active.

## Cycle 64: borrowed stop checks and readiness writeback (rejected)

Audit the production blocking/timer paths, not the Kernel. A loop already
holding a ManagedProcess asks Runtime::is_signal_stopped(pid), which searches
the task table again through its validated single-slot hint. Also, readiness
wakeup copies the entire 616-byte ReadinessWait back before scanning, although
PTY update_deadline modifies only its deadline. The stored deadline is needed:
PTY retry/read consult it for VMIN/VTIME. Do not remove the stored record during
retry or weaken stopped-task behavior merely to avoid a copy.

Candidate: use the existing borrowed signal_stopped getter in clock/readiness
loops, check cheap sleep/source eligibility first, and publish only the changed
deadline before scanning. Keep the eventual full writeback for an unready wait
because epoll may discover changed notification sources. No new cache, retained
bytes, heap allocation, service operation, permission change or Kernel change.
Both affected production files are shared by Alter/Linux and Alter/FreeBSD.

Verify actual optimized normal Linux code. Baseline wake_readiness_waiters
calls Runtime::is_signal_stopped at 0x1005219; its two pre-scan memcpy sites at
0x1005276/0x10052dd each copy 0x268 bytes. Candidate has no stop-lookup call in
this function and only one pre-scan copy; deadline publication is field-sized.
The final unready writeback remains. Do not infer a whole-program speedup or
total code-size reduction from these isolated instruction sites. There is no
temporary production profiler in this cycle.

Four regressions check stopped sleep/readiness deadline exclusion, stopped
nanosleep staying parked until continue, stopped ready pipe I/O staying parked
until continue, and PTY deadline visibility before scan while preserving the
interest/context snapshot. Fix the clock/readiness harnesses' old always-false
stop stubs to read modeled task state. The PTY visibility fixture models that
boundary, not the full line discipline; run the separate production PTY suite
as well. All 1,180 native tests (16 ignored), 69 clocks/readiness/PTY ASan tests,
the AArch64 Alter/Linux Release check and normal x86_64 image build pass.

Candidate Linux SHA-256:
f8cc1d6bd09717bc27f69708c982fd291bcee6892189317e7afa36ace43d3371.
FreeBSD a67d1b2c023170270fcba04340012da16254cc32c7a31c8103aacbd7afa58dd7;
rootfs 8736bb3b4ce90300af7dd1087a4b1ea59d7d1868cf82f4afc51ac0eb20879079.
Rebuild non-core rootfs rather than reusing old executables. Compare all 3,890
paths with cycle62: only the intended two Alter ELF contents/sizes change,
with other inode metadata unchanged. Alpha e025b8a5...7a8d and Kernel
312d579b...0c88 stay identical.

Run twelve sequential normal SMP4 boots in before/after/after/before order,
each with UNIX, process and three-session startup fixtures. All complete their
full functional footers without proc.err/shm.err/fork failure/panic/fatal fault.
But the after1 startup spans 07:09:36..07:29:37 UTC. Verify host power events:
16:10:18 Sleep, 16:23:59 DarkWake, 16:24:39 Sleep, 16:29:30 Wake +0900. Preserve
the entire first batch as functional checking, not speed evidence. No power
setting, sleep assertion or unrelated host process is changed.

Run a separate short UNIX ABBA comparison 07:33:11..07:34:58 UTC. The completed
07:35:08 audit finds no Sleep/DarkWake/Wake since 16:33:00 +0900. All four full
footers/error checks pass. Pooled medians of six samples per variant, ms:

| Normal awake workload | Cycle62 | Candidate | Change |
| --- | ---: | ---: | ---: |
| stat, 512 calls | 135.0620 | 141.4120 | +4.70% |
| Missing stat, 512 calls | 95.9755 | 101.6960 | +5.96% |
| UNIX write/read, 2048 iterations | 373.2735 | 387.2805 | +3.75% |
| UNIX write/recvmsg | 747.4250 | 744.3600 | -0.41% |
| EOF recvmsg | 434.2725 | 497.5985 | +14.58% |

The first candidate boot is slow in several unchanged controls too; preserve
its samples, and do not claim every slowdown is caused by the candidate.
Neither the aggregate nor the unchanged-control behavior establishes a 5%
gain. Reject both runtime changes, not just the less favorable samples.

Reverse only this cycle's production hunks with apply_patch. Both files match
their saved before bytes exactly; production-only patch forward checking
passes. Keep the four baseline-compatible semantic tests, remove experimental
lookup counters/assertions and rename tests to describe behavior rather than
the withdrawn implementation. All 1,180 native tests (16 ignored) and 69 ASan tests pass
again on the restored code. Normal services rebuild to exact Linux d6d0ef28...
6e07 / FreeBSD 89420d9f...7b9a; rootfs is exact d8688aa9...bdd, mode 0600.
Fresh restored startup/cleanup/same-ID reuse succeeds three times at 4810.111 /
5099.657 / 4789.001ms, serial 0fbt5pt9, with the complete cleanup=true footer
and no checked errors. These restoration checks are not comparative timings.

Snapshot /private/tmp/nanami-opt-cycle64.ZhxeYC preserves before files, before/
candidate ELFs, verified candidate rootfs gzip, initial/awake measurements,
all-path comparison and host-power evidence. Full candidate, production-only
and retained-tests patches use /private/tmp/nanami-opt-cycle64-*; the full
candidate test patch requires its saved before files. Retained-tests reverse
checking passes. Raw comparison logs use cycle64-{unix,process,startup}-
{before1,after1,after2,before2}; the independent short logs use unix-awake-
{before,after,after2,before2}. Remove only the closed rejected raw image after
verifying its archived rootfs; retain the accepted normal62 image and rootfs.

Next source lead, not a fix or measured cost claim: blocked I/O wakeup scans
FD readiness, checks PTY retry, then checks UNIX/pipe retry, potentially looking
up the same FD/resource again. Audit the separate poll/select, epoll, PTY,
UNIX and pipe contracts before considering a direct retry path. EOF, shared-FD
changes, EAGAIN, VMIN/VTIME, signals and failed copyout must remain transactional.
Do not extrapolate this lead to a 5% gain or revive the rejected copy candidate.
Kernel/FPU/ABI/IPC fastpath, versions and release/commit state remain unchanged;
the broader optimization goal remains active without a whole-system 5% claim.

## Cycle 65: readiness wakeup and provider diagnostic

The preceding turn makes progress: reject an ineffective candidate, restore
normal bytes and retain four semantic regressions. Revalidate current sources,
accepted cycle62 image and active no-Kernel boundary. No live VM/build is
inherited. Re-read the existing cycle49 investigation before acting on an
apparent universal UNIX dispatch FD lookup: it has already been tested and
rejected. Do not recreate that decoder without new supporting evidence.

Audit a different lead: wake_readiness_waiters scans FD readiness, then tries
PTY/UNIX/pipe operations. A direct retry might avoid duplicate checks, but an
irrelevant notification could then re-read guest iovecs/metadata even when no
data is available. First establish whether actual desktop startup uses this
path. Do not change blocking/FD/PTY contracts based only on source appearance.

Use the existing opt-in NANAMI_ALTER_PROFILE facility for temporary bounded
aggregate counters, not per-wakeup logging. Count wake calls/table slots/wait
records, scans/FD entries/ready results/errors by original syscall, and actual
PTY/UNIX/pipe retry attempts. Flush with the existing exit-triggered aggregates;
the trigger PID is not ownership of all accumulated samples. There are no new
elapsed timing probes, guest-memory accesses, service calls, timer settings or
Kernel changes. Instrumentation changes execution/logging costs: it is not a
normal speed comparison or exclusive CPU-time estimate.

Phase A, SMP4/normal devices: three real create/start/X11-ready/kill/delete/
same-ID-reuse sessions complete, final cleanup=true and no checked errors.
Flushed totals are 4,076 wake calls, 65,216 task-slot visits, 5,459 existing wait
records and 4,437 actual scans. All scan rows are poll(7) or pselect6(270):

| Syscall | Scans | FD entries | At least one ready | Errors | I/O retries |
| --- | ---: | ---: | ---: | ---: | ---: |
| poll | 4,135 | 4,135 | 611 | 0 | 0 |
| pselect6 | 302 | 1,472 | 225 | 0 | 0 |

There are no observed parked read/write/recv/accept scans or PTY/UNIX/pipe
retry attempts. Poll scans one FD; pselect averages 4.87 FDs, not a large
64-entry interest set. 81.16% of scans are unready. These observations rule out
the proposed direct-I/O retry specialization as a justified startup target in
this fixture; they do not prove those paths are unused by all applications.

Phase B adds bounded counters for wake-source masks and the freshly recomputed
interest-source union, with no additional descriptor lookup. These are separate
aggregate distributions, not paired source/target events. All masks in this
run fit the nine-provider domain; summed mask counts match the overall totals.
Another three startup/cleanup/reuse sessions pass, with 4,056 wakes, 64,896
slot visits, 5,245 existing waits and 4,273 scans. Poll:3,990 scans/entries,
601 ready; pselect: 283 scans, 1,413 entries, 222 ready. Again no errors or actual
I/O retries. Do not compare counter differences between these separately
instrumented runs as a performance improvement.

Wake masks: input (1)=51, pipe (8)=607, timer (16)=422, object (32)=239,
pipe|object (40)=6, UNIX (64)=2,709, PTY (256)=20, UNIX|PTY (320)=2.
Pure UNIX accounts for 66.79% of wake calls. Fresh interest masks:

| Interest provider union | Scans | FD entries | At least one ready |
| --- | ---: | ---: | ---: |
| Input (1) | 3 | 3 | 0 |
| UNIX (64) | 4,019 | 4,019 | 602 |
| UNIX + input (65) | 228 | 1,335 | 221 |
| UNIX + PTY (320) | 23 | 46 | 0 |

UNIX-only sets account for 94.06% of scans; 85.02% of these are unready.
Overall 80.74% are unready. Slot visits are a count, not proof that an active
waiter bitmap/index would reduce startup by 5%; the current table has only 16
slots. Avoid adding persistent indexing/invalidation complexity from counts
alone. Likewise, none of these values measures pure A9N IPC transport cost.

New source lead: unix_message::send marks READY_UNIX after every successful
enqueue, including an already nonempty/readable recipient queue. Enqueueing
more data there cannot make a blocked read newly readable. A future candidate
can consider notifying only an empty-to-nonempty packet-queue transition.
Use packets.is_empty, not queued_bytes==0: zero-length datagrams/seqpackets
are real readable records. Preserve checked copyin, complete rights duplication,
publication/rollback and all existing send errors. Current epoll explicitly
rejects edge/exclusive requests; preserve that behavior and level/one-shot
rearming rather than claiming edge support. Audit shared descriptors and live
poll interests too. This candidate is not implemented or credited as a gain.

Receive-side notification must remain: freeing bytes, a packet slot or queued
rights can unblock a writer. A record writer may require an entire message or
more rights capacity even while POLLOUT is already true, so a simplistic
full-to-not-full predicate could strand it. Connect/backlog, shutdown, option,
GC and descriptor/resource transitions retain their own wakeups. Do not turn
this finding into a broad transition-only notification policy without proofs.

Both diagnostic builds regenerate non-core rootfs from the same frozen cycle47
fixture and compare all 3,890 paths: only the two intended Alter ELFs differ.
Phase A Linux 8f57e684...7943, FreeBSD 8ab82e5b...cc47, rootfs c19fcac8...10e4;
phase B Linux 43608625...93ef, FreeBSD 10993863...408c, rootfs 1cda3237...4b5f.
Full hashes and exact comparisons are preserved in the snapshot below. Alpha
e025b8a5...7a8d and Kernel 312d579b...0c88 stay identical. Diagnostic startup
numbers in runner logs are not normal comparative speed evidence.

Snapshot /private/tmp/nanami-opt-cycle65.JGrZqv preserves original production
files, normal and both diagnostic ELF pairs, both verified compressed rootfs
archives, readiness-phase-{a,b}.json and all-path comparison JSON. Complete
temporary patches are /private/tmp/nanami-opt-cycle65-readiness-diagnostic.patch
and /private/tmp/nanami-opt-cycle65-readiness-sources-diagnostic.patch. Logs use
cycle65-{profile,sources-profile}-{build,startup}; serial suffixes 4e_r2tos and
8wsr2xcz preserve all flushes and final functional footers.

Disk headroom requires removing only closed owned diagnostic raw images and
recreating generated rootfs from its verified archive. Check exact paths,
backups/hashes and lsof before deletion; never delete user inputs or the accepted
normal62 image. Both diagnostic rootfs archives remain recoverable. After
measurements, reverse only the second probe's hunks with apply_patch. Both
production files match their before snapshots byte-for-byte; both archived
probe patches pass forward checking. No profiling counters/hooks remain.

Rebuild normal services and restore exact Linux d6d0ef28...6e07 / FreeBSD
89420d9f...7b9a and rootfs d8688aa9...bdd, mode 0600. All 1,180 native tests
(16 ignored) and 97 clocks/readiness/PTY/UNIX ASan tests pass. Three normal startup/cleanup/
reuse sessions pass at 4403.129 / 4678.712 / 4780.414ms, serial 61e3xenb, full
cleanup=true footer and no checked errors or profile output. Restoration is
not speed evidence. Kernel/FPU/ABI/IPC fastpath, versions and commit/release
state are unchanged. The full optimization goal stays active; next validate
the narrowly scoped already-readable send notification candidate, not the
unobserved direct-I/O path or rejected cycle49/64 changes.

## Cycle 66: directional UNIX readiness (work reduction retained)

Previous turn: progress through two real diagnostics, exact probe removal and
normal validation. Revalidate current sources, accepted cycle62 image and the
no-Kernel boundary. Implement the measured notification lead rather than the
unobserved direct-I/O specialization. Preserve existing unrelated dirty work.

First candidate sends READY_UNIX only when the recipient packet queue was
empty before successful publication. Use packet presence, not queued bytes:
zero-length datagrams/seqpackets are readable records. All checked input,
rights duplication, ownership rollback, limits, EOF/HUP and receive-side
capacity notification stay intact. A native 128-send burst reports 128 old
notifications versus one candidate notification, preserving every byte. Three
new assertions fail on old code as expected; the whole-record writer test
already passes and remains a guard against an unsafe receive-side shortcut.

Send-only native1,184/16ignored, UNIX/readiness/PTY ASan82, ARM check and normal
image build pass. Four normal startup and eight communication/process boots
pass in before/after/after/before order. No own concurrent build/test/VM.
Startup 5036.3845 ->4926.3675ms (-2.18%) does not establish 5%; the difference
is about one 100ms observer interval. Unchanged copyout also moves by about5%,
while fork/exec is +2.60%. Do not attribute these controls to the send predicate
or accept it separately as a demonstrated speedup. Preserve all samples,
including stat224.101ms and UNIX422.597ms. The two experiments below are not
pooled across their many-hour host gap.

Refine the same stateless notification model: READY_UNIX(64) remains general
state change; new private READY_UNIX_READ(512) means newly readable data and
READY_UNIX_WRITE(1024) means released capacity. file_sources(file,events)
retains general changes and adds only requested read/write directions. Armed
epoll watches use the same mapping. Blocking UNIX calls subscribe by their
operation. A zero-event interest still subscribes to general changes. Connect,
accept, shutdown, options and GC continue to generate general notifications.
Receive always reports freed byte/packet/rights capacity: a whole-record writer
can be blocked even while POLLOUT is true. Coalesced bits are ORed, never cleared.

Check source/deadline eligibility before the existing PID stop lookup; do not
revive cycle64's borrowed-stop getter or changed writeback. Keep the stored PTY
deadline and full readiness snapshot behavior. There are no new Runtime/socket/
wait fields, retained allocations, wire messages, Kernel calls or synchronization
structures. io_uring's completion queue still publishes READY_OBJECT; its
current submissions execute immediately rather than parking on UNIX events.
Current edge/exclusive epoll requests remain explicitly unsupported.

Eight retained regressions cover readable transition/repeated queued writes,
zero-length records, partial-capacity whole-record writers, a 128-packet burst,
blocked reader data/last-close subscriptions, direction filtering with stopped
tasks, zero-interest HUP/NVAL, and epoll one-shot rearm/copyout failure. Generic
readiness tests count provider queries to prove unrelated events are not merely
rescanned and discarded. Normal native1,188/16ignored, focused ASan86 and
AArch64 Release checking pass, before and again after temporary profiling.

Build the combined normal candidate and compare all 3,890 rootfs paths: only
the two Alter ELFs change. Linux afaa9d27...1ae5, FreeBSD2aca481e...40e9;
rootfs390c7a74...d52e; full image
c120c66d53588a1120d87bb8c2207f7eb379ceda48f296621cc7e6fc04771ebd.
Alpha e025b8a5...7a8d and Kernel312d579b...0c88 remain identical. Normal
before/after/after/before startup runs each complete three cleanup/reuse
sessions; another eight normal boots cover communication and process controls.
The completed power-event audit finds no sleep transition in this timed window
(2026-11-03 00:50..00:59 +0900). Earlier sleep during artifact preparation is
not inside these samples. All twelve complete full footers/error checks.

Pooled medians, six samples per variant, ms; retain all raw per-boot rows:

| Normal combined workload | Baseline62 | Candidate | Change |
| --- | ---: | ---: | ---: |
| X11-ready startup | 5409.9080 | 5303.3255 | -1.97% |
| UNIX write/read,2048 | 390.2440 | 387.5125 | -0.70% |
| UNIX write/recvmsg,2048 | 799.2725 | 780.6225 | -2.33% |
| EOF recvmsg | 462.0370 | 449.7370 | -2.66% |
| Existing stat,512 | 139.0800 | 143.5110 | +3.19% |
| Missing stat,512 | 105.8845 | 112.1415 | +5.91% |
| getpid,2048 | 43.1415 | 40.6125 | -5.86% |
| Checked getresuid | 180.9980 | 183.5880 | +1.43% |
| fork/wait,8 | 23.4260 | 24.0615 | +2.71% |
| fork/exec/wait,8 | 769.3985 | 733.7240 | -4.64% |
| fork,128 VMAs,8 | 20.6290 | 19.5180 | -5.39% |
| fork,256 VMAs,8 | 19.6260 | 18.9735 | -3.32% |

Do not claim the unchanged getpid/fragmented-fork rows as causal 5% wins. Native
construction controls also move: BusyBox whole lifecycle +10.81%, Xfe +3.25%,
with no Linux ELF execution in those controls. Keep the slow second candidate
boot (UNIX695.014ms, stat220.686ms), not just favorable samples. First and
repeated startup sessions are distinct observations; observer quantization,
host load and these controls preclude a firm 5% speed claim.

One additional normal GUI boot passes two complete X11/FLWM/wbar sessions,
real input, Terminal/Editor/Information launch and Exit/restart cleanup. Serial
eewjg3z2 has every per-session marker and final cleanup=true, no checked errors.
Inspect both Information screenshots: real OS information, blue desktop and
dock are present. This is not exhaustive validation of every installed app.

Use the exact cycle65A bounded count probe on the combined candidate, then on
a fresh coarse baseline. Both diagnostic images clone the same normal boot
prefix and differ only in their archived ext2/Alter ELF pair. Build profile
services/rootfs only, avoiding any Kernel rebuild/date change. Verify the
65-MiB prefix SHA b989d841...b9e0 on both and ext2 hashes. The diagnostic
comparison is not timed speed evidence; instrumentation changes execution and
log costs. Three actual startup/cleanup/reuse sessions per image pass.

| Flushed diagnostic work | Fresh coarse baseline | Directional |
| --- | ---: | ---: |
| Wake calls | 4059 | 3892 |
| Scans | 4409 | 2449 |
| At least one ready | 809 | 817 |
| Unready scans | 3600 | 1632 |
| Scan errors | 0 | 0 |

Observed scans fall 44.45%, unready scans 54.67%, with comparable useful ready
results. Counters aggregate until guest-exit flush, not exclusive CPU, IPC
transport, a complete account of startup, or process-owned totals. Candidate
poll scans 2210/ready 607; pselect 239/ready 210. Fresh baseline 4132/604 and 277/205.
Earlier cycle65 counts and this matched check agree on substantial redundant
work; they are not pooled as repeated timing evidence.

Decision: retain this small, allocation-free work reduction and clearer event
interests, not a new cache or index. The 5% speed target and full optimization
goal remain open. Do not relabel the 44% scan reduction as 44% startup/CPU gain.
Next measure remaining general-state notifications before adding complexity.

Remove both temporary source hooks through apply_patch and compare exact normal
combined bytes. Rebuild normal services to Linux afaa9d27...1ae5 / FreeBSD
2aca481e...40e9; project rootfs stays 390c7a74...d52e, mode 0600. All 1,188
native tests (16 ignored) and 86 ASan tests pass again. Keep the tested normal combined image and
all source/ELF/rootfs/measurement archives under
/private/tmp/nanami-opt-cycle66.F2r7B4. Candidate and diagnostic patches are
nanami-opt-cycle66-directional-readiness-{candidate,diagnostic}.patch. Full
measurements.json preserves both normal phases, all controls/outliers, both
fresh diagnostics and GUI; separate JSON records source/extent comparisons.
Baseline62 image is restored
byte-for-byte after temporary compression; its verified archive also remains.
Remove only closed diagnostic raw images after verifying rootfs archives.

Capacity failures were build-artifact duplication, not runtime memory growth.
The user identified old thirdparty/tinycore builds and explicitly approved
deleting their saved guest data too. Validate exact non-symlink roots, no Git
tracked files and no open handles, then delete 11 old build trees. Preserve
build, build-desktop, active build-desktop-process, downloads and sources.
Directory usage 7.7G -> 1.9G; disk free immediately afterward 8.6GiB. Generated
builds can be recreated; removed saved guest data has no new backup. The exact
list/checks are in old-tinycore-build-cleanup.json. Initial failed partial
images are discarded; no incomplete image is timed. Kernel/FPU/ABI/IPC
fastpath, versions and commit/release state remain unchanged.

## Cycle 67: UNIX collection frequency before bulk-close changes

Inspect actual scalar close, exec CLOEXEC close and checked process teardown.
Closing a UNIX FD can trace every live process FD table and queued-rights graph.
Do not introduce a Runtime deferral flag, change release ownership, suppress
general-state readiness or split descriptor lifecycle merely from source-level
loop counts. In particular, a closed FD may need a POLLNVAL notification even
when another alias keeps its socket alive.

Temporarily extend the opt-in Alter profiler with aggregate collection calls,
empty calls, actual root slots/UNIX roots, swept endpoints and enclosing exec/
exit bulk-close counts. Same-actor local counter intervals include service
waits and descheduling; they are not exclusive CPU or IPC transport cost.
Flush at the existing guest-exit point, before exit cleanup: later cleanup can
appear at the next flush, and the final tail can be absent. Trigger PID does
not own every sample. No per-FD UART or Kernel change is introduced.

Build services and a separate diagnostic rootfs only; do not invoke the Kernel
build across the date rollover. Clone the accepted cycle 66 image and replace
its 65-MiB-offset ext2. Prefix SHA-256 stays b989d841...b9e0; Kernel stays
312d579b...0c88 and Alpha e025b8a5...7a8d. Compare all 3,890 rootfs paths:
only /bin/alter-linux and /bin/alter-freebsd contents/sizes differ; inode IDs,
modes, owners, links and timestamps remain identical. Diagnostic rootfs is
4610dc95d895cec558162fd9bdc34b202876c3b2b291acab8197d2b66d1c074d;
diagnostic Linux 1a81a97e...72a2, FreeBSD 23aa4897...7d43d.

Three actual startup/kill/delete/same-ID reuse sessions pass, cleanup=true,
without checked fatal/proc/SHM/fork/panic errors. Serial suffix 6gsyi9hr.
Observed flushed aggregates: 50 collections, zero empty collections, 9,792 FD
root slots, 145 UNIX root references and 36 swept sockets. Sixteen collection
calls occur inside process-exit close batches; zero occur inside exec CLOEXEC
close batches. Summed local collection intervals are 933,000 counter ticks,
not a conversion to elapsed seconds or a complete account of teardown.

The same flush window records mmap 1,737 calls / 3,944,904,000 handler ticks,
clone 41 / 1,927,037,000 and exec 46 / 627,390,000; all emitted handler totals
sum to 9,248,838,000 ticks. Do not treat summed handler ranges as total CPU
time or claim a startup speed percentage. Instrumented startup samples
4514.798 / 5112.240 / 5106.428ms have no matched normal control. The GC trace
does not support bulk-close refactoring as a material startup lever. No batch
candidate, persistent GC state or notification shortcut is deployed.

Remove all three temporary hooks with apply_patch; cmp confirms exact saved
normal source bytes. Rebuild normal Alter and verify Linux afaa9d27...1ae5,
FreeBSD 2aca481e...40e9, rootfs 390c7a74...d52e and unchanged Alpha/Kernel.
Full native tests pass 1,188 / 16 ignored; UNIX/readiness/PTY ASan passes 86.
Three further normal startup/cleanup/reuse sessions pass with no probe output
or checked errors, serial suffix p8iwhmjg. Normal startup 4518.874 / 4890.206 /
4896.114ms verifies restoration, not a paired improvement over instrumentation.
git diff --check, diagnostic forward-apply checking and the clean A9N worktree
check pass. No runtime, version, commit or release change remains from cycle 67.

Archive /private/tmp/nanami-opt-cycle67.uyagKh retains originals, normal/profile
ELFs, a verified compressed initial diagnostic rootfs, measurements and rootfs
comparison. Probe patch: /private/tmp/nanami-opt-cycle67-unix-gc-diagnostic.patch.
After both VMs and gzip/hash checks terminate and lsof finds no open handles,
remove only this cycle's generated raw diagnostic image/rootfs; retain normal
cycle 66 image and reproducibility artifacts. No saved user guest data is
removed here. The broader optimization goal remains active. Follow the costly
mapping/construction paths with evidence; do not repeat rejected final-permission
APIs, alias-associativity tuning or larger image caches from older cycles.

## Cycle 68: capacity-aware Linux directory enumeration

Cycle 67 makes diagnostic progress: current GC counts rule out bulk FD cleanup
as a material startup lever. Revalidate current code before the next candidate.
Ordinary sys_mprotect already skips unchanged permissions; Tiny Core's ordinary
private forks cannot become shared-VM/vfork. Do not repeat these or the rejected
permission/publication, alias-associativity and snapshot-ceiling experiments.

Audit a different user-level data path: sys_getdents64 caps backing reads at 16
records and packed output at ALTER_IO_OFFSET=512, regardless of a larger valid
guest buffer. VFS records are 288 bytes; a valid Linux dirent64 is at most 280
bytes. Existing READ_DIR supports larger bounded counts. No new RPC is needed.

Forward compaction can safely use more of the existing shared buffer. Before
record i, previous packed records occupy at most i*280 bytes; source record i
starts at 512+i*288. Even the current maximum-sized output ends before its
source. Compile-time assertions enforce the initial-gap and per-record bounds.
Output is bounded by min(guest count, actual shared-memory size); backend count
is bounded by remaining input space and worst-case packed output capacity.
Keep the existing 16-entry small-buffer batch, including its rewind behavior,
but scale larger buffers. No retained fields, new heap buffer/cache, public
operation, Kernel call type or synchronization mechanism is added.

Shared POSIX open-description cookies, dup/fork sharing, lseek resumption,
root overlay shadowing, malformed-entry validation, checked guest copyout and
backend rollback remain in the same code path. Do not claim copyout is an
atomic guest-memory write; only the existing cursor rollback contract is kept.

Four new production-source tests first fail on old code because it cannot
produce the larger batches. After the change they pass; add a fifth extent
guard for a Word::MAX request with actual shared storage of only 1,088 bytes.
The nine directory tests cover 300 names, 255-byte names, padding/type/inode/
cookie preservation, short buffers, shared-cursor seeks, failed large copyout,
root shadowing across a batch boundary and untouched extent sentinels. Native
1,193 tests pass / 16 ignored; directory/ext2-directory ASan passes 14;
AArch64 Alter Release checking and formatting/diff checks pass.

Extend the integer-only desktop fixture with 64 complete /usr/local/lib scans
at both 512-byte and 32-KiB capacity. Validate record lengths, increasing cookies,
names and a boundary-aware checksum on every pass; close/reopen each stream.
Also run eight fork/exec/wait executions of Tiny Core's unchanged BusyBox ls -1,
with stdout/stderr directed to /dev/null and every exit status checked. Preserve
the existing UNIX/stat workloads and frame/process/startup controls. The new
fixture SHA-256 is 59617c25...10ae and is identical in both variants. Do not pool
these times with older cycles' shorter desktop fixture.

Clone the cycle 47 frozen overlay without altering it, replace only this test
ELF, then package baseline and candidate rootfs from the same inputs. Compare
all 3,890 paths: only /bin/alter-linux and /bin/alter-freebsd contents/sizes differ;
inode IDs, names, modes, owners, links and timestamps match. Baseline Linux
afaa9d27...1ae5, candidate fea651de...a794; candidate FreeBSD ad97c4e1...fdea.
Alpha e025b8a5...7a8d and Kernel 312d579b...0c88 remain exact accepted bytes.
Avoid a full Kernel build across the date rollover. Both image prefixes retain
b989d841...b9e0; rootfs partition hashes match their packaged inputs:
before 9c2e1ff5...9388, after 1bee528b...a938. Normal images are
/private/tmp/nanami-opt-cycle68-before.img (0df3aa05...fa9e) and
/private/tmp/nanami-opt-cycle68-after.img (2befc624...698c).

Run before1/after1/after2/before2, each with communication/directory, process and
three-session startup workloads: twelve sequential normal four-CPU boots,
normal devices, no profiling or own concurrent build/test/VM. All complete full
serial footers without checked fatal/proc/SHM/fork/panic errors. The completed
power-event audit finds no Sleep/DarkWake/Wake transition within 2026-11-03
01:55:16..02:01:46 +0900. This does not measure unrelated host CPU load.

Pooled medians, six samples per variant, milliseconds:

| Workload | Before | After | Change |
| --- | ---: | ---: | ---: |
| Directory scans, 64, 512-byte buffer | 554.2425 | 563.4175 | +1.66% |
| Directory scans, 64, 32-KiB buffer | 552.5845 | 105.8240 | -80.85% |
| Unmodified BusyBox ls, 8 | 2227.8595 | 2138.4410 | -4.01% |
| X11-ready startup | 5037.4825 | 5007.0215 | -0.60% |
| Existing stat, 512 | 132.6860 | 132.3195 | -0.28% |
| Missing stat, 512 | 96.5795 | 97.8635 | +1.33% |
| UNIX write/read, 2048 | 381.4625 | 379.9295 | -0.40% |
| UNIX write/recvmsg, 2048 | 761.6950 | 774.8735 | +1.73% |
| EOF recvmsg | 437.1185 | 442.3065 | +1.19% |
| Checked getresuid | 175.3150 | 173.2310 | -1.19% |
| fork/wait, 8 | 20.7175 | 19.6110 | -5.34% |
| fork/exec/wait, 8 | 690.6645 | 695.9880 | +0.77% |

Large-buffer per-boot medians are 552.075 / 553.094ms before and 108.499 /
101.274ms after. Every pass retains 474 entries and checksum
18414638227522594487. Actual getdents calls for 64 scans fall 2,560 -> 384;
small-buffer calls stay 2,560. This removes repeated packing/copy/validation/
dispatch work, not evidence of expensive IPC transport. Whole BusyBox ls is
below the 5% target and startup is essentially unchanged; do not relabel the
directory result as a whole-system 81% gain.

Keep all contrary controls. Native suspended Alpha construction/reap moves
+6.08%, native Xfe spawn +5.80%, reap +12.89%, whole Xfe lifecycle +4.42%,
without executing Linux code in those regions. Fragmented fork also moves
-10.69%/-7.51%, getpid -3.25% and 128-VMA copyout +2.08%; do not attribute these
to directory batching or select only favorable rows. Full raw samples/tables
are in measurements.json and all serial directories are preserved.

Two actual GUI sessions in one normal candidate boot pass X11/FLWM/wbar,
real input, Terminal/Editor/Information launch and Exit/restart cleanup.
Serial suffix m4fm6nwx contains every per-session marker and cleanup=true;
both Information screenshots are visually inspected. This is not exhaustive
testing of every application. The optional five-application check initially
times out because the older frozen fixture lacks its pointer probe and test
script; preserve that failed attempt (y6cq512k), not a runtime-regression claim.
A separate test-only image adds those two assets for a follow-up, without
altering either timed comparison image or its measurements.

Decision: retain the bounded, allocation-free directory optimization. The
declared large-buffer enumeration workload meets the 5% target; the broader
optimization goal remains active. Kernel, FPU, service boundaries, ABI, IPC
fastpath, versions and commit/release state are unchanged. Source snapshots,
normal ELFs, verified rootfs archives and complete evidence are under
/private/tmp/nanami-opt-cycle68.EcwwEY; candidate patch is
/private/tmp/nanami-opt-cycle68-directory-batching-candidate.patch.

The optional application attempt reaches an authoritative timeout/runner exit1;
do not restart merely because observation is silent. Its source fixture lacks
both /bin/nanami-desktop-pointer and /usr/local/bin/nanami-desktop-test. Preserve
the failure log/screens. Once that VM is terminal, build a separate application
fixture with the existing pointer source and desktop-apps.sh. Compare all paths:
3,890 -> 3,892 entries, with only those two content/non-inode-metadata additions.
Fresh inode assignments move 3,763 comparison records; do not claim inode IDs
are identical here. All common file contents/modes/owners/links/timestamps,
including runtime ELFs, match the tested normal candidate. Application rootfs
is a8978a14...f4e7. This diagnostic fixture is not pooled with timed inputs.

The repaired fixture passes real pointer coordinates after motion reversal,
and all five upstream GUI applications map their identified, viewable windows:
Files/Xfe, Images, Paint, Calculator and Clock. Final windows=true cleanup=true
and runner exit0, no checked errors; serial suffix 2anssw_e. Inspect the Xfe
screenshot: real folder tree, file list and dock are present. This validates
application launch, not exhaustive interaction or performance for every app.

Update only the generated active rootfs after confirming its old SHA matches
390c7a74...d52e, no open handle, and a verified exact old gzip archive remains.
Stage/verify the candidate and atomically install rootfs 1bee528b...a938, mode
0600. Do not overwrite existing saved boot images in build, build-desktop or
build-desktop-process. The tested current boot artifact remains the normal
/private/tmp/nanami-opt-cycle68-after.img. Remove only closed owned before/apps
diagnostic images and duplicate raw rootfs after gzip-test and streamed hashes;
the archived rootfs plus retained 65-MiB boot prefix reproduce these inputs.
Keep snapshots, ELFs, extended C fixture, probe assets, all raw logs and both
normal and application rootfs archives. final-verification.json records final
checks and cleanup; the broader optimization objective remains active.

## Cycle 69: readdir-driven dentry prefill rejected

Use the existing opt-in syscall profiler to identify the remaining cost of
stock Tiny Core BusyBox ls after cycle 68. The exit-triggered records are
global windows, not per-process ownership. Preserve the first child-trigger
window (25) separately: its 8,844 getdents calls include the preceding raw
directory benchmark. The 23 later windows (26..48) each contain 472 lstat
calls, 17 mmap calls and six getdents calls. Across those clean windows,
lstat records 10,856 calls / 2,794,651,000 counter ticks, mmap 391 /
1,355,655,000 and getdents 138 / 26,752,000. These spans include service waits,
descheduling and profiling/UART perturbation, not exclusive CPU or normal
benchmark time. Do not attribute all 8,982 directory calls to ls, subtract
timestamps across cores, or claim IPC transport is the bottleneck.

Try populating the existing 128-entry dentry cache only for live records
actually returned by Ext2 read_directory. Do not retain inode permissions or
other metadata, allocate another cache, add a service operation or modify
Kernel. Add two production-source native checks for returned-only seeding,
deleted/skipped records and zero capacity; both fail on old code as expected.
The candidate passes 1,195 native tests / 16 ignored, 16 focused ASan tests
and AArch64 Ext2 Release checking. This establishes semantics, not speed.

Add a baseline-compatible real-filesystem regression before timed loops:
enumerate a temporary directory, keep its old regular-file inode pinned,
unlink the name, recreate it as a directory, then rename out/back and remove.
Check ENOENT, current object type and cleanup after each transition. The
integer-only C fixture prints DIRECTORY-CACHE-FRESHNESS-PASS. It is captured
on the same line as TINYCORE-PERFORMANCE-PASS; the verifier must accept a
leading space/prefix, not mistakenly demand column zero. All four normal
communication boots pass this check.

Freeze the same rootfs in both normal variants, a6c472f8...5757. It includes
the above C regression plus the two GUI probe assets repaired in cycle 68;
do not pool timings with cycle 68's older fixture. Rootfs content is identical
between variants. Extract both boot archives: of 21 entries, only
./bin/ext2-server changes (158,728 -> 158,904 bytes; 28dcd199...9710 ->
dd65d10e...7d5d). Alpha remains e025b8a5...7a8d, Linux fea651de...a794,
FreeBSD ad97c4e1...fdea and Kernel 312d579b...0c88. Pin the existing build
timestamp to 2026-11-02 during full image packaging across midnight; confirm
the installed Kernel bytes are unchanged, not merely its version string.

Run before1/after1/after2/before2, each communication, process and three-session
startup: 12 sequential normal four-CPU boots with normal devices and no own
concurrent build/test/VM. Every full serial footer and cleanup passes with no
checked fatal, process, SHM, fork or panic error and no profile output.
The completed power-event audit finds no Sleep/DarkWake/Wake transition in
2026-11-03 02:35:30..02:41:57 +0900; it does not measure unrelated host load.

Pooled medians, six samples per variant, milliseconds:

| Workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Directory scans, 64, 512-byte buffer | 562.8680 | 672.7130 | +19.52% |
| Directory scans, 64, 32-KiB buffer | 107.2055 | 186.6740 | +74.13% |
| Unmodified BusyBox ls, 8 | 2138.8095 | 2079.5035 | -2.77% |
| Existing stat, 512 | 140.0735 | 134.8320 | -3.74% |
| Missing stat, 512 | 98.1120 | 96.9980 | -1.14% |
| UNIX write/read, 2048 | 378.2355 | 378.8600 | +0.17% |
| UNIX write/recvmsg, 2048 | 769.1790 | 747.1220 | -2.87% |
| EOF recvmsg | 441.5025 | 445.9030 | +1.00% |
| X11-ready startup | 4990.2300 | 4985.5295 | -0.09% |
| fork/wait, 8 | 19.0645 | 18.6215 | -2.32% |
| fork/exec/wait, 8 | 679.4370 | 690.9160 | +1.69% |
| Checked getresuid | 178.6000 | 174.2665 | -2.43% |
| 128-VMA checked copyout | 183.9095 | 177.8970 | -3.27% |
| 256-VMA checked copyout | 179.6490 | 180.6880 | +0.58% |

Both candidate large-directory boot medians (184.020 / 193.378ms) are worse
than both baselines (104.097 / 107.349ms). Small-directory boot medians are
679.149 / 657.323 versus 545.577 / 571.753ms. The same 474 entries, checksum
18414638227522594487, 2,560 small-buffer and 384 large-buffer calls remain.
This is a repeatable regression, not a tradeoff justified by the sub-5% ls
gain. Linear 128-slot cache maintenance for every returned name is consistent
with the added source work; no exclusive CPU-cost attribution is measured.

Preserve contrary native controls too: suspended Alpha construction/reap
-2.03%, spawn -7.79%, reap +11.10%, Negoto creation +6.30%, native Xfe spawn
-1.41%, reap -14.12%, full BusyBox lifecycle -4.50% and full Xfe lifecycle
-4.54%. Those native regions do not execute Linux programs. Getpid +0.91%,
getuid -1.39%, fragmented fork +3.93%/-0.40%. Full raw samples, tables,
serial directories and verifications are in measurements.json; no row is
discarded to manufacture a 5% claim.

Decision: reject the candidate. Restore Ext2 directory source and its native
test file byte-for-byte to their pre-cycle snapshots; remove the two
candidate-only mocks/tests. Keep only the baseline-compatible C freshness
regression. Rebuild a normal image and verify the embedded Ext2/Alpha and
Kernel hashes above, with normal Alter ELFs. Restored native 1,193 / 16 ignored,
ASan 14 and three actual startup/cleanup/reuse sessions pass; serial suffix
d4ydv2w4, no checked errors or diagnostics. Restored image SHA-256 is
45142cae...d580. Cycle 68's directory batching remains intact.

Update only the generated active rootfs after confirming its previous hash
1bee528b...a938, no open handles and the verified exact cycle-68 gzip backup.
Stage/hash the new a6c472f8...5757 rootfs and atomically install it. This changes
test assets only, not the accepted runtime policy; saved user boot images are
not overwritten. Keep the normal restored image, cycle-68 normal image,
source/ELF snapshots, rejected patch and all logs; remove only closed owned
diagnostic images and duplicate raw rootfs after testing archives and hashes.
Evidence is under /private/tmp/nanami-opt-cycle69.2tR7qq; rejected full patch
is /private/tmp/nanami-opt-cycle69-readdir-cache-candidate.patch. That patch
also includes the retained C guard, so whole-patch reverse checks are not a
valid proof of restoring just the two rejected files.

The broader optimization goal remains active. No new runtime speedup is
adopted here. Kernel, ABI, IPC fastpath, service architecture, FPU, versions
and commit/release state are unchanged.

## Cycle 70: borrowed cached-block inspection rejected

Cycle 69 produces concrete negative evidence and restores the accepted runtime.
Keep Kernel unchanged and do not retry eager readdir prefill or expand a cache.
Inspect the actual lstat path instead: Ext2 copies cached directory/inode-table
blocks into its device buffer before decoding them, even for read-only lookup.

Evaluate scoped immutable cache access. Add BlockCache::get, retaining the same
CLOCK reference update, and have load copy from that view. Add with_read_block:
validate the existing block/SHM extent, invoke a non-retaining reader callback
on a private cache hit, otherwise use the existing read/retry path and invoke
the callback on device-buffer bytes. A completely dirty cache can still bypass
insertion. Decode inode-table entries and search directory blocks inside the
callback, returning owned metadata/inode numbers before mutating the runtime.
Do not expose a persistent raw cache pointer or allow another runtime read
while a borrowed view is being consumed. Writes retain their staged-buffer,
dirty-data, flush-error and invalidation rules. No new cache, heap buffer,
retained field, protocol, service, Kernel operation or synchronization is added.

The extracted immutable directory search keeps first-live-match behavior,
validates every record before it, honors the final block extent, and does not
inspect malformed bytes after an already-found match. It compares actual
parent/name data, never a hash or client-provided metadata. Readdir batching
and dentry-cache population are unchanged. Four production files add 38 net
lines; the candidate Ext2 ELF grows 158,728 -> 159,336 bytes (+608).

Add five production block-cache/I/O tests and three directory-parser tests.
Cover cold miss versus a private cache hit with untouched staging sentinels;
dirty reads after uncertain failed writeback; a fully dirty cache that bypasses
new read insertion without losing writes; invalid extents, short reads and
retryable/non-retryable errors before any reader callback; explicit CLOCK
second-chance behavior plus 1,024 mixed store/load/view operations. Directory
checks cover live/deleted records, first duplicate, malformed data before/
after a match, truncated extents and an unaligned 255-byte name.
Candidate native 1,201 tests pass / 16 ignored; storage/directory ASan 49 pass;
AArch64 Ext2 Release checking passes. These are correctness/work-removal
checks, not proof of faster Tiny Core operation.

Package the same a6c472f8...5757 frozen rootfs as cycle 69. Of 21 boot-archive
entries, only ./bin/ext2-server changes: 28dcd199...9710 -> d7ad86ce...108e.
Normal Linux fea651de...a794, FreeBSD ad97c4e1...fdea, Alpha e025b8a5...7a8d
and Kernel 312d579b...0c88 remain exact. Pin the pre-existing Kernel timestamp
during packaging; no Kernel source or generated Kernel code changes.
Baseline image: /private/tmp/nanami-opt-cycle69-restored.img (45142cae...d580).
Candidate: /private/tmp/nanami-opt-cycle70-after.img (9348f729...bf9f).

Run before1/after1/after2/before2, each communication/directory, process and
three-session startup: 12 sequential four-CPU normal-device boots with no
profile output or overlapping own build/test/VM. All authoritative workload
footers pass, including cache-freshness transitions in each communication boot
and cleanup/reuse in startup boots; no checked fatal/proc/SHM/fork/panic errors.
The complete power-event audit finds no Sleep/DarkWake/Wake transition within
2026-11-03 02:56:38..03:03:02 +0900. This is not a host CPU-load measurement.

Pooled medians, six samples per variant, milliseconds:

| Workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Unmodified BusyBox ls, 8 | 2137.2355 | 2090.1605 | -2.20% |
| Existing stat, 512 | 140.0510 | 131.0640 | -6.42% |
| Missing stat, 512 | 99.0735 | 95.8640 | -3.24% |
| Directory scans, 64, 512-byte buffer | 548.6465 | 566.1005 | +3.18% |
| Directory scans, 64, 32-KiB buffer | 101.3485 | 107.3305 | +5.90% |
| X11-ready startup | 4789.8830 | 4994.5185 | +4.27% |
| UNIX write/read, 2048 | 377.8840 | 378.1880 | +0.08% |
| UNIX write/recvmsg, 2048 | 756.1680 | 747.6885 | -1.12% |
| EOF recvmsg | 424.0805 | 429.5120 | +1.28% |
| fork/wait, 8 | 20.1550 | 20.3280 | +0.86% |
| fork/exec/wait, 8 | 685.3375 | 673.3480 | -1.75% |
| Checked getresuid | 177.8375 | 171.4505 | -3.59% |
| 128-VMA checked copyout | 178.1440 | 174.8995 | -1.82% |
| 256-VMA checked copyout | 180.1795 | 174.7035 | -3.04% |

Actual ls per-boot medians are 2145.022 / 2129.449ms before and 2105.468 /
2074.853ms after, below the declared 5% target. Keep the contrary stat result:
139.910 / 141.328 before versus 130.691 / 131.642ms after. Do not hide it,
but do not use that one favorable row as proof of a broadly useful policy.
Large enumeration moves 100.540 / 102.157 -> 108.493 / 103.309ms: both
candidate boot medians are worse than both baseline medians. Enumeration
retains 474 entries, checksum 18414638227522594487 and existing request counts.
Callback/copy elimination is a source/native-test fact, not exclusive CPU,
IPC-transport or whole-system speed attribution.

Keep every native control too: suspended Alpha construction/reap +2.31%,
spawn +6.17%, reap -6.18%, Negoto creation +9.88%, native Xfe spawn +2.25%,
reap -2.37%, full BusyBox lifecycle -3.78% and full Xfe lifecycle +2.86%.
These native regions do not execute the Linux programs. Getpid -0.79%,
getuid -2.08% and fragmented fork -1.87%/-5.45% also vary. Complete raw
samples/tables, per-boot medians, serial paths and the power audit are in
measurements.json, not reduced to a selected headline.

Decision: reject the global borrowed-read candidate. The actual ls gain is
below target and directory/startup controls move adversely. Restore all seven
production/test files byte-for-byte to pre-cycle snapshots, including the
candidate-only mocked read failures and all eight tests. Preserve the complete
patch and source/ELF snapshots so this experiment remains inspectable. No
candidate runtime abstraction or test requiring it remains in the worktree.

Normal rebuild restores Ext2 28dcd199...9710, with unchanged Alpha/Kernel/Alter.
The complete restored init ELF matches the accepted cycle-69 init byte-for-byte;
the rootfs remains a6c472f8...5757. Restored native 1,193 / 16 ignored and
storage/directory ASan 41 pass. A final normal three-session startup/cleanup/
reuse check passes at 4604.052 / 4887.897 / 4993.189ms, full cleanup=true and
runner footer, serial suffix xb0ntqpa; no checked errors or diagnostics. These
are restoration checks, not pooled comparison samples. final-verification.json
records the final checks and owned-image cleanup. Keep the accepted normal
cycle-69 and cycle-68 images,
verified rootfs archives, all raw logs, source snapshots and candidate ELF/patch;
do not accumulate another full duplicate restored image after verification.
The rebuilt temporary restored disk has different disk-image bytes
(a9127226...9714); its boot payload and rootfs, not that disk metadata, are
the restoration proof. Saved user boot images are never overwritten.

Evidence directory: /private/tmp/nanami-opt-cycle70.FjCwhG.
Full rejected patch: /private/tmp/nanami-opt-cycle70-borrowed-block-candidate.patch.
The broader optimization goal remains active. This cycle adopts no new speed
claim or runtime change. Kernel, ABI, IPC fastpath, FPU, service architecture,
versions and commit/release state remain unchanged.

## Cycle 71: phase-separated filesystem work diagnostic

Cycle 70 supplies useful negative evidence: borrowed cached-block reads remove
copies in production-source tests, but do not establish a worthwhile stock-ls
gain and worsen other controls. Do not repeat a global copy/cache rewrite or
claim its favorable hot-stat timing proves that mechanism. Measure the work
actually executed by each filesystem workload first. Kernel remains unchanged.

Add temporary server-global counters to Ext2's request, inode, dentry and block
read paths. Count cache queries/hits/slots, inode calls/misses, directory records
examined by lookup_in_directory, successful cached block copies and uncached
device attempts/blocks. Accumulate scan lengths locally before updating the
diagnostic counters; do not add an atomic operation for every visited slot.
The standalone temporary module uses relaxed atomics and is removed afterward,
not adopted as a production profiling subsystem.

A private copy of the integer-only C performance fixture adds existing sync(2)
calls after its reference directory scan and each filesystem sample, plus one
after the UNIX tail. The normal repository C source and normal fixture stay
untouched. Ext2 reports and resets the counters after servicing each existing
VFS_REQUEST_SYNC. No new IPC command, Kernel API, capability or routing path.
Instrumented elapsed values are saved in raw logs but excluded from normal
speed comparisons: counters and synchronization perturb the workload.

One normal-device four-CPU diagnostic boot passes its full performance and
runner footers, the cache-freshness transitions and checksum
18414638227522594487. Serial suffix aqzca12c. Exactly 17 ordered boundaries
0..16 each contain all request/dentry/block groups, the expected POSIX proxy
PID 17 and sync status zero. Boundary0 includes earlier bootstrap, freshness
mutations and the reference scan; preserve it separately, not as ls work.
Boundaries1..3 are 64 small-buffer scans, 4..6 large scans, 7..9 eight stock
BusyBox ls executions, 10..12 existing stat and 13..15 missing stat; 16 is
the UNIX tail. These are server-global intervals including each fork/exec
lifecycle, not exclusive ownership by one Linux child.

Representative warm intervals (all individual rows remain in measurements.json):

| Workload | Inode calls / misses | Dentry queries / misses | Lookup records | Cached blocks / copied bytes | Device attempts |
| --- | ---: | ---: | ---: | ---: | ---: |
| 64 scans, 512-byte buffer (boundary3) | 3712 / 0 | 896 / 0 | 0 | 17280 / 17694720 | 0 |
| 64 scans, 32-KiB buffer (boundary6) | 1536 / 0 | 896 / 0 | 0 | 1216 / 1245184 | 0 |
| Stock ls, 8 (boundary9) | 54926 / 4356 | 31528 / 4118 | 904399 | 29378 / 30083072 | 0 |
| Existing stat, 512 (boundary12) | 8192 / 0 | 4608 / 0 | 0 | 0 / 0 | 0 |
| Missing stat, 512 (boundary15) | 5632 / 0 | 4096 / 0 | 0 | 0 / 0 | 0 |

Lookup records means the name-search loop, not all readdir parsing. A zero
in that column does not mean directory enumeration performs no work.
Copied bytes include every successful block-cache load in the interval,
not only the two consumers changed by cycle70.

Each stat interval has exactly 512 path requests plus its sync, with no
directory-read request. After the first positive interval's two inode/dentry
misses and four 1KiB copies, later positive intervals do no block copying or
device I/O at all. The first negative interval has one uncached name lookup,
474 lookup records and 15 cached blocks; later intervals have none. Thus a
cached-block-copy candidate must not use steady hot-stat timing as evidence
that it removed substantial copying there. Repeated inode/dentry-cache
access and service work still happen: positive512 visits 388096 dentry slots,
negative512 visits 333312. These counts are not CPU shares or pure IPC costs.

Keep cold and warm ls separate. Boundary7 includes 192 device attempts /
3428 blocks and 35510272 cached-copy bytes. Boundaries8/9 have no device read,
904389/904399 lookup records, 4357/4356 inode misses and 4118 dentry misses
each. Both still visit about 2.7 million dentry read slots and exactly 527104
store slots (4118 * 128); duplicate miss/store scanning is real work but
not a measured speed fraction. All three ls intervals have 3920 path requests
and 48 backing readdir requests. The UNIX tail has only its sync and zero
observed filesystem work, rather than an assumed POSIX/storage round trip.

The same 64 directory scans copy 17.7MB with a small buffer versus 1.25MB
with a large one. Source review identifies a distinct restart lead:
Alter's directory.rs correctly seeks back to start+consumed when not every
prefetched VFS entry fits the Linux buffer. Ext2's directory.rs reuses its
per-handle cursor only for exact cached.index == start_index; that rewind
otherwise starts again at block0. This explains an available rescan route,
not a measured per-cursor breakdown or an established wall-time gain.
Do not fix it by dropping the shared-cookie/copyout-rollback contract.

Two separate follow-ups remain unimplemented. Actual stock-ls name scans
remain an end-to-end target: assess a tiny advisory last-found cached-block
hint with parent/extent/lifetime validation, metadata-write invalidation and
the original complete search as fallback. The
[upstream Linux Ext2 lookup](https://raw.githubusercontent.com/torvalds/linux/master/fs/ext2/dir.c)
also uses a saved successful page as its search start and wraps on a miss;
this is design research, not copied implementation. Invalid-directory
duplicate ordering needs explicit consideration before adopting a hint.

The read-ahead rollback lead can preserve already-validated per-handle block
positions without changing name-lookup order. Consider retaining the live
entry index at a block's start, reusing backward positions only inside that
block and valid forward positions, and falling back for earlier blocks or
changed generations/inodes. A u32 checkpoint may fit Cursor's current padding
without changing its existing usize fields or narrowing arbitrary Word cookies;
verify actual size rather than assume it. Prove read reduction with
production-source rollback/forward-seek/deletion/error tests, then use matched
normal benchmarks. This smaller-buffer target must not be advertised as a
stock-ls or startup gain without actual evidence. next-candidate.json preserves
both leads and their required safety checks; neither is implemented here.

Diagnostic rootfs 80f99007...b265 contains only the private sync-enabled
performance ELF (c3d46770...6340) as a new test input. Instrumented Ext2
is excluded from rootfs as a core service. Compare all 3892 paths against
the normal rootfs: only that C fixture's contents/size change (10976 ->11264B);
inode IDs, names, modes, owners, links and timestamps all match. The comparison
is preserved in rootfs-comparison.json. Instrumented Ext2
e20b0320...0d25 is in the boot payload; normal Alter fea651de...a794,
Alpha e025b8a5...7a8d and Kernel 312d579b...0c88 stay exact. Profile image
70bca451...0f02 and full init/kernel snapshots are archived as evidence.
Rootfs-profile.img.gz passes gzip-test and its decompressed SHA matches.

Restore main.rs and block_io.rs byte-for-byte and remove the temporary
statistics.rs module after archiving it and the complete diagnostic patch.
Normal C source also matches its pre-cycle snapshot. Rebuild normal Ext2
28dcd199...9710 and verify the entire rebuilt init ELF matches accepted
cycle69. Active rootfs stays a6c472f8...5757. Restored native 1193 / 16 ignored
and storage/directory ASan41 pass. Three normal startup/cleanup/reuse sessions
pass at 4505.943 / 4786.737 / 4987.806ms, serial suffix 66t4zgci, all full
footers and no checked errors or profile output. Do not pool restoration
times with any earlier speed comparison.

Keep accepted normal cycle69/cycle68 images, verified normal/profile rootfs
archives, exact source/ELF snapshots, all raw logs, counter rows and the patch.
Only closed owned profile/restoration images and duplicate raw profile rootfs
are removed after verification; complete disk metadata is rebuildable, not
archived byte-for-byte. Saved user boot images are untouched. Evidence:
 /private/tmp/nanami-opt-cycle71.bptYWS; diagnostic patch:
 /private/tmp/nanami-opt-cycle71-ext2-work-diagnostic.patch.
final-verification.json records restoration and cleanup. No new production
optimization or 5% gain is claimed. Kernel, ABI, IPC fastpath, FPU, service
architecture, versions and commit/release state are unchanged; goal stays active.

## Cycle 72: block-local directory restart rejected despite fewer reads

Use cycle71's source-backed read-ahead rollback lead, without Kernel changes.
Extend the per-handle Cursor with a u32 live-entry index at its block start.
It fits the old padding: 40 bytes per cursor, 2568 bytes for the actual
64-handle x86_64 cache. Keep incoming cookies and ordinary index/block/offset
fields Word-sized. Generated live-entry counts fit Ext2's u32 directory size;
do not truncate arbitrary caller cookies.

Reuse only a matching inode/generation. For a rewind within its validated
block, replay that block from its saved live-entry count. For a valid forward
seek, continue from the cached position. Earlier positions fall back to block0.
Retain completed blocks as checkpoints and skip them without I/O on a forward
resume; EOF keeps the last block. Metadata invalidation, error commit rules,
shared descriptor positions, root shadowing and copyout rollback remain intact.

Eight new tests include four that first fail on old code solely because it
reads64/65 blocks rather than1; old functional output remains correct.
Check partial rollback, exact block boundary, forward seek, EOF/Word::MAX,
generation/inode invalidation, failed backward reads, 2048 mixed oracle
queries and unchanged cache size. Candidate native1201/16ignored, ASan58 and
AArch64 Ext2 Release checking pass. This does not establish faster execution.

Reuse the exact cycle71 sync-enabled diagnostic rootfs80f99007...b265 and the
same counters. All17 boundaries/footer/checksum pass; source probes are later
removed byte-exact. Only boundaries1..3 change: cached block loads17280 ->4224,
copied bytes17694720 ->4325376 (-75.56%), with2560 backing directory requests,
474 entries and the same checksum. Every other recorded numeric field in all
17 boundaries matches, including large scans, stock ls, inode/dentry/device
counts and UNIX tail. Actual x86_64 reports cursors-bytes=2568; native storage
size is also unchanged. This proves work removal, not CPU or speed percentages.

Restore all probes to the candidate source/binary and compare rebuilt init to
the frozen normal candidate. Only Ext2 differs among21 embedded boot records:
28dcd199...9710 ->20a06fd4...714b; its ELF remains158728 bytes. Rootfs is the
same a6c472f8...5757 in both variants. Alpha e025b8a5...7a8d, normal Alter and
Kernel312d579b...0c88 are unchanged. Candidate image8c4f5b3b...08b7.

Run before1/after1/after2/before2, each directory/communication, process and
three-session startup:12 sequential four-CPU normal-device boots, no profiles
or overlapping own build/test/VM. Every full footer, checksum/freshness test
and cleanup/reuse passes; no checked fatal/proc/SHM/fork/panic errors.
No Sleep/DarkWake/Wake transition is found in2026-11-03 03:41:56..03:48:21
+0900; this is not a measurement of unrelated host load.

Pooled medians, six samples per variant, milliseconds:

| Workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| Directory64, 512-byte buffer | 544.8060 | 568.0865 | +4.27% |
| Directory64, 32-KiB buffer | 102.6225 | 111.1890 | +8.35% |
| Stock BusyBox ls8 | 2133.3055 | 2159.6430 | +1.23% |
| Existing stat512 | 137.6070 | 134.0005 | -2.62% |
| Missing stat512 | 105.3160 | 99.5800 | -5.45% |
| UNIX write/read2048 | 379.2510 | 374.9405 | -1.14% |
| UNIX recvmsg2048 | 752.9365 | 763.0660 | +1.35% |
| EOF recvmsg | 441.6980 | 430.9620 | -2.43% |
| X11-ready startup | 4987.9770 | 4887.9410 | -2.01% |
| fork/wait8 | 20.9435 | 17.9525 | -14.28% |
| fork/exec/wait8 | 686.0460 | 689.2710 | +0.47% |
| Checked getresuid | 176.0580 | 165.7965 | -5.83% |

Small-scan boot medians544.321/545.291 ->570.668/565.505ms are adverse, not
an established5% gain. Large scans90.962/106.383 ->113.714/109.758ms also move
adversely. Preserve all controls, including favorable fork/copyout rows whose
regions do not exercise this directory restart. Native Alpha construction/reap
-2.03%, spawn -6.68%, reap -16.41%, full BusyBox lifecycle +1.59%, full Xfe
+3.94%, getpid -2.29%, getuid +3.11%, fragmented copyout +0.50%/+2.81% and
fragmented fork +1.11%/+3.39%. Full samples/tables are in measurements.json.
A75.56% local-work reduction is not a75.56% whole-operation improvement.

Decision: reject the runtime candidate, rather than select an unrelated
favorable row. Restore directory.rs exactly to its pre-cycle bytes. Retain only
three general, baseline-compatible mutation/handle-reuse, failed-backward-read
and mixed-seek/oracle regressions; remove feature/performance-only tests and
the size guard. Restored native1196/16ignored and ASan53 pass. The entire
normal rebuilt init matches accepted cycle69; active rootfs stays unchanged.
Three restoration startup/cleanup/reuse sessions pass, serial wnivaabg, at
4412.912/4889.868/4989.071ms, no checked errors or instrumentation.
Keep sources/ELFs, rejected patch, profile evidence, all raw logs and rootfs
archives; remove only closed owned trial/duplicate images after verification.
No new runtime gain is adopted here. Kernel/ABI/IPC fastpath/FPU/versions and
commit/release state are unchanged; the broad goal stays active.

A read-only routing audit supplies a new lead instead of more count-only
tuning. Actual serial shows POSIX17/Alter18 oncore2 and Ext2PID3 oncore3.
ipc_port.cpp's direct reply-receive condition explicitly requires equal core
affinity; process_manager.cpp's direct-switch helper takes a remote schedule/
reschedule path otherwise. This is an eligibility/config fact, not a measured
percentage of Kernel fastpath hits or proof that pure IPC dominates elapsed
time. POSIX read_dir forwards synchronously from its single-threaded request
handler. Keep Kernel untouched and assess metadata-broker locality using the
existing priority22 preferred-core2 directive, not new scheduling code or
hard-coded service names. Priority21 locality was previously retained; do not
repeat the rejected all-brokers-on-Alpha-core0 experiment.

Cycle72 evidence: /private/tmp/nanami-opt-cycle72.5h71YJ.
Rejected patch: /private/tmp/nanami-opt-cycle72-directory-restart-candidate.patch.
profile-comparison.json, measurements.json and final-verification.json retain
the complete distinction between reduced work, unchanged outputs and adverse
normal timings. Any subsequent placement experiment is a separate candidate
and must not pool these times.

## Cycle 73: retain configurable metadata-broker locality

Follow cycle72's routing audit, not its rejected directory cursor change.
The x86_64 boot list adds only `@placement 22 2` alongside the existing
priority21 preference. Actual normal serial moves Ext2PID3 from CPU3 to CPU2,
with POSIX17/Alter18 on CPU2. AHCI remains CPU3/priority23, framebuffer CPU2/
priority24, and Linux workers retain priority4 and round-robin CPUs1/2/3.
No service-specific rule, new scheduler code, cache, allocation or RPC is added.
The preference grants no authority, consumes the usual automatic slot and loses
to exact per-image affinity. Missing CPU2 falls back normally. AArch64's boot
list is unchanged; no ARM speedup is claimed.

Read-only source inspection finds that the existing direct reply-receive
branch checks equal caller/server affinity, and the direct-switch helper
requests remote scheduling otherwise. POSIX's directory forwarding is
synchronous. This makes the local branch eligible for more metadata traffic;
it is not a measured fastpath-hit rate or proof that pure IPC dominates time.
Kernel source, ABI and fastpath instructions are unchanged.

Freeze rootfs a6c472f8...5757 and the Release build date. Comparing all21
embedded boot records finds only ./nanami/boot-list different (1554 ->1564
bytes); every embedded executable is byte-identical. Normal Ext2 remains
28dcd199...9710, Alpha e025b8a5...7a8d, Alter/Linux fea651de...1a794,
Alter/FreeBSD ad97c4e1...9fdea and Kernel312d579b...0c88. The active rootfs and
the candidate image's entire rootfs partition match. Baseline image45142cae...
d580; candidate7bfdd563...78c1. No timing instrumentation is present.

Run before1/after1/after2/before2, each directory/communication, process and
three-session startup:12 sequential four-CPU normal-device boots, no overlapping
own builds/tests/VMs. A lost orchestration response is recovered without
starting a duplicate VM: retain the first eight completed logs, confirm no live
QEMU/image handles, then run the remaining four once. Keep all12 full serial
logs, not only the abbreviated stdout. Every required footer, process cache
freshness, directory freshness and startup cleanup/reuse passes. Each directory
run reports474 entries, checksum18414638227522594487, small-calls2560x3 and
large-calls384x3. No checked fatal/proc/SHM/fork/panic or profile error appears.
No Sleep/DarkWake/Wake transition is found in the audited2026-11-03
04:03..04:15+0900 interval; this does not establish unrelated host load.

Initial paired medians, six samples per variant (Negoto create12), milliseconds.
Negative change is less elapsed time, not a pure CPU or IPC cost:

| Workload | Before | Locality | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 28.7630 | 28.4710 | -1.02% |
| alpha-lifecycle-xfe-3 | 27.6050 | 29.7085 | +7.62% |
| alpha-reap | 0.3905 | 0.4180 | +7.04% |
| alpha-spawn | 2.7550 | 2.8775 | +4.45% |
| alpha-spawn-reap-8 | 24.8805 | 26.2820 | +5.63% |
| alpha-xfe-reap | 0.6245 | 0.5530 | -11.45% |
| alpha-xfe-spawn | 5.2210 | 5.4760 | +4.88% |
| busybox-ls-8 | 2132.6210 | 1970.0775 | -7.62% |
| copyout-128-vmas | 176.2095 | 172.9850 | -1.83% |
| copyout-256-vmas | 180.4780 | 179.6520 | -0.46% |
| copyout-normal | 178.5035 | 172.8645 | -3.16% |
| directory-large-64 | 103.9585 | 79.9055 | -23.14% |
| directory-small-64 | 560.7485 | 445.3145 | -20.59% |
| fork-128-vmas-8 | 18.7430 | 17.9065 | -4.46% |
| fork-256-vmas-8 | 19.1195 | 17.5845 | -8.03% |
| fork-8 | 19.1520 | 20.1105 | +5.00% |
| fork-exec-8 | 683.4180 | 670.8890 | -1.83% |
| getpid-2048 | 40.9460 | 40.2985 | -1.58% |
| getuid-2048 | 40.2915 | 39.2230 | -2.65% |
| missing-stat-512 | 100.7905 | 78.0490 | -22.56% |
| negoto-create | 7.6325 | 7.0920 | -7.08% |
| startup | 4936.6765 | 4789.3185 | -2.98% |
| stat-512 | 138.1710 | 115.3170 | -16.54% |
| unix-2048 | 382.9590 | 383.3120 | +0.09% |
| unix-recvmsg-2048 | 771.7975 | 750.3660 | -2.78% |
| unix-recvmsg-eof-2048 | 431.1070 | 434.3735 | +0.76% |

Small-scan boot medians571.040/540.992 ->444.692/445.937ms, large scans
103.000/108.947 ->79.782/80.257ms and stock ls2103.351/2169.020 ->
1984.257/1911.009ms improve on both sides of the reversed order. Existing stat
134.197/138.922 ->114.319/117.117ms and missing stat101.610/100.643 ->
78.210/76.694ms agree. The stock-ls gain exceeds the cycle's5% target, not the
whole-system/startup gain: real X11-ready startup is only -2.99%.

Keep contrary controls. In particular full native Xfe lifecycle is +7.62%,
pure Alpha construction/reap +5.63%, fork/wait +5.00%; do not hide these behind
the metadata result. Repeat the process suite separately, in
after3/before3/before4/after4 order, without pooling this later interval into the
initial table. All four complete with exit0, full process/cache-freshness
footers and no checked errors. The2026-11-03 04:19:20..04:21:15+0900 power audit
also finds no Sleep/DarkWake/Wake transition.

Repeat process medians, six samples per variant (Negoto create12), milliseconds:

| Workload | Before | Locality | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 31.2825 | 28.6145 | -8.53% |
| alpha-lifecycle-busybox-8-construction | 27.6365 | 24.8650 | -10.03% |
| alpha-lifecycle-busybox-8-prepare | 2.9575 | 3.3865 | +14.51% |
| alpha-lifecycle-busybox-8-release | 0.4695 | 0.4770 | +1.60% |
| alpha-lifecycle-xfe-3 | 29.2335 | 30.2880 | +3.61% |
| alpha-lifecycle-xfe-3-construction | 17.4890 | 16.5995 | -5.09% |
| alpha-lifecycle-xfe-3-prepare | 10.9220 | 12.4750 | +14.22% |
| alpha-lifecycle-xfe-3-release | 0.8675 | 0.9485 | +9.34% |
| alpha-reap | 0.4180 | 0.4060 | -2.87% |
| alpha-spawn | 2.6175 | 2.9430 | +12.44% |
| alpha-spawn-reap-8 | 25.2845 | 27.4240 | +8.46% |
| alpha-xfe-reap | 0.7580 | 0.6590 | -13.06% |
| alpha-xfe-spawn | 6.0255 | 5.5140 | -8.49% |
| copyout-128-vmas | 172.8170 | 182.1250 | +5.39% |
| copyout-256-vmas | 177.4030 | 183.6845 | +3.54% |
| copyout-normal | 171.8515 | 173.0700 | +0.71% |
| fork-128-vmas-8 | 20.0760 | 19.8725 | -1.01% |
| fork-256-vmas-8 | 19.5680 | 20.1835 | +3.15% |
| fork-8 | 19.4660 | 20.0685 | +3.10% |
| fork-exec-8 | 672.0260 | 676.2310 | +0.63% |
| getpid-2048 | 38.7625 | 41.2095 | +6.31% |
| getuid-2048 | 39.1315 | 39.7820 | +1.66% |
| negoto-create | 6.9540 | 7.8040 | +12.22% |

The repeat's native Xfe preparation10.922 ->12.475ms (+14.22%) includes
direct VFS reads and source allocation/copy; it is not exclusive filesystem
CPU time. Full lifecycle29.2335 ->30.288ms (+3.61%), while its derived
construction interval is -5.09%. The native fixture runs on CPU3, so its direct
VFS route changes from local to remote when VFS moves to2. This is a plausible
tradeoff supported by routing and elapsed breakdown, not proof that all the
difference is IPC. Repeated Alpha BusyBox construction/reap +8.46%, getpid
+6.31%, fragmented copyout +5.39%/+3.54% and Negoto create +12.22% also remain
visible. Other controls vary in both directions; do not claim a universal gain.

Validation: full native1196/16ignored; ASan62 (boot-policy9, Ext2-directory8,
Linux-directory9, storage36). Test the additional preference on1..4 CPUs,
exact-affinity precedence, preserved automatic order, unrelated priorities and
authority isolation. An actual one-CPU normal process run completes exit0,
full process/cache footers, with every observed affinity0 and no checked
errors. It demonstrates fallback, not a one-CPU speedup.

A separate four-CPU real GUI run completes exit0 with two desktop sessions:
X11/FLWM/wbar, USB mouse drag, keyboard input, dock Terminal/Editor/Information
launch, Exit and restart cleanup all pass. Both Information screenshots are
visually inspected; session2 has no other terminal that could fake its command
marker. Serial nqrks2b5, one-CPU serial9fmr4vnp. Together with comparisons and
repeat controls,18 boots finish; there is no live QEMU/image handle at handoff.

Decision: retain the minimal, editable x86_64 preference for Tiny Core's
metadata workload, with the native bulk-read/control tradeoffs explicit.
This is a7.62% stock-ls and16.54..23.14% metadata/enumeration improvement,
not a universal5% gain or a completed optimization goal. No Kernel/FPU,
permission, protocol, service boundary, version, commit or release change.
Existing saved thirdparty boot images are not overwritten. The normal candidate
image remains /private/tmp/nanami-opt-cycle73-after.img; the accepted baseline
and evidence are kept for subsequent comparison.

Evidence: /private/tmp/nanami-opt-cycle73.jnAVGs/measurements.json preserves
the initial raw samples, complete markers and all control rows.
control-repeat.json preserves the separate repeated samples, native elapsed
breakdown and GUI/one-CPU validation. final-verification.json records artifact
identity and the retained policy. Source/test delta is saved separately as
/private/tmp/nanami-opt-cycle73-metadata-locality-candidate.patch.

## Cycle 74: last-block directory search rejected

Revalidate the retained cycle73 configuration, clean Kernel worktree and frozen
rootfs. Investigate the repeated name search seen in cycle71, not the rejected
cycle72 readdir-cookie checkpoint. Keep Kernel, CPU preferences, client code,
permissions and all service protocols unchanged.

Move the existing lookup into the directory module for direct native coverage,
then try a single process-wide inode/block hint. A successful name search saves
its logical block. The next cold name in the same directory starts there,
compares full names and wraps through the earlier prefix before caching a miss.
Existing metadata-write invalidation clears the hint; directory/inode bounds
are checked before reuse. No per-handle/name table or allocation is added.

The old, mechanically moved implementation passes12 of15 tests; the three
failures concern only extra reads, not returned objects. Adjacent cold names
re-read65 blocks rather than1, and a sequential test reads56649 blocks.
The candidate passes those bounds plus independent mixed-name/mutation and
directory-cookie oracles. Add a further failure test before timing: an unreadable,
unmapped or malformed hinted block must not hide an earlier readable name.
The first prototype fails it. Revise the range scanner to retry the original
full search after a failed hint; errors never populate the negative cache.
Final candidate native1204/16ignored, ASan70 and AArch64 Release checking pass.

Freeze the normal candidate before diagnosis. All21 embedded records compare
equal except Ext2, 158728 ->159120 bytes, SHA28dcd199...9710 ->
b26677d7...dde16. Alpha e025b8a5...7a8d, Kernel312d579b...0c88 and normal
rootfs a6c472f8...5757 are unchanged. Extracted init matches the built init;
the full candidate rootfs partition matches the active rootfs.
Final normal candidate image SHAab07a23f...cb15.

Run before1/after1/after2/before2, each filesystem/communication, process and
three-session real X11-ready startup:12 sequential four-CPU normal-device boots,
all exit0, without own builds/tests/hashes/other VMs overlapping the timed runs.
Every full footer, process and directory cache-freshness guard and startup
cleanup/reuse passes. All directory checks give474 entries, checksum
18414638227522594487, small-calls2560x3 and large-calls384x3.
No checked fatal/proc/SHM/fork/panic/profile errors occur.
No Sleep/DarkWake/Wake transition is found in2026-11-03 04:39:22..04:45:41+0900;
unrelated host load is not measured.

Normal pooled medians, six samples per variant (Negoto create12), milliseconds.
Negative change is less elapsed time, not exclusive CPU or IPC cost:

| Workload | Before | Candidate | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 28.2250 | 29.8930 | +5.91% |
| alpha-lifecycle-xfe-3 | 30.3120 | 33.2295 | +9.62% |
| alpha-reap | 0.3725 | 0.4105 | +10.20% |
| alpha-spawn | 2.5765 | 2.6360 | +2.31% |
| alpha-spawn-reap-8 | 23.9670 | 25.4035 | +5.99% |
| alpha-xfe-reap | 0.5775 | 0.5850 | +1.30% |
| alpha-xfe-spawn | 5.3320 | 5.3545 | +0.42% |
| busybox-ls-8 | 1956.7575 | 1906.9030 | -2.55% |
| copyout-128-vmas | 176.7030 | 175.5010 | -0.68% |
| copyout-256-vmas | 173.9185 | 176.7330 | +1.62% |
| copyout-normal | 177.5170 | 172.8915 | -2.61% |
| directory-large-64 | 80.1675 | 77.4855 | -3.35% |
| directory-small-64 | 453.6165 | 448.0805 | -1.22% |
| fork-128-vmas-8 | 20.1180 | 19.8320 | -1.42% |
| fork-256-vmas-8 | 19.5780 | 17.9860 | -8.13% |
| fork-8 | 17.1675 | 20.2150 | +17.75% |
| fork-exec-8 | 671.8395 | 666.5960 | -0.78% |
| getpid-2048 | 39.7015 | 40.3155 | +1.55% |
| getuid-2048 | 39.4440 | 39.8545 | +1.04% |
| missing-stat-512 | 75.3990 | 75.0660 | -0.44% |
| negoto-create | 6.5275 | 7.3755 | +12.99% |
| startup | 4837.7365 | 4795.8950 | -0.86% |
| stat-512 | 115.4875 | 109.9390 | -4.80% |
| unix-2048 | 379.0955 | 374.4375 | -1.23% |
| unix-recvmsg-2048 | 759.8785 | 741.4025 | -2.43% |
| unix-recvmsg-eof-2048 | 429.5075 | 431.9065 | +0.56% |

Stock ls boot medians1936.890/1976.625 ->1910.157/1903.649ms improve only
modestly; pooled -2.55% and startup -0.86% do not meet the target. Preserve
ordinary fork +17.75%, Negoto create +12.99% and native full Xfe lifecycle
+9.62%, rather than selecting unrelated favorable fragmented-fork/copyout rows.
Warm stat mostly bypasses the modified directory loop; its -4.80% movement
does not establish a lookup-hint gain.

Before discarding the hypothesis, diagnose actual work with the same private
sync-delimited fixture rootfs80f99007...b265 used in cycle71. Add temporary
request/dentry/inode/block counters plus hint starts, misses, metadata writes
and failed hints. Accumulate record visits locally before reporting, not with
an atomic per record. This extra boot exits0; all17 boundaries contain four
groups, request status0/pid17, complete workload/freshness/checksum markers and
no checked errors. Accept space-padded runtime log labels and require the
complete68 groups; an initial parser matching only unpadded labels was fixed.
Sync fences delimit server-global work, not exclusive child ownership or CPU.

Compare only work counts with the prior cycle71 diagnostic. It used the same
private rootfs, but CPU placement differed; it is not a paired speed baseline.
All previously recorded numeric fields match in every window except record
visits/cached block loads/copied bytes for ls7..9 and first stat10.

| Diagnostic window | Directory records, old -> hint | Cached loads, old -> hint | Hint starts / misses |
| --- | ---: | ---: | ---: |
| First stock ls8 | 904378 ->109100 | 34678 ->14805 | 3307 /4116 |
| Warm stock ls8 | 904389 ->109969 | 29379 ->9528 | 3306 /4118 |
| Warm stock ls8 | 904399 ->105019 | 29378 ->9400 | 3318 /4118 |

Warm record visits fall87.84%/88.39%, cached block copies67.57%/68.00%.
Both warm windows have zero device reads, zero metadata writes and zero
failed hints. Thus this is actual work removal, not an unused hint or constant
invalidation. Yet it yields only the modest normal speed result above.
First stat instead examines67 ->76 records and copies4 ->7 cached blocks;
keep that contrary count too. Later hot stat/missing-stat and directory/UNIX
windows retain their old counts. Instrumented elapsed times are not speed
evidence and are not pooled into the normal table.

Name-cache work remains unchanged:31528 queries, 27346 positive/64 negative
hits, 2734595..2772405 read-slot checks and527104 update-slot checks per warm
ls8. This supplies another lead, not proof that those checks dominate CPU.
Cycle54's allocation-free same128-slot hash-chain index (+512 bytes) was
rejected on stat/startup, without this later stock-ls workload. Do not blindly
repeat its old stat/startup experiment or assume gains add; any revisit needs
new current-baseline stock-ls/GUI/process measurements and exact cache semantics.

Decision: reject this standalone runtime/test candidate. Restore main.rs,
directory.rs, block_io.rs and the native directory test exactly to pre-cycle
bytes; remove the temporary statistics module. No lookup refactor/hint,
test/mock expansion or work probe remains. Restored native1196/16ignored,
ASan62 and AArch64 Release checking pass. The rebuilt normal init is byte-exact
to retained cycle73, as are normal Ext2/Alpha/Kernel hashes. Three restored
X11-ready startup/cleanup/reuse sessions pass, serial6zix6_45, at
4400.868/4797.501/4782.026ms, no checked errors/probes. Active rootfs is unchanged.

Keep all candidate/profile source snapshots, both prototype and final ELF/init,
normal measurements, diagnostic work/comparison and logs. Preserve the final
error-safe patch /private/tmp/nanami-opt-cycle74-directory-lookup-final-candidate.patch;
the earlier non-final patch lacks the hint-error fallback and must not be used
as the final candidate. Remove only the three closed, generated trial/profile/
duplicate-restored disk images; their sources/ELFs and normal/private rootfs
remain rebuildable, not byte-identical raw-disk archives. Keep the accepted
/private/tmp/nanami-opt-cycle73-after.img and all saved user thirdparty images.

Evidence: /private/tmp/nanami-opt-cycle74.m8QYMH/measurements.json,
profile-work.json, profile-comparison.json, final-verification.json and
next-candidate.json. Profile serial hw5p9k00; all68 records are retained.
No new runtime gain is adopted. Kernel/ABI/IPC fastpath/FPU/versions and
commit/release state stay unchanged; the broad optimization goal remains active.

## Cycle 75: indexed name cache and combined lookup hint rejected

Keep the accepted cycle73 runtime, frozen normal rootfs and CPU placement as
the baseline. Revisit the cycle54 index only because the later real-ls
diagnostic exposes roughly3.3million cache-slot checks per warm ls8. That
new workload is not evidence of a CPU bottleneck.

Try an allocation-free hash-chain index over the same128 FIFO payload slots:
128 u16 bucket heads plus128 u16 links, exactly512 additional bytes. Full
parent/name equality resolves collisions; parent hashing includes all bits.
Compute the new-key hash once during insertion. Hits/updates do not refresh
FIFO order, and invalidation preserves the old handling of holes, parents,
inode aliases and negative entries. Keep the cache-name limit subordinate to
the filesystem's existing maximum, not the reverse.

Seven tests cover positive/negative names, FIFO updates/holes, all invalidation
forms, maximum/invalid names, the512-byte bound, a forced128-entry collision
chain with removals/eviction/refill, high parent bits, and20000 mixed mutations
against an independent linear oracle. Candidate native1203/16ignored, focused
ASan69 and AArch64 Release checking pass. All21 embedded records match the
baseline except Ext2158728 ->159672 bytes, SHA28dcd199...9710 ->
fde9002a...d1d99; normal image SHAa03f35a5...369.

Run twelve sequential four-CPU normal-device boots in
before1/after1/after2/before2 order, each filesystem/communication, process and
three-session actual X11-ready startup. No own build/test/hash/other VM overlaps.
Every full footer, directory/process cache-freshness guard and startup
cleanup/reuse passes; no checked fatal/proc/SHM/fork/panic/profile errors.
Directory results retain474 entries, checksum18414638227522594487,
small calls2560x3 and large calls384x3. No Sleep/DarkWake/Wake is found in
2026-11-03 05:10:26..05:16:46+0900; unrelated host load is not measured.

Pooled normal medians, six samples per variant (Negoto create12).
Negative change is less elapsed time, not exclusive CPU time:

| Workload | Before ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 30.2165 | 29.2730 | -3.12% |
| alpha-lifecycle-xfe-3 | 32.4410 | 30.7965 | -5.07% |
| alpha-reap | 0.4440 | 0.4750 | +6.98% |
| alpha-spawn | 2.8275 | 2.8775 | +1.77% |
| alpha-spawn-reap-8 | 25.9280 | 26.0990 | +0.66% |
| alpha-xfe-reap | 0.5885 | 0.5965 | +1.36% |
| alpha-xfe-spawn | 5.3625 | 5.5215 | +2.96% |
| busybox-ls-8 | 2003.2555 | 1929.1720 | -3.70% |
| copyout-128-vmas | 173.0640 | 175.9285 | +1.66% |
| copyout-256-vmas | 180.8225 | 175.4635 | -2.96% |
| copyout-normal | 173.2360 | 173.6475 | +0.24% |
| directory-large-64 | 79.6360 | 77.4270 | -2.77% |
| directory-small-64 | 457.5905 | 432.6785 | -5.44% |
| fork-128-vmas-8 | 18.0015 | 19.9675 | +10.92% |
| fork-256-vmas-8 | 18.8120 | 17.7650 | -5.57% |
| fork-8 | 18.7595 | 17.4870 | -6.78% |
| fork-exec-8 | 659.7710 | 659.2120 | -0.08% |
| getpid-2048 | 40.7265 | 39.5700 | -2.84% |
| getuid-2048 | 40.4380 | 39.2835 | -2.85% |
| missing-stat-512 | 77.2060 | 75.1545 | -2.66% |
| negoto-create | 6.7410 | 7.3775 | +9.44% |
| startup | 4887.0835 | 4785.9800 | -2.07% |
| stat-512 | 117.4680 | 113.9185 | -3.02% |
| unix-2048 | 373.5365 | 376.8870 | +0.90% |
| unix-recvmsg-2048 | 761.0545 | 752.1330 | -1.17% |
| unix-recvmsg-eof-2048 | 434.1550 | 438.6135 | +1.03% |

The actual stock-ls target improves only3.70% and startup2.07%; small-buffer
enumeration -5.44% is retained but does not redefine the chosen target.
Fragmented fork +10.92% and Negoto create +9.44% are kept alongside favorable
unchanged controls; none establishes causation by cache indexing.

Then test the same index combined with cycle74's final, error-safe global
last-block lookup hint. Use only the final fallback implementation: an
unreadable/unmapped/malformed late hinted block retries the original full
search, so it cannot hide a readable earlier name or cache a false absence.
The combined candidate passes native1211/16ignored, ASan77 and AArch64 Release
checking, including16 directory tests. All21 records again differ only in Ext2,
now159856 bytes, SHA51c96183...529e; image SHA3f530985...65e0.
Rootfs a6c472f8...5757, Alpha e025b8a5...7a8d and Kernel312d579b...0c88
are unchanged for both candidates.

Use a separate complete twelve-boot normal comparison, not pooled samples
from the first trial and not a sum of independent speedup percentages.
All correctness/cleanup/freshness/checksum/call-count guards pass as above.
No own build/test/hash/other VM overlaps, and no Sleep/DarkWake/Wake is found
in2026-11-03 05:20:21..05:26:40+0900.

| Workload | Before ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 31.2460 | 29.0705 | -6.96% |
| alpha-lifecycle-xfe-3 | 31.5370 | 30.5810 | -3.03% |
| alpha-reap | 0.3690 | 0.3840 | +4.07% |
| alpha-spawn | 2.5885 | 2.5940 | +0.21% |
| alpha-spawn-reap-8 | 23.7165 | 25.0235 | +5.51% |
| alpha-xfe-reap | 0.5920 | 0.6330 | +6.93% |
| alpha-xfe-spawn | 5.4205 | 5.5200 | +1.84% |
| busybox-ls-8 | 1954.7675 | 1904.8115 | -2.56% |
| copyout-128-vmas | 173.5935 | 173.8610 | +0.15% |
| copyout-256-vmas | 180.4650 | 182.0315 | +0.87% |
| copyout-normal | 172.3340 | 175.5640 | +1.87% |
| directory-large-64 | 79.9160 | 78.4420 | -1.84% |
| directory-small-64 | 464.3755 | 447.1080 | -3.72% |
| fork-128-vmas-8 | 19.4455 | 19.7335 | +1.48% |
| fork-256-vmas-8 | 20.6275 | 20.6415 | +0.07% |
| fork-8 | 17.6555 | 19.5605 | +10.79% |
| fork-exec-8 | 677.5740 | 652.5320 | -3.70% |
| getpid-2048 | 40.0675 | 39.4445 | -1.55% |
| getuid-2048 | 39.4555 | 40.1440 | +1.75% |
| missing-stat-512 | 74.4450 | 75.0830 | +0.86% |
| negoto-create | 6.3760 | 6.5175 | +2.22% |
| startup | 4742.8075 | 4896.0015 | +3.23% |
| stat-512 | 112.7060 | 113.1825 | +0.42% |
| unix-2048 | 379.0125 | 377.7960 | -0.32% |
| unix-recvmsg-2048 | 771.3690 | 750.8810 | -2.66% |
| unix-recvmsg-eof-2048 | 435.6070 | 437.6680 | +0.47% |

Combined stock-ls -2.56% remains below the target, startup instead +3.23%,
ordinary fork +10.79% and native spawn/reap +5.51%. Do not select unrelated
favorable native lifecycle or fork-exec rows as proof of a cache speedup.
Neither candidate meets acceptance; keep both negative results and patches.

Before another communication experiment, read the actual user-level and
Kernel IPC paths without modifying them. Ordinary clients already specify
short1..5-word request lengths; x86_64 has six payload register words.
Normal READY_TO_RECEIVE call/direct reply-receive eligibility does not have a
five-word cutoff. Combined reply-receive remains used except for intentional
deferred-notification preservation. TLS initialization usually returns after
the ready atomic load, not another configuration syscall. The proposed
RAX-only fault reply is already implemented and tested in cycle5; ordinary
Linux returns use one Word, with wider changed-PC/signal/FreeBSD handling.
Do not repeat it as a new optimization or trim messages based on a nonexistent
fastpath cutoff. This audit does not measure fastpath hit counts or IPC CPU
share, and produces no new runtime change.

Restore main.rs, directory.rs, the native directory test and test manifest
exactly to their pre-cycle bytes; delete only this candidate's new cache module
and cache test. Existing untracked syscall_profile.rs and all prior changes
are preserved. Restored native1196/16ignored, ASan62 and AArch64 Release
checking pass. The rebuilt normal init is byte-exact to the saved baseline,
and Ext2/Alpha/Alter Linux/FreeBSD/Kernel/rootfs hashes match accepted values.
Three actual X11-ready startup/cleanup/reuse sessions pass, serial6l_ev920,
4442.092/4874.914/4798.476ms, without checked errors/probes. These are restoration
checks, not additional paired speed samples. No new GUI regression is claimed
for candidates that were not adopted.

Keep /private/tmp/nanami-opt-cycle73-after.img as the accepted normal image.
Remove only the two closed candidate disk images and the duplicate restored
disk after verification; preserve source snapshots, ELF/init snapshots,
patches, logs and normal rootfs for rebuilding, not byte-exact raw-disk recovery.
At the user's cleanup request, remove three unused generated
thirdparty/tinycore build*/probe-target caches (about490MiB) and __pycache__;
retain saved boot images, rootfs, downloaded inputs and source. Disk free
space moves roughly4.2 ->4.9GiB during cleanup; APFS/shared extents mean
nominal file sizes are not guaranteed physical reclaimed bytes.
After deleting the verified duplicate-restored image, final free space is
about5.2GiB. No measurement VM or checked image handle remains live.

Evidence: /private/tmp/nanami-opt-cycle75.L5NmEo/measurements.json and
joint-measurements.json preserve every raw sample, footer and contrary row.
final-verification.json records the exact restoration checks and removed paths.
Final snapshots/patches are separately retained. No candidate runtime/test
refactor remains. Kernel/ABI/IPC fastpath/FPU/versions and commit/release state
stay unchanged; the broad optimization goal remains active.

## Cycle 76: separate fork/exec stages and initialization-cache work

The previous goal turn makes progress by closing two failed cycle75 trials,
restoring exact accepted bytes, completing native/ASan/ARM and three normal
startup/reuse checks, preserving negative evidence and reclaiming generated
disk space. No new runtime gain was adopted. Revalidate that current Alpha,
Alter Linux/FreeBSD, Kernel and installed normal rootfs executables match the
accepted cycle73 state before investigating a different path. Keep Kernel,
FPU, permissions, service protocols, scheduling and versions unchanged.

Read the actual loader, private clone, dynamic linker, guest-string snapshot
and fault-reply paths first. Existing file snapshots, contiguous private runs,
page-local argv/env capture and cycle5's one-Word Linux return are already
present. Final-permission construction and several copy-window variants were
previously rejected; do not rename those old ideas as new optimizations.

Add temporary15-stage counters around exec path resolution, argv/env snapshot,
load/translation, interpreter preparation, image replacement, CLOEXEC/runtime
reset, interpreter installation, protection tracking and final stack/label/
register setup; fork separately measures Alpha spawn/VM clone, registers,
managed-child/TID installation, FD inheritance, mapping/memfd inheritance and
remaining setup/resume. Use existing opt-in local timestamps and exit flushes.
They include service waits, interrupts and descheduling, not exclusive CPU
or IPC transport time. Record successful stage completions only, and preserve
their counts alongside syscall counts. Aggregates are global to the single
Alter loop; a trigger PID does not own all work in its flush. Force-killed
live tails can be absent. Do not add nested stage/syscall totals.

The first build uses the frozen normal rootfs and emits no profile records:
Alter is loaded from rootfs /bin/alter-linux, not the core-service CPIO.
That boot's three passing startup checks are not a valid profile or paired
speed result. Correct the artifact path instead of inferring empty work.
Clone the rootfs privately, install the measured ELF and compare its extracted
bytes exactly. Installed normal ELF is also byte-identical to the saved
baseline. e2fsck -fn passes. Original rootfs a6c472f8...5757 stays unchanged.

Correct installed profile Linux SHA649e9760...15a4, private rootfs
SHA540e8689...7324; verify the boot image's full rootfs partition has that
same hash. Initial profile init is byte-identical to baseline; no core CPIO
record changes. Image SHA2bf05369...edfd. Its three actual X11-ready startup/
kill/delete/same-ID reuse sessions pass with no checked fatal/proc/SHM/fork/
panic/exec failure. Serial m1f_6ros. Instrumented startup times are retained
only in raw evidence, never compared against normal performance samples.

An initial parser expected unpadded labels. Fix it to accept the runtime's
space-padded brackets and require all15 stage names and nonzero clone/exec
syscall totals; never accept zero groups as a valid profile. Final evidence
has405 stage records from27 exit flush triggers, covering41 clone and46
exec calls. Counts match across all stages within each operation:

| Stage | Completed calls | Elapsed local-counter ticks | Share of its stage subtotal |
| --- | ---: | ---: | ---: |
| exec-resolve | 46 | 10014000 | 1.59% |
| exec-strings | 46 | 20952000 | 3.34% |
| exec-load | 46 | 39975000 | 6.36% |
| exec-interpreter-prepare | 46 | 10697000 | 1.70% |
| exec-replace | 46 | 409261000 | 65.16% |
| exec-reset | 46 | 3501000 | 0.56% |
| exec-interpreter-install | 46 | 87865000 | 13.99% |
| exec-track | 46 | 28417000 | 4.52% |
| exec-stack | 46 | 17372000 | 2.77% |
| fork-spawn | 41 | 1869156000 | 98.74% |
| fork-registers | 41 | 1191000 | 0.06% |
| fork-install | 41 | 9449000 | 0.50% |
| fork-files | 41 | 10771000 | 0.57% |
| fork-mappings | 41 | 730000 | 0.04% |
| fork-finish | 41 | 1775000 | 0.09% |

Fork subtotal1893072000 ticks covers99.92% of
its outer1894579000 ticks; exec subtotal628054000 covers
99.79% of outer629352000 ticks. These are
same-owner local-counter spans, not a cross-core clock subtraction.
Mmap is1748 calls/3914434000 ticks in the emitted coverage; it includes
nested Alpha work and must not be summed with another profiler's internal
subtotals. Reported syscall counts cover exit flushes, not necessarily every
operation in all three container lifetimes.

Alpha construction/VM copying occupies98.74% of the fork stage subtotal,
FD inheritance0.57%, mapping/memfd metadata0.04%. Image replacement occupies
65.16% of the exec subtotal; argv/env capture is3.34%, not the leading stage.
There is no measured basis here for complicating FD duplication or repeating
pointer-array batching as the next5% startup lever. These are elapsed-handler
ratios, not claims that memory copy alone consumes those shares.

Next, model actual initialization-window accesses without changing its2048
real mappings, indexing, lifetime or MAP/UNMAP failure handling. Replay each
physical-page identity through FIFO2-way and4-way2048-tag tables and a direct
4096-tag table, while counting actual ready/Frame-matching cache hits.
Temporary model metadata is diagnostic-only, not additional guest aliases.
It assumes successful hypothetical fills and does not simulate failure/
rollback costs; do not adopt a cache design solely from these counts.

The second diagnostic keeps the exact installed profile Alter/private rootfs.
Only Alpha changes in all21 embedded records,1381992 ->1391520 bytes,
SHAe025b8a5...7a8d ->f1592c50...4454. Kernel remains312d579b...0c88.
Model image SHA3a583a3a...7f8. Three further actual startup/cleanup/reuse
sessions pass, no checked fatal/proc/SHM/fork/panic errors, serial mkqqmqiw.
There are52 complete, monotonic cumulative model records, with identical
access streams across models; final totals are217752 lookups:

| Policy replay | Slots | Misses | Change from actual direct2048 |
| --- | ---: | ---: | ---: |
| Current actual direct | 2048 | 159748 | 0.00% |
| FIFO 2-way model | 2048 | 156827 | -1.83% |
| FIFO 4-way model | 2048 | 151551 | -5.13% |
| Larger direct model | 4096 | 134959 | -15.52% |

The first report already includes24207 lookups/24029 misses for every policy.
Do not call it a pre-container fence or attribute the subsequent delta to
three complete exclusive startups. The last totals include non-fork page
construction as well. Neither model records exclusive CPU time.

A same-capacity4-way model saves only8197 misses,5.13% of misses, not5.13%
of application time;2-way saves2921. Doubling tags saves24789 misses,15.52%,
but doubles alias coverage from8 to16MiB and consumes another2048 slots in
Alpha's fixed16384-entry bootstrap tracker. Common heap/stack/copy/cache
accounting would leave1024 instead of3072 records for other mappings.
Do not silently spend that resource headroom or claim a normal speedup.
No associativity/capacity production candidate is adopted in this cycle.

The larger remaining opportunity is private-copy volume. Read-only sharing
cannot ignore native Frame/AddressSpace authority, later mprotect upgrades,
supervisor writes, independent lifetime or failed-spawn cleanup. Ask explicitly
whether an optional restricted Linux-guest capability mode may be added,
retaining existing native mode and Linux mmap/mprotect. No permission change,
sharing, COW or new Kernel work is implemented while that choice is pending.
This is a proposed direction, not a promised speedup or completed goal.

Archive the five temporary source changes as
/private/tmp/nanami-opt-cycle76-diagnostics.patch and restore each file exactly.
The pre-existing untracked common/syscall_profile.rs is preserved, not deleted.
Patch forward-check and all five cmp checks pass. Restored native1196/
16ignored, focused ASan113/2ignored and Alter/Linux AArch64 Release checking
pass. Normal rebuilt init is byte-exact to the saved baseline, as are independent
Alpha/Alter Linux/FreeBSD and Kernel hashes; original rootfs stays exact.
Three normal actual X11-ready startup/cleanup/reuse sessions pass, serial
e23nrzvg,4447.493/4885.218/4889.304ms, no profile or checked failure output.
These are restoration checks, not a new paired performance comparison.

Keep accepted /private/tmp/nanami-opt-cycle73-after.img, raw diagnostic ELF/init
snapshots, patch, full serial logs, profile-work.json, model-work.json and the
verified private rootfs gzip. Remove only closed generated profile/model/
duplicate-restored disk images and the archived private raw rootfs after their
handles are terminal and lsof is empty. Sources/inputs remain rebuildable, not
byte-identical raw-disk backups. No live measurement VM remains at handoff.

Evidence directory: /private/tmp/nanami-opt-cycle76.R3SU5o.
Only diagnostic documentation/evidence is retained. Kernel/ABI/IPC fastpath,
FPU, permission policy, default runtime and version/commit/release state remain
unchanged. No new5% end-to-end gain is established; the full optimization goal
remains active.

## Cycle 77: grouped permission metadata rejected

The preceding goal turn makes diagnostic progress: installed executable
identity is verified,15 fork/exec stages and same-stream mapping-cache models
are recorded, all probes are restored, and native/ASan/ARM/three normal startup
checks pass. No production speedup is adopted. The optional restricted-Linux
capability-mode question remains unanswered; do not change authority or share
private pages. Investigate the large mmap path within the existing policy.

Source review finds no repeated whole-file read on a snapshot hit: private file
mmap already reserves unbacked NONE metadata, then initializes from the pinned
snapshot or a short-read-safe stream. Existing copy/initial-permission proposals
were evaluated in prior cycles. Consider another representation: Alpha retains
one BTree permission entry per non-default page even across dense equal-protection
regions. Do not presume its lookup cost dominates the measured copy intervals.

A run-length proposal can require new allocation when removing an interior
page splits a run. Avoid adding that failure point to forget/rollback/reclaim
without a stronger allocator contract. Instead prototype one tree entry for
eight independent permission Words. A named key partitions exact addresses by
within-page offset and eight-page group; even unaligned/full-Word addresses
remain distinct. Default7 still means no explicit permission, NONE retains
backing, and no Frame/content/capability is shared. Clearing an interior value
requires no allocation; all-default blocks are removed. Free an empty BTree
root leaf rather than retaining its enlarged node.

Add three general production-source regressions: exact keys/full-Word values
at page/group/usize boundaries; allocation-denied interior/all-default clears
and Frame removal without touching neighbors; and30000 mixed sparse/unaligned/
high-address mutations against an independent exact-key map. Existing frame,
permission-only, sparse clone and mapping-hint tests remain. All15 ordinary
VM tests also pass against the archived original source override. Retain these
general contracts regardless of the candidate's speed result.

Add an opt-in permission-only host allocation footprint probe for contiguous
and8/64-page-strided entries. It counts requested allocator layout bytes and
calls, excluding Frame AVL data and allocator/physical-RAM overhead. Initially
it incorrectly assumes every empty BTree frees its root immediately: baseline
and first prototype retain192/896 bytes before drop. Record that retention
rather than changing the baseline or treating it as a leak; both free every
allocation on drop. The final candidate explicitly drops its empty root.
Final measured layouts, not whole-system RAM or application timings:

| Permission entries | Page stride | Before bytes | Candidate bytes |
| --- | ---: | ---: | ---: |
| 512 | 1 | 17184 | 9056 |
| 4096 | 1 | 139968 | 76320 |
| 16384 | 1 | 561024 | 309248 |
| 512 | 8 | 17184 | 76320 |
| 4096 | 8 | 139968 | 619392 |
| 16384 | 8 | 561024 | 2481536 |
| 512 | 64 | 17184 | 76320 |
| 4096 | 64 | 139968 | 619392 |
| 16384 | 64 | 561024 | 2481536 |

At16384 contiguous permissions,561024 ->309248 bytes is44.88% less, with2728
->340 allocations. At either sparse stride,561024 ->2481536 is4.42x as much.
This is a serious representation tradeoff, not universal memory savings.
Empty retention is192 ->0; post-drop live bytes are zero for both. Do not
select only the dense row or silently restrict native sparse-address support.

Candidate native1199/17ignored and focused ASan124/5ignored pass. Alpha's
AArch64 Release check passes using the repository target and alpha-entry
feature; two initial command invocations omitted that feature or referenced
a nonexistent generated target and are corrected, not production failures.
All21 embedded CPIO records differ only in Alpha,1381992 ->1400080 bytes,
SHAe025b8a5...7a8d ->81d9d289...2559. Candidate image SHA34638b09...180b.
Normal rootfs a6c472f8...5757, Alter Linux/FreeBSD and Kernel312d579b...0c88
remain exact. Revalidate accepted cycle73 image SHA7bfdd563...8c1 before timing.

Run before1/after1/after2/before2, each filesystem/communication, process and
three-session actual X11-ready startup:12 sequential normal-device four-CPU
boots, all exit0, without overlapping own build/test/hash/other VM. Every
complete footer, process and directory cache-freshness guard, startup cleanup/
same-ID reuse passes. Directory results retain474 entries, checksum
18414638227522594487, small calls2560x3 and large calls384x3. No checked
fatal/proc/SHM/fork/panic/profile errors occur. No Sleep/DarkWake/Wake is found
in2026-11-03 06:01:42..06:08:00+0900; unrelated host load is not measured.

Pooled normal medians, six samples per variant (Negoto create12). These are
elapsed times, not exclusive CPU or IPC cost. Lifecycle prepare/release rows
are nested child intervals and must not be added to their enclosing lifecycle:

| Workload | Before ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 31.0730 | 29.3980 | -5.39% |
| alpha-lifecycle-busybox-8-prepare | 3.3895 | 3.1335 | -7.55% |
| alpha-lifecycle-busybox-8-source-release | 0.5030 | 0.5050 | +0.40% |
| alpha-lifecycle-xfe-3 | 31.5015 | 31.2460 | -0.81% |
| alpha-lifecycle-xfe-3-prepare | 13.2275 | 12.9195 | -2.33% |
| alpha-lifecycle-xfe-3-source-release | 0.9145 | 0.9210 | +0.71% |
| alpha-reap | 0.4090 | 0.4085 | -0.12% |
| alpha-spawn | 2.6710 | 2.6780 | +0.26% |
| alpha-spawn-reap-8 | 24.0590 | 25.2520 | +4.96% |
| alpha-xfe-reap | 0.5880 | 0.5675 | -3.49% |
| alpha-xfe-spawn | 5.3890 | 5.4020 | +0.24% |
| busybox-ls-8 | 1951.3340 | 1962.7125 | +0.58% |
| copyout-128-vmas | 172.2400 | 177.2450 | +2.91% |
| copyout-256-vmas | 175.6985 | 174.7975 | -0.51% |
| copyout-normal | 174.6620 | 173.1575 | -0.86% |
| directory-large-64 | 84.8275 | 84.5370 | -0.34% |
| directory-small-64 | 440.3380 | 444.0435 | +0.84% |
| fork-128-vmas-8 | 18.8305 | 18.0770 | -4.00% |
| fork-256-vmas-8 | 18.2595 | 17.8065 | -2.48% |
| fork-8 | 18.8465 | 17.2740 | -8.34% |
| fork-exec-8 | 660.3050 | 668.8090 | +1.29% |
| getpid-2048 | 39.6640 | 41.0420 | +3.47% |
| getuid-2048 | 39.3530 | 39.3120 | -0.10% |
| missing-stat-512 | 78.1945 | 77.6085 | -0.75% |
| negoto-create | 6.3055 | 6.9080 | +9.56% |
| startup | 4846.6050 | 4752.8465 | -1.93% |
| stat-512 | 115.0480 | 109.8460 | -4.52% |
| unix-2048 | 387.6540 | 370.5745 | -4.41% |
| unix-recvmsg-2048 | 758.4765 | 753.1705 | -0.70% |
| unix-recvmsg-eof-2048 | 428.9695 | 416.4495 | -2.92% |

Startup -1.93% is less than one100-ms observer interval, not the5% target.
Keep stock ls +0.58%, fork-exec +1.29%, Negoto create +9.56% and native pure
spawn/reap +4.96%, along with favorable ordinary fork -8.34% and native
BusyBox lifecycle -5.39%. Ordinary-fork boot medians17.567/19.057 before
versus17.231/17.317 after show why the pooled favorable row alone is not a
whole-system improvement. Native construction mostly uses default permissions;
its favorable movement does not prove a permission-metadata benefit.
Sparse metadata's4.42x footprint cannot be justified by the modest target
startup result. Reject the runtime candidate rather than selecting a passing
sub-workload or redefining the target as only contiguous allocation bytes.

An initial result parser incorrectly matches ns within spawn-ns and misses
alpha-xfe's intervening bytes field. Correct it to require a standalone ns
field and explicit spawn/kill-reap decoding. Assert every existing26 control
metric plus four lifecycle child metrics with correct sample counts.
measurements-final.json is authoritative; measurements.json is retained only
as the initial, invalid alias parse with parser-note.md. Raw serial data and
all substantive target/contrary timing rows are unchanged.

Restore vm_space.rs exactly to its pre-cycle bytes. Keep only the three
baseline-compatible regressions and the generic ignored density/drop probe.
Archive runtime source/ELF/init and the candidate patch; repair a missing
trailing blank context line in that text archive and confirm git apply --check
passes on the restored source. The retained test delta is archived separately.
No grouped-permission type, key normalization, cache or empty-root policy remains
in production. Restored native1199/17ignored, ASan124/5ignored and Alpha ARM
checking pass. The rebuilt normal init is byte-exact to the saved baseline;
independent Alpha/Alter Linux/FreeBSD, Kernel and original rootfs hashes match.

Three restored actual X11-ready startup/cleanup/reuse sessions pass, serial
nf8jwyzb,4504.832/4791.232/4891.734ms, with no checked failure/profile output.
These are restoration checks, not extra paired speed samples. No candidate
GUI validation is claimed for a rejected runtime change.

Keep /private/tmp/nanami-opt-cycle73-after.img as the accepted normal image
and all saved user thirdparty images/rootfs. After terminal handles and empty
lsof checks, remove only the closed candidate and duplicate-restored disk
images. Preserve source/test snapshots, raw ELF/init, patches, footprint.json,
full logs and measurements-final.json; they permit rebuilding but are not
byte-exact raw-disk backups.

Evidence directory: /private/tmp/nanami-opt-cycle77.fHOhau. Runtime patch:
 /private/tmp/nanami-opt-cycle77-permission-block-candidate.patch; retained tests:
 /private/tmp/nanami-opt-cycle77-regressions.patch.
Before another VM metadata representation change, distinguish actual mapping
preflight/protection/content work rather than assuming dense host lookup gains
explain startup. Native sparse behavior and allocation-free release remain
invariants. Capability policy awaits an explicit user choice; it is unchanged.
Kernel/ABI/IPC fastpath/FPU/versions and commit/release state stay unchanged.
No new5% end-to-end gain is established; the full optimization goal stays active.

## Cycle 78: isolate private-file mmap work; remove stale build trees

The cycle77 runtime candidate was rejected and restored. Keep its new general
regressions, the accepted cycle73 image and the existing capability policy.
Do not repeat a VM metadata representation change without evidence separating
content construction from metadata work.

Temporarily add eight opt-in file-mmap intervals and16 protection/fixed shapes.
Counters include failed results, use the existing local diagnostic clock, and
flush outside the sampled handler on ordinary exits. They are global to one
Alter loop, not owned by the PID triggering a flush; killed tails are absent.
File-stat also covers shared delegation while the other stages describe private
construction. Elapsed spans include interrupts, descheduling and service waits;
they are not exclusive CPU time or measurements of Kernel IPC overhead.

Alter is a rootfs application, not part of the core init archive. Install the
exact diagnostic ELF into a private rootfs clone, verify its extracted bytes,
check ext2 with all five read-only e2fsck passes and verify the final disk's
rootfs partition hash. Merely rebuilding Alter against the unchanged normal
rootfs does not test its new probes. Diagnostic Linux ELF SHA-256 starts01f83d,
private rootfs b13a10 and boot disk6c549a; whole init and Alpha remain exactly
normal. The normal rootfs a6c472 is never overwritten.

Three real instrumented X11-ready/cleanup/reuse sessions pass (serial ejlsiy9_).
The parser requires all eight padded log labels and nonempty data:191 stage
records over27 exit triggers. Captured outer syscall9 is1758 calls and
4178675000 ticks. Stage aggregates are:

| Stage | Calls | Elapsed ticks | Logical bytes | Errors |
| --- | ---: | ---: | ---: | ---: |
| file-stat | 1477 | 67524000 | 372037184 | 0 |
| reserve | 1441 | 310002000 | 373051392 | 0 |
| preflight | 1441 | 2544000 | 373051392 | 0 |
| source | 1441 | 380677000 | 361988936 | 0 |
| cached-init | 1425 | 2330732000 | 350490624 | 0 |
| stream-init | 16 | 355541000 | 22560768 | 0 |
| hardware-protect | 1441 | 328272000 | 373051392 | 0 |
| metadata-commit | 1441 | 12998000 | 373051392 | 0 |

File-stat is1.62% and preflight plus metadata commit0.37% of the captured outer
mmap elapsed total; cached initialization is55.78%, reserve7.42%, source9.11%,
stream8.51% and hardware protection7.86%. These have different populations from
the outer anonymous/shared/private aggregate. Do not turn these ratios into
whole-startup CPU fractions or sum nested syscall/stage subtotals.

Private shapes are361 nonfixed R mappings (195510272 mapped bytes),358 fixed R
(59756544),361 fixed RW (4874240) and361 fixed RX (112910336). All1441 captured
private completions succeed. Whole-file R followed by fixed segment mappings is
a possible repeated-work lead, not permission to retain old private bytes:
MAP_FIXED must discard dirty content and existing native Frame authority can
modify apparently readonly pages. No copy elision, sharing or new capability
restriction is implemented.

Try a separate read-only host QEMU plugin against the accepted normal image,
identifying Alpha by exact immutable text plus CR3 rather than low PC values
shared by different services. Local SDK register enumeration exposes cr3, but
the complete execution run fails its validity gate: root0,483938195 failed or
unexpected-length register reads and zero counted Alpha blocks in every phase.
All three guest startups still pass (serial y5b5fp4j); that is not a successful
profile. Preserve tb-profile-invalid.json and the raw TSV/log/plugin sources;
reject every CPU/work conclusion and all speed comparisons from this run.
No Kernel/FPU or guest-memory write is used for this host diagnostic.

Restore vm.rs and the pre-existing untracked syscall_profile.rs byte-for-byte;
do not delete the latter. Archive the probe patch with intact trailing context
and verify git apply --check on the restored files. Normal rebuilt init,
Alpha and Linux ELF match their saved bytes; independent FreeBSD, Kernel312d57
and original rootfs a6c472 hashes match. Native1199/17ignored, ASan124/5ignored
and Alter ARM checking pass. Three further normal X11-ready/cleanup/reuse
sessions pass, serial oka2kvwo,4404.003/4888.604/4794.212ms, with no checked
failure or profile output. These verify restoration, not paired speed gains;
host archive work overlaps part of this check. Its harness exits successfully.

The user explicitly permits removing stale thirdparty Tiny Core build files.
Resolve exact untracked nonsymlink directories and check that no process has
them open; remove only thirdparty/tinycore/build and build-desktop (about732MiB
allocated before removal). Keep build-desktop-process, its current rootfs and
boot disk, pinned downloads, profiles and source. These two older disposable
trees can be rebuilt, but guest writes within their removed images are not
preserved. This latest explicit cleanup supersedes the earlier blanket policy
of keeping old thirdparty images, only for these identified obsolete trees.

Evidence directory: /private/tmp/nanami-opt-cycle78.Sul0bL. The diagnostic patch
is /private/tmp/nanami-opt-cycle78-mmap-diagnostic.patch. Keep raw ELFs, init,
source snapshots, logs, valid mmap counts and the invalid host-profile record.
Compress and hash-check the private profile rootfs before removing closed raw
trial images. The gzip decompresses exactly to rootfs b13a10. Empty lsof checks
and terminal handles permit removal of the owned321MiB profile disk,321MiB
duplicate-restored disk and256MiB private raw rootfs; the37MiB gzip, raw ELFs
and source patch remain. Accepted cycle73 and the current user disk are kept.
No production optimization, Kernel/ABI/IPC fastpath, authority,
FPU, version, commit or release change is adopted; no new5% gain is established.

## Cycle 79: valid CR3-tagged execution counts; reject AVL height guard

Cycle78 makes diagnostic and cleanup progress, restores every probe and exact
normal executable, and keeps the accepted cycle73 runtime. Its mmap spans do
not establish exclusive CPU time. Repair the separate read-only host profiler
before selecting another data structure from elapsed RPC timing alone.
Alter's small preflight/commit intervals do not rule out Alpha's per-page
bookkeeping inside the initialization RPC. Distinguish the service work from
the Kernel IPC transition rather than assigning a whole RPC span to IPC cost.

The host's QEMU11.1.1 API7 register-read buffer must be reset between reads.
A bare-QEMU probe confirms repeated successful eight-byte CR3 reads when its
GByteArray is cleared. Clear both that buffer and the prefix buffer before
the full-text ownership check; do not change guest memory, the Kernel or FPU.
Run the repaired private host plugin against accepted image7bfdd5. Identify
Alpha by all360437 immutable text bytes at its ELF text base, then require
that vCPU's matching CR3 for every counted block. Low PC values alone would
confuse different native services.

All three actual X11-ready/cleanup/reuse sessions pass, serial msf7mq1v.
Validity gates pass: discovered CR3=0x5000, three phase markers, zero register
read failures, zero wrong-core Alpha blocks and nonempty phases whose bin
totals equal the recorded weights. Phase0 includes boot and the first session:
184942592 block entries/869481785 translated-instruction-weighted entries.
Warm phases1/2 record148877312/150142976 blocks and697803777/703386166 weights.
The four visible StaticAvlTree<16384> functions insert_at/remove_at/rebalance/
take_min together account for26.07%/25.53% of those warm weighted entries.
process_frame_for_page is4.87%/4.90%, PhysicalAllocations::take5.77%/5.83%.

These are TB-entry weights, NOT retired instructions, CPU-time shares,
exclusive file-mmap attribution or speedups. REP/faulting TBs can distort
instruction weighting. Sixteen-byte bins approximate symbol boundaries; a
shared memcpy/helper belongs to all its callers. The marker observer may lag
by4095 entered Alpha TBs; markers follow teardown, so these are broad session
windows, not exact exclusive startup fences. Preserve raw TSV/logs and
tb-profile.json. No normal timing comparison includes this plugin.

Prototype a user-level static-AVL height guard: after a child mutation, skip
ancestor height recomputation/rebalancing if that child's height is unchanged.
This is valid even when a rotation changes the child's root. Keep public APIs,
fixed capacity, free-list reuse, failure behavior, key/value widths and object
layout unchanged. No new heap allocation, mapping cache, capability policy,
message protocol or Kernel operation is added. Do not modify the separate
dynamic AVL tree in the same experiment.

Add three baseline-compatible regressions and a private native-only invariant
walker. Check all14400 pairs of five-key insertion/deletion permutations,
including0/usize::MAX keys;50000 mixed mutations against BTreeMap; and zero/full
capacity, failed insertion, updates and repeated refill. After every tested
mutation verify ordering, exact stored heights, AVL balance, unique live/free
ownership and complete accounting of initialized slots. These tests also pass
with the original production source override. Candidate native1202/17ignored,
focused ASan128/6ignored and Alpha ARM checking pass.

Build a normal candidate with every profiler unset and the pinned epoch.
Compare all21 core archive entries: only ./nanami/alpha changes, e025b8 ->
cef644. Candidate disk2d9af6 retains normal rootfs a6c472 and unchanged Kernel.
An initial archive parser finds an instruction/string occurrence of070701;
reject that parse and require a complete104-hex-digit newc header plus all21
records/trailer before accepting identity. No timing run uses that invalid
parse. Source/test/ELF/init snapshots are retained under /private/tmp/nanami-opt-cycle79.taeQ5B.

Twelve ordinary boots use process/UNIX/startup profiles in ABBA image order,
four vCPUs, normal PS/2/user networking, one VM at a time and no own build/test/
hash jobs overlapping timings. UTC window 2026-11-02T21:47:17.896727+00:00 through
2026-11-02T21:53:38.363411+00:00. Every boot passes. Check actual raw serial, strip
OSC markers, require standalone ns fields (never the suffix of spawn-ns), all30
existing metrics with their sample counts, and retain all six frame controls.
Directories retain474 entries/FNV18414638227522594487 and2560/384 getdents calls;
cache-freshness, process, smoke and startup cleanup/same-ID reuse gates pass.
Each main row pools six samples per image, Negoto12; do not discard cold values.

Results in milliseconds; negative change means shorter elapsed time:

| Metric | Before | Candidate | Change | Independent process repeat |
| --- | ---: | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 29.7500 | 29.1940 | -1.87% | -6.54% |
| alpha-lifecycle-busybox-8-prepare | 3.2655 | 3.4955 | 7.04% | -10.44% |
| alpha-lifecycle-busybox-8-source-release | 0.4265 | 0.5250 | 23.09% | 10.05% |
| alpha-lifecycle-xfe-3 | 31.5915 | 31.2475 | -1.09% | -1.73% |
| alpha-lifecycle-xfe-3-prepare | 12.9075 | 12.9005 | -0.05% | -4.61% |
| alpha-lifecycle-xfe-3-source-release | 0.8490 | 1.0705 | 26.09% | 30.46% |
| alpha-reap | 0.4060 | 0.3570 | -12.07% | 9.03% |
| alpha-spawn | 2.8530 | 2.4330 | -14.72% | 11.33% |
| alpha-spawn-reap-8 | 27.0670 | 24.0565 | -11.12% | 4.13% |
| alpha-xfe-reap | 0.6690 | 0.6250 | -6.58% | 6.00% |
| alpha-xfe-spawn | 5.7355 | 5.3855 | -6.10% | -3.22% |
| busybox-ls-8 | 1940.8895 | 1958.6585 | 0.92% | — |
| copyout-128-vmas | 170.3590 | 170.3000 | -0.03% | 3.11% |
| copyout-256-vmas | 172.2135 | 176.3340 | 2.39% | 0.84% |
| copyout-normal | 170.8450 | 170.9870 | 0.08% | 1.50% |
| directory-large-64 | 77.3220 | 76.5485 | -1.00% | — |
| directory-small-64 | 442.8455 | 433.2580 | -2.16% | — |
| fork-128-vmas-8 | 22.4720 | 18.2860 | -18.63% | -1.57% |
| fork-256-vmas-8 | 19.4475 | 18.0920 | -6.97% | 1.29% |
| fork-8 | 17.8090 | 19.9880 | 12.24% | 13.27% |
| fork-exec-8 | 675.1815 | 677.3760 | 0.33% | -0.34% |
| getpid-2048 | 40.5730 | 39.8975 | -1.66% | 0.17% |
| getuid-2048 | 39.8795 | 39.4680 | -1.03% | -0.97% |
| missing-stat-512 | 76.8850 | 77.3415 | 0.59% | — |
| negoto-create | 7.6805 | 6.9170 | -9.94% | 13.02% |
| startup | 4948.0280 | 4882.5255 | -1.32% | — |
| stat-512 | 109.6135 | 113.1705 | 3.25% | — |
| unix-2048 | 366.6470 | 359.0960 | -2.06% | — |
| unix-recvmsg-2048 | 746.0195 | 761.2490 | 2.04% | — |
| unix-recvmsg-eof-2048 | 422.9790 | 426.3980 | 0.81% | — |

The unchanged frame routines are additional controls, eight samples per image
per row,120 frames each. All damage counts match their existing fixtures:

| Frame control | Before ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| workload-0-legacy-false | 63.0505 | 63.1040 | 0.08% |
| workload-0-legacy-true | 64.8635 | 62.4635 | -3.70% |
| workload-1-legacy-false | 61.5695 | 61.6735 | 0.17% |
| workload-1-legacy-true | 62.6855 | 62.1390 | -0.87% |
| workload-2-legacy-false | 116.7180 | 117.9515 | 1.06% |
| workload-2-legacy-true | 120.4730 | 119.1760 | -1.08% |

The first favorable pure Alpha construct/reap result (-11.12%) is not enough:
ordinary Linux fork worsens12.24%, while actual desktop startup improves only
1.32%. Run a separate process-only BAAB comparison rather than selecting the
constructor result. Four further boots all pass, UTC 2026-11-02T21:54:42.956103+00:00
through 2026-11-02T21:56:25.634976+00:00. Recheck all21 process metrics and freshness
gates, six samples per image (Negoto12), without pooling it into the first trial.
Ordinary fork again worsens13.27%; pure Alpha construct/reap instead worsens
4.13%, Negoto create13.02%. The repeat's favorable BusyBox lifecycle -6.54% and
all contrary child-stage numbers remain in repeat/measurements.json.
The matching macOS power-log lines contain PreventSleep assertions, not actual
Sleep/DarkWake/Wake transitions; no transition is observed for the trial window.
This does not prove an idle host or quantify unrelated host load.

Reject the runtime candidate. Do not attribute the regression to a particular
branch, cache or IPC path without further evidence. Restore static_avl.rs
byte-for-byte; retain only the three general regressions/invariant walker.
Archive the runtime patch and test delta separately, checking forward/reverse
application on the appropriate restored/retained sources. Rebuilt Alpha and
whole init match their saved normal bytes. Restored full native1202/17ignored,
focused ASan128/6ignored and Alpha ARM checking pass. Three actual normal
X11-ready/cleanup/reuse sessions pass, serial u8k1ziun,
4312.254/4882.723/4900.889ms, with no checked failures/profile output. These
are restoration checks, not extra paired speed samples; the live harness
exits successfully. Kernel312d57, rootfs a6c472 and both Alter ELF hashes
remain exact. Full restoration gates are retained in final-verification.json.

New evidence changes the next investigation: separate physical-allocation-list
lookup misses from successful removal/reuse before adding another index.
Current mapping_release.rs already removes fresh page references in reverse
VA order; do not propose that implemented optimization again. Whole-extent
deferred reuse and page-sized records can require different searches. The
profile is a lead, not proof that these scans dominate wall time or that
reusing/sharing a backing page is safe. Preserve ownership, private contents,
rollback and existing native authority.

Keep the accepted cycle73 normal image and current user rootfs/build. Remove
only closed owned candidate/duplicate-restored disks after snapshots, terminal
handles and empty lsof checks; patches/ELFs can reconstruct them, not restore
their exact raw-disk bytes. No candidate GUI success is claimed for rejected
code. Kernel/ABI/IPC fastpath/FPU, capability policy and version/commit/release
state remain unchanged. No new5% end-to-end gain is established; the full goal
remains active.

After terminal handles and empty lsof checks, remove only the two owned321MiB
cycle79 candidate/restored raw disks. Accepted cycle73 and the current user's
build/rootfs/downloads are retained; source/ELF/init/patches and full evidence
remain, but the removed raw disks are not byte-for-byte archived.

## Cycle 80: classify allocation-list scans; evaluate per-owner extent counts

Cycle79 is progress, not a retained runtime speedup: it obtains valid
CR3/full-text-tagged Alpha work counts, rejects the AVL height guard after an
independent fork regression, restores exact runtime/ELFs and retains three
general regressions. Kernel/capability policy and accepted cycle73 stay unchanged.
The work profile identifies PhysicalAllocations::take as a new lead, not an
exclusive CPU-time measurement. Current release already walks fresh page
references in reverse VA order; inspect the remaining searches instead.

Temporarily instrument four user-level files: physical_allocations.rs,
process.rs, alpha/process_requests.rs and alpha/exec_profile.rs. With the
existing NANAMI_EXEC_PROFILE=1 switch, count find/take list visits separately
for active/deferred ownership, one/many requested pages and success/failure.
Record calls, single-page nodes, wrong-size records at the requested VA,
requested-page totals and maximum scan length. Flush global manager counters
after completed reap, outside the sampled lookup, not as ownership attributed
to the triggering PID. Killed/unflushed tails may be absent. The diagnostic
Cell fields and serial output perturb execution; no elapsed values from this
run are speed comparisons.

Build/profile identity gates: all21 core records are parsed through real
newc headers and trailer; only Alpha changes, e025b8 -> c8fc83. Profile disk
75b7e1 uses unchanged rootfs a6c472 and Kernel312d57. Alpha is a core archive
member; no replacement rootfs is needed here. Three actual X11-ready/cleanup/
same-ID-reuse sessions pass, serial2wv9r1h4, with no checked failures.
Parse149 allocation.work rows across34 completed-reap triggers, requiring
nonempty active-find and deferred-take groups and bounded visit counters:

| Operation / class / result | Calls | Nodes visited | Single-page nodes | Same VA, wrong size | Maximum scan |
| --- | ---: | ---: | ---: | ---: | ---: |
| active.find.many.false | 1321 | 2302591 | 2302561 | 1306 | 5033 |
| active.take.one.true | 50956 | 6498399 | 6498334 | 0 | 2778 |
| deferred.take.one.false | 227 | 644592 | 644592 | 0 | 3837 |
| deferred.take.many.false | 2841 | 2908882 | 2908882 | 1516 | 3837 |
| active.find.one.true | 436 | 6701 | 6700 | 0 | 2606 |
| deferred.take.one.true | 165 | 165 | 165 | 0 | 1 |
| active.find.one.false | 54 | 203637 | 203637 | 0 | 3772 |
| active.find.many.true | 14 | 1851 | 1834 | 0 | 1826 |
| active.take.many.true | 14 | 1851 | 1834 | 0 | 1826 |

Total captured visits are12568669. Failed many-page searches visit5211473
nodes;5211443 of these are single-page records. This is work-count evidence,
not41% of CPU or startup time. Some owners genuinely have multi-page extents:
14 successful active find/take requests must retain their exact fallback.
Successful one-page take still visits6498399 nodes; this proposal does not
remove those scans or change private-page copy work.

Prototype a coarse exact eligibility summary in the existing PID index:
Owner stores the list head and the number of records whose page_count >1.
A many-page find/take can stop before walking the list only when that count
is zero. Other sizes keep exact base/size matching and newest-first semantics.
Update the count on insertion and individual removal; remove the whole
summary with its PID. It counts references/records, not unique physical RAM
or total pages. Keep memory-owner PID normalization, global physical reference
counts, duplicate mappings and shared-extent lifetime unchanged. No new page
index, kernel call, IPC operation, capability restriction or shared content
is introduced. Generalize the private Index value type, not its key/probing/
backward-shift deletion algorithms.

Initial Owner uses two usize fields. Per-page Node remains48 bytes, but
Option<(PID,Owner)> grows24 ->32 bytes in the native64-bit layout assertions.
New regressions test exact mixed sizes/duplicates/PID locality,20000 mixed
operations against an independent Vec model, and count updates for head/
middle/tail removal and PID reuse. Two API-only regressions also pass against
the untouched original implementation (five standalone tests total).
Initial native1208/17ignored, core ASan104/2ignored and Alpha ARM checks pass.
The runtime is frozen after formatting; a later native-only generic-value
collision test is not claimed as an executed initial-layout test.

Restore every diagnostic source exactly before prototyping; archive its
baseline-relative patch and check it on that restored state. Normal profiler-
unset builds change only Alpha. Initial candidate Alpha30fb03/disk99a20b,
rootfs a6c472 and Kernel312d57 are independently identified. Twelve normal
ABBA boots use4 vCPUs, normal PS/2/user networking, one VM at a time and no own
build/test/hash jobs during timings. Actual serial parsing retains all30
metrics and six frame controls, three samples/boot (Negoto six), including
cold samples. Directory474/FNV18414638227522594487 and2560/384 call counts,
freshness, process/smoke and startup cleanup/reuse gates all pass.

The initial layout is not adopted: startup +1.28%, ordinary fork +14.25%,
pure Alpha construct/reap +7.87%, despite favorable fragmented-fork movement.
Do not assume either the field size or an ABI cost caused these regressions.
Symbol inspection does not show an out-of-line Index::get for either value
type; do not claim a measured sret/call overhead.

A second representation uses a documented one-based NonZeroUsize node head.
Node index0 is encoded as1; no physical address or capability is encoded here.
Zero provides Option's empty state, allowing the additional count without
increasing PID-index slot size. Owner::new/head_index keep conversion local,
rather than spreading masks or implicit tagged integers. Vec node indices
cannot reach usize::MAX. Index's empty Default requires neither K nor V to
have a dummy Default value. Both PID-slot and per-page Node sizes match the
original in native layout assertions and ARM compilation.

Add an explicit colliding two-word Owner-value index test for growth, wrapped
clusters, backward-shift deletion and reinsertion. It belongs to the second
layout's validation. Compact native1210/17ignored, core ASan105/2ignored and
Alpha ARM checks pass. Compact source/ELF/init snapshots are saved separately;
Alpha3af688/disk f4ba26 change only the Alpha core record. Rootfs/Kernel stay
exact, and every diagnostic flag is unset for normal comparisons.

A separate12-boot compact ABBA comparison passes all the same gates.
Main results are milliseconds (before / candidate); negative change is shorter.
The two columns use their own matched baselines, not a direct cross-layout
timing comparison:

| Metric | Initial layout before / after | Change | Compact before / after | Change |
| --- | ---: | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 30.8135 / 29.4955 | -4.28% | 30.2345 / 28.4190 | -6.00% |
| alpha-lifecycle-busybox-8-prepare | 3.4715 / 3.3185 | -4.41% | 3.1615 / 3.0755 | -2.72% |
| alpha-lifecycle-busybox-8-source-release | 0.4755 / 0.4705 | -1.05% | 0.4645 / 0.5045 | 8.61% |
| alpha-lifecycle-xfe-3 | 30.8825 / 32.5370 | 5.36% | 30.4760 / 30.7720 | 0.97% |
| alpha-lifecycle-xfe-3-prepare | 12.5880 / 13.6445 | 8.39% | 12.7900 / 12.5395 | -1.96% |
| alpha-lifecycle-xfe-3-source-release | 0.8775 / 1.0050 | 14.53% | 0.8855 / 0.9720 | 9.77% |
| alpha-reap | 0.3785 / 0.3950 | 4.36% | 0.3830 / 0.3790 | -1.04% |
| alpha-spawn | 2.5540 / 2.8270 | 10.69% | 2.7700 / 2.5760 | -7.00% |
| alpha-spawn-reap-8 | 23.3475 / 25.1850 | 7.87% | 26.1580 / 24.1525 | -7.67% |
| alpha-xfe-reap | 0.6605 / 0.6540 | -0.98% | 0.6355 / 0.5640 | -11.25% |
| alpha-xfe-spawn | 5.4565 / 6.5450 | 19.95% | 5.6440 / 5.2130 | -7.64% |
| busybox-ls-8 | 1950.0600 / 2004.1715 | 2.77% | 1975.4805 / 1969.4435 | -0.31% |
| copyout-128-vmas | 172.8730 / 180.2375 | 4.26% | 174.0390 / 173.8840 | -0.09% |
| copyout-256-vmas | 178.1610 / 178.7960 | 0.36% | 175.8880 / 171.4715 | -2.51% |
| copyout-normal | 171.2340 / 177.3740 | 3.59% | 169.5585 / 170.0570 | 0.29% |
| directory-large-64 | 77.5785 / 80.5840 | 3.87% | 77.4340 / 77.7050 | 0.35% |
| directory-small-64 | 445.1695 / 438.1280 | -1.58% | 435.1035 / 445.8080 | 2.46% |
| fork-128-vmas-8 | 20.1345 / 18.7440 | -6.91% | 18.1845 / 17.4870 | -3.84% |
| fork-256-vmas-8 | 19.5735 / 20.4470 | 4.46% | 18.2205 / 18.1085 | -0.61% |
| fork-8 | 18.3400 / 20.9535 | 14.25% | 18.5200 / 18.0190 | -2.71% |
| fork-exec-8 | 669.5210 / 678.0720 | 1.28% | 669.0515 / 643.5100 | -3.82% |
| getpid-2048 | 39.2935 / 40.1580 | 2.20% | 39.6675 / 38.5535 | -2.81% |
| getuid-2048 | 38.7835 / 39.2290 | 1.15% | 39.3150 / 38.3545 | -2.44% |
| missing-stat-512 | 80.2365 / 81.2480 | 1.26% | 76.4805 / 76.9245 | 0.58% |
| negoto-create | 7.1155 / 7.3425 | 3.19% | 6.8995 / 6.7970 | -1.49% |
| startup | 4787.6660 / 4849.0505 | 1.28% | 4895.1590 / 4744.1375 | -3.09% |
| stat-512 | 112.9260 / 113.4495 | 0.46% | 110.9890 / 110.4100 | -0.52% |
| unix-2048 | 378.8190 / 384.9910 | 1.63% | 380.1795 / 370.7515 | -2.48% |
| unix-recvmsg-2048 | 757.9135 / 752.1830 | -0.76% | 790.4825 / 742.1495 | -6.11% |
| unix-recvmsg-eof-2048 | 438.9825 / 436.0055 | -0.68% | 434.9590 / 431.0965 | -0.89% |

Keep all six frame-control rows in each measurements.json too. Compact initial
startup -3.09% and fork-exec -3.82% coincide with movement in unchanged getpid/
getuid and frame controls; do not treat these as established causal speedups.
Pure construction's pooled favorable -7.67% also has contradictory boot
medians. Run a full independent compact BAAB repeat, preserving all30 metrics
and frame controls, before deciding whether to retain this code. Its terminal
result, final disposition and restoration/GUI gates are recorded after the
specific live handles finish.

Evidence directory: /private/tmp/nanami-opt-cycle80.6SwJ3j. Diagnostic patch:
 /private/tmp/nanami-opt-cycle80-allocation-diagnostic.patch.
Profile, initial-layout, compact and independent-repeat evidence stay separate.
Kernel/ABI/IPC fastpath/FPU and capability policy remain unchanged; no version,
commit or release action is requested or performed.

The independent compact BAAB repeat finishes successfully:12 further normal
boots, all30 metrics and six frame-control groups, UTC
2026-11-02T22:29:54.893406+00:00 through 2026-11-02T22:36:15.262555+00:00.
Keep it separate from compact/measurements.json; do not pool trials to rescue
a favorable row. Repeat main medians are milliseconds, six samples per image
(Negoto12):

| Metric | Before | Compact | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 30.9840 | 28.7275 | -7.28% |
| alpha-lifecycle-busybox-8-prepare | 3.7090 | 3.2500 | -12.38% |
| alpha-lifecycle-busybox-8-source-release | 0.4425 | 0.5055 | 14.24% |
| alpha-lifecycle-xfe-3 | 30.8190 | 30.6945 | -0.40% |
| alpha-lifecycle-xfe-3-prepare | 12.8855 | 12.6200 | -2.06% |
| alpha-lifecycle-xfe-3-source-release | 0.9750 | 1.0105 | 3.64% |
| alpha-reap | 0.3975 | 0.4690 | 17.99% |
| alpha-spawn | 2.6940 | 2.7150 | 0.78% |
| alpha-spawn-reap-8 | 25.8205 | 26.5685 | 2.90% |
| alpha-xfe-reap | 0.5810 | 0.6760 | 16.35% |
| alpha-xfe-spawn | 5.4455 | 5.6375 | 3.53% |
| busybox-ls-8 | 1996.6365 | 1986.4460 | -0.51% |
| copyout-128-vmas | 173.6030 | 170.8370 | -1.59% |
| copyout-256-vmas | 172.7725 | 180.6595 | 4.56% |
| copyout-normal | 171.5530 | 172.6090 | 0.62% |
| directory-large-64 | 79.3025 | 80.2780 | 1.23% |
| directory-small-64 | 450.8460 | 443.1040 | -1.72% |
| fork-128-vmas-8 | 20.2195 | 19.8150 | -2.00% |
| fork-256-vmas-8 | 18.9195 | 18.6200 | -1.58% |
| fork-8 | 17.6775 | 20.3610 | 15.18% |
| fork-exec-8 | 676.7945 | 689.0545 | 1.81% |
| getpid-2048 | 40.1535 | 39.6050 | -1.37% |
| getuid-2048 | 40.1615 | 39.4720 | -1.72% |
| missing-stat-512 | 79.4880 | 75.5795 | -4.92% |
| negoto-create | 6.9065 | 7.2225 | 4.58% |
| startup | 4892.6005 | 4895.3525 | 0.06% |
| stat-512 | 116.1325 | 109.8670 | -5.40% |
| unix-2048 | 380.6350 | 385.5260 | 1.28% |
| unix-recvmsg-2048 | 748.3785 | 764.6130 | 2.17% |
| unix-recvmsg-eof-2048 | 429.3695 | 435.0795 | 1.33% |

Repeat frame controls, eight samples per image for120 frames:

| Frame control | Before | Compact | Change |
| --- | ---: | ---: | ---: |
| workload-0-legacy-false | 61.6985 | 62.2525 | 0.90% |
| workload-0-legacy-true | 62.3240 | 63.3420 | 1.63% |
| workload-1-legacy-false | 62.1895 | 61.9105 | -0.45% |
| workload-1-legacy-true | 61.6800 | 61.8425 | 0.26% |
| workload-2-legacy-false | 118.4440 | 116.9955 | -1.22% |
| workload-2-legacy-true | 119.0070 | 117.9115 | -0.92% |

Actual startup is essentially unchanged (+0.06%), fork-exec +1.81% and ordinary
fork +15.18%. The fork boot medians reveal a notably slower second candidate
boot; retain it rather than trimming it. The repeat's favorable BusyBox
lifecycle -7.28%, stat -5.40% and every contrary source-release/reap measurement
remain visible. Pure Alpha construction's initial pooled improvement does not
repeat (+2.90%). The independent run does not establish the targeted startup
gain. Reject the compact runtime too; neither size nor an sret/ABI explanation
is proven to cause a particular timing result. Baseline visit counts are an
opportunity bound, not a claimed measured reduction in the uninstrumented
candidates.

Across both layouts and the independent repeat,36 ordinary comparison boots
pass all checked freshness, directory identity/call-count, process, smoke and
X11-ready/cleanup/reuse gates. Matching macOS log lines are display/sleep-
prevention assertions, not observed Sleep/DarkWake/Wake transitions in the
windows; no own build/test/hash jobs overlap timed comparisons. Other host
load is not measured or excluded by this check.

Restore the complete production prefix of physical_allocations.rs to its
saved bytes, including the original usize-valued Index. Keep only sample/
fields helpers and the two baseline-compatible API regressions under cfg(test).
Remove the Owner type, counters, encoding/conversion, generic-value runtime
change and two candidate-specific count/layout/collision tests. Other three
diagnostic source files are byte-exact. No profiling probe remains beyond the
unchanged pre-existing opt-in framework.

Archive the complete compact candidate separately from its runtime-only delta.
The runtime-only patch applies to the restored source with retained tests;
the retained-regression patch reverse-checks on that source. Baseline-only
diagnostic patch was checked before prototyping. Current full native
1206/17ignored (two new tests compiled by two fixtures), focused core ASan
103/2ignored and Alpha ARM checks pass. Rebuilt Alpha e025b8 and whole init
match the saved normal files exactly; Kernel312d57, rootfs a6c472 and Linux/
FreeBSD ELF identities remain exact.

Three further actual normal startup/cleanup/same-ID-reuse checks pass,
serial hc72edxl,4400.642/4782.512/4999.440ms, with no checked error/profile
output. Their harness is terminal0. These are restoration checks, not
additional paired speed samples, and no candidate GUI pass is claimed.

The new next lead is successful one-page unlinking:6498399 of captured
visits remain outside the coarse filter. Before adding a permanent per-page
index or wider nodes, inspect a request-local linear/batch removal using the
existing ownership snapshot and unchanged per-page record layout. Preserve
newest-first duplicate selection, exact reference counts, shared extents,
sparse/NONE backing, prevalidation and the current unmap-before-metadata-
transfer failure boundary. Do not re-propose reverse-VA release, already
implemented. Large private-page copies and bootstrap bookkeeping remain
separate leads; this experiment does not prove either one's wall-time share.

Keep accepted cycle73 and the user's current build/rootfs/downloads. After
terminal handles, empty lsof checks and source/ELF/init/patch retention, remove
only the four closed owned321MiB cycle80 profile/initial/compact/duplicate-
restored raw disks. Evidence can rebuild their code but is not a byte-exact
raw-disk backup. Kernel/ABI/IPC fastpath/FPU, capability policy, versions and
commit/release state are unchanged. No new5% end-to-end gain is established;
the full optimization goal remains active.

## Cycle 81: evaluate request-local batch removal of page references

Cycle80's valid diagnostic counts 6,498,399 visits for 50,956 successful
one-page active-list removals. This is work evidence, not a CPU-time fraction.
Reverse-VA release is already implemented. Test whether removing selected
records together reduces this remaining repeated traversal, without a permanent
VA index, wider per-page Nodes, a Kernel change or a capability-policy change.

Prototype a private, opaque physical-page snapshot for mapping release only.
It retains the stable sorted selection of the newest one-page record at each
VA, plus a request-local node identity. A validation pass checks PID and every
selected current record before any ownership mutation. A second pass unlinks
those exact nodes and leaves older duplicates, unrelated records and other
owners intact. Both passes stop once every selected record is accounted for.
Insert returned allocations into deferred ownership in the original reverse-VA
order. A shared unlink helper also serves the existing single-record take.
Whole extents and shared reservations retain their existing dispatch paths.

All hardware UNMAPs still precede ownership transfer. A failed UNMAP retains
every active reference, including backing for PROT_NONE; retry must finish the
remaining work safely. Snapshot validation does not authorize a Frame access.
Source alias invalidation, owner resolution and target/reaper checks remain.
Per-page persistent Node layout stays 48 bytes in the native assertion.
Temporary selection entries grow from 32 to 40 bytes each; their stable-sort
scratch can grow too. This is explicitly not a zero-memory-cost change.

Three prototype-only tests cover newest duplicate selection, shared reference
counts, wrong-owner/stale atomic rejection, layout and 12,000 mixed operations
against independent newest-first selection/sequential-removal oracles.
Add two general handler regressions: mixed insertion order with sparse/NONE
backing and preserved older duplicates; every UNMAP failure prefix followed by
a successful retry. These two also pass on exact original production files:
core105 passed/2 ignored before reapplying the frozen prototype. Prototype
native1214/17 ignored, core ASan108/2 ignored and Alpha ARM checks all pass.

Pin SOURCE_DATE_EPOCH=1793577600 and unset every diagnostic switch.
Candidate Alpha SHA256 starts 4b2c2508, init b3ccf210, disk f14a7904.
Parse and compare all21 core CPIO records: only ./nanami/alpha differs.
Linux fea651de, FreeBSD ad97c4e, Ext2 28dcd199, Kernel312d579b and normal
rootfs a6c472f8 stay exact. Save full source/ELF/init/identity/patch evidence
under /private/tmp/nanami-opt-cycle81.dnCbYl.

Run12 normal ABBA boots, then12 independent normal BAAB boots, one VM at a
time with4 vCPUs and normal devices. No own build/test/hash jobs overlap these
timings. Both sets pass smoke/process/cache-freshness gates and three actual
X11-ready/cleanup/same-ID-reuse checks per startup boot. Directory contents
remain474 entries/FNV18414638227522594487, with2560 small-buffer and384
large-buffer calls. Parse all30 metrics, retain cold samples, three per boot
(Negoto six), and keep both comparison sets separate. Below are milliseconds;
negative change is shorter, not an attribution of CPU or IPC cost.

| Metric | ABBA before / prototype | Change | BAAB before / prototype | Change |
| --- | ---: | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 30.4790 / 28.5085 | -6.47% | 28.3930 / 29.8235 | 5.04% |
| alpha-lifecycle-busybox-8-prepare | 3.3670 / 3.1440 | -6.62% | 3.2340 / 3.3630 | 3.99% |
| alpha-lifecycle-busybox-8-source-release | 0.4515 / 0.4195 | -7.09% | 0.3955 / 0.5475 | 38.43% |
| alpha-lifecycle-xfe-3 | 30.6865 / 31.6700 | 3.20% | 31.1760 / 33.4880 | 7.42% |
| alpha-lifecycle-xfe-3-prepare | 12.8250 / 12.6690 | -1.22% | 13.3250 / 13.2930 | -0.24% |
| alpha-lifecycle-xfe-3-source-release | 0.9400 / 0.9215 | -1.97% | 0.8770 / 0.9345 | 6.56% |
| alpha-reap | 0.3875 / 0.4075 | 5.16% | 0.4060 / 0.4430 | 9.11% |
| alpha-spawn | 2.6525 / 2.8020 | 5.64% | 2.7815 / 2.6890 | -3.33% |
| alpha-spawn-reap-8 | 24.6740 / 26.8145 | 8.68% | 26.1330 / 25.9315 | -0.77% |
| alpha-xfe-reap | 0.5605 / 0.6235 | 11.24% | 0.7205 / 0.7440 | 3.26% |
| alpha-xfe-spawn | 5.4370 / 5.6755 | 4.39% | 5.7400 / 5.8115 | 1.25% |
| busybox-ls-8 | 1974.5165 / 1971.1290 | -0.17% | 1972.9515 / 1959.6320 | -0.68% |
| copyout-128-vmas | 171.2965 / 177.7125 | 3.75% | 183.3860 / 174.6585 | -4.76% |
| copyout-256-vmas | 180.1230 / 174.2170 | -3.28% | 177.3650 / 176.4485 | -0.52% |
| copyout-normal | 172.9215 / 172.5365 | -0.22% | 176.7570 / 174.4040 | -1.33% |
| directory-large-64 | 81.1980 / 80.0550 | -1.41% | 81.6670 / 80.3595 | -1.60% |
| directory-small-64 | 450.4935 / 451.1335 | 0.14% | 443.2040 / 448.9550 | 1.30% |
| fork-128-vmas-8 | 19.2895 / 18.6500 | -3.32% | 18.6170 / 18.9940 | 2.03% |
| fork-256-vmas-8 | 19.1110 / 17.8025 | -6.85% | 18.4080 / 19.3710 | 5.23% |
| fork-8 | 20.1230 / 17.6175 | -12.45% | 19.4125 / 19.8150 | 2.07% |
| fork-exec-8 | 661.5020 / 671.4165 | 1.50% | 675.3965 / 669.5015 | -0.87% |
| getpid-2048 | 40.2810 / 38.9140 | -3.39% | 39.3880 / 42.8485 | 8.79% |
| getuid-2048 | 39.8650 / 39.3035 | -1.41% | 39.4540 / 40.1050 | 1.65% |
| missing-stat-512 | 80.7175 / 81.6900 | 1.20% | 77.3750 / 80.9050 | 4.56% |
| negoto-create | 6.6725 / 7.6270 | 14.30% | 6.5805 / 7.2040 | 9.47% |
| startup | 4841.5135 / 4895.2870 | 1.11% | 4883.1430 / 4891.4890 | 0.17% |
| stat-512 | 115.2085 / 115.4550 | 0.21% | 111.0625 / 114.4895 | 3.09% |
| unix-2048 | 385.3660 / 376.6475 | -2.26% | 382.2470 / 372.0320 | -2.67% |
| unix-recvmsg-2048 | 767.2545 / 748.9345 | -2.39% | 768.8135 / 740.4700 | -3.69% |
| unix-recvmsg-eof-2048 | 428.4015 / 432.1985 | 0.89% | 430.5795 / 436.7560 | 1.43% |

Frame controls retain eight samples per image, each120 frames, with checked
damage totals0/61440/57600000 pixels:

| Control | ABBA change | BAAB change |
| --- | ---: | ---: |
| workload-0-legacy-false | -0.50% | -2.27% |
| workload-0-legacy-true | 2.83% | -1.32% |
| workload-1-legacy-false | 0.11% | -0.78% |
| workload-1-legacy-true | 0.96% | -0.84% |
| workload-2-legacy-false | 2.11% | -2.99% |
| workload-2-legacy-true | -0.88% | -3.33% |

Reject the runtime prototype. Initial ordinary fork -12.45% and BusyBox
lifecycle -6.47% do not reproduce: independent fork is +2.07%, BusyBox
lifecycle +5.04%, fragmented fork256 +5.23% and Negoto create +9.47%.
Startup is only +1.11%/+0.17%; favorable UNIX and directory movement is
preserved too, not selected as proof of this batch-removal benefit.
Pure Alpha construction shifts +8.68%/-0.77%; unchanged getpid and frame
controls move as well. These measurements establish no repeatable new5%
application improvement. Do not attribute them to node size, Kernel IPC,
host load or instruction-cache placement without separate evidence.

Restore all three production files byte-for-byte and retain only the two
baseline-compatible handler regressions. The complete prototype, including
its internal tests, remains in /private/tmp/nanami-opt-cycle81-batch-candidate.patch;
git apply --check succeeds against the restored production files.
Restored native1208/17 ignored, core ASan105/2 ignored and Alpha ARM checks
finish successfully. Alpha and the entire init match their saved pre-trial
bytes exactly; the Kernel and normal rootfs hashes also remain exact.
Three further actual normal startup/cleanup/same-ID-reuse checks pass,
serial2mupavbt,4510.901/4887.866/5004.279ms, with no checked error/profile
output and a terminal0 harness. These are restoration checks, not additional
paired speed samples. No prototype GUI pass is claimed.

After terminal handles, empty lsof checks and retained source/ELF/init/patch
evidence, remove only the two closed owned321MiB cycle81 trial/restored raw
images. Keep accepted cycle73 and the user's current Tiny Core build/rootfs,
sources and downloads. Saved code can rebuild trials but is not a byte-exact
raw-disk backup. Both measurements.json files, all30 primary metrics, six
frame controls, serial logs and final-verification.json remain available.
The forward prototype and reverse retained-regression patches check cleanly.

This experiment does not justify a permanent VA index or a wider Node.
Any next candidate must address a separately verified cost and preserve the
current private-page/Frame authority model; instruction-weighted work and
elapsed service intervals are still not exclusive CPU-time measurements.
Kernel, IPC fastpath, FPU, capability policy, versions and commit/release
state remain unchanged. No new repeatable5% application gain is established;
the optimization goal remains active.

## Cycle 82: replace bootstrap frame AVL with a fixed exact index, then reject

Cycle81 rejects request-local batch unlinking and restores exact runtime/ELFs.
Inspect the temporary-mapping churn behind cycle79's valid Alpha work profile:
cached alias replacement repeatedly inserts/removes frame records in the
allocation-free BootstrapVmSpace AVL. Evaluate this without changing mapping
calls, private storage, permissions or capability authority.

Prototype a private FrameIndex submodule under vm_space. Use32768 fixed
buckets for the same16384-mapping limit, full-word hashing and backward-shift
deletion, not tombstones. Keys need not be aligned; zero and usize::MAX remain
valid keys/values. Existing-key updates succeed at capacity, failed insertion
does not mutate state, and reuse does not consume capacity permanently.
The page-table AVL and eight-region readiness hint remain unchanged.
Native layout checks give786440 bytes for the new frame index, smaller than
the old16384-node tree. No heap or new per-process index is introduced.
This trades the AVL logarithmic bound for worst-case linear collision probing;
half occupancy does not prove a short collision chain.

Four prototype-only tests cover full updates/refill, every removal position
in wrapped collision clusters,30,000 mutations against an independent
BTreeMap and native layout. Two general VM regressions cover25,000
allocation-free full-width-key updates/removals and independent copied state.
Both general tests first pass on exact original production files:
vm-index17/3 ignored. The existing full-capacity/readiness/no-allocation
checks remain. During the prototype only, replace the old exact-layout
equality by an upper bound; restore the original equality on rejection.

Prototype native1226/17 ignored, focused ASan37/4 ignored and Alpha ARM
checks pass. Pin SOURCE_DATE_EPOCH=1793577600 and unset all diagnostics.
Candidate Alpha SHA256 starts a363b74a, init f0eaf70b, disk0a19c7a9.
Compare all21 core CPIO records: only ./nanami/alpha changes.
Kernel312d579b, normal rootfs a6c472f8, Linux fea651de, FreeBSD ad97c4e,
Ext228dcd199 and accepted cycle73 disk7bfdd563 remain exact.

Run12 ordinary ABBA boots with4 vCPUs/normal devices, one VM at a time,
without own build/test/hash jobs during timings. All smoke/process/freshness
and three-session startup cleanup/same-ID-reuse gates pass. Retain all30
primary metrics and six frame controls, including cold samples, three per
boot (Negoto six). Directory contents remain474 entries with
FNV18414638227522594487 and2560/384 small/large buffer calls.
Medians are milliseconds; negative change is shorter.

| Metric | Before | Prototype | Change |
| --- | ---: | ---: | ---: |
| alpha-lifecycle-busybox-8 | 29.4265 | 32.9480 | 11.97% |
| alpha-lifecycle-busybox-8-prepare | 3.3370 | 4.4105 | 32.17% |
| alpha-lifecycle-busybox-8-source-release | 0.4365 | 0.4720 | 8.13% |
| alpha-lifecycle-xfe-3 | 30.2615 | 32.4770 | 7.32% |
| alpha-lifecycle-xfe-3-prepare | 12.8460 | 12.5290 | -2.47% |
| alpha-lifecycle-xfe-3-source-release | 0.8805 | 0.9465 | 7.50% |
| alpha-reap | 0.3910 | 0.4120 | 5.37% |
| alpha-spawn | 2.7670 | 2.9125 | 5.26% |
| alpha-spawn-reap-8 | 26.8765 | 26.6240 | -0.94% |
| alpha-xfe-reap | 0.6415 | 0.6575 | 2.49% |
| alpha-xfe-spawn | 5.4115 | 5.8195 | 7.54% |
| busybox-ls-8 | 1954.0750 | 1978.3725 | 1.24% |
| copyout-128-vmas | 175.0030 | 177.0015 | 1.14% |
| copyout-256-vmas | 176.2665 | 174.2560 | -1.14% |
| copyout-normal | 174.0025 | 175.9935 | 1.14% |
| directory-large-64 | 82.0700 | 81.1805 | -1.08% |
| directory-small-64 | 443.3070 | 458.5080 | 3.43% |
| fork-128-vmas-8 | 20.0195 | 18.3200 | -8.49% |
| fork-256-vmas-8 | 19.5600 | 18.6320 | -4.74% |
| fork-8 | 18.3655 | 20.5805 | 12.06% |
| fork-exec-8 | 675.0995 | 668.9160 | -0.92% |
| getpid-2048 | 40.9265 | 40.9325 | 0.01% |
| getuid-2048 | 41.2125 | 40.0345 | -2.86% |
| missing-stat-512 | 80.1380 | 76.3025 | -4.79% |
| negoto-create | 6.7820 | 7.9515 | 17.24% |
| startup | 4798.0910 | 4845.9445 | 1.00% |
| stat-512 | 112.9415 | 114.8190 | 1.66% |
| unix-2048 | 387.8085 | 378.3115 | -2.45% |
| unix-recvmsg-2048 | 778.6305 | 757.2050 | -2.75% |
| unix-recvmsg-eof-2048 | 438.4370 | 436.6870 | -0.40% |

Frame controls retain eight samples per image of120 frames and the same
checked damage totals0/61440/57600000 pixels:

| Control | Before | Prototype | Change |
| --- | ---: | ---: | ---: |
| workload-0-legacy-false | 62.5020 | 62.3315 | -0.27% |
| workload-0-legacy-true | 63.4800 | 62.5610 | -1.45% |
| workload-1-legacy-false | 62.6065 | 61.9730 | -1.01% |
| workload-1-legacy-true | 61.8075 | 61.9390 | 0.21% |
| workload-2-legacy-false | 118.7330 | 118.6535 | -0.07% |
| workload-2-legacy-true | 120.9635 | 120.8665 | -0.08% |

Reject the runtime replacement: startup +1.00%, ordinary fork +12.06%,
BusyBox lifecycle +11.97%, Xfe lifecycle +7.32% and Negoto create +17.24%.
Preserve favorable fragmented fork128 -8.49%, missing-stat -4.79% and UNIX
movement too; they do not establish a general or causal representation gain.
Pure Alpha creation/reap is only -0.94%, fork-exec -0.92%.
No independent speed repeat is used to rescue this unfavorable candidate,
and no prototype GUI pass is claimed.

Separately, reuse cycle79's read-only QEMU plugin to ask whether the intended
Alpha work was reduced at all. Clear register buffers before CR3 reads and
memory buffers before prefix/full .text comparison. Validate each frozen
ELF's entire immutable text at0x1000 before identifying the Alpha CR3.
Run both candidate and current baseline through three actual startup/cleanup/
reuse sessions, separately from the ordinary speed comparison.

Both profiles identify root0x5000, all3 markers, and zero read errors or
wrong-core executions. Text lengths are360437 before and362965 candidate;
serial logs are kzvhftkh before and2de1vylr candidate. Marker observation
can lag by4095 Alpha TB entries. Phase0 includes boot and the first session;
phases1/2 cover the subsequent session/cleanup intervals. Symbol assignment
near16-byte bin boundaries is approximate, and inlined work cannot be
attributed completely to standalone helper symbols.

| Phase | Before blocks | Prototype blocks | Before weighted | Prototype weighted | Weighted change |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 183926784 | 126959616 | 866157435 | 605403763 | -30.10% |
| 1 | 147992576 | 105029632 | 693999996 | 498098191 | -28.23% |
| 2 | 147443712 | 105824256 | 690880454 | 500591319 | -27.54% |

These are TB entries weighted by translated instruction count, NOT retired
instructions or CPU-time shares. REP, faults, stalls, Kernel work and other
processes are not accounted as exclusive time. The intended Alpha work
reduction is nevertheless present: steady weighted totals fall about28%
and the old16384-node static-AVL hotspots disappear. Successful
PhysicalAllocations::take stays around40.09million weighted in both phase1
profiles. Normal application timings still do not improve. Instrumented
startup timings are not normal speed evidence.

This changes the next action: do not select another Alpha metadata layout
solely from Alpha-local weighted percentages. Broaden attribution to the
whole syscall/service path, including waits and other execution domains,
using read-only instrumentation where possible. This does not prove IPC,
Kernel spinlocks or host load dominate and authorizes no Kernel change.

The final byte verifier catches a diagnostic-artifact issue:
llvm-objcopy --dump-section without a distinct output rewrites its private
input ELF snapshot too. Normal build artifacts/images and saved init ELFs
were not passed as those inputs. Recover the exact original Alpha snapshots
from the untouched init CPIO records with validated binary ranges: restored
hashes are e025b8a5 and a363b74a. Independently parse their ELF section tables
and verify both extracted .text blobs byte-for-byte against the original
CPIO payloads. Thus the profiles match the intended code; preserve the
rewritten derivatives and elf-artifact-recovery.json for provenance.
Future extraction must use a separate output or read-only ELF parsing.

Restore vm_space.rs byte-for-byte, remove only the prototype submodule and
restore the exact-layout assertion. Keep the two baseline-compatible general
regressions. Full forward candidate and reverse regression patch checks pass.
Restored native1210/17 ignored, ASan144/6 ignored over four targets and Alpha
ARM checks pass. Complete Alpha/init bytes match pre-trial originals; all21
CPIO records and Kernel/rootfs/other-service/accepted-image hashes are exact.

Three final actual normal startup/cleanup/same-ID-reuse sessions pass,
serial waa3biuq,4400.334/4799.898/4994.135ms, terminal0, no checked errors or
profile output. These are restoration checks, not paired speed samples.
Evidence, source/ELF/init snapshots, raw profile bins, both parsed profiles
and measurements.json remain under /private/tmp/nanami-opt-cycle82.opEcez.
Candidate patch: /private/tmp/nanami-opt-cycle82-frame-index-candidate.patch.
General test patch: /private/tmp/nanami-opt-cycle82-regressions.patch.

After terminal handles and empty lsof checks, remove only the two closed
owned321MiB cycle82 trial/restored raw images. Keep accepted cycle73 and the
user's current Tiny Core build/rootfs, sources and downloads. Saved evidence
can rebuild code but is not a byte-exact raw-disk backup. Kernel, ABI, IPC
fastpath, FPU, capability policy, versions and commit/release state remain
unchanged. No new5% application speedup is established; the goal stays active.

## Cycle 83: whole-path code-image work and a separate halted-vCPU observation

Cycle82's smaller bootstrap frame index reduces Alpha instruction-weighted
TB entries about28%, but ordinary application timings do not improve.
Reject that layout and preserve exact runtime. Change the diagnostic scope
before choosing another Alpha metadata representation. This cycle changes
neither production nor test source, and claims no runtime speedup.

Build read-only host plugins against the installed QEMU11.1.1 plugin SDK,
with C11 -O2 -Wall -Wextra -Werror. Kernel, ABI, IPC paths, FPU and capability
policy are untouched. Reuse the accepted cycle73 normal-device4-vCPU image
and existing three-session X11-ready/cleanup/same-ID-reuse workload.
No other own VM/build/test/hash job overlaps a measurement run.

The first plugin counts instruction-weighted TB entries across all cores,
rather than just Alpha. Validate Alpha's complete360437-byte .text at0x1000
and the Kernel's complete87810-byte .text at0xffff800000202000 against
unchanged saved ELFs before accepting attribution. Clear QEMU byte arrays
before every register/prefix/full-text read. Kernel high-address code outside
the verified text and unidentified low-address code retain separate buckets.
Only Alpha/core0 consumes marker lines; the phase number is atomic and
histograms are per-vCPU. Marker observation may lag4095 Alpha TB entries.
Phase0 includes boot; subsequent phases cover session/cleanup intervals.

Serial e02qeyv_ passes all three sessions. Alpha root0x5000 and full Kernel
code match; read errors/wrong-core counts are zero. First steady phase totals
1,670,115,398 weighted entries: Alpha41.03%, Kernel text18.61%, other low
addresses40.36%, other high0%. The second gives41.16%/18.40%/40.43%/0%.
These are work shares, NOT CPU-time, latency or waiting shares.
Core1's unidentified low-address work467,101,960 exceeds core2's147,006,663;
do not assume the POSIX/VFS/Alter broker core dominates everything else.

Extend a separate plugin to identify eight native code images. Read CR3 on
low-address execution and compare entire native text at0x1000000 when an
observed root transition or image-entry address requires reclassification.
Do not retain a negative identity across such transitions. Count code outside
a matched image's text as unidentified, not as that service.
Save full ELF/text hashes and verify extraction with read-only ELF parsing;
unlike cycle82's initial tool misuse, no objcopy input rewriting is used.
Native text ranges differ from Alpha's0x1000 base.

All eight images are observed and fully matched. The parser requires all144
phase/core/domain rows, even zeros, and exact histogram-to-domain sums.
Service labels below denote matched code images, not authority or unvalidated
PID/CR3 identities:

| Image | Text bytes | First matched root | Full verifications | ELF SHA256 prefix |
| --- | ---: | --- | ---: | --- |
| honoka | 203214 | 0x13c809000 | 13424 | 58c45cc4ba93 |
| alter-linux | 512722 | 0x13bc09000 | 30333 | fea651de3b56 |
| posix | 58194 | 0x13ba09000 | 21248 | 864ff18c5ee5 |
| vfs | 111246 | 0x13f809000 | 11742 | 28dcd1994391 |
| fb | 33314 | 0x13d809000 | 559 | 81b56b1c7e5f |
| input | 26885 | 0x13dc09000 | 14 | f735f9ee77e1 |
| hpet | 27630 | 0x13e809000 | 11049 | 612612c27d5b |
| ps2 | 29406 | 0x13e209000 | 38 | 8c73b87a548b |

Serial sw7i9l9z passes three actual sessions; Kernel/Alpha matches remain
valid, all3 markers arrive and read/wrong-core errors remain zero.
Steady work totals are2,105,581,203 and1,797,351,685. Keep this execution
separate from the first profile; added full-image validation lengthens the
run and increases periodic work. Do not pool the different-observer runs.

| Code image/domain | Phase1 weighted | Share | Phase2 weighted | Share |
| --- | ---: | ---: | ---: | ---: |
| alpha | 695121003 | 33.013% | 675001565 | 37.555% |
| kernel-text | 322013362 | 15.293% | 308060809 | 17.140% |
| honoka | 798078441 | 37.903% | 601579702 | 33.470% |
| alter-linux | 61714898 | 2.931% | 42630638 | 2.372% |
| vfs | 90406837 | 4.294% | 72532665 | 4.036% |
| posix | 2378932 | 0.113% | 1736832 | 0.097% |
| fb | 14064927 | 0.668% | 15380047 | 0.856% |
| hpet | 1803275 | 0.086% | 1380683 | 0.077% |
| input | 0 | 0.000% | 0 | 0.000% |
| ps2 | 0 | 0.000% | 0 | 0.000% |
| other-low | 119999528 | 5.699% | 79048744 | 4.398% |
| other-high | 0 | 0.000% | 0 | 0.000% |

Within Honoka's first steady-phase work, fbdev_damage::capture accounts for
73.20%, blend_color22.87% and blit_bgra32_from_alpha2.43%, using approximate
16-byte symbol bins. These correspond to actual reviewed source paths:
stable capture of unmodified Linux fbdev memory and per-pixel alpha blending.
Capture already reads samples once and renders stable snapshots. Cycle28's
row-major replacement failed full-update controls; do not retry it blindly.
The generic blend path recalculates channel masks and performs per-channel
weighted rounding; exact common-format integer blending is a separate
candidate, with arbitrary-format fallback and unchanged pixels required.

Alter work includes reserve_none_mapping22.34% and memory_access_allowed20.50%
of its local weighted total. VFS work includes allocation bitmap, block cache
and directory scans. POSIX's own work is small in this workload. Kernel's first
profile includes capability_node::traverse_slot27.15% of Kernel weighted work,
address-space/page-table mapping helpers and context switching. Contended
spin_lock is about1.02% of Kernel weighted work. None of these numbers proves
an IPC bottleneck, IPC fastpath hit rate, exclusive CPU cost or syscall delay.
The grouped native symbol data and unknown-boundary bins remain in the raw/
parsed evidence rather than dropping small or contrary groups.

Instrumented guest startup samples rise to8.58/8.76/8.97s for the first
profile and11.94/13.43/12.04s for native attribution, versus normal around
4.3--4.9s. Thus periodic Honoka capture is sampled for longer, inflating its
work volume. Its apparent share is not proof that it dominates normal startup.
REP helpers, faults, instruction latency, host scheduling and TCG translation
also prevent instruction-weighted counts from being CPU-time fractions.

Add a third, separate lightweight plugin with no translation/TB callbacks.
QEMU idle/resume callbacks record each vCPU's halted/not-halted state on the
host CLOCK_MONOTONIC timeline. A reader thread observes the same serial
markers with nominal10ms polling; maximum observed poll gap is21.217ms,
not a guaranteed guest-event timestamp bound. Per-vCPU event arrays are locked
during append/snapshot. Clip spans against the observed phase boundaries.
Every steady core has exact halted+not-halted coverage and zero duplicate
transitions/errors; phase0 excludes each CPU's pre-initialization interval.

This measures halted-vCPU occupancy, not per-task blocked time or exclusive
guest CPU execution. Not-halted intervals can include host descheduling,
emulation/translation and device work; do not call them busy CPU time.
HLT time does not identify which IPC/service/scheduler condition caused it.

Serial nyhufzxk passes all three sessions. Its guest startup samples
4499.742/4894.703/4893.962ms remain near the separate normal control.
Observed steady interval lengths are4970.681/4979.802ms, including
session/cleanup gaps; they are not exactly the measured startup call spans.

| Phase | CPU | Halted ms | Not halted ms | Halted share | Halted intervals |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 0 | 1776.657 | 3194.024 | 35.74% | 15243 |
| 1 | 1 | 4225.825 | 744.856 | 85.02% | 4749 |
| 1 | 2 | 3506.548 | 1464.133 | 70.54% | 19727 |
| 1 | 3 | 4138.018 | 832.663 | 83.25% | 7641 |
| 2 | 0 | 1802.609 | 3177.193 | 36.20% | 15174 |
| 2 | 1 | 4311.071 | 668.731 | 86.57% | 5081 |
| 2 | 2 | 3298.136 | 1681.666 | 66.23% | 16769 |
| 2 | 3 | 4477.859 | 501.943 | 89.92% | 3973 |

Core0 is not halted for roughly3.18--3.19s per steady interval, whereas
core1 is halted about85--87%. This is a reason to avoid replacing the startup
bottleneck with a rendering claim based on weighted shares alone. Retain
rendering throughput/latency and process-startup metrics as separate gates.
The machine is not continuously executing on all four vCPUs, but task-specific
wait attribution remains unproved by this halted-state observation.

A final plugin-free normal control passes three actual startup/cleanup/reuse
checks, serial wisfoewt,4311.754/4780.412/4796.339ms, terminal0 and no checked
error/profile output. This is an integrity control, not a paired speed trial.
No new image is built and no runtime or test file is changed. The cycle82
native1210/ASan144 results are prior validation, not rerun claims for this cycle.

Final Alpha e025b8a5, whole init1b193ac0, Kernel312d579b, rootfs a6c472f8,
accepted image7bfdd563 and all eight saved/live native ELFs/text slices are
rechecked exact. There is no source to restore, no Kernel/API/IPC fastpath
change, and no version/commit/release action. No raw disk is created/deleted.
All three plugins/harnesses/parsers, ELF/text provenance, raw logs and parsed
JSON are retained under /private/tmp/nanami-opt-cycle83.HSwLQQ.

Next choose using lower-observer elapsed/CPU timing and fixed rendering work,
not only Alpha-local or globally instruction-weighted percentages. A conditional
kernel/user transition observer could measure host vCPU-thread CPU intervals
while accounting for its own callback work; this would still include TCG costs,
not bare-metal CPU time. Any RGB-blend candidate must preserve exact rounding,
unused bits, all opacity endpoints, arbitrary channel formats, clipping and
GPU failure behavior, and pass normal GUI plus startup/control comparisons.
The full optimization goal remains active; this diagnostic cycle establishes
no new5% application speedup.

## Cycle84: exact framebuffer blending and an unresolved lifecycle tradeoff

Follow cycle83's rendering lead with fixed work and ordinary QEMU timing,
not instruction-weighted attribution. No profiler plugin, Kernel change,
IPC protocol/fastpath change, FPU work or capability-policy change is used.
All experimental files and raw/parsed evidence are under
`/private/tmp/nanami-opt-cycle84.eOo84Z`.

The final candidate caches the three immutable channel masks when creating a
Framebuffer. In the standard RGB8 layout it combines red/blue in independent
16-bit integer lanes and uses an exact divide-by255 fold. Green uses the same
rounding. This is bit-identical to `(foreground*alpha + background*(255-alpha)
+127)/255`, preserves unused background bits, and keeps the original opaque
and transparent shortcuts. Other layouts retain the generic channel arithmetic
with precomputed masks. Screen format is immutable and private to Framebuffer;
both RAM and GPU constructors initialize the cache. Measured x86 object size
is184 ->192bytes, not a per-pixel or per-process allocation. GPU-format
eligibility, clipping, failure behavior and public interfaces are unchanged.
Actual Release blend-function disassembly contains no new SIMD/FPU operations.

Two preliminary variants are not selected. A per-pixel format comparison gives
BGR +18.63% despite RGB -75.7%; caching only a format boolean makes the generic
case still worse. Their complete six-boot comparisons remain in the top-level
and `stage2/` evidence. The final mask-cache candidate and its measurements
are in `framebuffer.stage3.rs`, `candidate-stage3.patch` and `stage3/`.

Two retained, baseline-compatible native tests compare an independent wide
scalar reference against the public drawing methods: all256^3 channel-pair /
opacity combinations, plus RGB/BGR, RGB565, RGB10, a32-bit channel, overlapping
and out-of-range masks, unused bits, clipping, fill/blit, padding and canaries.
They pass before the optimization too. Final candidate native1212 /17ignored,
rendering ASan42 /2ignored and actual ARM Honoka checking pass. Shared mocks
run with `--test-threads=1`.

### Fixed drawing work and real service timings

A private, opt-in native guest compiles the actual original and candidate
Framebuffer modules with real runtime/client libraries and normal Release
options. The initial fixture's invalid display-port constructor is caught in
binary preflight and corrected before any timing run; no panic/empty-work
result enters the comparisons. The fixture fills ordinary shared RAM with
800x600 pixels and distinct source/destination strides, runs12 blits per timed
sample, compares full outputs, and checks padding/tail canaries. Four balanced
AB/BA orders across two independent boots give8 samples per variant/case.

| Micro workload | Before ms | Candidate ms | Change |
| --- | ---: | ---: | ---: |
| RGB8 alpha127 | 161.901 | 51.087 | -68.45% |
| RGB8 alpha247 | 160.970 | 51.884 | -67.77% |
| BGR8 alpha127 | 158.604 | 104.536 | -34.09% |
| RGB565 alpha127 | 157.505 | 104.318 | -33.77% |
| Opaque copy control | 6.978 | 7.311 | +4.77% |
| Transparent no-op/timer control | 0.063 | 0.062 | -1.60% |

All96 rows pass complete pixel/checksum/padding/canary gates. Do not turn a
no-op dominated by timer overhead into a throughput claim. Inlining/caller
context differs from actual Honoka, so micro timings are not the adoption gate.

The same guest also creates a600x400 window through the existing Honoka API,
attaches its real logical surface, fills only pixels after the damage queue,
then issues24 synchronous requests per sample. Opacity alternates120/121;
the opaque control moves x100/101. Every measured request changes state.
Honoka renders to idle before replying, so these intervals include IPC,
scheduling, composition and presentation submission. They are not exclusive
CPU time, scanout completion or desktop FPS. Both variants use the same guest
ELF; only the installed Honoka differs. Destroy and client-mapping release
must succeed. No extra protocol or runtime instrumentation is introduced.

| Real service workload | Initial ABBA | Independent BAAB |
| --- | ---: | ---: |
| Half-opacity update | -26.51% | -31.31% |
| Opaque window movement | -23.68% | -26.23% |

All eight real-service boots /64 rows pass. Initial complete-update medians
are680.180 ->499.848ms and764.119 ->583.176ms per24 requests. Parsed evidence
retains cold samples, complete raw rows and per-boot medians in
`stage3/blend-results.json` and `service-repeat/blend-results.json`.

### Ordinary Tiny Core controls: do not hide the slower measurements

Run the identical normal-device4-vCPU fixtures sequentially in12 ABBA boots,
using accepted cycle73 versus the clean candidate image without the private
benchmark executable. Keep all30 metrics, all six old/new capture controls,
474 directory entries/FNV18414638227522594487, unchanged2560/384 call counts,
cache freshness, fork cleanup and three-session startup/reuse checks. All
correctness gates pass. A separate four-boot BAAB process repeat retains all21
process metrics; it is not a selective rerun containing only favorable cases.

| Normal metric | Initial change | Process repeat |
| --- | ---: | ---: |
| Real OCI start to X11 readiness | +1.19% | not repeated |
| Linux fork/exec | -0.90% | -0.68% |
| Linux fork | -0.38% | +5.08% |
| Fork with128 VMAs | -0.97% | +0.09% |
| Fork with256 VMAs | -5.24% | +8.32% |
| Alpha-only constructor/reaper loop | +5.62% | +10.98% |
| BusyBox prepare/construct/reap/release | +7.64% | +0.98% |
| BusyBox preparation | +10.92% | +0.16% |
| BusyBox source release | +12.95% | -14.98% |
| Xfe prepare/construct/reap/release | +4.17% | +0.21% |
| Xfe spawn breakdown | +8.52% | +0.86% |
| Negoto prepare/create | -11.05% | +7.28% |

Initial directory/stat/AF_UNIX cases remain within-2.42..+3.36%; the six
capture controls stay within-2.31..+1.39%. Full raw values, every boot median,
all favorable and unfavorable metrics are in `normal-results.json` and
`process-repeat/normal-results.json`. No aggregate synthetic score is used.
The timing variation and unchanged Alpha binary do not prove the repeated
constructor/reaper penalty can be ignored or that its cause is local Alpha
work. Conversely, these measurements do not demonstrate a Kernel/IPC defect.

The final clean candidate additionally passes the real two-session GUI test
in one VM: Xfbdev/FLWM/wbar, terminal/editor/Information launch, drag, real
input, links, exit, cleanup and restart. Information output is inspected in
both sessions' screenshots. Logs/screenshots:
`/var/folders/11/zz8m96m934bc6stlvxb3w_k40000gn/T/nanami-negoto-qemu-hbbxaso_`.

### Handoff and current runtime state

The reproducible24--31% Honoka update gain exceeds5% for this particular
service workload, not for overall Tiny Core startup or every application.
Ask whether to accept it as a rendering/lifecycle tradeoff or continue until
the constructor/reaper regression is resolved. Pending that choice, restore
the exact original production Framebuffer source and Honoka ELF58c45cc4;
keep the two general regression tests, not the experimental runtime change.
Restored native1212 /17ignored, rendering ASan42 /2ignored and ARM checks pass.
No version, commit or release action is taken. The overall optimization goal
is not complete.

Normal Alpha e025b8a5, whole init1b193ac0, Kernel312d579b, user rootfs a6c472f8
and accepted cycle73 image7bfdd563 remain exact. The private candidate contains
Honoka13a321e2, confirmed by dumping the installed `/bin/honoka` and comparing
bytes. Preserve `/private/tmp/nanami-opt-cycle84-after.img` (7d2853fb) and its
clean `normal-after.rootfs.img` (f00e027b), plus candidate source/ELFs, all
harnesses/parsers and measurements. These are candidates, not the accepted
baseline. Keep the user's current Tiny Core build, boot image and downloads.
Remove only the closed private before/after benchmark raw images and their
two benchmark-only rootfs clones; they can be regenerated from retained guest
and Honoka ELFs and the unchanged normal rootfs.

## Cycle85: common mask cache, broader controls and spawn logging diagnosis

The previous turn produced evidence and a held candidate, not a completed
optimization goal or a verified wait. Recheck current source, live goal,
artifacts and normal hashes. Do not block all work on the pending rendering /
lifecycle tradeoff choice: compare a smaller common optimization and investigate
the affected path. Kernel, ABI, IPC fastpath, FPU and capability policy remain
unchanged. Evidence is `/private/tmp/nanami-opt-cycle85.QOQG24`.

### A smaller, format-independent rendering candidate

Cache only the three immutable channel masks in both Framebuffer constructors.
Keep the original generic channel arithmetic, rounding, GPU eligibility and
all drawing branches; do not add the RGB8 packed arithmetic or a per-pixel
format dispatch. Source delta is11 net lines. Actual x86 Framebuffer size is
184 ->192bytes. Release blend-function size is497 ->266bytes, although total
text grows by448bytes; do not describe this as an overall binary-size reduction.
Actual function disassembly uses no SIMD/FPU operations.

The retained cycle84 reference/padding/format tests still pass: native1212 /
17ignored across100targets, rendering ASan42 /2ignored and actual ARM Honoka
checking. Preserve the complete candidate source/ELF and patch before restoring.
No new production/test API or speculative runtime cache is retained.

Use the same private real-library drawing guest and exact pixel/checksum /
padding/canary gates as cycle84. Two micro boots retain96 rows across all six
cases, four balanced orders,12 frames/sample. Four actual-service ABBA boots
retain32 rows,24 synchronous changed requests/sample and successful window /
mapping cleanup. Only installed Honoka differs; builds/profilers/other VMs do
not run during timing.

| Workload | Elapsed change |
| --- | ---: |
| RGB8 alpha127 blit | -19.79% |
| RGB8 alpha247 blit | -19.33% |
| BGR8 alpha127 blit | -20.23% |
| RGB565 alpha127 blit | -19.18% |
| Opaque copy control | -4.21% |
| Transparent no-op/timer control | -8.40% |
| Real half-opacity window update | -19.83% |
| Real opaque window movement | -17.28% |

Do not claim meaningful throughput from the timer-dominated transparent case.
Service intervals include scheduling, composition and presentation submission,
not exclusive CPU time, scanout or whole Tiny Core FPS. Every measured service
request changes opacity or position and completes rendering before the reply.
All cold/control rows and per-boot medians are in `blend-results.json`.

### Three-way ordinary process comparison

Six plugin-free, normal-device4-vCPU boots run the unchanged full21-metric
process fixture in original1, masks1, packed1, packed2, masks2, original2 order.
Original is accepted cycle73; packed is the held cycle84 candidate; masks is
the new common-cache candidate. All correctness/cleanup/freshness gates pass.
The two original boots are shared controls for both comparisons, not two
independent control series. Keep all pairwise results and original provenance
under `threeway/`, including unfavorable and contrary cases.

| Ordinary metric versus original | Common masks | Packed RGB8 + masks |
| --- | ---: | ---: |
| Alpha-only constructor/reaper loop | +9.97% | +3.82% |
| Individual Alpha spawn breakdown | +5.65% | +11.84% |
| BusyBox lifecycle | +1.28% | -5.96% |
| Linux fork | -1.41% | -0.44% |
| Linux fork/exec | -0.51% | -1.28% |
| Negoto preparation | +3.85% | +14.59% |

The common cache does not resolve the concerning control. This also does not
identify the RGB8-specific branch as its cause: Alpha's implementation/binary
is identical, and these are system elapsed-time comparisons. Do not infer a
Kernel or IPC-transport defect from the result.

### Independent phase-distributed and storage-layout controls

Check an apparent file-footprint hypothesis with ELF headers and actual sizes.
Original Honoka is6,899,152bytes; both candidates are6,899,160bytes. All loaded
segment page counts and the whole-file staging page count are unchanged. The
common-cache text is0x31b8e versus0x319ce in the same page range; rodata/data/BSS
layout is identical. The eight-byte non-loaded metadata difference does not
justify a file-page-count explanation. No padding or stripping is implemented.

Separately discover a real staging difference: debugfs rewriting creates sparse
holes/rearranges indirect blocks. Original inode60 uses13,532 sectors; rewritten
versions use13,522. Build a fourth negative control by rewriting the exact old
Honoka ELF with the same method, dumping it back and confirming byte equality.
This is a private experimental rootfs, not a change to the user's rootfs.

A separate real-library native guest preloads the unchanged564,554-byte Tiny
Core BusyBox ELF. It measures12 bursts of eight suspended Alpha constructors /
kill/reap operations per boot, waiting137ms outside each interval to sample
different periodic-work phases. The first sample is retained. Its own output
is deferred until after all measurements; Alpha's ordinary logs remain enabled.
The guest does not resume Linux code, and this is an Alpha-only workload with
different setup from the standard Negoto fixture, not a whole Linux benchmark.
Sleep does not prove the vCPUs or caches were idle/cold.

Eight sequential symmetric boots use original1, rewritten1, masks1, packed1,
packed2, masks2, rewritten2, original2. Every boot validates12 complete rows and
successful mapping cleanup; all768 child lifecycles and96 rows pass. Images use
the same guest binary, Alpha and Kernel. No profile output is allowed.

| 137ms-gap control | Pooled median ms | Change versus original |
| --- | ---: | ---: |
| Original ELF/storage | 41.999 | baseline |
| Byte-identical original, rewritten | 42.042 | +0.10% |
| Common masks | 37.768 | -10.07% |
| Packed RGB8 + masks | 37.811 | -9.97% |

Retain all samples, means and boot medians in `phase-control-results.json`;
rewritten per-boot medians44.712/38.105ms also demonstrate variation. Do not
replace the ordinary adjacent-burst gate with this more favorable workload.
It rules out a universal Alpha slowdown claim, not all regressions; it does
not establish that sparse rewriting caused the earlier adjacent-burst result.
Both candidates remain held rather than silently accepting a tradeoff.

### Quantify the logging path before changing its architecture

Source inspection confirms Nun and nanami-runtime debug writers call
`a9n_abi::arch::debug_call::write_char` per character. The existing DebugCall
handler accepts one character and routes to polled UART output. This is the
debug syscall path, not capability IPC call/reply-receive fastpath. No existing
bulk debug operation is found; do not invent one or change Kernel implicitly.

Temporarily add `NANAMI_SPAWN_PROFILE=1` instrumentation around successful
spawn entry, the ordinary forced log and its completion. Use same-Alpha-core
counter stamps, preserving the existing log; emit diagnostic results only
after the measured internal span. The old exec/reap report flag remains off.
This is a private diagnosis, not a normal timing comparison. Preserve the two
original source files, diagnostic sources, minimal patches and Alpha ELF.

One normal-device diagnostic process boot passes the complete fixture. A strict
parser selects exactly eight unique suspended rootfs-linux spawns immediately
preceding each of the three ordinary Alpha-loop rows:24 unique lifecycles.
Every row satisfies prepare+log=total. Aggregate entry-through-log-completion
is55,519,000counter ticks; ordinary-log span is19,876,000ticks, or35.80%.
Per-burst fractions are33.88%,36.79%,36.99%.

These are elapsed counter spans including Kernel work, interrupts and
descheduling, not CPU-time shares. The denominator excludes reply/reap and
the extra diagnostic report. Reports can perturb later scheduling and cache
state. Diagnostic whole-loop times39.276/33.237/33.124ms must not be used to
claim a normal speedup or an exact normal logging fraction. The evidence does
identify character-at-a-time forced output as a substantial measured stage,
not merely formatting overhead. Raw/parsed data are `spawn-profile.log` and
`spawn-profile-results.json`; all24 rows are retained.

### Restore and next action

Restore Framebuffer, process_requests and exec_profile byte-for-byte against
their snapshots. Restore independent/embedded Alpha and whole init, then
actually rebuild with the profiling variable absent to regenerate normal CPIO
and fingerprints. Recheck Alpha e025b8a5, whole init1b193ac0, Honoka58c45cc4,
Kernel312d579b, user rootfs a6c472f8 and accepted cycle73 image7bfdd563. Full
restored native1212 /17ignored, rendering ASan42 /2ignored and ARM checks pass.
The rebuilt normal image completes all three real startup/cleanup/reuse
sessions at4409.528/4997.828/4997.822ms, with no spawn/share/profile errors.
No version, commit or release action is taken; the full goal remains active.

Retain source/ELF snapshots, guest sources, harnesses, parsers, raw logs and
JSON. Preserve the clean held cycle85 candidate image/rootfs as well as
cycle84's candidate and the accepted/user artifacts. Delete only closed owned
benchmark, diagnostic, phase-control and redundant restored raw images and
their experiment-only rootfs clones; these are reproducible from saved inputs.

The next substantive opportunity is a user-level logging path that removes
per-character debug syscalls from Alpha's critical work without suppressing
ordinary messages. Before implementation, resolve bounded-buffer/full-queue
behavior, loss/order guarantees, bootstrap and fault fallback, memory ownership
and driver shutdown, UART permissions under driver-manager, priority-safe
draining, and dependency cycles: the log consumer must not need an Alpha RPC
while Alpha is waiting for it. Keep UART handling out of generic libraries and
Alpha's device-independent implementation. Reuse existing IPC, notifications
and shared memory; do not change Kernel/IPC fastpath or merely move unbounded
work outside a timed interval. Verify actual output/drain and full ordinary
application controls, not only the favorable logging-stage fraction.

## Cycle 86: bounded user-level log offload — reject the runtime prototype

Previous goal-turn audit: the intervening capacity-status turn did not advance
the optimization goal. Revalidate the available work and implement the logging
experiment identified in cycle 85, rather than treating the status update as
progress or waiting on an unrelated rendering tradeoff decision.

Evidence directory: `/private/tmp/nanami-opt-cycle86.FOZbE0`. Preserve the
accepted cycle 73 image and both held rendering candidates. Kernel source,
capability policy, IPC fastpath, versions and the user's Tiny Core rootfs are
not changed. This experiment deliberately retains the original spawn messages;
it does not obtain its numbers by turning off diagnostic output.

### Implement a finite transport without an Alpha service dependency

Build a private, architecture-neutral `log-server` boot component. Alpha owns
a single 8,192-byte SPSC payload in a page-aligned, three-page allocation from
its existing permanent heap. Map Root-owned Frames into the consumer's domain
without granting Frame authority, following the managed-buffer mapping model.
The allocation stays alive for the boot, including partial installation and
unconfirmed-stop failures; retries cannot allocate an unbounded series of
buffers. This is not zero-RAM overhead: another ordinary process also requires
the existing 2 MiB object arena, its normal stack, tables and bookkeeping.

An explicit trusted boot-policy `log=1` role authorizes the sole consumer, not
an image/service name. `affinity=last` keeps it off the ordinary round-robin
sequence and resolves to CPU 0 on a single-core configuration. Use priority 3.
An unbound notification in slot 23 avoids interrupting the attach CALL through
the default bound notification. Two experimental Alpha requests attach the
consumer and read transport health; neither adds a Kernel API. Once attached,
the consumer only waits on its notification and emits through the existing
debug backend. It never needs an Alpha RPC to drain the buffer.

Preserve the original ASCII/per-format-chunk behavior, prefixes and CRLFs.
Critical errors, bootstrap and reentrant formatting retain the original writer.
The queue never spins waiting for a lower-priority consumer: on full it wakes
and retries once, then stops the consumer using the existing acknowledged
SUSPEND, drains retained bytes and permanently switches to synchronous output.
Unconfirmed stop/corrupt metadata retains the backing and reports the failure.
Private cursors and atomic payload bytes bound accesses even if shared cursor
metadata is invalid. Power-of-two capacity preserves phase at machine-word
wrap. The normal mechanism still performs one debug syscall per emitted byte,
and adds a notification per line; it removes these calls from Alpha's immediate
formatting path, not from the system's total work.

Review fixes initialize every byte of the mapped allocation explicitly, reject
exec of the authorized sink and avoid a locked exchange on every ordinary
Alpha request when no sink-stop is pending. Preserve both initial and final
ELFs. These final review changes are not silently attributed to the earlier
normal timing series, whose images remain fixed during all twelve boots.

Exactly-once output is not promised at an abrupt consumer-stop boundary. The
backend write and shared acknowledgement are separate operations: takeover
can repeat the one in-flight byte, but retains the suffix. A specific test
models this boundary. The real single-core full-buffer test below has no
in-flight consumer and verifies every expected spawn record exactly once.

### Verify boundedness and actual output completion

The initial full native suite passes 1,223 tests / 17 ignored. The final
prototype passes 1,226 / 17, including the real router, bootstrap/reentrancy,
notification failure, full-buffer fallback, stop failure, explicit padding,
cursor corruption/wrap and concurrent producer/consumer reuse tests. The
transport/router ASan suite passes 13 tests. ARM consumer and Alpha compilation
checks pass. The consumer x86 ELF is 17,528 bytes in the initial build.

A separate real-library native guest preloads the unchanged 564,554-byte Tiny
Core BusyBox image. Measure twelve bursts of eight suspended constructors /
kill/reap operations per boot, with 137 ms between bursts. Keep the first
sample and all raw rows. Record both return and subsequent observed output
completion, not just enqueue time. The after-image must report an active sink,
zero pending bytes and zero fallback/retention flags; an unprivileged caller's
attach attempt must be denied. The before-image has no such API and emits its
ordinary synchronous logs. Reports are printed after all measurements.

Four sequential ABBA boots pass all 384 child lifecycles. Independently parse
the captured serial output to verify one complete original spawn record per
expected PID. Retain all 48 timing rows in `control-results.json`:

| Fixed-gap control | Before median ms | After median ms | Change |
| --- | ---: | ---: | ---: |
| Constructor/reaper return | 38.4405 | 34.0300 | -11.47% |
| Including observed complete drain | 38.5065 | 34.2165 | -11.14% |

Every after-row observes zero pending bytes at its first health poll; the
positive result is not a growing output backlog. The extra after health RPCs
are included in the drain measurement. A separate 96-child preflight also
passes. This remains an Alpha-only, phase-distributed workload, not a claim
about whole Linux execution, CPU-time shares or general system performance.

The final reviewed image additionally passes an SMP=1 stress run with 160
consecutive child lifecycles and no deliberate yield to the lower-priority
consumer. Assert one fallback, zero retained bytes and successful cleanup.
Serial readback verifies all 160 unique PIDs each have exactly one complete
original spawn record. The queue therefore does not rely on another CPU or
an unrelated device interrupt for progress. Logs are in
`nanami-negoto-qemu-dfphazf5/serial.log` under the host temporary directory.

### Ordinary workloads contradict a general performance win

Run twelve normal-device, SMP=4 boots in ABBA order, measuring process, UNIX /
filesystem and three-session startup/reuse profiles separately. No competing
VM, build, test or hash job runs during these measurements. All twelve boots,
all 30 timing metrics, six framebuffer controls and the exact directory/hash
checks pass. Retain every sample, including unfavorable results, in
`comparison-initial.json`; the eight-byte/format rendering candidates are not
part of either image. Initial candidate SHA-256 is
`dcb8f542ea61fe579077b6917a09afb04b105322240996ea16d0abce0377cb1f`.

| Ordinary metric | Change |
| --- | ---: |
| Individual Alpha spawn | -29.64% |
| Individual Alpha reap | +184.10% |
| Adjacent eight-child Alpha loop | -2.60% |
| BusyBox lifecycle | -4.80% |
| Xfe lifecycle | +7.11% |
| Linux fork | +25.15% |
| Fragmented fork, 128 / 256 VMAs | +11.35% / +17.57% |
| Linux fork/exec | -4.56% |
| Negoto create | -5.41% |
| Tiny Core create/start through X11 readiness | -0.21% |
| stat / missing-stat | -5.85% / -5.87% |

The positive fixed-gap control does not override the ordinary workload gate.
The much shorter spawn with a much longer follow-up reap is consistent with
moving serialized output work into later requests; it does not prove a new
Kernel/IPC defect or identify an exact scheduler/lock-time fraction. All
Kernel calls still have to be performed. Per-boot variation and contrary
filesystem/Negoto results are preserved, not hidden behind one favorable
aggregate. No uniform 5% application improvement is established, while the
prototype adds another process, authority/mapping lifetime and failure paths.
Reject it rather than accepting substantial architecture growth for a
favorable stage-only latency.

### Restore and next substantive action

Archive the final implementation and tests in `logging-candidate.tar`, the
initial/final Alpha and init ELFs, private guest source and harnesses, raw
serial output and strict parsers. Restore every edited production/test source
against the pre-cycle snapshots; remove the experimental component, protocol,
API requests and boot-policy extensions. Keep prior accepted and user changes.
Rebuild and revalidate the original configuration before handing off.

The restored build actually regenerates normal CPIO/init and recovers exact
Alpha `e025b8a5`, whole init `1b193ac0`, Honoka `58c45cc4`, Kernel `312d579b`,
user rootfs `a6c472f8` and accepted image `7bfdd563` identities. All nine edited
source/test files with pre-cycle snapshots compare byte-identically. Restored
native 1,212 / 17 ignored, rendering ASan 42 / 2 ignored and ARM checks pass.
A fresh normal-device boot completes all three real startup/cleanup/reuse
sessions at 4,398.430 / 4,768.821 / 4,784.846 ms, with no spawn/share failures
or experimental logger. No commit, release or version change is performed.

Restore the canonical rejected-image path to the initial revision actually
used by the twelve normal timing boots. Retain that image and the initial
`control-after.img`, which also preserves the original timed guest binary.
The later guest source additionally implements overflow mode; do not silently
substitute it for the original controlled timing workload. `evidence.json`
records the image/ELF provenance and final restore checks. Remove only six
closed, owned redundant reviewed/control/overflow/restored image or temporary
rootfs files. Source/ELF snapshots, the initial timing images, all measurements,
accepted/held candidates and current user inputs remain; free space returns
from about 3.7 GiB to 4.7 GiB.

Next reduce actual output work while retaining each spawn event and every
value, rather than adding another consumer. For the representative PID 24
Linux spawn, a readable `spawn pid=24 rootfs-linux.elf cpu=1 prio=4` line with
the unchanged Nanami INFO prefix is 61 bytes versus the current 85: 28.24%
fewer byte-wise debug calls. This is only an exact byte-count opportunity,
not a measured speedup. Test the compact message with unchanged synchronous
delivery, all PID/image/CPU/priority values retained, and full ordinary
application controls. No Kernel or messaging-protocol change is required.

## Cycle 87: compact synchronous creation logs — accept the two-line change

Previous turn is progress: implement, measure and reject deferred logging,
restore the exact runtime and identify work that can actually be eliminated.
Revalidate current sources and use the accepted cycle 73 image as this cycle's
unchanged before control. Evidence is in
`/private/tmp/nanami-opt-cycle87.KL4qCo`.

### Remove redundant text, not events or fields

Change only the two creation-log format literals in `process_requests.rs` and
`threads.rs`. Keep every previously logged event, its INFO prefix and CRLF,
PID, full image name / `fork-vm` marker, logical CPU and priority. Keep the
existing synchronous Nun writer and its position before the successful reply.
Do not change verbosity, error handling, callback order, placement or shared-VM
thread logging policy. No new function, service, queue, cache, allocation,
notification, Kernel operation or message-protocol field is added.

```text
[Nanami][  INFO] [smp] process pid= 24 image=rootfs-linux.elf affinity=1 priority=4
[Nanami][  INFO] spawn pid=24 rootfs-linux.elf cpu=1 prio=4
```

Including CRLF, the representative message is 61 instead of 85 bytes: 28.24%
fewer character-wise debug calls for that record, not a claim of 28% faster
process creation. The fork path uses the same shorter vocabulary. Update the
USB same-core diagnostic instructions to recognize `spawn pid=... cpu=0`,
while documenting the old image's `[smp] process ... affinity=0` form.

### Check the actual output and an independent control workload

Reuse the exact previously captured native guest ELF (`control.elf`, SHA-256
`a038343a97237ebd2563f50139106f1a0b9c6e222d3b404d9d6a36a9988f891c`)
and identical ext2 bytes in both private control images. Do not recompile a
different fixture for the candidate. Both images lack cycle 86's experimental
logging API/consumer; the guest verifies `buffered=false` and no drain polling.
Its one unsupported status probe is outside the measured workload.

Four sequential ABBA boots execute 12 bursts of eight suspended constructors /
kill/reap operations each, separated by 137 ms. Keep the first sample and all
48 rows. A strict raw-byte parser verifies all 384 expected PIDs have exactly
one complete record with the original image, valid CPU, priority 4 and CRLF.
Per before boot, these 96 records occupy 8,160 bytes; per candidate boot,
5,875 bytes. The 28.00% observed reduction differs slightly from the single
example because candidate PID lengths vary without the old width-three pad.

This phase-distributed Alpha-only control improves only 1.74% through return
(37.4505 to 36.8005 ms); the subsequent clock stamp improves 1.69%. Do not call
the old fixture's `drained-ns` field asynchronous drain time here: both writes
are synchronous and complete before the corresponding reply.

The cycle 85 logging fraction was measured inside `spawn_process_from_elf`,
not over the full request/lifecycle. Source inspection confirms the request's
`read_process_memory_into_static_buffer` occurs before that function and PCB
publication/reap occur afterward. Its 35.8% fraction cannot be multiplied by
the byte reduction to predict an equivalent whole-application improvement.

### Repeat all ordinary controls, not just the favorable one

Run the normal-device SMP=4 process, UNIX/filesystem and three-session startup /
reuse profiles in ABBA order: twelve boots. Then independently repeat the
entire set in BAAB order: another twelve boots. Frozen input images are used
throughout, with no competing VM, build, test or hash job during measurements.
All 24 boots, every one of the 30 metrics, six framebuffer controls and exact
directory/hash assertions pass. Retain all raw samples and both comparison
sets in `comparison-results.json` and `repeat/results.json`.

| Ordinary metric | Initial ABBA | Independent BAAB |
| --- | ---: | ---: |
| Linux fork | -9.53% | -12.21% |
| Fragmented fork, 128 VMAs | -3.30% | -10.04% |
| Fragmented fork, 256 VMAs | -6.42% | -7.75% |
| Individual Alpha spawn | -7.49% | -5.40% |
| Individual Alpha reap | +1.11% | +18.65% |
| Eight-child Alpha loop | -11.32% | -3.59% |
| BusyBox lifecycle | -8.18% | -3.96% |
| Xfe lifecycle | -0.08% | -3.01% |
| Linux fork/exec | +0.24% | -0.60% |
| Negoto prepare/create | -11.48% | +7.10% |
| Tiny Core start through X11 readiness | +4.40% | -0.08% |
| Directory-small | -1.93% | +0.63% |
| Directory-large | -8.18% | +0.26% |
| Missing stat | +4.61% | +0.04% |
| UNIX recvmsg | +4.99% | -0.35% |

The ordinary Linux fork fixture really calls x86-64 syscall 57 eight times,
waits for each child, and separately tests exec and fragmented VMs; this is
not merely a native Alpha constructor measurement. Serial audits find 103
unique raw `fork-vm` records in each of the eight normal process boots, before
and after, with valid CPU/priority fields. Keep all 824 audited records/counts
and provenance in `fork-records.json`; do not claim that application PIDs or
image labels were removed to save output.

Accept the two-literal runtime change: it removes actual debug-call work with
no added architecture/lifetime cost, and the Linux fork / fragmented-256-VMA
workloads exceed the 5% target in both independent comparisons. This is not a
uniform system speedup. Initial startup degradation does not repeat, but no
reliable startup gain is established either. Negoto and several sub-stages
remain variable or unfavorable; preserve both series and do not erase them
with a pooled favorable average. The fixed-gap control remains below 5%.

### Verify compatibility and preserve the new baseline

Native 1,212 tests / 17 ignored, rendering ASan 42 / 2 ignored and ARM Alpha
checks pass. A separate SMP=1 boot passes all 96 suspended child lifecycles;
raw serial readback verifies every PID/image/priority record exactly once with
`cpu=0` and CRLF. Its serial log is in host temporary directory
`nanami-negoto-qemu-_2p1y9nh`. This brings the controlled output checks to 480
records, in addition to the 824 raw-fork records in the ordinary comparisons.

Retain the accepted normal image `/private/tmp/nanami-opt-cycle87-after.img`,
SHA-256 `7813f4baf9fbbde1c7dd1a0aea06e15a7644ac6c1d53bc300378a2cd77b1467a`.
Alpha is `50ad25783c35a46f32341d440b7f5e3ed07dc7d36a522464d6b500842c001849`;
whole init is `444c4a9111a8380f9509df3a54c1dd3fa0ee2a25e7281cc2bf2d189e111d5d78`.
Kernel remains exactly `312d579b`, Honoka `58c45cc4`, user rootfs `a6c472f8`
and the historical accepted cycle 73 image `7bfdd563`. Neither held rendering
candidate is included. No version, commit or release action is taken.

Keep snapshots, minimal patches, the unchanged guest ELF, input fingerprints,
raw logs, parsers and all results. Only closed owned control-image/rootfs
clones may be removed; the accepted image and user inputs remain. Subsequent
experiments must compare against this accepted compact-log baseline rather
than inadvertently crediting these savings to another candidate.

After all processes close, remove exactly four private control-image/rootfs
clones. The accepted cycle 87 image, historical/held images, user files, the
exact 56,080-byte control ELF and inode/fingerprint information remain. The
control ext2 input was `7de947385c5323c9a635f53d1a1cb3872bccc20fe60982f73a26b6641744284e`;
its file content is also recoverable from the prior retained control image.

Next inspect full-request image staging and the cleanup path's separate Node
REVOKE/REMOVE followed by process-Generic revocation. The current Kernel's
existing Generic REVOKE traverses generated descendants, and table-pool
release uses independent Alpha-owned Generic owners. These are reasons to
investigate a simpler ownership-consistent cleanup, not proof that changing
the order is safe or faster. Verify aliases, shared-VM/deferred reaping,
SMP-stop guarantees and partial-failure retention before any such change;
leave Kernel and IPC fastpath untouched.

## Cycle 88: one-pass physical ownership retirement — rejected

The before image is the accepted cycle87 image, not the historical cycle73
baseline. Keep the compact synchronous creation logs on both sides. Kernel,
IPC, scheduler placement, image caches, mapping/permission operations, all
root revoke/remove operations, page-table release and process-arena reset
remain unchanged. No FPU, new service, message operation or capability policy
is introduced.

First use the existing `NANAMI_EXEC_PROFILE=1` option for one diagnostic
process boot. All177 reap records are complete. Their aggregate elapsed
counter spans sum to72,048,000 ticks:

| Stage | Ticks | Share of these spans |
| --- | ---: | ---: |
| Copy-window invalidation | 1,710,000 | 2.37% |
| Root revoke | 12,721,000 | 17.66% |
| Root removal | 329,000 | 0.46% |
| Independent table release | 3,335,000 | 4.63% |
| Physical RAM release | 24,704,000 | 34.29% |
| Deferred allocation cleanup | 10,415,000 | 14.46% |
| Reference removal | 7,267,000 | 10.09% |
| Arena reset | 2,746,000 | 3.81% |
| Entry / VM metadata removal | 8,821,000 | 12.24% |

These are heterogeneous, instrumented request spans including descheduling
and interrupts, not exclusive CPU shares. Additional diagnostic output also
perturbs later requests. They motivate examining user-level retirement;
they do not prove a corresponding application speedup or justify changing
the Kernel to remove the comparatively small explicit REMOVE stage.

The existing normal reaper collects last-reference RAM allocations into a
temporary Vec, frees those ranges, then traverses ownership metadata again
to remove references. Prototype a fallible list drain: release the resource
before recycling each successful reference, retain the unreleased suffix on
failure, and reuse the existing per-PID and physical-range indexes. A last
active reference does not free RAM still held by the deferred index. Shared
VM children retain the owner's raw-PID ownership, and the existing shared-VM
reap deferral and resource-quarantine checks remain in place.

Six focused native tests cover successful retirement/root-slot reuse,
failure after a successful prefix and retry without duplicate release,
cross-process sharing, same-process aliases, shared-VM child isolation and
a deferred alias retaining RAM. These tests validate the prototype metadata
API; they do not assert that an entire failed Alpha REAP RPC is restartable
after its root capability has already been destroyed. No stronger Kernel or
OS error/retry guarantee is introduced.

Build with all diagnostic switches unset, normal devices, SMP=4 and identical
guest fixtures/rootfs. Run12 sequential ABBA boots covering process, UNIX/
directory and desktop-startup workloads, then12 independent BAAB boots.
Both comparisons pass all30 expected metrics, six framebuffer controls, the
474-entry directory checksum `18414638227522594487`, cache freshness, ordinary
logging and startup/cleanup markers. No build, other VM or profiler runs
concurrently with the timed workload. Preserve every result, not just the
favorable stages. Median elapsed changes, negative is faster:

| Workload | ABBA | Independent BAAB |
| --- | ---: | ---: |
| alpha-lifecycle-busybox-8 | -0.41% | -5.52% |
| alpha-lifecycle-busybox-8-prepare | +2.94% | -5.72% |
| alpha-lifecycle-busybox-8-source-release | +7.82% | +0.50% |
| alpha-lifecycle-xfe-3 | +4.38% | -1.82% |
| alpha-lifecycle-xfe-3-prepare | +3.50% | -3.52% |
| alpha-lifecycle-xfe-3-source-release | +1.65% | +4.67% |
| alpha-reap | -7.89% | -1.05% |
| alpha-spawn | -3.43% | +0.98% |
| alpha-spawn-reap-8 | +3.86% | +5.76% |
| alpha-xfe-reap | -0.23% | -9.98% |
| alpha-xfe-spawn | -4.25% | -5.65% |
| busybox-ls-8 | -6.24% | -1.56% |
| copyout-128-vmas | +0.08% | -1.15% |
| copyout-256-vmas | -2.26% | -1.00% |
| copyout-normal | -3.33% | -1.48% |
| directory-large-64 | +2.86% | -0.57% |
| directory-small-64 | -4.56% | -0.58% |
| fork-128-vmas-8 | -14.48% | -6.02% |
| fork-256-vmas-8 | -5.05% | -5.30% |
| fork-8 | -14.54% | -3.01% |
| fork-exec-8 | -2.93% | -2.48% |
| getpid-2048 | +2.16% | -2.66% |
| getuid-2048 | +0.02% | -1.87% |
| missing-stat-512 | +1.12% | -0.68% |
| negoto-create | -5.47% | -5.13% |
| startup | -1.29% | -3.39% |
| stat-512 | +0.95% | -0.06% |
| unix-2048 | -0.74% | -1.68% |
| unix-recvmsg-2048 | -10.47% | -1.78% |
| unix-recvmsg-eof-2048 | -3.80% | -2.24% |

The six framebuffer-control deltas are retained separately in both result
JSON files; they range -2.91%..+0.90% initially and -1.53%..+0.89% on repeat.
The fragmented-fork and Negoto gains repeat. The ordinary constructor/reaper
loop also repeatedly gets slower; the first pure-fork, reap and recvmsg gains
must not be presented as independently reproduced5% improvements.

An additional four-boot ABBA control reuses the exact56,080-byte ELF from
cycle87, SHA256 `a038343a97237ebd2563f50139106f1a0b9c6e222d3b404d9d6a36a9988f891c`.
Each boot creates/kills/reaps96 suspended children in12 bursts separated by
137ms. Both images use synchronous logging; an unsupported logging-status
probe occurs once outside timing, with no buffer/consumer API added. The
control rootfs is byte-identical between variants, SHA256
`7de947385c5323c9a635f53d1a1cb3872bccc20fe60982f73a26b6641744284e`.
All384 child records retain complete PID/full image/CPU/priority fields and
CRLF exactly once. Returned time is30.1725 ->31.1195ms (+3.14%); the immediate
second timestamp is30.240 ->31.183ms (+3.12%). This different workload does
not erase any ordinary result and does not show a5% gain either.

Decision: reject this runtime candidate. Removing the temporary Vec and a
walk is not sufficient reason to accept the repeated constructor/reaper
regression as a new default. Do not claim that every streaming-retirement
design is necessarily slower, or that these tests identify cache misses,
IPC transport or a particular Kernel path as the cause. Retain the measured
tradeoff and investigate cleanup work with its batching/cache behavior intact
before adding another persistent index or changing capability teardown order.

Evidence is `/private/tmp/nanami-opt-cycle88.xCeKMP`: diagnostic parser/raw
serial path and `baseline-profile.json`, exact before/candidate sources and
minimal patches, all24 ordinary boot logs, `comparison-results.json`,
`repeat/results.json`, control scripts/ELF/raw logs and `control-results.json`.
The rejected normal image `/private/tmp/nanami-opt-cycle88-after.img` is
`5ae938c4da4597d87ff6101853d517b896a946a263d97f96ae1cd8897e9ee67a`;
its Alpha is `dbdf15a74b3df46b8b28137c3a071e0b7d3d74b3fbc712f69e25494edb85aa2e`
and init is `a11d23ba64c5e294e2efb7d90ff6bc5d30c6da5d0d393592a3dccf26dafa232a`.
Candidate native1218/17 ignored, core ASan111/2 ignored and ARM checks pass.
Archive the six prototype-only tests with the candidate, not as a dead API
in the normal runtime.

Restore the production sources and rebuild normally: Alpha
`50ad25783c35a46f32341d440b7f5e3ed07dc7d36a522464d6b500842c001849`
and init `444c4a9111a8380f9509df3a54c1dd3fa0ee2a25e7281cc2bf2d189e111d5d78`
again exactly match cycle87. The freshly generated raw image has SHA256
`11faad091b70959ef335cb3b1817f26b8ee8caca8bce58019a3b88e817128bbe`;
do not describe the entire regenerated image as byte-identical to the accepted
one. Kernel312d579b, Honoka58c45cc4, Alter fea651de, Ext228dcd199
and the user's rootfs a6c472f8 remain unchanged. Cycle87's accepted image
`7813f4baf9fbbde1c7dd1a0aea06e15a7644ac6c1d53bc300378a2cd77b1467a`
is retained and unchanged. No version, commit or release change is made.

Restored native1212/17 ignored, core ASan105/2 ignored and ARM checks pass.
The restored normal image also completes three actual TinyCore desktop
startup/cleanup/reuse sessions with no diagnostic profiling enabled. This
restoration is a correctness/identity check, not another speedup comparison.

After all VM/build sessions terminate, remove only six closed owned trial
artifacts: `before-profile.img`, `control-before.img`, `control-after.img`,
`control.rootfs.img`, `rootfs.img` and `restored.img` inside the cycle88
directory. These generated inputs are rebuildable; the control rootfs remains
recoverable from the retained cycle86 control image. Keep cycle88's rejected
normal image, all exact source/ELF snapshots and raw logs, cycle87's accepted
image, historical/held images and every user TinyCore input. Logical deleted
file sizes total1,883,310,080bytes; APFS sharing means this is not a claim about
physical space recovered.

## Cycle 89: audit the unmeasured syscall round trip and cross-core cost

Cycle88 is progress: complete and reject the retirement prototype, preserve
both its favorable and contrary results, restore exact accepted bytes and
verify normal startup/reuse. The current default remains cycle87. The user
then questions whether the small isolated wins miss an order-of-magnitude
cost and specifically asks whether QEMU IPI handling is unusually slow.

First locate the original cycle1 before image (`48d04314...69054`), extract
its init/Kernel/rootfs privately and assess a matched cumulative comparison.
Do not modify the original image or describe a sequence of per-cycle gains as
a cumulative benchmark. Its Kernel SHA6913f87c differs from the current
312d579b; a future user-level cumulative comparison must explicitly hold the
Kernel constant and audit runtime/data/fixture differences. This preparation
does not yet establish a cumulative result and is superseded in this turn by
the user's requested large-path investigation.

Source inspection finds an important measurement boundary. Existing
`common/fault.rs` stamps immediately around `dispatch_syscall`. It excludes
fault delivery, service-event decoding, process resolution, reply generation,
the event-loop readiness/signal tails, reply-receive and execution resumption.
The normal combined IPC direct reply-receive branch in `ipc_port.cpp` requires
`client->fault_reason == NONE`; Linux syscall faults instead apply the fault
reply context, mark the client scheduled and take the receive path. Fault
delivery still has its own direct-switch attempt. Do not conflate ordinary
IPC fastpath with this complete fault round trip or change either Kernel path
on the basis of source inspection alone.

Use the existing opt-in `NANAMI_ALTER_PROFILE=1`, with no production source or
Kernel change. Save the normal Alter ELF first, compile only diagnostic Alter,
and install it in a private rootfs clone. Explicitly verify installation; a
rootfs-loaded Alter is not changed by merely rebuilding init. Package the
private rootfs under an unchanged accepted cycle87 image prefix. Add an
integer-only private raw Linux probe that checks getpid results, samples
16,384 calls three times and reports both its local counter delta and guest
monotonic interval outside each timed loop. The standard native wrapper's
PROCESS-PERFORMANCE-PASS here marks this diagnostic probe, not the original
fork/fragmented-memory performance fixture. Do not parse it as a normal
30-metric comparison or claim a runtime speedup.

The first probe completes49,153 getpid calls on PID57/CPU2. Diagnostic handler
total is94,444,000 counter ticks, about1,921 ticks/call. The guest loop's three
intervals are313.185/314.353/309.076ms. Its counter intervals closely track
guest nanoseconds. Both this guest and AlterPID18 are on CPU2; a direct
cross-core IPI for each guest-to-Alter handoff cannot explain that whole span.
Other cores, Kernel locking, device activity and scheduling remain active.

Next keep one compiled timed loop and fork three sequential ordinary private
children. Their actual spawn records place PID58 on CPU3, PID59 on CPU1 and
PID60 on CPU2; Alter stays on CPU2. Each child performs the same three16,384-
getpid intervals and verifies every returned PID. The parent waits/checks
each exit. The existing profiler's exit-flushed totals contain49,153 getpid
calls for each child, independently matching the guest count plus its initial
PID query. No log is emitted inside the measured getpid loops.

| Child / placement | Median whole getpid round trip | Mean dispatch span |
| --- | ---: | ---: |
| PID58, CPU3 -> Alter CPU2 | 52.265us | 2,589.8 ticks |
| PID59, CPU1 -> Alter CPU2 | 51.960us | 2,554.9 ticks |
| PID60, CPU2 -> Alter CPU2 | 19.282us | 1,965.4 ticks |

Guest counter-ticks/ns ratios over these long intervals are0.99970/0.99978/
0.99937 respectively. This supports an approximate local counter-unit
calibration for this QEMU experiment, not cross-core absolute timestamp
subtraction or a general real-hardware clock guarantee. The dispatch spans
include timestamp overhead and possible descheduling, not exclusive CPU time.
They are about2us while complete same-core calls are about19us; optimizing
only the body neglects the larger elapsed interval outside that measurement.

Cross-core whole calls are about2.7 times the same-core result. This is not
an isolated IPI latency measurement: guest scheduling, fault IPC, interrupts,
locks, context restoration, the host and child execution order are not held
constant independently. No real-hardware/KVM control has been run, so the
entire difference cannot be attributed to a QEMU defect or to A9N alone.
Local calls still have a substantial remaining span. Neither10x nor100x
whole-application improvement is established by this diagnostic.

The host is arm64; `qemu-system-x86_64 --version` reports11.1.1 and `-accel help`
lists only tcg. QEMU's MTTCG design documentation describes per-vCPU host
threads when enabled, BQL serialization for MMIO/interrupt-state updates,
cross-vCPU synchronization and potentially substantial strong-guest/weak-host
memory-ordering overhead:
https://www.qemu.org/docs/master/devel/multi-thread-tcg.html
These mechanisms motivate separating emulator overhead from OS implementation
work. This experiment does not determine the actual single/multi TCG thread
mode, count IPIs, measure individual IPI delivery or prove host-thread wakeup
as the cause. Do not tune or weaken Kernel/IPC correctness for this result.

Both diagnostic boots pass the private probe, wrapper cache checks and cleanup
without checked fatal/proc/SHM errors. First serial is
`nanami-negoto-qemu-u47fdy56`; the three-worker serial is
`nanami-negoto-qemu-u0bbny_7`, under the host temporary directory. Evidence is
`/private/tmp/nanami-opt-cycle89.3JMFwU`, including both probe sources/ELFs,
normal/profile Alter ELFs, install/build logs, `parse-floor.py` and
`floor-results.json`. The first failed direct build used the stable Cargo
shim; retry the same build using the explicit installed nightly Cargo and
keep the failure log. No production source edit, affinity change, API change
or accepted optimization is made in this investigation.

## Cycle 90: verify fault-context copies, reject the small runtime change

Previous cycle89 is progress: identify the profiling boundary and a2.7x
cross-core whole-syscall penalty, retain calibrated observations and restore
normal Alter bytes without Kernel edits. Continue toward the full objective,
not a claim that this fault diagnostic optimizes applications by10--100x.

Inspect vDSO/clock possibilities and existing startup profiles before adding
a new protocol or shared clock implementation. No vDSO is implemented here.
The current Linux initial stack lacks AT_SYSINFO_EHDR; clock handlers use
existing services and checked copyout. This is a candidate direction, not
evidence of dominating whole-startup work. Do not grant MMIO access or weaken
the still-unresolved guest mapping-authority policy to manufacture a shortcut.

Inspect the actual Release ELF next. The main fault loop really calls memcpy
for184-byte x86_64 hardware-context moves at three sites on the ordinary path;
many other memcpy calls in the main function are one-time Runtime setup, not
per-fault work. The linked memcpy already uses REP MOVSB/MOVSQ, so replacing
its scalar algorithm is not a new finding. Before symbols are main0x100e760/
size0x2494, handle_fault0x1006660/size0x11d9 and decode_service_event size0xe5d.
Preserve disassembly rather than infer performed copies from Rust syntax.

Prototype borrowing the pending reply during receive and the owned
ServiceEvent snapshot during fault handling, for both Linux and FreeBSD.
FaultEvent carries a lifetime-bound reference to that snapshot, not the live
IPC buffer which nested calls may overwrite. Required signal/legacy/full
reply snapshots remain owned. No new enum variant, message, cache, authority
change, register count or Kernel operation is introduced. Generated main code
has one rather than three184-byte copy sites, eliminating two such moves per
ordinary round trip. The larger handler text is also retained as contrary
footprint evidence, not assumed to be faster.

Full native1212/17 ignored, x86 Linux and FreeBSD builds, AArch64 Linux and
FreeBSD checks, and ASan4 reply-prefix plus26 signal/delivery tests pass.
Freeze the final candidate after its snapshot-lifetime comment; install its
exact ELF into a private rootfs. Both sides use the same unmodified cycle89
three-child getpid probe and normal, non-profiled runtime. Kernel/Alpha and
image prefixes remain accepted cycle87 bytes. e2fsck -fn passes both rootfs.

Run four sequential normal-device SMP4 boots in ABBA order with no build,
test, profiler, hash job or other owned VM overlapping. Each probe child
checks16,384 getpid results three times; child placements are CPU3/1/2 with
Alter on CPU2. Preserve all36 samples and their counter/monotonic calibration,
complete init/dynamic/interactive/cache freshness markers and error checks.
This custom diagnostic replaces the raw process fixture; it does not represent
a normal30-metric whole-application comparison.

| Worker CPU | Before round trip | Candidate round trip | Change |
| --- | ---: | ---: | ---: |
| 1, cross-core | 50.514us | 50.937us | +0.84% |
| 2, same-core | 18.904us | 18.822us | -0.43% |
| 3, cross-core | 50.891us | 51.385us | +0.97% |

Decision: reject this candidate. Two actual copy removals do not reproduce
the5% target, much less explain an order-of-magnitude application improvement.
Do not keep a changed internal borrow ABI solely because its assembly has
fewer copies. Restore all three source files, rebuild Linux/FreeBSD normally
and verify exact fea651de/ad97c4e1 ELF hashes. Kernel312d579b stays unchanged.
No version/commit/release or runtime permission change is made.

Evidence is `/private/tmp/nanami-opt-cycle90.iwmAec`: exact before/candidate
sources, normal ELF snapshots, disassembly, build/native/ASan/ARM logs,
four boot reports, `floor-trials.jsonl`, `parse-floor.py` and
`floor-results.json`. Timed candidate Linux is
`e1f69c0f4de384bf39d67ee9a019e3231c8e6fd5476740a15f3a3113d2b6845f`;
before/after raw probe images are8fae3342/32d3e454. These are experiment
artifacts, not accepted default images. The pre-comment intermediate ELF is
also retained but was not used in these timings.

Re-read current raw-task creation and address-space mapping authority before
reviving the larger fork-sharing direction. The guest root contains PCB,
AddressSpace and Frame-node capabilities; valid mapping attributes are not
restricted to a supervisor's recorded Linux permissions. Current private
copies isolate parent/child contents, but simply sharing currently read-only
physical pages would let a guest use native A9N mappings to bypass that new
protection. Do not describe this as an existing cross-process memory-write
vulnerability: the unsafe sharing has not been introduced.

Ask explicitly whether Linux guests may have an isolated native capability
root while ordinary native processes and Kernel behavior remain unchanged.
The user requests an explanation instead of choosing permission. Therefore
no authority restriction, read-only sharing or COW is authorized or implemented.
Explain that capability placement is a prerequisite, not an optimization by
itself, and that COW needs supervisor-write/mprotect/fault/refcount/lifetime
handling too. The earlier229MB inventory covers three startups, not memory
bandwidth or startup CPU share, and proves neither10x nor100x performance.

## Cycle 91: test guest/Alter co-location with existing scheduling policy

The previous explanation turn does not count as performance progress. Recheck
current hashes and the actual placement code before continuing: accepted87
Alpha50ad2578, init444c4a91, Alterfea651de and Kernel312d579b are unchanged.
Cycle56's broker-only probe was not a guest-co-location experiment. Test the
remaining hypothesis using the existing generic `@placement 4 2` declaration,
without new scheduling code, migration, authority changes or a Kernel patch.
This class includes native priority4 applications, not exclusively Linux guests.

Make a private image with just this packaged boot-list change, then immediately
restore the source configuration and exact normal init. Candidate Alpha itself
is byte-identical to baseline; candidate init is d7d62c1b. Candidate raw image
is ca319b17. Rootfs readback from both images hashes to the exact same
a6c472f8 input. Two initial cmp commands reach EOF instead of establishing the
intended bounded comparison; use complete256MiB `dd` readback and SHA-256 to
verify identity, rather than treating their chained shell status as proof.

Twelve normal-device SMP4 boots, ABBA order, finish all30 metrics and six
framebuffer controls, with unchanged fixtures, directory counts/checksums and
complete init/dynamic/interactive/restart footers. No own VM/build/test/hash job
overlaps the measurements. Serial confirms ordinary fork children58/59/60 move
from CPU3/1/2 to CPU2/2/2; Alter stays CPU2. The original main getpid control was
already on CPU2, so its neutral result is not a cross-core latency comparison.

| Workload | Before | Co-located | Change |
| --- | ---: | ---: | ---: |
| Tiny Core startup through X11 readiness | 4928.453ms | 5133.542ms | +4.16% |
| Linux fork, eight children | 17.448ms | 16.851ms | -3.42% |
| Linux fork/exec, eight children | 684.529ms | 669.551ms | -2.19% |
| Fragmented256-VMA fork | 16.712ms | 17.300ms | +3.52% |
| Negoto create | 6.152ms | 6.635ms | +7.84% |
| Large-buffer directory scan | 78.882ms | 87.112ms | +10.43% |
| BusyBox ls | 1963.174ms | 1929.328ms | -1.72% |
| UNIX write/read | 388.817ms | 376.534ms | -3.16% |

Reject the placement change: no ordinary metric reaches the5% improvement
target, startup and several controls are worse, and pinning compute workers
would sacrifice parallel scheduling. Do not promote the isolated2.7x getpid
diagnostic into a whole-TinyCore improvement. All samples remain in
`/private/tmp/nanami-opt-cycle91.fDxoZS/results.json`, with scripts, frozen
inputs, boot reports and source/ELF snapshots. No scheduling preference from
this cycle remains in the shipped boot list. Kernel remains unchanged.

## Cycle 92: isolate Linux guest capabilities before VM sharing

The user explicitly approves the explained user-level authority change.
Implement a separate executing capability namespace, not mass capability moves
or object destruction. Alpha retains each private management root and its
PCB/AddressSpace/Frame/page-table capabilities. A lazily created, permanently
empty two-slot Node under Alpha serves all sealed executing tasks; it belongs
to no guest arena and is never copied into a guest-addressable slot.

Add optional `PROCESS_SEAL_CAPABILITIES`0x1030. Only the target's reaper may
request it; self, Alpha, missing/exited targets and nonzero reserved arguments
are rejected. First suspend through the existing SMP-safe Kernel operation,
configure only the PCB root, then mark success. Failed suspension/allocation/
configuration never publishes the seal. Authorized repeats are no-ops. The
trusted supervisor calls before first execution; this is not retroactive
revocation of aliases previously delegated to another process.

Both Linux launch paths opt in before execution. Native and ordinary FreeBSD
launches do not. Raw fork/shared-VM children inherit the seal during their
existing PCB configuration; exec's AddressSpace-only configuration retains it.
There is one extra initial-launch RPC, not a new per-syscall or per-fork RPC.
The first implementation sends five request words, including explicit reserved
zeroes. Cycle93's source recheck corrects the initial stale-MR concern: Alpha's
decoder explicitly substitutes zero for arguments outside message_length, so
the two-word form is also safe. Do not infer this from Kernel zero-fill.
Kernel source, ABI and IPC fastpath remain unchanged. Eager private copying
is intentionally still present; this authority boundary alone is not COW or
a claimed speedup.

Five native tests execute the production handler against modeled Kernel calls:
authorization/reserved/exited checks, shared empty-root reuse, unchanged
management descriptors, idempotence, suspend/allocation/configure failures and
fresh/reused native-process flags. Full native1217/17 ignored, core ASan110/2,
ARM Alpha/Linux/FreeBSD checks and TinyCore builder32 tests pass. Preserve the
two initial mistyped native-test target failures rather than reporting them as
passes. The corrected target is `core-process`.

Real integer-only guest checks invoke A9N CapabilityCall directly, verify legacy
descriptor denial through initial/fork/shared-VM/exec states, and exercise Linux
mmap/mprotect/munmap, independent private writes, shared writes and exact wait
statuses. An unsealed image fails with status10; sealed SMP1/SMP4 images pass.
Keep initial inline-probe evidence but use a separate assembly ABI helper in
the final fixture, avoiding a new compiler-specific inline-assembly extension
in C. Also address both slots at the empty one-bit root's exact depth.

Freeze before/after runtimes and run twelve ordinary normal-device SMP4 ABBA
boots, then independently repeat all twelve in BAAB order. Every boot passes
all30 metrics, six frame controls, exact directory work/checksum assertions,
complete init/dynamic/interactive markers and repeated-start cleanup. No own
build/test/hash/other VM overlaps timed work. Keep both series separately:

| Metric | ABBA | Independent BAAB |
| --- | ---: | ---: |
| Tiny Core startup | -0.92% | +0.08% |
| Ordinary Linux fork | +7.34% | -0.12% |
| Fragmented256-VMA fork | -2.19% | -8.09% |
| Linux fork/exec | +2.04% | -0.95% |
| getpid | -1.02% | +1.46% |
| getuid | +0.99% | +1.01% |
| Normal checked copyout | +0.89% | -0.97% |
| Alpha eight-child loop | +8.89% | -2.70% |
| Individual Alpha spawn | +6.78% | -2.98% |
| Individual Alpha reap | +5.28% | -0.49% |
| BusyBox lifecycle | +1.48% | -8.42% |
| Xfe lifecycle | +6.74% | +1.27% |
| Negoto creation | +0.96% | +1.78% |
| UNIX recvmsg | +0.97% | -3.58% |
| Large directory scan | +1.37% | -2.21% |

The first series'7--9% fork/constructor regressions do not reproduce, but this
does not prove equal speed in every path. Preserve the modest contrary Xfe,
Negoto and credential results as well as favorable ones. No reliable overall
speedup is established and no5% optimization win is claimed. Retain this as
the user's explicitly requested prerequisite for sharing, not as an excuse to
declare the broader optimization goal achieved.

Evidence is `/private/tmp/nanami-opt-cycle92.t0Lrg7`: before sources, patches,
build/test logs, exact runtime ELFs, functional reports, `results.json` and
`reverse/results.json`, scripts, all raw boot samples and their serial paths.
Timed images are preserved as `before-perf.img`942c398b and
`after-perf.img`b785e938. Original paths in the trial records refer to the
identical `before.img`/`after.img` files; do not overwrite those timed inputs.
These images contain the earlier assembly probe0412fcc7, which is never
executed in the ordinary performance workloads.

The final fixture additionally checks both exact-depth9 descriptors for the
two-slot execution root, for258 rejected descriptors in each of four states.
Its stronger runner requires exactly three distinct successful exit records,
including the parent, so an accidental inherited sentinel FD cannot bypass
the child/exec checks and produce a false PASS. Final fixture740ff9ba passes
both SMP1 and SMP4; an unsealed control fails with the expected status10.
Keep the earlier inline/assembly probe results as separate evidence, not mixed
with these final checks. The new `--with-tests` recipe compiles the fixture
with a separate assembly helper; its added TinyCore/bin entry naturally changes
future directory-inventory fingerprints, not these frozen474-entry workloads.

Final `after-final.img`32333919 differs from the timed after image only by the
stronger, otherwise unused fixture and ext2 metadata. Alpha is
eb199435c1389b98a4237ef26a6f633db7d9ce89cbd9b62d4c814f41d8797f2c;
Alter/Linux is c6abed0242b107b42fa518f593a609f1dfb012e6d6e18fe3ec46a73994c7d9f3;
init is409c84a5d975aaf28edf154cb0b437a5cd7107d403a4b17b07b8c987ff61fe51.
Kernel remains exactly312d579b and A9N's worktree remains clean. User input
rootfs a6c472f8 is not overwritten; use the paired new Alpha/Alter image rather
than combining the new core with an old reused rootfs's unsealed Alter ELF.
Both final rootfs images pass `e2fsck -fn`.

The final real interactive GUI passes two complete desktop sessions in one
SMP4 normal-device boot: X11/FLWM/wbar, title drag, actual input, launcher links,
Terminal/Editor/Information launch, independent command markers, Exit and
same-ID restart cleanup. Serial ends `TINYCORE-DESKTOP-PASS x11=true input=true
cleanup=true`; log is `desktop-final.log`, serial directory suffixgduu9ckz.
Inspect the first apps screenshot as well: the terminal, editor and launcher
are actually rendered, not merely a process-ready marker. No COW, read-only
page sharing, vDSO, version bump, commit or release is introduced.

Retain the approved capability-isolated runtime as the new functional baseline,
not as a measured5% speed win. Later sharing work must start from this paired
Alpha/Alter rootfs rather than silently reverting to accepted87's unsealed
Linux ELF. The broader optimization goal remains active.

Remove exactly four closed cycle90 floor-image/rootfs clones, then the closed
cycle91 restoration image and cycle92 intermediate core-only image. Retain
their source/ELF snapshots and logs; these six generated artifacts can be
rebuilt. User inputs, accepted/held earlier images, and final/timed cycle92
images remain. The new runtime is a functional authority-boundary change,
not fulfillment of the entire requested optimization objective.

## Cycle 93: account for deferred RAM owners before sharing

Start from the capability-isolated cycle92 baseline. The first prototype below
is rejected; only the final partitioned runtime is retained as its successor.

Find a real RAM-lifetime defect in the separate active/deferred reference
tables. A private mapping can be explicitly shared with another process;
releasing the source then reusing its VA can clear the peer's live page.
Last-reference decisions during release and exec also omitted deferred peers.
Four new public-API regressions fail against the original implementation.

Prototype one reference count and recycled ownership-node pool across active
and deferred states. Moving to deferred preserves the record and reference
without decrement/reincrement. Reuse requires exactly one total owner. The
independent mixed-operation model now includes both states; transition tests
check unchanged reference-index storage and bounded node reuse. Explicit
shared-memory reservation/release semantics and Kernel code are unchanged.

An untimed native QEMU fixture shares a page with a suspended peer, releases
and remaps the source, checks zero-filled new backing and unchanged peer data,
then cleans up. The original image fails in round0: peer data becomes zero.
The unified prototype passes eight rounds on both SMP1 and SMP4. Native1225 /
core ASan116, ARM Alpha/Linux/FreeBSD and builder32 tests pass. The shortened
two-word sealing request also passes actual capability denial through initial,
fork, shared-VM and exec states on SMP1/SMP4, with exactly three distinct
successful exits in each run.

Freeze the same ordinary probe binaries and run twelve normal-device SMP4
ABBA boots. All30 metrics, six frame controls, exact474-entry directory work
and full workload footers pass. The initial prototype gives startup-1.23%,
ordinary fork-2.21%, fork/exec-1.64%, Alpha eight-child loop-3.56%, and complete
BusyBox/Xfe lifecycles+0.90%/+0.47%. Contrary controls include fragmented128-VMA
fork+7.36%, stat+6.89%, Negoto creation+4.35% and BusyBox source release+15.03%.
Do not promote selected faster spawn/reap stages into a whole-system win.
The SDK sealing request also changed from five words to two, so these results
are not an isolated reference-table microbenchmark.

Reject that contiguous-index prototype for deployment: the full GUI workflow
fails at the Information launcher with an Alpha8MiB allocation failure. A
matched original-image control completes both entire desktop sessions. Keep
this failure, not only the favorable host and short QEMU checks. Evidence is
`/private/tmp/nanami-opt-cycle93.XmdgVz`, including the immutable first
before/after images, `results.json`, `desktop.log`, negative/positive RAM tests
and `desktop-before-control.log`.

Replace the single reference index with16 high-hash-bit partitions, each
retaining the low-bit open-addressed index. Counts still span active/deferred
owners; this is storage partitioning, not separate ownership domains. The
Alpha heap remains32MiB. A65,537-reference test checks every lookup/removal,
less total slot storage than the original8MiB index and no partition over
512KiB for that workload. Native1227 / core ASan117 pass. This revised runtime
is under `partitioned/`. No COW or read-only fork-page sharing is enabled.

The diagnostic build confirms the failing allocation's origin exactly:
`key-bytes=16 entries=65536 old-slots=131072 new-bytes=8388608`, immediately
followed by the8MiB allocation panic. Preserve `desktop-diagnostic.log` and
its serial path. Remove the diagnostic logging from production before building
the partitioned candidate. Extract its init payload before deleting the closed
diagnostic disk to recover space; the failed ordinary prototype, all comparison
images, source/ELF snapshots and accepted cycle92 remain available.

The final partitioned runtime passes eight real page-reuse rounds on both
SMP1/SMP4, both exact capability-denial probes, and two complete normal-device
SMP4 GUI sessions. Terminal/Editor/Information launch, real input, independent
command markers, Exit and same-ID restart all pass. Inspect the first apps
screenshot: terminal, editor and launcher actually render. Serial directory
suffix is `lzc5g8pu`; `partitioned/functional-run.log` records all five runs.

Run twelve fresh ordinary SMP4 ABBA boots against the same cycle92 runtime
and frozen probes. All30 metrics and six frame controls pass, including exact
474-entry directory/checksum/work assertions and complete workload footers.
No own build, test, hash job or other VM overlaps timed work. Preserve the full
results in `partitioned/results.json`, not just selected favorable timings:

| Metric | Before | Partitioned | Change |
| --- | ---: | ---: | ---: |
| Tiny Core startup through X11 readiness | 4950.807ms | 5115.591ms | +3.33% |
| Ordinary Linux fork, eight children | 18.198ms | 17.567ms | -3.47% |
| Linux fork/exec, eight children | 693.232ms | 673.263ms | -2.88% |
| Fragmented128-VMA fork | 19.430ms | 16.790ms | -13.59% |
| Fragmented256-VMA fork | 17.582ms | 16.763ms | -4.66% |
| Alpha eight-child loop | 23.556ms | 25.155ms | +6.79% |
| Complete BusyBox lifecycle | 29.962ms | 27.479ms | -8.29% |
| Complete Xfe lifecycle | 33.109ms | 32.829ms | -0.85% |
| Negoto create | 6.346ms | 6.770ms | +6.69% |
| BusyBox ls | 1978.243ms | 2032.723ms | +2.75% |
| getpid | 41.002ms | 40.143ms | -2.09% |
| getuid | 40.730ms | 40.539ms | -0.47% |
| Normal checked copyout | 172.481ms | 171.621ms | -0.50% |
| stat | 115.991ms | 113.580ms | -2.08% |
| UNIX write/read | 368.682ms | 376.694ms | +2.17% |
| UNIX recvmsg | 730.053ms | 745.105ms | +2.06% |

This is one balanced comparison, not an independently repeated5% optimization
claim. In particular, do not hide the slower startup, constructor loop, Negoto
and socket controls. Retain the final implementation for the independently
demonstrated RAM-lifetime repair, not as fulfillment of the broader speed goal.

Final image is `partitioned/after.img`, SHA25620f3be7b9d9ef754fed61db642d54457e75aa4864eb571b6e76907d3a5114927.
Its paired rootfs is `rootfs.img`, SHA2566013d95fde007564385d5ee2dfd17c364777647d8b82f5251b1dad9fdc8b0b5a.
Alpha is379b9c4102b0060b69b2c94cbb2abc2f29126bcd4bb4d570b320503a979396df;
Alter/Linux isfb20fa00e5189ce1d6ce694b875ec82b254052688b5d2e1c64343a135ae1962c;
init isdc8d7e1946bc78d80a93ed0331a9ab71620fe03584a4707a475396c282dab519.
The packed Kernel remains exactly312d579b; its source worktree remains clean.
Final rootfs passes `e2fsck -fn`; original user inputs are not overwritten.
No version bump, commit or release is introduced. The optimization goal stays
active; direct initial permission mapping is a separate next candidate, not
an untested change mixed into these frozen results.

## Cycle 94: reject final-permission private-fork construction

The previous turn made concrete progress: cycle93 repaired a demonstrated
active/deferred RAM lifetime error and passed full GUI reuse. Its partitioned
runtime remains the accepted baseline. No repeated blocking condition exists.

Inspect the real fresh private-fork path. It initially maps copied pages RWX,
then walks the snapshot again and changes nondefault protections. Prototype
mapping each accessible page with its final permissions through a strict
attribute-aware MemoryManager entry. Preserve explicit shared-memory semantics,
private byte copies and sparse/unbacked reservations. Retained PROT_NONE backing
still prepares intermediate tables with a temporary leaf, then unmaps it.

Do not retain an intermediate design that splits allocation runs at every
permission boundary. Instead borrow the existing per-page VM snapshot and keep
the original contiguous allocation batching, including alternating protections
and Frame-node boundaries. Existing-destination copying stays on the generic
path. Validate masks/counts before allocating; failed MAP, MAP-after-install
and failed PROT_NONE UNMAP remain covered by owned rollback.

A new 520-page read-only construction regression fails against the original
path before implementation. The final prototype passes native 1,236 tests /
17 ignored, focused ASan 165 / 2 ignored and the AArch64 Alpha Release check.
Tests cover strict failure propagation, final first-map attributes, initialized
prefix/tail bytes, alternating rights without allocation splitting, sparse
and retained PROT_NONE backing, later independent writes and failed publication.
No Kernel, Alter, rootfs, protocol, scheduling or normal probe change is made.

Freeze inputs and run twelve ordinary normal-device SMP4 ABBA boots, followed
by twelve independent BAAB boots. Every boot passes all 30 metrics, six frame
controls, exact directory work/checksum assertions and complete workload
footers. No own build/test/hash job or other VM overlaps timed work. Preserve
both series, including favorable complete BusyBox-lifecycle timings:

| Metric | ABBA | Independent BAAB |
| --- | ---: | ---: |
| Ordinary Linux fork | +14.22% | +13.77% |
| Fragmented128-VMA fork | +0.60% | +2.55% |
| Fragmented256-VMA fork | +6.30% | +8.28% |
| Linux fork/exec | +6.89% | -1.84% |
| Alpha eight-child loop | +14.67% | -2.65% |
| Complete BusyBox lifecycle | -9.22% | -5.55% |
| Complete Xfe lifecycle | +2.85% | -3.99% |
| Negoto creation | +21.24% | +11.97% |
| Tiny Core startup | +4.45% | -5.03% |
| getpid | +3.02% | +9.41% |
| getuid | +4.73% | +2.10% |
| Normal checked copyout | +5.58% | +4.18% |
| UNIX write/read | +9.69% | +2.52% |
| UNIX recvmsg | +4.78% | +5.22% |

Unchanged controls vary too; do not attribute every delta to a specific
instruction or ignore the favorable lifecycle result. Nevertheless, ordinary
fork and Negoto repeatedly regress. Fewer source-level mapping calls do not
prove a useful whole-path improvement. Reject the prototype and restore all
seven source/test files through a checked patch. Each restored file matches
its pre-experiment snapshot exactly. Candidate sources, tests, ELFs and images
remain archived in `/private/tmp/nanami-opt-cycle94.G1SW4a` with complete
`results.json` and `reverse/results.json` and all raw boot/serial references.

Candidate image is8005eb87fbb28494419527b54f560b32b297de73518065f0e9beba8c3508160d;
candidate Alpha iseee26818905e32e429166bc54613c85bdb691e940f5d0da9cc727727f971ed93;
candidate init isbc2e8319464f04b440cd31870207b7d9d3a0e9e5f6a86b54055a42ecdee7cb31.
Both timing variants retain rootfs6013d95f, Alterfb20fa00 and Kernel312d579b.

The restored normal build reproduces Alpha379b9c41, initdc8d7e19 and Alterfb20fa00
byte for byte. All 1,227 restored native tests pass / 17 ignored. A separate
normal-device SMP4 boot completes three fresh X11-readiness startup/kill/delete/
same-ID cycles: `restored-startup.log`, serial directory suffix `ot4ji3_h`.
The accepted cycle93 paired image/rootfs are unchanged. No candidate helper,
permission-snapshot argument, version bump, commit or release remains.

Review the prior evidence before choosing another copy-avoidance path. Although
sys_vfork still delegates to ordinary copying fork, cycle31 already recorded
51 ordinary clone calls with flags0x1200011, not VM-sharing/VFORK requests.
Do not repeat the unsupported inference that vfork dominates this workflow or
silently change ordinary fork semantics. Any future use of that old diagnostic
must inspect its actual raw evidence and account for recipe/runtime changes.

Fresh current-Kernel source audit also corrects an earlier reasoning premise:
IDLE has a separate `idle_address_space_storage`; switching to it calls the
HAL context switch and x86_64 loads its distinct CR3, clearing the previous AS
owner bit. Do not claim that current IDLE retains guest CR3. Running remote
suspension waits for the completed context switch; already noncurrent targets
take a different path. However, ordinary SUSPEND cancels IPC waits, rewinds an
invalid-kernel-call fault and clears fault state. Therefore safe user-level VM
quiescence needs correct task-state/fault ownership handling; blindly suspending
and resuming every task can replay a serviced syscall. This is a concrete next
audit for read-only sharing/COW, not a completed implementation, a generic ARM
coherence guarantee, or permission to modify Kernel fastpaths. The broader
optimization goal stays active.

## Cycle 95: final-owner alias retirement and deferred failure safety

Continue from accepted cycle93B, not rejected cycle94. The Linux execution-root
isolation approved by the user remains in place. Kernel, IPC fastpath, Alter,
placement, FPU, image caches and mapping/permission operations are unchanged.
Read-only private-page sharing and writable COW are not enabled in this cycle.

The sharing audit finds a concrete lifetime bug. The old reaper snapshots only
ranges with a global reference count of one. If one process holds both final
aliases, neither enters that snapshot; later bulk removal drops their metadata
without freeing RAM. Active/deferred aliases in the same process can similarly
hide the final owner until the active references have retired. Deferred exec
cleanup also removes only caps selected as unique RAM owners, leaving other
local Frame aliases behind when a registered peer retains the RAM.

Keep active retirement batched. Its immutable snapshot groups non-unique ranges
only, and selects one release when every remaining reference belongs to the
local active stage. Unique private pages neither allocate an alias buffer nor
sort. Retire active references before cleaning deferred records. Deferred
retirement removes every local capability and frees RAM only at its final
active/deferred reference. Its fallible list drain commits each successful
record, restoring the failed record and suffix on error; it allocates no extra
snapshot and does not restore already-freed prefix ownership. Raw child PIDs
must not resolve to and drain a shared-VM owner's records.

An initial batched deferred version passes normal functionality, but review
finds unsafe retention of a successful RAM-free prefix after a later error.
Archive that V2 and retain the corrected V3. Native tests cover same-PID aliases,
mixed active/deferred states, registered peer owners, shared-VM isolation,
complete capability removal, and failures after successful RAM/capability
release. A4,000-step independent ownership model now checks last-local-stage
snapshots rather than reproducing the old count-equals-one bug. Do not infer
whole REAP/EXEC retryability after partial Kernel teardown or same-slot retry
after only some capabilities in an allocation have been removed.

The real untimed `--ram-alias-retirement` probe gives a suspended peer two aliases
of a1MiB shared allocation, retires the original owner, then kills/reaps the
final peer. After two warmups, the old init loses exactly4,194,304 bytes across
four measured retirements. The final candidate passes SMP1 and SMP4 with no
corresponding loss. This uses the same new native fixture on both sides, not an
old fixture that silently lacks the option. An initial fixture preparation
mistake requests peer zero through an SDK that rejects it; preserve that log
but do not count it as evidence for the runtime bug.

Final native tests pass1,239 / 17 ignored; core ASan passes129 / 2 ignored,
the ARM Alpha check and three runner predicate tests pass. Final real runs also
pass private-page reuse and execution-root isolation on SMP1/SMP4. A normal
SMP4 GUI boot completes two full X11/FLWM/wbar sessions, Terminal/Editor/
Information launch, input, Exit and same-ID restart. Inspect the second session's
actual Information screenshot; serial directory suffix is `ehnua9d_`. V2 has its
own separate successful functional evidence and is not the shipped variant.

Freeze the final source/ELFs, use byte-identical ordinary rootfs and fixtures,
normal devices and SMP4. Run12 ABBA boots and12 independent BAAB boots with no
own build, hash job or second VM running concurrently. Both comparisons pass
all30 expected metrics, six frame controls, the474-entry directory checksum
`18414638227522594487`, freshness and full startup/cleanup markers. Median
elapsed changes, negative is faster:

| Workload | ABBA | Independent BAAB |
| --- | ---: | ---: |
| alpha-lifecycle-busybox-8 | -10.68% | +10.95% |
| alpha-lifecycle-busybox-8-prepare | -3.58% | +11.27% |
| alpha-lifecycle-busybox-8-source-release | -2.10% | +14.17% |
| alpha-lifecycle-xfe-3 | -9.98% | +3.32% |
| alpha-lifecycle-xfe-3-prepare | -4.37% | +5.81% |
| alpha-lifecycle-xfe-3-source-release | -4.05% | +12.30% |
| alpha-reap | -2.42% | +12.62% |
| alpha-spawn | -7.34% | +2.91% |
| alpha-spawn-reap-8 | -9.94% | +3.28% |
| alpha-xfe-reap | -22.86% | +15.43% |
| alpha-xfe-spawn | -6.66% | +5.61% |
| busybox-ls-8 | -0.06% | +0.90% |
| copyout-128-vmas | +1.00% | +3.36% |
| copyout-256-vmas | -2.24% | -0.60% |
| copyout-normal | +4.62% | +3.80% |
| directory-large-64 | +2.93% | -5.08% |
| directory-small-64 | -1.90% | -3.97% |
| fork-128-vmas-8 | -8.82% | -12.26% |
| fork-256-vmas-8 | -3.47% | -4.94% |
| fork-8 | -6.20% | -12.05% |
| fork-exec-8 | -8.35% | -0.69% |
| getpid-2048 | -0.29% | +3.87% |
| getuid-2048 | +1.74% | +3.59% |
| missing-stat-512 | +4.88% | -1.32% |
| negoto-create | -2.99% | +2.09% |
| startup | -3.22% | +2.89% |
| stat-512 | +1.78% | +0.92% |
| unix-2048 | -1.58% | -1.17% |
| unix-recvmsg-2048 | +1.74% | +0.39% |
| unix-recvmsg-eof-2048 | +5.83% | +1.04% |

Frame-control deltas span-0.98%..+1.22% initially and-0.84%..-0.16% on repeat;
their complete results remain in the JSON evidence. Ordinary fork and fragmented
128-VMA fork repeat a5% improvement, but startup, Negoto and lifecycle gains do
not. Copyout-normal is slower in both comparisons. Do not erase these results,
claim a uniform system speedup, or attribute every delta to a specific cache,
IPC or IPI path. Retain this as a correctness foundation with workload-specific
fork gains; the broader optimization goal is not complete.

Evidence is `/private/tmp/nanami-opt-cycle95.puhCod`: before/V2/final source and
ELF snapshots, baseline regression and final functional logs, both complete
ordinary result JSON files and24 raw trial logs. The final ordinary image is
`after.img` SHA256`22c9066773a050e79ed1bd43dd1dc77008c7b21c6c587df4e18a218cda401ce1`,
paired with rootfs`6013d95fde007564385d5ee2dfd17c364777647d8b82f5251b1dad9fdc8b0b5a`.
Alpha is`df45dc2e0a57003fabfff8d918d9fc17164e820dca6974a31a92a2fb41103568`,
init`284b5c7730f21a902c92b7548c2779813d7a8bcc57bb6523456975a0755c649d`.
Alter remains`fb20fa00` and Kernel remains`312d579b` with a clean A9N source
worktree. The unmeasured V2 ordinary image was removed to make build space only
after its sources/ELFs and separate functional inputs were preserved. User input
images/rootfs, accepted images, timed inputs and logs were not removed.

Next implement actual private sharing rather than another context-copy or
priority micro-experiment. Before physical retargeting, prove whole-VM
quiescence without replaying accepted parked syscalls; include CLONE_VM peers
with different TGIDs. Intercept every privileged write, CAS and writable
protection change and invalidate stale Alpha copy-window aliases for all VM
peers, not just the faulting PID. The fault payload already contains address
and architecture code; Linux currently ignores the latter and overloads
`FaultEvent.kernel_call_number` with the former for non-syscall faults. No
Kernel ABI addition follows merely from needing those fields. Kernel and IPC
remain unchanged; this is a next-step audit, not enabled COW or an ARM TLB proof.

## Cycle 96: sealed read-only fork sharing and VM quiescence

Start from accepted cycle95 V3. Linux execution roots stay empty; Alpha retains
management capabilities. Add an explicit PROCESS_FORK option for private R/RX/X
sharing, not an implicit change to legacy Native/FreeBSD forks. Production
non-x86 backends retain eager copies. RW and retained PROT_NONE pages still copy.

Alpha shares only tracked, owned one-page RAM with known-private references,
matching child Frame metadata and no explicit shared-buffer reservation.
A separate BTreeSet records private aliases without changing permission bits,
descriptors, object identity or Kernel structures. Resolve ownership once per
request, with a sorted VA snapshot, rather than a full allocation-list walk per
page. Replacement commits swap active/deferred records in place without a
reference-count gap. Recycle retired capability slots, not still-live VAs.

The supervisor must quiesce all source-VM users before opt-in fork, and all
target-VM users before DETACH. A read-only PTE alone does not prove a running
peer has discarded an earlier writable TLB entry. The x86 path uses existing
synchronous Suspend/ACK and the existing context-switch/CR3 contract; no new
Kernel shootdown, API or IPC-fastpath branch is added. Already delivered faults,
handled parked waits, stopped tasks and prepared children must not be stopped
and resumed again. Mark constructor-time children prepared until their intended
resume; clear the conservative sharing flag after successful EXEC_MEMORY.

QUERY is side-effect free and allocation-free. DETACH preserves contents and
R/W/X; the final owner only drops its marker. Normal checked copyout adds no
prepare RPC. Cold privileged writes, write upgrades and existing-destination
copies prepare explicitly. Alpha guards proxy writes, CAS, W grants and writable
exports so a forgotten prepare cannot create a writable shared alias.
There is no overloaded error/retry convention or automatic copy replay.
Failed retargeting retains both backings and the marker; this is not a promise
of whole-VM retryability after arbitrary partial Kernel teardown.

Invalidate the existing owner-scoped copy cache before retargeting. Its lookup
and invalidation already canonicalize CLONE_VM peers to their memory owner;
do not add the all-peer cache index proposed by the older audit note.
Detach reads through the permanent physical-pool Frame, not a child capability
slot that it then deletes. Existing pool-cache physical identity remains valid.

A real syscall-free CLONE_VM regression found an independent scheduling bug:
threads were created at priority20 while initial tasks and ordinary fork used4.
On SMP1 the high-priority child starved its parent, so no parent-side detach
could even start. Use the named LINUX_TASK_PRIORITY=4 for both clone paths.
The same unbounded-spin fixture fails before this correction and passes after
it with the same Kernel. Do not attribute that starvation to an IPC deadlock,
IPI cost or a new Kernel regression.

Validation:

- Final native release suite:1255 passed,17 ignored.
- Final core/quiescence ASan:145 passed,2 ignored. ARM Alpha/Alter checks and
  all3 runner predicate tests pass. C fixtures are integer-only; FPU is unchanged.
- Production sharing/detach host tests use real VM/allocation indices and host
  pages actually aliased by modeled capabilities/PTEs. Cover byte isolation,
  descendants, unique-owner detach, PROT_NONE, stable indexed commits, recycled
  slots, external aliases, invalid callers and map/cap-removal/restore failures.
- Guard tests include different-TGID VM peers, current faults, parked waits,
  prepared/stopped tasks, partial Suspend failure, failed detach/operation,
  failed Resume cleanup and a fork barrier even before any sharing marker.
- Final real SMP1/SMP4 C runs pass fork write upgrades, descendants, retained
  PROT_NONE, parked read preservation and a syscall-free running VM peer.
  Capability sealing remains intact through ordinary fork, shared VM and exec.
- Final native SMP1/SMP4 RAM probe repeats three measured pairs after warmup:
  eager4775936 bytes versus shared581632, saving exactly4194304 bytes each time.
  It also rejects undetached proxy writes/W grants, isolates a detached write,
  reaps the source root first, reads the surviving child and exercises last-owner
  detach. Raw serial footers explicitly contain source-reap=true.
- Final GUI passes two complete sessions in one boot, Terminal/Editor/Information
  launch, real input, Exit/restart and second-session Information commands.
  Visually inspected the Information window; final serial suffix tgabkg7_.

Retain untimed failed evidence too. An initial no-NIC run timed out before the
fixture launched. One V1 GUI attempt stopped in Kernel initialization before
Alpha; its Kernel bytes are unchanged, but this alone does not identify a cause.
A V3 RAM run used a changed success-footer predicate against an older fixture:
the guest completed its old footer but the harness waited for the new field.
That invalidated trial is not performance or source-reap evidence. The corrected
runner waits for a complete line and final tests explicitly require the new
source-reap footer. These failures are separate from the successful matched
24 normal-device timed boots; do not erase or count them as passes.

Both independent12-boot comparisons pass all30 metrics and six frame controls,
with zero parsed errors, exact474-entry directory FNV18414638227522594487,
freshness, complete metric footers, startup and cleanup. No own build/test/hash
job or other VM overlapped timed workloads. Ordinary timing fixtures and user
input rootfs were not replaced by the new functional-only probes. Negative
elapsed deltas mean faster:

| Workload | ABBA | Independent BAAB |
| --- | ---: | ---: |
| alpha-lifecycle-busybox-8 | +2.14% | +1.18% |
| alpha-lifecycle-busybox-8-prepare | +6.92% | -0.80% |
| alpha-lifecycle-busybox-8-source-release | -5.61% | +20.72% |
| alpha-lifecycle-xfe-3 | -3.75% | -1.52% |
| alpha-lifecycle-xfe-3-prepare | -7.45% | -2.01% |
| alpha-lifecycle-xfe-3-source-release | +2.27% | +14.41% |
| alpha-reap | +5.92% | -5.22% |
| alpha-spawn | -1.22% | -8.56% |
| alpha-spawn-reap-8 | +5.73% | -10.89% |
| alpha-xfe-reap | -2.81% | -4.64% |
| alpha-xfe-spawn | -3.38% | -4.11% |
| busybox-ls-8 | +0.96% | -0.12% |
| copyout-128-vmas | +1.64% | +1.51% |
| copyout-256-vmas | -0.07% | -0.84% |
| copyout-normal | +2.09% | +0.43% |
| directory-large-64 | +0.46% | -0.18% |
| directory-small-64 | -1.30% | +1.08% |
| fork-128-vmas-8 | -10.42% | -17.00% |
| fork-256-vmas-8 | -7.57% | -13.98% |
| fork-8 | -5.16% | -13.03% |
| fork-exec-8 | -0.52% | -0.17% |
| getpid-2048 | -0.91% | +0.06% |
| getuid-2048 | -2.07% | -1.86% |
| missing-stat-512 | +0.93% | -0.59% |
| negoto-create | -9.12% | +7.73% |
| startup | -6.58% | -4.35% |
| stat-512 | +0.50% | -1.27% |
| unix-2048 | -3.61% | +0.32% |
| unix-recvmsg-2048 | +0.76% | +0.81% |
| unix-recvmsg-eof-2048 | +1.51% | +0.79% |

Frame controls span-0.12%..+0.81% and-0.40%..+0.51%. Fork and fragmented forks
repeat at least5%; startup improves6.58% /4.35%, not a repeated5% win.
Negoto-create changes-9.12% /+7.73% and does not reproduce its improvement.
Copyout-normal is+2.09% /+0.43%, copyout128 is+1.64% /+1.51%, and several
source-release controls regress. Preserve all results. Retain actual RO sharing,
isolation and the priority correction, with workload-specific gains; do not
claim a uniform5% system win or10--100x application speedup. Writable-page,
fault-driven COW is not enabled and the broader optimization goal remains active.

Evidence: /private/tmp/nanami-opt-cycle96.UN682G, including before/after images,
both complete result JSON files,24 raw timing trial logs, failed runs and final/
source/ELF/functional snapshots. Before is accepted95 image22c90667.
Final ordinary image SHA256:
7d78da23863e4fb906fad0008334fdbb9e40f097314a4f180253b84b903ff727.
Paired rootfs45259636401fd409f5c139ca9c8b6b898d4b210ed4d7b7537303a9aefd872c66.
Alpha8ac25372eafe5383a4f4dccde829a177e5fadc8b4c9ccee569c47241cf68d759,
init6990331c0a77e215349cc4d2b83110135fd5fae084dfa4f0c7da1cd3781e3526,
Alterb320a11cde5065b28e65052d739bdbdaa95be58592c354e35c0f8d6734a80f5e.
Final functional image74573557f675105a93890bcb932d27eed09ac45f43715a3f874a4854c6a87dea;
its native probe isf8ffe6a4caacc80aa413af367ab83dea453ab82b2d77325b10c07fe7d88b25c7.
Kernel312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88
and the A9N source worktree are unchanged. Original user rootfs remainsa6c472f8.

Disk cleanup removed only the verified unused native debug cache, this cycle's
accidental default-target check cache and one unmeasured regenerable scratch
image before rebuilding it with block-differential rootfs updates. Preserve
accepted/timed inputs, user rootfs, source/ELF snapshots and raw logs. No version
bump, commit or release was requested or performed in this cycle.

## Cycle 97: sealed exec retirement candidate — paused by user

The user requested a temporary stop during the first timing comparison on
2026-11-03. Stop the runner with SIGINT; its guest harness executes cleanup.
The unified command terminates with130, the interrupted guest log records
KeyboardInterrupt, and lsof finds no remaining users of this cycle's before,
after or final functional disk. No VM or benchmark remains running.

The working tree retains the candidate, not an accepted performance change.
Accepted96 remains the baseline. No version bump, commit, release, Kernel
change or broader-goal completion occurs. Do not resume automatically.

Candidate changes:

- For a sealed task with exclusive VM ownership, unmap present leaves before
  bulk-clearing each Frame-chunk Node's contents. Retain the Nodes/arenas for
  reuse; never rewind generic allocation or delete a live peer's Frame alias.
- Move the complete active RAM chain onto its existing deferred suffix without
  decrementing/rebuilding references. Free RAM only after all old leaves and
  local capabilities are gone; commit successful releases and retain failures.
- Preserve pre-installation validation errors. Treat errors after destructive
  sealed exec installation as fatal; Alter must not resume the old image.
  Ambiguous transport/protocol failures of the exec RPC also terminate the task.
- Native/unsealed exec and the Kernel/IPC fastpath remain unchanged.

Validation before pausing:

- Final native suite1264 passed,17 ignored; ASan core/exec/quiescence153 passed,
  two ignored. Alpha/Alter ARM checks and three runner tests pass.
- Real SMP1/SMP4 Linux fork/isolation and native-capability denial tests pass.
  The exec child first receives ENOENT from a missing executable, then succeeds
  at self-exec while the parent's two-MiB read-only mapping stays intact.
- Native RO probes pass three pairs per SMP mode, each saving4194304 bytes,
  preserving write guards/isolation and a child surviving source-root reap.
- Two full Tiny Core GUI sessions pass Terminal/Editor/Information, real input,
  launchers, Exit and restart cleanup. Logs: nanami-negoto-qemu-53j77nrw.
- First untimed attempt omitted nanami-fork-readonly from the functional image;
  the CLI reported spawn failure before guest launch. Preserve that failed
  image/log. Corrected final image includes dumped-and-compared test binaries.

Timing is incomplete: only before1 process, UNIX and startup boots completed.
The user interrupted after1 process; no completed after samples, second order,
aggregate comparison, speedup or five-percent claim is valid. Resume later in
a fresh trial directory with matched complete ABBA/BAAB sequences; preserve the
partial trials rather than overwriting or silently counting them as successes.

Evidence directory: /private/tmp/nanami-opt-cycle97.dUst2F. before/ contains the
pre-candidate sources; final/source/ and final ELF snapshots contain the tested
candidate. Reuse existing build recipes; disk updates were block-differential
APFS clones, not a rewrite of the original user rootfs.

Candidate ordinary image09f3b7e554fe1982dc39db54a3c813cac34b75dd940f7a09fbf2a11bc31de069,
paired rootfsb9683b457fa30d6fa5d1a5a64eb10c7b1c37b26f339ebbedca708af81eb99bd2.
Alpha575b058a566e6bd63a85ac6939d5dc98d5e46c1c40b420750547e5098da9f2eb,
init00542ffee073c5043eec61adcdf556d6e64411064f658b058847ceb3d02f2852,
Alter5a9a57ce94ed9c9353e83d698f0291fe25bc163e2e26571c7ba5c6bd13500cee.
Final functional imagefd3a03e26a00f449a3db92d08deeb364b566681af1874728b694182ac4cf512b.
Kernel remains312d579bd1098ca2a5bf1186a92e03524e39add7d50cc5e896d7da05d43d0c88.




















