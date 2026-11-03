# Tiny Core desktop performance (2026-10-30)

## Follow-up: grouped permission metadata rejected (2026-11-03)

Eight independent permission Words per tree entry reduce dense host metadata
44.88% but grow sparse layouts4.42x. Twelve normal four-CPU comparison boots
all pass: startup -1.93%, stock ls +0.58%, fork-exec +1.29%, Negoto +9.56%,
ordinary fork -8.34%. Preserve both directions; do not equate a dense allocation
or favorable sub-workload with a whole-system5% improvement. Restore exact
production source/ELFs, retaining only three generic regressions and an ignored
footprint probe. Native1199/ASan124, ARM checking and three restored startup/
cleanup/reuse sessions pass. Kernel/authority/default runtime stay unchanged.
[Cycle77](optimization-cycles.md#cycle-77-grouped-permission-metadata-rejected)
preserves all raw samples, sparse tradeoffs, parser corrections and restoration.

## Follow-up: fork/exec stage diagnosis (2026-11-03)

Fifteen temporary stages distinguish Alpha construction/VM copy from descriptor
and metadata work:98.74% versus0.57% FD inheritance within captured fork spans;
exec VM replacement is65.16%, argv/env capture3.34%. These are partial global
exit-flush elapsed intervals, not CPU shares or whole-startup percentages.
A second diagnostic replays the same217752 initialization-cache accesses;
same-capacity4-way removes5.13% of misses, not5% of application time, while a
larger cache consumes fixed tracker headroom. Do not adopt either speculative
path without evidence. Installed rootfs identity and complete padded-label
coverage are verified. Restore exact sources/ELFs, pass native1196/ASan113,
ARM checking and three normal startup/cleanup/reuse sessions. No Kernel,
authority or runtime change is retained; optional safe readonly sharing awaits
the user's capability-policy decision.
[Cycle76](optimization-cycles.md#cycle-76-separate-forkexec-stages-and-initialization-cache-work)
keeps all counts, artifact checks, limitations and restoration evidence.

## Follow-up: name-cache index and combined hint rejected (2026-11-03)

Two separate normal twelve-boot comparisons test a same128-slot FIFO cache
with a512-byte hash index, then that index plus the final error-safe directory
hint. Stock ls moves -3.70%/-2.56%, startup -2.07%/+3.23%; all adverse process
controls remain explicit. Reject both, restore exact accepted runtime/tests,
and pass native1196/ASan62, ARM checking and three actual X11-ready startup/
cleanup/reuse sessions. An IPC audit confirms that ordinary register-sized
calls and RAX-only Linux replies are already in place; it is not a new speedup.
Remove only closed duplicate trial disks and unused generated probe-build
caches, keeping accepted images, rootfs, sources and evidence.
[Cycle75](optimization-cycles.md#cycle-75-indexed-name-cache-and-combined-lookup-hint-rejected)
preserves both full tables and the restoration/cleanup evidence.

## Follow-up: lookup hint rejected (2026-11-03)

A global last-successful-block hint removes about88% of actual ls directory
record visits, yet normal stock ls improves only2.55%, startup0.86%, and several
process controls regress. Reject it after12 paired normal boots. Remove all
probes and restore exact code/tests; native1196/ASan62, ARM checking and three
restored startup/reuse sessions pass. Name-cache slot scans remain about
3.3million per ls8; assess that new workload without assuming it is CPU time
or repeating the older stat/startup-only index experiment unchanged.
[Cycle74](optimization-cycles.md#cycle-74-last-block-directory-search-rejected)
keeps all normal timings, contrary counts, tests and restoration evidence.

## Follow-up: configurable metadata locality (2026-11-03)

Keep VFS near POSIX/Alter using the existing x86_64 priority22 CPU2 preference,
without changing executable bytes, priorities, authority, service boundaries
or Kernel IPC. Twelve paired normal boots reduce stock ls7.62%, existing
stat16.54%, missing stat22.56% and small/large enumeration20.59%/23.14%.
Startup is only2.99% shorter. Native cross-core bulk reads and several process
controls regress; keep those results and a separate four-boot repeat explicit.
Native1196/ASan62, two real GUI sessions with Information after restart, and
one-CPU fallback pass. No single configuration is claimed fastest universally.
[Cycle73](optimization-cycles.md#cycle-73-retain-configurable-metadata-broker-locality)
preserves all raw samples, contrary controls and artifact identity.

## Follow-up: count reduction is not a speedup (2026-11-03)

A block-start checkpoint fits existing padding and reduces actual small-buffer
directory cached loads17280->4224, with identical request counts/results and
every other diagnostic field unchanged. Yet normal small/large enumeration
moves+4.27%/+8.35%; reject it. All12 comparison boots pass. Restore exact runtime,
retaining3general regressions; native1196/ASan53 and three restoration sessions
pass. Review actual routing/fastpath eligibility before another local-work guess.
[Cycle72](optimization-cycles.md#cycle-72-block-local-directory-restart-rejected-despite-fewer-reads)
preserves both the strong work-count result and contrary timing evidence.

## Follow-up: separate hot-stat and real-ls work (2026-11-03)

Existing sync requests delimit 17 diagnostic counter windows without a new
protocol. Warm stat512 copies no filesystem block and makes no device read;
warm stock ls8 still examines904k lookup records. Small-buffer64 scans load
17280 cached blocks versus1216 for the same large-buffer enumeration. Source
review finds that the required read-ahead rollback misses Ext2's exact-cursor
reuse condition; preserve that rollback and investigate safe per-block resume.
Investigate actual ls name-search repetition separately, not as a promised
whole-system gain. Every temporary probe is removed byte-for-byte; native1193,
ASan41 and three normal startup/cleanup/reuse sessions pass. No Kernel change.
[Cycle 71](optimization-cycles.md#cycle-71-phase-separated-filesystem-work-diagnostic)
preserves all counts, source-based inferences, limits and the next candidates.

## Follow-up: reject borrowed cached-block inspection (2026-11-03)

An allocation-free candidate inspects cached Ext2 inode/directory bytes through
scoped immutable callbacks instead of staging a full block on every read.
All 12 normal comparisons pass, but stock BusyBox ls gains only 2.20% while
large enumeration and startup move +5.90%/+4.27%. Preserve stat -6.42% too;
that favorable row alone does not justify adoption. Restore all candidate
production/test files exactly; native 1,193 / ASan 41 and three normal startup/
cleanup/reuse sessions pass after restoration.
No new runtime change, cache, protocol or Kernel change remains.
[Cycle 70](optimization-cycles.md#cycle-70-borrowed-cached-block-inspection-rejected)
records all source proofs, tests, samples and adverse controls.

## Follow-up: reject readdir cache prefill (2026-11-03)

Normal reversed-order runs show that populating the existing cache on every
returned directory entry slows small/large enumeration 19.52%/74.13%, despite
a 2.77% stock BusyBox ls gain. Reject this policy and restore exact accepted
runtime bytes, preserving cycle 68's large-buffer batching. Retain only a
baseline-compatible unlink/recreate/rename cache-freshness regression.
All 12 comparison boots, restored native 1,193 / ASan 14 and three normal
startup/cleanup/reuse sessions pass. The first profile exit window contains
earlier enumeration work; only the later 23 windows establish 472 lstat and
six getdents calls per ls. Counter spans are not exclusive CPU measurements.
[Cycle 69](optimization-cycles.md#cycle-69-readdir-driven-dentry-prefill-rejected)
preserves all samples, negative results, hashes and restoration evidence.

## Follow-up: directory enumeration batches (2026-11-03)

Alter's real getdents64 path previously returned at most 512 bytes and read at
most 16 backing records, even for a 32-KiB guest buffer. Pack forward within
the existing shared buffer instead: Linux records are at most 280 bytes versus
288-byte VFS records, and the 512-byte initial gap protects the current/unread
source. Compile-time assertions enforce those bounds. Larger batches reserve
worst-case output space; small-buffer batching, cookies, root shadowing and
copyout-failure rollback remain intact. No new allocation, cache or protocol.

Normal reversed-order comparisons enumerate the same 474 /usr/local/lib
entries and checksum. Sixty-four large-buffer scans fall 552.585 -> 105.824ms
(-80.85%); getdents calls fall 2,560 -> 384. Small-buffer scans are +1.66%,
unmodified Tiny Core BusyBox ls -4.01%, startup -0.60%. Only the large-buffer
enumeration target establishes a 5% gain. Preserve all process/native controls
and raw samples rather than calling this a whole-system 81% improvement.
Native 1,193 / 16 ignored, ASan 14, ARM checking, 12 comparison boots and two
actual GUI/restart sessions pass; Kernel and Alpha are unchanged.
[Cycle 68](optimization-cycles.md#cycle-68-capacity-aware-linux-directory-enumeration)
records the source proof, new frozen fixture, hashes and complete results.

## Follow-up: directional UNIX readiness (2026-11-03)

Retain separate internal data/capacity notifications and the existing general
UNIX state-change notification. Poll/select interests and armed epoll watches
subscribe to their requested direction plus general changes; zero-event waits
still receive HUP/error/invalid-FD state changes. Receive-side capacity release
is never suppressed just because POLLOUT was already set. Sending into an
already-readable packet queue needs no extra read notification.

Matched, same-Kernel/Alpha startup diagnostics observe scans 4,409 -> 2,449,
unready scans 3,600 -> 1,632 and ready scans 809 -> 817. This is not a 44% CPU
or startup gain. Normal reversed-order startup changes -1.97%; socket/process
controls vary and all samples, including slow ones, are retained. The 5% speed
target remains unproven. Native 1,188 tests, 86 ASan tests, ARM checking,
24 comparison boots across both candidates and a real two-session GUI check
pass. Temporary instrumentation is removed and normal hashes restored.
[Cycle 66](optimization-cycles.md#cycle-66-directional-unix-readiness-work-reduction-retained)
preserves full results, the narrower acceptance rationale and disk cleanup.

## Follow-up: actual readiness wakeups (2026-11-02)

Two separate diagnostic runs each complete three actual X11-ready startup/
cleanup/reuse sessions. The first records 4,437 scans, all poll/pselect and no
parked read/write retries. A provider breakdown in the second records 2,709
UNIX-only wakeups of 4,056 total. UNIX-only interest sets account for 4,019
scans but only 602 ready results. This is not exclusive CPU or startup speed
attribution. Do not add a direct-I/O path that this workload does not exercise.
Instead audit redundant send notifications when the peer packet queue is
already readable; retain receive-side capacity/rights-release notifications.
Both temporary probes are removed byte-for-byte, normal ELFs/rootfs restored,
and 1,180 native tests, 97 ASan tests and three more normal startup/reuse
sessions pass. No production optimization or 5% gain is claimed in this
diagnostic cycle. [Cycle 65](optimization-cycles.md#cycle-65-readiness-wakeup-and-provider-diagnostic)
preserves counts, artifacts, applicability limits and the next candidate.

## Follow-up: readiness stop checks and state copying (2026-11-02)

An allocation-free candidate replaces repeated PID lookup with the already
borrowed task's stop flag and publishes only the PTY deadline before a readiness
scan. Optimized x86_64 code removes the lookup and a 616-byte copy. All semantic
checks pass, but this alone is not desktop speed evidence: a short awake ABBA
comparison gives UNIX write/read +3.75% and recvmsg -0.41%, with substantial
movement in unchanged controls. Reject the runtime candidate, preserve all
measurements and restore exact cycle62 binaries/rootfs. Retain four general
stop/continue/deadline tests; 1,180 native tests, 69 ASan tests and three restored
startup/reuse sessions pass. The earlier twelve-boot batch overlaps host sleep and
is functional evidence only. Details and the next unimplemented retry-path
lead are in [Cycle 64](optimization-cycles.md#cycle-64-borrowed-stop-checks-and-readiness-writeback-rejected).

## Follow-up: UNIX payload allocation experiment (2026-11-02)

A one-spare/4KiB-capacity candidate removes all256 payload allocations in a
native warm round-trip fixture without removing required ownership copies.
Do not count that as measured desktop speed. The first normal comparison
overlaps verified macOS sleep and is preserved but not used for speed claims.
Fresh awake ABBA UNIX runs give write/read -3.33% and recvmsg +0.09%, below a
convincing 5% gain; unchanged EOF also moves. Reject the runtime cache/budget,
restore exact cycle62 binaries/rootfs and retain two general semantic tests.
Native1,176/ASan28 and three restored startup/reuse sessions pass. All raw
samples, power-event evidence, candidate artifacts and limits are in
[Cycle 63](optimization-cycles.md#cycle-63-bounded-unix-payload-reuse-rejected).

## Follow-up: Alpha bootstrap mapping readiness (2026-11-02)

Retain eight completed table regions rather than only the last in Alpha's
allocation-free bootstrap tracker (+64 bytes). Guest trackers are unchanged.
A separate diagnostic predicts many fewer repeated depth checks; normal
benchmarks, not that model, justify adoption for native suspended construction/
reap: 27.391 -> 25.207ms (-7.97%), with all three candidate boot medians below
all baseline medians. Full BusyBox/Xfe input/construction/cleanup also improve
in aggregate, but those workloads never execute the Linux ELFs. No desktop
startup gain is established. Ordinary fork +4.25%, UNIX write/read +4.20% and
one slow recvmsg boot are retained, not hidden behind the favorable results.
Eighteen comparison boots, native1,174/ASan55, ARM compile, two-session real GUI
and one-core process checks pass. Kernel is untouched. Full data and scope are
in [Cycle 62](optimization-cycles.md#cycle-62-bounded-bootstrap-page-table-readiness-history).

## Follow-up: file-mmap work attribution (2026-11-02)

A temporary opt-in probe covers three complete startup/cleanup/reuse sessions.
Snapshot hits serve 1,624 of 1,783 file-source requests; 135 more load a whole-file
snapshot, and only 24 bypass for files above the 2-MiB ceiling. Streaming copies
just 4,944 bytes through the partial-page tail. This does not justify a region
cache or unsafe tail storage. Most measured initialization elapsed ticks belong
to snapshot-backed requests, including service waits: not exclusive CPU or a
normal-build speedup. All probes are removed; exact normal executable/rootfs/
image hashes, native 1,170 / ASan 33 and three further startup/reuse sessions
pass. Full counts and attribution boundaries are in
[Cycle 61](optimization-cycles.md#cycle-61-file-mmap-source-and-initialization-diagnostic).

## Follow-up: namespace reply placement (2026-11-02)

Receiving at SHM offset zero and viewing the metadata/path prefix in place
removes two real memmove sites without a new RPC or Kernel change. But twelve
balanced normal QEMU boots give stat -0.74%, missing stat +0.63%, startup +0.28%
and BusyBox input/construction/cleanup +4.96%. Reject the runtime change rather
than count copy elimination as a verified startup gain. Restore exact normal
ELFs/rootfs and retain three independent namespace-wire regressions; native
1,170 / ASan 15 and three startup/reuse sessions pass. The rootfs comparison
checks all 3,890 paths, not merely newly built binary names.
[Cycle 60](optimization-cycles.md#cycle-60-direct-namespace-reply-placement-rejected)
preserves full measurements, codegen, disk recovery and the next mmap-work lead.

## Follow-up: request-scoped inode-kind reuse (2026-11-02)

Two read-only ext2 walk candidates reduce repeated inode checks without
cross-request state. The seeded version keeps the fresh root guard and reduces
guard-plus-three-component work from seven reads to four, with 16 stack bytes.
Native 1,173 / ASan 54 / ARM compile pass, but balanced normal QEMU gives only
stat -2.18%, missing stat -0.23% and startup +0.02%. Reject both candidates,
restore exact cycle57 runtime and validate three more startup/reuse sessions.
No new adapter/cache/test target or Kernel change remains. All observations,
including shifted UNIX controls, and the next result-buffer copy lead are in
[Cycle 59](optimization-cycles.md#cycle-59-request-scoped-inode-kind-reuse-rejected).

## Follow-up: bounded namespace-root hints (2026-11-02)

A four-entry ext2 root-binding cache passes mutation/SHM/error regressions and
hits 72.17% of a separately instrumented startup prefix. Balanced normal QEMU
comparisons still give only stat -3.31%, missing stat -2.50% and startup +1.06%
(within polling/session variation). Reject it: no persistent root hints or
extra invalidation work is retained. Exact cycle57 runtime is restored, with
1,167 native/48 focused ASan tests and three startup/cleanup/reuse sessions
passing. Kernel is unchanged. Full candidate, all samples and the next
request-scoped inode-reuse lead are recorded in
[Cycle 58](optimization-cycles.md#cycle-58-bounded-namespace-root-binding-hints-rejected).

## Follow-up: configurable compatibility-broker locality (2026-11-02)

Keep Alter's existing CPU and group priority21 compatibility/session brokers
through `@placement 21 2`. No built-in image names, extra RPC or Kernel change.
Exact affinity wins; unavailable CPUs preserve automatic placement, including
the normal round-robin sequence. Leave VFS near AHCI: grouping priority22 as
well regresses large-ELF input and is rejected.

The final changed/normal/normal/changed QEMU trials give stat -12.89%, missing
stat -15.04% and startup 5201.9090 -> 4991.4025ms (-4.05%). Getpid/getuid are
+0.47%/+0.26%; Xfe input/construction/reap is -1.12%. All samples, contrary
substages and the rejected wider configuration remain in
[Cycle 57](optimization-cycles.md#cycle-57-configurable-broker-locality-without-kernel-changes).
Native1167/ASan149, ARM compile, one-CPU process and two-session GUI checks
pass. This is not a whole-system/hardware 5% claim; the goal remains active.

## Follow-up: broker and guest placement tradeoffs (2026-11-02)

An Alpha-only placement probe keeps Linux guests, Honoka and drivers on their
original CPUs while co-locating synchronous brokers. Balanced QEMU trials give
startup 5132.5355 -> 4393.7045ms (-14.40%), stat -37.32% and recvmsg -32.81%,
without any Kernel change. But separate controls show getpid/getuid about
2.8 times slower after separating the main guest from Alter. Do not adopt this
configuration as an overall win or retain its temporary role-selection rules.
Next isolate guest/supervisor placement, keeping fork/thread parallelism under
test. [Cycle 56](optimization-cycles.md#cycle-56-isolate-ipc-locality-without-changing-kernel)
records favorable and contrary observations and the unchanged placement controls.

## Follow-up: measured stat stages and owner lookup (2026-11-02)

Separate local-counter probes show where stat waits and checked-copy work occur;
the actor intervals include nested waits and must not be summed as CPU costs.
Generated-code review already finds no whole ProcessEntry copy in the relevant
hot paths. A smaller borrowed-owner candidate gives stat -0.21%, missing stat
+2.84%, ordinary fork -10.20% and actual startup +2.10% (one polling step).
Reject it, retain one semantic regression and restore exact normal runtime.
Native 1162 and focused ASan 140 pass. The existing Kernel direct Reply-Receive
branch requires same-core affinity, unlike the currently spread Alpha/Alter/
POSIX/ext2 placement; investigate that policy without changing Kernel.
[Cycle 55](optimization-cycles.md#cycle-55-stat-latency-attribution-and-borrowed-owner-lookup-rejected)
keeps complete measurements, codegen corrections and restoration checks.

## Follow-up: name-cache index evaluation (2026-11-02)

Real ext2 startup lookup counts average 46.74 visited cache slots at a 96.08%
hit rate. A 512-byte fixed-slot index preserves the existing 128-entry FIFO
cache, including negative entries and invalidation, but matched actual timings
are mixed: ordinary stat -1.82%, missing stat +2.30%, startup +0.03%. Reject the
runtime/test-module candidate and restore exact normal binaries. All twelve
comparison boots and three further restored startup/reuse sessions pass.
[Cycle 54](optimization-cycles.md#cycle-54-fixed-slot-indexed-dentry-lookup-rejected)
records methods, all observations, oracle/collision tests and disk recovery.

## Follow-up: path-walk allocation experiment (2026-11-02)

Ext2 request-local scratch reuse and exact output reservation reduce a fixed
deep-path host probe from 151 to 80 allocation calls. They do not improve real
Tiny Core stat/startup: matched normal-image medians are +3.22% / -0.05%.
Reject the runtime candidate, preserve its evidence and retain only namespace
semantic tests. The restored normal image completes three additional startup,
cleanup and same-ID reuse sessions; Kernel remains unchanged.
[Cycle 53](optimization-cycles.md#cycle-53-request-local-path-walk-buffer-reuse-rejected)
includes all host observations, contrary controls and full comparison results.

## Follow-up: isolated fork controls (2026-11-02)

Keep Kernel unchanged, as explicitly requested. Four diagnostic boots hold Alter
fixed while pinning old/new Alpha, then four normal boots hold Alpha and other
services fixed while replacing only Alter. Ordinary fork does not consistently
reproduce the earlier penalty; normal Alter-only results are -1.58% for fork
and +0.37% for fork/exec, with mixed fragmented-fork controls. No new runtime
optimization or overall 5% claim follows. Original observations remain recorded,
and all temporary probes/build overrides are removed. Full methods, raw sample
references and restoration checks are in
[Cycle 52](optimization-cycles.md#cycle-52-isolate-fork-controls-without-kernel-changes).

## Follow-up: ordered checked-copy vectors (2026-11-02)

The recvmsg output name length, control length and flags now use one ordered
Alpha request instead of two copy requests with a combined tail/fallback. Each
field still has whole-range permission preflight, input fields/padding are
untouched, and a failed later field keeps prior output but not queue consumption
or newly installed rights. The additive generic protocol is documented in
[message.md](../../nanami/doc/message.md#161-alpha-vm操作); it is not a Kernel API.

Four balanced normal-image trials give write/recvmsg 836.7795 -> 739.9225ms
(-11.57%) and EOF recvmsg 524.1200 -> 425.1995ms (-18.87%), six samples per side.
Plain read/write is +0.26%; startup is +0.34%, below the readiness polling step.
Process controls are also measured and extended in reverse order because
ordinary fork initially becomes slower. That issue remains open; do not call
this a whole-system 5% gain or assert fork equivalence from GUI success.
[Cycle 51](optimization-cycles.md#cycle-51-ordered-checked-memory-transfer-vectors)
retains all raw initial/extension samples, constructor controls and GUI evidence.

## Follow-up: UNIX dispatch ordering evaluation (2026-11-02)

Cycle 49 tests a user-level-only decoder that avoids consulting FD state for
unrelated syscalls. Four matched normal-image trials in both orders show
getpid/getuid about 1% slower, socket loops essentially unchanged, and startup
4990.2905 -> 4988.7025ms (-0.03%). Fragmented-fork medians are about 5% higher,
while unchanged native-constructor controls also vary. A lower operation count
is not established whole-workload improvement. Reject the candidate and restore
cycle48's exact Linux/FreeBSD binaries; preserve general routing/FD-reuse tests.
See [cycle 49](optimization-cycles.md#cycle-49-classify-unix-requests-before-descriptor-lookup-rejected)
for all scoped results and restoration evidence. No Kernel or IPC changes.

## Follow-up: request-local UNIX I/O descriptions (2026-11-02)

Cycle 48 decodes the bounded iovec/header description directly in its only
consumer, avoiding a large by-value Result and its two generated copies.
The iovec limit, input snapshot/error order, packet commit and ancillary-FD
semantics are unchanged. No Kernel, Alpha or IPC fastpath code changes.

Four sequential normal-image boots in before/after/after/before order use the
same cycle47 observer, Linux fixtures and Tiny Core overlay. Six observations
per side give plain UNIX read/write 376.375 -> 370.3735ms (-1.59%), write/recvmsg
840.7195 -> 844.9865ms (+0.51%), and EOF recvmsg 522.8855 -> 523.5575ms (+0.13%).
Startup is separately 5039.3785 -> 4984.5315ms (-1.09%), below the 100ms
readiness observation interval. These results do not establish a 5% speedup,
nor a clear recvmsg/startup improvement. Retain the smaller copy/storage path
as a cleanup; do not add cycle47's earlier gains to this comparison.
See [cycle 48](optimization-cycles.md#cycle-48-decode-unix-io-descriptions-in-place)
for raw samples, generated-code evidence and regression checks.

## Follow-up: UNIX recvmsg result copies (2026-11-02)

UNIX recvmsg writes its control length and flags together, without overwriting
input pointers or padding. A failed combined copy falls back to the original
whole-u64, then u32 stores, keeping partial-error and queued-FD behavior. No
Kernel, IPC fastpath, permission cache or service protocol is changed.

The expanded `desktop-performance.c` uses the same integer-only Linux ELF in
both images. It validates actual protected-page failures before timing three
samples each of send/recvmsg and EOF loops, alongside unchanged stat and plain
socket controls. The corrected runner waits for the report footer before
terminating QEMU. An earlier truncated report is retained but excluded in full,
not treated as zero/fast work. In the balanced, corrected-runner comparison,
send/recvmsg is 972.0285 -> 844.0535ms (-13.17%) and EOF receive is
647.4445 -> 522.881ms (-19.24%). Plain read/write changes -0.72%.

Whole desktop startup is measured separately: 4997.8545 -> 5038.069ms (+0.80%),
below the 100ms readiness-observation interval, not an established improvement.
Do not equate the fixed socket loops with desktop FPS or exclusive IPC cost.
Full samples, the incomplete-report diagnosis and final GUI acceptance are
recorded in [cycle 47](optimization-cycles.md#cycle-47-coalesce-adjacent-unix-recvmsg-result-fields).

## Follow-up: eliminate fork reload and private-mmap double fill (2026-10-31)

Alter fork now requests a suspended private VM snapshot from Alpha
(`PROCESS_FORK`, `0x102d`). It does not reopen an executable, restage an ELF,
parse program headers, build an initial stack or copy writable data over a
freshly loaded image. Alpha walks the current backed pages, initializes new
private RAM directly from those pages and preserves their permissions, including
retained PROT_NONE backing. Unpopulated holes remain holes. The existing raw-task
root/PCB construction is shared with THREAD_CREATE; that operation still aliases
the original VM. This is eager copying, not COW or an atomic stop of all siblings.
Alter retains FD/signal/VMA inheritance and reconnects MAP_SHARED aliases.

Private file mmap reserves virtual space without anonymous backing, then asks
Alpha to initialize each new page from caller-owned source bytes. Complete file
pages are copied once; only the suffix after initialized bytes is zeroed.
Validated cached file content is used directly, while uncached content streams
through the existing POSIX read window. Arbitrary short reads buffer only their
partial page. Old dirty MAP_FIXED backing is removed first, final permissions
are installed before returning, and failed construction releases its partial
extent. Newly allocated pages are registered immediately, removing both temporary
frame Vecs and avoiding orphaned suffix allocations on failure. Failed MAP leaves
its Frame tracked so cleanup must confirm UNMAP before reuse.

No POSIX service is bypassed, and no A9N source or IPC fastpath is changed.
The additive Nanami wire details are recorded in `nanami/doc/message.md`.
Host validation passed 1,010 tests (nine opt-in tests ignored); focused page,
stream and process tests passed 56 tests under AddressSanitizer (one ignored).
AArch64 Release checks passed for both Alpha and Alter/Linux.

Four-vCPU QEMU with normal input/network devices passed the expanded raw file
mapping fixture (cached and over-2-MiB uncached mmap, EOF tail, dirty MAP_FIXED
replacement, fork after unlinking its own executable, read-only and retained
PROT_NONE data, and private-write isolation). It also passed shared aliases,
CLONE_VM, namespace checks and two Tiny Core desktop sessions in one boot with
Terminal/Editor/Information launch, real input and Exit/restart cleanup. The
log is `/private/tmp/nanami-single-init-desktop4.log`.
The final build also passed one-vCPU Tiny Core init, stock dynamic BusyBox,
root/symlink/pipe and fork/exec checks, ps, interactive job control, Ctrl-C and
cleanup (`/private/tmp/nanami-single-init-console1.log`).

Four-vCPU QEMU/TCG, three-sample medians with the same timed guest fixture:

| Workload | Before this change | Final build |
| --- | ---: | ---: |
| Eight fork/wait | 100.569 ms | 81.526 ms |
| Eight fork/exec/wait | 2,472.460 ms | 2,338.233 ms |
| Eight native Alpha spawn/kill/reap | 222.869 ms | 208.142 ms |

Fork timings include teardown, and the native control also varies. These are
observed whole-workload comparisons, not exclusive measurements of copy cost
or a statistically established application-startup speedup. The final build
retains ordinary per-fork process logging (`image=fork-vm`); an intermediate
unlogged run is not used for this comparison. Logs are
`/private/tmp/nanami-vma-after2-qemu.log` and
`/private/tmp/nanami-single-init-final-perf4.log`. The final image is also built
at `thirdparty/tinycore/build-desktop-process/nanami-tinycore.img`.

## Follow-up: Alter-local VMA bookkeeping (2026-10-31)

The earlier service-path discussion is architectural, not evidence that IPC
transport is a bottleneck. Whole spawn/exec/I/O timings include handler work,
copying, allocation, Linux startup code and reclamation. Counting hops cannot
attribute that time to IPC, and does not justify bypassing POSIX as the next
performance change. This follow-up keeps all endpoints and IPC implementations
unchanged and examines Alter's own emulation work instead.

Every guest copyin/copyout checked permissions by scanning all 512 VMA slots,
including unused slots. VMA insertion, coverage, protection preflight and
fragment removal repeatedly scanned the same sparse table. The process/FD
lookup hint, small fault-context snapshot, trace-disabled gate and native
fault reply path were already present; they are not newly claimed optimizations.

MappingTable now exposes only a compact active prefix. Its maximum remains
512 VMAs, and the process snapshot remains one word. The active length is in
the shared allocation, not copied into each process: CLONE_VM aliases observe
splits, inserts and exec/reset immediately. Removal preserves remaining-entry
order. Capacity preflight remains separate from active length, and unused old
entries after reset are never consulted. There is no permission cache and no
skipping of Alpha's checked memory access.

Release-mode host benchmark of the production `memory_access_allowed`, with
only process lookup mocked, three samples of 500,000 successful checks:

| Active VMAs | Sparse 512-slot median (ms) | Compact median (ms) |
| --- | ---: | ---: |
| 8 | 114.082 | 3.485 |
| 32 | 114.363 | 17.420 |
| 128 | 122.299 | 44.910 |

This benchmark makes no IPC, syscall or device calls; it measures Alter-local
bookkeeping only. It is not an application-startup speedup. Reproduce with:

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --offline \
  --release --test linux-mappings \
  permission_check_cost_at_normal_and_fragmented_vma_counts \
  -- --ignored --nocapture --test-threads=1
```

The raw Linux process fixture additionally performs three samples of 2,048
getresuid calls, each validating three output values, with ordinary VMAs and
then 128 additional disjoint PROT_NONE reservations. The three checked Alpha
copyout requests per syscall and all IPC endpoints remain the same before and
after. This is a separate end-to-end check, not the no-IPC host benchmark.
The runner waits for the final captured workload marker, not just the summary
prefix, so it cannot terminate QEMU while the timing payload is still being
written to UART.

Four-vCPU QEMU/TCG, separate sequential boots with the identical guest fixture,
median of three samples per boot:

| Workload | Sparse table | Compact table |
| --- | ---: | ---: |
| 2,048 getresuid, ordinary VMAs | 527.871 ms | 506.978 ms |
| 2,048 getresuid, 128 extra VMAs | 521.068 ms | 512.618 ms |
| Eight fork/exec/wait | 2,475.852 ms | 2,472.460 ms |
| Eight Alpha spawn/kill/reap, unchanged control | 227.061 ms | 222.869 ms |

The end-to-end copyout differences are small (about 4% and 1.6%), with ordinary
host/TCG variation also visible in the unchanged control. Fork/exec is essentially
unchanged. The host routine's roughly 33-fold improvement at eight VMAs must not
be presented as a startup improvement or proof that startup latency is solved.
Session logs are `/private/tmp/nanami-vma-{before2,after2}-qemu.log`.

Additional regressions compare 5,000 mixed VMA operations against an independent
page-permission model and cover compact splits/merges/reset, shared-VM alias
length/permission changes, 512-entry exhaustion/reuse, cross-VMA access and
failure-before-mutation. At that stage, larger issues included fork reconstructing a
fresh ELF before overwriting mutable state from the parent, and file mmap
allocating/zeroing anonymous memory before copying its contents. These are
duplicate emulation work, not evidence of expensive IPC transport; the later
fork/private-mmap follow-up above removes them.

Validation for this change: the full native suite passed 998 tests (nine
opt-in tests ignored), the focused VMA/readiness suites passed 41 tests under
AddressSanitizer, and the AArch64 Alter/Linux Release check passed. Four-vCPU
QEMU with normal input/network devices completed two Tiny Core desktop sessions
in one boot, including Terminal/Editor/Information launch, real input and
Exit/restart cleanup. The desktop log is
`/private/tmp/nanami-vma-desktop4.log`. A9N sources and its IPC fastpath were
not modified.

## Follow-up: process construction and file snapshots (2026-10-31)

The process probe separates three workloads: eight suspended Alpha ELF
spawn/kill/reap operations (no Linux execution or POSIX traffic inside the
timed region), six Negoto create-only operations (delete outside the timer),
and three samples each of eight Linux fork/wait and fork/exec/wait operations.
The last workload runs the unmodified Tiny Core BusyBox `true`, including its
dynamic linker and libraries. Only the integer-only test harness is custom.
The fork and fork/exec timings include teardown, not just construction. Cold
Negoto create is one observation per boot; warm measurements must not be
presented as cold-start improvements.

Four-vCPU QEMU/TCG, sequential boots, identical timed Linux fixture:

| Workload | Before | After | Interpretation |
| --- | ---: | ---: | --- |
| First Negoto create | 43.331 ms | 36.244 ms | Single cold observation per boot; treat as tentative |
| Warm Negoto create, median of five | 11.045 ms | 7.971 ms | About 28% lower |
| Eight fork/exec/wait, median of three | 2,790.934 ms | 2,343.803 ms | About 16% lower, includes linker and teardown |
| Eight fork/wait, median of three | 102.564 ms | 94.498 ms | Earlier after-run was 101.246 ms; do not overinterpret this control |
| Eight Alpha spawn/kill/reap, median of three | 219.353 ms | 215.524 ms | Small difference; not evidence of a major construction speedup |

A cache-only intermediate run did not improve fork/exec: its median was
2,840.325 ms. Additional same-core handler profiling pointed to repeated
library mmap reads and anonymous-page setup; the final build removes that
profiling. An earlier after-build without reusable source staging measured
2,543.205 ms for fork/exec. A subsequent after-run measured 2,390.861 ms and
8.576 ms for fork/exec and warm create respectively, consistent with the
direction but showing normal host/TCG variation. These are workload measurements,
not a claim that every application or the whole desktop is 16% faster; startup and
reclamation costs are significant. Only one before boot was measured, so the
cold-start difference is particularly tentative. Logs are stored under
`/private/tmp/nanami-process-{before,cache,profile,final,final2,final3}-perf4.log` for
this session, not committed as test artifacts.

The final probe also takes three additional warm Alpha-only samples separating
spawn from kill/reap: medians are 3.774 ms construction and 21.848 ms kill/reap.
This explains why the combined Alpha workload barely improves even though it
has no POSIX hop. These extra samples run before the final create tests and do
not warm Alter's file-content cache; the before probe had only the original
24 Alpha spawn/kill/reap warmups. They are a stage breakdown, not a before/after
claim about capability-reclamation speed.

The former ELF cache retained only its first two pathnames forever. Exec and
PT_INTERP loads did not use it, and pathname reuse could return obsolete bytes.
That change introduced eight LRU content snapshots, at most 2 MiB each, with optional
parsed ELF metadata. Every use first validates the opened file's inode/content
token and size through POSIX/VFS. This works across hardlinks, and avoids stale
hits after same-size writes, truncate, replacement, or token-table eviction.
Cache fill checks the token again after reading; zero tokens are uncacheable.
Whole-file MAP_PRIVATE reads can populate the same cache, so the linker's
overlapping fixed segment maps and subsequent processes avoid rereading and
relaying library bytes. Partial mappings do not prefetch whole files.
MAP_SHARED retains its existing VFS-owned coherent read-page protocol.

Eviction overwrites a reusable cache buffer rather than releasing and allocating
one on every miss. Alpha currently defers some released physical allocations,
so a release-only LRU would have bounded entries but unbounded retired backing.
Buffers grow geometrically up to 2 MiB: active cache backing is at most 16 MiB,
and its accumulated retired growth plus active backing stays below 32 MiB
(excluding transient allocations and the existing uncached large-image buffers).
Failed reads invalidate content identity but retain buffer ownership for reuse.
The 2026-11-01 cycle-31 follow-up uses 48 small-file slots within the same
16-MiB data budget and explicitly protects a main ELF during PT_INTERP loading.
Its eight-boot normal-device startup comparison observes a 4.3% pooled median
reduction, below the 5% cycle target; no broader application-throughput claim
is made. See [cycle 31](optimization-cycles.md#cycle-31-bounded-small-file-snapshot-working-set)
for all samples, the slower 6.32-second observation and the admission/lifetime
regressions.

Alpha's single-page allocation path no longer allocates two temporary Vecs per
ELF/stack page. A fixed 1,024-slot initialization window caches Alpha's own RAM
Frame mappings, not disposable process aliases. These capabilities outlive
child revoke/reap. Collisions unmap before replacement; failed MAP/UNMAP stays
tracked and is not a cache hit. The cache occupies a bounded 4-MiB virtual window
plus metadata/page tables; it does not allocate a duplicate 4 MiB of page data.
Every newly allocated page is still initialized. Source ELF staging reuses this
window for owned RAM and retains transient mapping for foreign/shared pages.
The private ELF snapshot copy is retained. Initial/exec stacks copy only the
populated tail, not the entire 16-KiB scratch buffer.

### IPC path and architecture choices

Ordinary file operations follow `Linux fault -> Alter -> POSIX -> VFS -> storage`.
POSIX owns native FDs/OFDs, offsets, sharing and locks; VFS owns ext2 handles,
the block cache and persistence. Delegated direct I/O already removes a POSIX
payload relay, but it does not remove these control IPC hops. Local Linux
signals, pipes, AF_UNIX and task bookkeeping do not all traverse POSIX. Alpha
ELF staging, page construction, memory cloning and capability reclamation are
separate costs; bypassing POSIX does not remove them.

Validated snapshots reduce repeated reads and copies while retaining authority
boundaries. A direct Alter-to-VFS file backend is a possible architectural
change, not a demonstrated cure for these timings. It requires moving/reusing OFD,
offset, dup/fork, locking, SCM_RIGHTS and teardown semantics, not simply changing
the endpoint of read/write. Sharing implementation would avoid duplicating
those semantics, but this alone gives no IPC cost measurement. A separate Alpha image/clone
operation could avoid constructing a fresh ELF child before fork copies the
parent, but needs explicit snapshot ownership, mapping permissions and failure
cleanup. Neither restructuring is silently included here. A9N and its IPC
fastpaths are unchanged.

```sh
python3 thirdparty/tinycore/build.py --desktop --image --with-tests --offline \
  --output-dir thirdparty/tinycore/build-desktop-process
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-process/nanami-tinycore.img \
  --tinycore-process-perf --normal-devices --smp 4 --timeout 300
```

The probe also runs real cached-ELF freshness checks after the timed workloads:
same-size pwrite, hardlink, rename/replacement, invalid ELF and ftruncate must
change execution results or return ENOEXEC. Keep one timing VM running at a time.
Rebuilding an image while its VM is running is not supported.

Validation: 995 native tests pass (eight standalone timing tests ignored).
The cache, initialization window, token, storage and truncate targets pass 60
AddressSanitizer tests; the final cache/window/token targets were rerun separately
after the last borrowing/buffer-reuse changes. The builder's 20 Python tests and
AArch64 Alter/Linux Release check pass. Four-vCPU QEMU passes the raw syscall,
rooted namespace and shared-file fixtures plus two desktop sessions with real
Terminal/Editor/Information input and Exit/restart. One-vCPU QEMU passes the real
X11 pointer-coordinate/reversal checks and Files, Images, Paint, Calculator and
Clock window/cleanup tests. Process benchmarks and real ELF-cache freshness pass
on four vCPUs. The boot image is
`thirdparty/tinycore/build-desktop-process/nanami-tinycore.img`.

## Follow-up: rooted metadata lookup and sysinfo (2026-10-31)

Path-based stat/lstat/newfstatat/statx and access/faccessat previously resolved
the guest path, translated its canonical name to a host path, then asked POSIX
to stat it. Both requests traversed POSIX/VFS and walked the filesystem. Alter
now requests an optional `PathMetadata` prefix with the rooted RESOLVE reply.
This returns the final inode, size, full mode and VFS type in the same operation,
including zeroed metadata plus a canonical path for an allowed missing leaf.
It removes the second filesystem RPC/walk for both successful and negative
lookups. Existing path-only clients keep the old format. Virtual mounts and
the hosts/resolver fallback remain local. No cross-syscall cache, weakened
symlink check, synthetic stat success or Kernel IPC fastpath change is involved.
The POSIX namespace proxy also borrows its session rather than copying its
environment and FD arrays into the handler/input-copy calls.

The fixed-work Linux fixture performs three samples of 512 newfstatat calls
to `/usr/local/lib/libX11.so.6`, then to a nonexistent sibling. It verifies
each syscall result; timers surround the loop, including guest copyin/copyout
and the complete emulated syscall path. Compare one QEMU/TCG VM at a time,
with four vCPUs, normal PS/2/user networking and otherwise idle host conditions.
These measurements are not whole-desktop FPS or physical-device results.

Sequential four-vCPU QEMU runs, three samples per image, median elapsed time:

| Complete syscall workload | Before (ms) | After (ms) | Reduction |
| --- | ---: | ---: | ---: |
| 512 successful newfstatat calls | 209.202 | 141.871 | 32.2% |
| 512 missing-file newfstatat calls | 170.875 | 108.008 | 36.8% |
| 2,048 AF_UNIX round trips (control) | 382.208 | 376.121 | 1.6% |

The small control-workload difference is not evidence of an IPC fastpath
speedup. A first after-build, before removing session copies, also improved
the metadata workloads (158.273 / 117.043 ms medians); do not attribute the
entire difference between the two after-runs to session borrowing alone.
The before image is `build-desktop-sysinfo-before`; the final image is
`build-desktop-sysinfo`. Both include the identical integer-only performance
fixture and use the same Kernel sources and configuration.

`sysinfo` uses the existing live timer and Alpha memory/process information
providers. No hard-coded RAM size, fabricated runnable load or extra Kernel
API is introduced. Its 112-byte Linux layout, padding, memory units, rounding,
errors, checked pointer bounds and read-only/cross-page destinations have
native and raw-syscall regressions. The rooted namespace fixture checks
symlinks, renamed dirfds, fresh chmod results, statx and missing-hosts fallback.

Validation: 975 native tests pass (eight timing benchmarks ignored); the
production sysinfo/rooted lookup/path-walk targets pass 12 AddressSanitizer
tests. The Tiny Core builder's 20 Python tests and AArch64 Alter/Linux Release
compile check pass. With normal QEMU devices, one vCPU passes the console
profile, real X11 pointer checks and all five GUI application windows with
cleanup; four vCPUs pass the raw syscall/namespace/shared-file tests and two
desktop sessions with Terminal/Editor/Information input and Exit/restart.
The tested final image is `build-desktop-sysinfo/nanami-tinycore.img`.

```sh
python3 thirdparty/tinycore/build.py --desktop --image --with-tests --offline \
  --output-dir thirdparty/tinycore/build-desktop-sysinfo
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-sysinfo/nanami-tinycore.img \
  --tinycore-perf --normal-devices --smp 4 --timeout 240
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-sysinfo/nanami-tinycore.img \
  --tinycore-desktop --normal-devices --smp 4 --timeout 300
```

## Follow-up: capture batching and pointer drift (2026-10-31)

The following table is historical. The current `--tinycore-perf` capture
baseline is frozen at the already-accepted 2026-11-01 implementation in
`fbdev-capture-baseline.rs`, with four balanced-order samples and independent
expected damage areas. It no longer compares against the pre-batching loop
used in this table. See [cycle 28](optimization-cycles.md#cycle-28-real-desktop-capture-profiling-and-a-rejected-row-major-candidate)
for the later real-desktop diagnostics, controlled regression and restored
same-algorithm control. These fixtures do not report whole-desktop FPS.

Honoka compares eight sampled pixels per aligned group, combining differences
before deciding whether to update the capture. Alignment is checked once per
tile row, not once per pair. Odd alignment, padding and tail pixels retain a
bounded scalar path. All comparisons remain exact, and drawing still uses the
same captured values; the source is never reread after comparison. There is no
new FPU/Kernel support, reduced refresh rate or skipped input sample.

At that stage, the integration probe's `--tinycore-perf` mode ran the production capture and
a test-only snapshot of the previous loop in the same x86_64 QEMU/TCG VM.
Three samples per workload, 800x600, 120 frames, four vCPUs:

| Workload | Previous median (ms) | Batched median (ms) | Reduction |
| --- | ---: | ---: | ---: |
| Idle | 174.344 | 97.633 | 44.0% |
| One changed pixel/frame | 175.819 | 96.966 | 44.8% |
| Entire framebuffer changes | 275.915 | 146.153 | 47.0% |

Each before/after pair asserts identical total damage area (0 / 61,440 /
57,600,000 pixels respectively). Native tests also compare exact rectangles,
captured pixels, alignment/tails and unchanged stride padding against the
reference. These are **capture-loop timings, not whole-desktop FPS or X11
round-trip latency**. The AF_UNIX fixture still runs separately; its median is
382.150 ms per 2,048 round trips in this run, not evidence of a new IPC speedup.

The legacy PS/2 stream now receives events before the 128-record evdev ring
can overwrite relative movement. Its independent byte queue retains residual
motion and the final button state for slow readers. The evdev and PS/2 readers
no longer drain each other's records, and PS/2 readiness does not depend on
stale evdev records. An ingestion test delays reading across 3,200 absolute
samples and verifies the final nested coordinates. The compositor and Alter
both clamp absolute positions to the fixed guest framebuffer: viewport-only
resizing crops rather than scales that framebuffer. Otherwise coordinates
beyond an X screen edge corrupt the next relative baseline.

Build a fresh disk and run the real X pointer / GUI application checks:

```sh
python3 thirdparty/tinycore/build.py --desktop --image --with-tests --offline \
  --output-dir thirdparty/tinycore/build-desktop-input
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-input/nanami-tinycore.img \
  --tinycore-perf --normal-devices --smp 4 --timeout 240
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-input/nanami-tinycore.img \
  --tinycore-apps --normal-devices --smp 4 --timeout 240
```

Native suite: 967 passed, eight ignored benchmarks. AddressSanitizer on the
changed capture/compositor/input/console/fadvise paths: 45 passed, one ignored
benchmark. AArch64 Alter/Linux and Honoka Release compile checks also pass. The syscall
fixture checks fadvise64's six hints, argument/error handling, file data and
offset preservation; these advisory hints do not trigger a storage RPC.

QEMU validation uses normal PS/2/network devices: four vCPUs pass actual X11
QueryPointer positions after a reversing mouse burst, all five GUI application
windows and Exit cleanup. One vCPU passes two desktop sessions in the same boot,
including Terminal/Editor/Information launch, input and cleanup in both. The
final rebuilt disk also passes the four-vCPU console profile and updated raw
syscall fixture, including fadvise data/offset preservation. None of these
tests require a physical-device or Kernel modification.

This pass changes Nanami userspace only. A9N sources, IPC fastpath, fault ABI,
priority policy and guest clock semantics are unchanged. Existing user images
are preserved; the generated image is under `thirdparty/tinycore/build-desktop-performance/`.

## fbdev damage and composition

Unmodified Xfbdev does not publish Nanami damage rectangles. Alter still sends
its existing 60 Hz presentation notifications, but Honoka no longer treats
every notification as a full-window change. It compares exact 32x16 pixel
tiles, updates a stable RAM capture and composes only changed regions. Adjacent
horizontal tiles and vertically matching spans merge; a fully changing screen
still produces one rectangle rather than one presentation IPC per strip.

Rendering uses the captured pixels, not a second read of the live producer.
Otherwise a producer could change between comparison and drawing, leaving the
capture equal to the next source frame while the display retained different
pixels. Native damage-queue clients keep their original path and allocate no
capture. fbdev captures use reclaimable mappings outside the compositor's small
heap, are released on detach/close, and fall back to full redraw if allocation
fails. Viewport resizing refreshes the newly visible capture without changing
guest resolution or stride.

For an 800x600 surface:

| Workload | Previous composition area | New composition area |
| --- | ---: | ---: |
| One unchanged presentation | 480,000 pixels | 0 |
| One changed pixel, interior tile | 480,000 pixels | 512 |
| Full-frame change | 480,000 pixels | 480,000, one rectangle |

The ignored host benchmark changes one pixel per frame across 600 frames:
288,000,000 full-redraw pixels become 305,152 tile pixels. This is an exact work
count, not an FPS claim. Detection still reads source/capture RAM at 60 Hz and
needs one extra four-byte pixel buffer (about 1.83 MiB at 800x600). It does not
add hardware dirty-page tracking or eliminate the framebuffer notification timer.

## AF_UNIX payloads and allocations

`send`/`write`/`writev`/`sendmsg` copy guest bytes directly into the owned packet
allocation. `read`/`readv`/`recvmsg` copy the receive snapshot directly into the
guest. Both retain checked guest memory access and existing chunk limits; no
Kernel API or capability transfer is added. The POSIX SHM staging copy in each
direction is removed. Snapshot-before-dequeue semantics, copy-fault rollback,
stream ancillary barriers, datagram boundaries and SCM_RIGHTS ownership remain.

Receive allocation follows queued payload up to the next applicable barrier,
not the caller's maximum buffer length. A measured-allocator test requests
65,536 bytes with an eight-byte packet and observes an eight-byte allocation.
It also verifies that payload copyin/copyout use direct owned buffers. A second
bounded packet-header pass computes this size; large payloads still use one
contiguous receive snapshot, not one IPC per queued packet.

## Process lookup and logs

Alter remembers one process-table slot for repeated lookups within a syscall.
Every hit checks the current PID and live PCB; removal, slot reuse and moving
entries cannot return a stale process. Misses keep the existing linear search.
The hint is one word, not an owning reference or an authorization cache. FD,
terminal and badged-fault lookups use the same validated helper. A production
helper test performs 10,000 warm lookups with exactly 10,000 identity checks.

Ordinary stdio close/dup2 operations and missing PATH candidates no longer
write synchronous diagnostic lines to the serial port. `strace` retains those
diagnostics; unexpected exec errors and unsupported-syscall reports remain.
No syscall result or permission check is suppressed.

## Reproduction

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test fbdev-damage --test honoka-motion --test linux-unix \
  --test alter-process-index -- --test-threads=1
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test fbdev-damage -- --ignored --nocapture --test-threads=1
python3 thirdparty/tinycore/build.py --desktop --image --with-tests --offline \
  --output-dir thirdparty/tinycore/build-desktop-performance
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-performance/nanami-tinycore.img \
  --tinycore-perf --normal-devices --smp 4
python3 tests/negoto/qemu-smoke.py \
  --image thirdparty/tinycore/build-desktop-performance/nanami-tinycore.img \
  --tinycore-desktop --normal-devices --smp 4 --timeout 240
```

Choose a fresh output directory; do not use `--rebuild` on a disk containing
guest data. The QEMU runner uses private disk/firmware snapshots. The integer-only
performance fixture performs three samples of 2,048 eight-byte AF_UNIX stream
round trips, validates every received value and samples monotonic time only
outside the loops. These QEMU/TCG measurements are not physical-machine latency
or graphical FPS. Compare one VM at a time under otherwise idle host conditions.

## Validation and timing boundary

- Complete native suite: 952 passed, eight explicitly ignored benchmarks.
- AddressSanitizer: 53 passed across framebuffer capture, the real compositor,
  AF_UNIX handlers and validated process-slot lookup; one benchmark ignored.
- x86_64 Release image built; AArch64 Alter/Linux and Honoka compile checks pass.
- Normal-device QEMU with one/four cores passes real X11 keyboard input, window
  dragging, terminal close/relaunch from wbar, editor/info launch and explicit
  desktop Exit. Existing ABI and shared-file-mapping fixtures also pass.

Sequential QEMU/TCG runs, three samples per image, same fixed eight-byte workload:

| CPUs | Before median (ms / 2,048 round trips) | After median |
| --- | ---: | ---: |
| 1 | 217.312 | 220.042 |
| 4 | 388.642 | 396.937 |

These samples do **not** demonstrate lower small-message round-trip latency.
The removed copies are tiny in this workload, its receive buffer is only eight
bytes and it has one active Linux process. Do not describe allocation/composition
work reductions as a measured IPC-speed or whole-system FPS gain. Large X11
receive-buffer allocation and idle/sparse rendering improvements are established
by the production-path allocation, rectangle and pixel tests above.

## Restart regression (2026-10-31)

The desktop runner now launches the same container twice in a single boot.
Each session must execute fresh keyboard-written markers from its initial
terminal, a newly dock-launched Terminal, and Information, then exit and reap.
Markers are removed before each launch so the previous session cannot satisfy
the next session's assertions.

The pre-fix image passes the first session but fails during the second launch:
the dynamic loader cannot mmap libc (`EINVAL`). The same fixed-size page-table
pool also caused `image-pages` spawn failures and cascading shared-map rollbacks.
Page-table capabilities/backing pages previously accumulated across exit/exec,
and PID-dependent temporary VAs accumulated permanent Alpha tables. The fix
reclaims per-address-space tables and reuses a single scratch VA, without Kernel
changes or increasing the 1024-table capacity. Host tests cover pool exhaustion,
recovery and 2,000 address-space lifetimes; the original performance timings above
are historical measurements of the earlier optimization pass.

Validation: 956 native tests passed (eight ignored benchmarks), 10 profile-builder
tests passed, x86_64 Release image generation and AArch64 Alpha compile checks
passed. QEMU normal-device runs with one and four cores both completed two
desktop sessions in one boot, including second-session Information commands and
final cleanup. Neither run reported spawn failures, shared-map rollback errors
or missing intermediate page tables. The runner waits for the Information
command's guest-written result before clicking Exit; PTY initialization may flush
keys sent before the shell becomes ready. The second session closes every other
interactive terminal, so another terminal cannot satisfy that check.

## Rich desktop profile (2026-10-31)

The pinned desktop graph now adds Xfe, FlPicSee, mtPaint, Galculator and the Flit
clock. The dock uses explicit BusyBox wrappers, while FLWM menu links target
the original ELF programs. Home folders, a welcome document and a sample image
are included; Xfe's initial geometry fits the nested 800x600 display. The ext2
filesystem defaults to 256 MiB. Only FOX's runtime libraries are installed,
not its unrelated demos/GL dependencies; Xorg's tools
package contributes only xwininfo. No Mesa/LLVM stack is installed.

Large GUI/font workloads exposed the primary per-process metadata arena's
fixed size. Frame-node metadata now acquires additional 2-MiB Generic arenas
on demand, resets them only after the last VM user is gone, and reuses them with
the root slot. This does not enlarge every small process's reserve or alter A9N.
Twenty repeated large-VM metadata lifetimes plateau at the same reserved RAM in
the production-path allocator tests; failed revoke never authorizes reuse.

The application smoke test runs the five GUI clients one at a time, checks
their actual window titles and `IsViewable` state, and retains screenshots with
the initial terminal closed. Native VFS reads verify independent result files
and final context cleanup. The observer polls at 100 ms, so its higher priority
does not saturate VFS/Alpha while Linux clients load. This is a startup/render
test, not a new performance measurement or complete feature audit.

Fluff's inotify initialization error and Dillo's failed TCP-loopback daemon are
not counted as working applications; those packages are excluded. GTK cache
maintenance is not a session-start prerequisite after that variant timed out.
Galculator's ordinary UI renders, but optional icon-theme lookup can warn.

Validation: 958 native tests passed (eight ignored benchmarks), 20 profile-builder
tests passed and AArch64 Alpha compile checking passed. One/four-core QEMU
application runs displayed all five clients and completed desktop Exit/context
cleanup. The final four-core restart run also completed two sessions in one
boot, including second-session Terminal and Information commands and cleanup.
Terminal input, like Information input, is retried after possible PTY startup
flushes; the launcher is clicked only once and success still requires a fresh
guest-written marker. Higher-priority lifecycle observers also poll at 100 ms.
These checks do not update the earlier performance table.
