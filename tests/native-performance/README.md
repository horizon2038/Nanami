# Native application heap performance

The ongoing evidence, acceptance rules and rejected experiments are recorded in
[Nanami / Alter optimization cycles](optimization-cycles.md).

Permission-checked private copyout retains its cached-only fast path. On a cold
miss, the current fault target may try ordinary write views before private
preparation only while all VM users are already quiescent. Alpha still requires
current RW, non-COW, certain ordinary RAM and matching Frame/ownership; prepare
every lease before any store. Refusal retains partial grants/unready pins and
uses the existing checked preparation and ordered fallback. No permission or
backing mutation is admitted by this attempt. Set NANAMI_COLD_WRITE_VIEWS=0 for
the earlier path. Tests cover peer states, permissions, uncertain/COW refusal,
cross-page no-store, ordered vector prefixes and unsupported scratch/size paths.
The accepted frozen source passes2686 native tests in both Debug and Release;
the focused default and optout suites each pass498, with2 ignored. Counted
preparation/view work falls in the Files launch census; whole-launch speedup
and100ms paint/input readiness remain unproven.

Supervisor page-view invalidation releases up to17 exact local pages in one
Nanami request. Validate the entire header first, retain ordinary per-page
ownership/UNMAP/TLB checks, and continue after an independent failure. Only
acknowledged bitmap entries become reusable; lost/malformed replies leave all
selected leases unready. Existing sparse-release validation makes a confirmed
hole retryable without discarding retained or uncertain backing. Tests cover
64slots/4requests, partial ACKs, bounded chunks, failures, no stores, lost replies,
hole retries, partial-object rejection and actual wire fields. Set
NANAMI_BATCH_PAGE_RELEASE=0 for scalar releases. Pair new Alpha and both Alters;
no Kernel ABI, dependencies, heap buffers or persistent fields are added.

Large supervised anonymous RW/RWX mappings can share initialized zero backing
under the explicit `OS_MAP_INITIALIZER_COW_ZERO` contract. Genuine huge leaves
are preferred; the small-page fallback keeps16 hot pages private and writable,
with the cold initialized tail logically writable/COW and absent from hardware.
Current Frame identity, references, rights, uncertainty and VM quiescence remain
mandatory. `OS_PRIVATE_WRITE_DETACH_POPULATE` repairs a missing or present write
fault and can detach at most64 neighboring writable COW aliases of the same
pool Frame. Other Frames and readonly/NONE/uncertain pages are unchanged.
Existing QUERY/DETACH/DEFERRED modes retain their contracts. Alpha and both
Alters must be updated together; this extends Nanami's service protocol, not
the Kernel ABI. `NANAMI_SHARED_WRITABLE_ZERO=0` selects ordinary anonymous RAM.
Native coverage checks automatic/fixed placement, zero bytes, cold PTE absence,
fork, copyout guards, partial reuse, huge preference, neighbor exclusion and
retention after MAP/UNMAP failures. KVM evidence records actual use and its
limits: request-level initial RAM savings are not global/lifetime RSS, and
prefetched neighbors do not prove an avoided-fault count or CPU speedup.

Directory name-cache misses can search borrowed warm block bytes without a
scratch copy. Existing positive/negative precedence and negative publication
rules remain: malformed/inode/type/map/I/O/bounds failures cache no miss. Tests
compare production block/dentry caches and parser with the prior scratch
scanner, including dirty invalidation, prefix/case/deleted names, early matches,
repair/retry and EOF. Borrowed names do not outlive search, and cache publication
uses only returned inode values. No cache budget or startup/CPU gain claim.

Symlink payload construction appends only the inline bytes in inode.size rather
than60 bytes followed by truncation; block-backed targets can borrow current
cached bytes directly. The ext2-link-target suite uses the actual reader/cache
and modeled inode/transport, checking all inline lengths0..60 with exact output
and zero reallocations, cold/warm partial blocks, dirty bytes, bounds/holes and
allocation/type/size/I/O error precedence. No path-resolution, authority or cache
budget change; allocation counts do not establish whole-RPC or startup savings.

Warm directory enumeration borrows the current cached block's EOF prefix and
parses/serializes it before releasing that borrow, avoiding scratch staging.
Cold reads use the same emitter after successful scratch I/O. Production cache
tests compare1/7/16-entry batches with full rescans, including dirty names and
deletions, generations, handle reuse, malformed output prefixes/retry and cached
bounds. Cookies, cursor publication, record bytes and CLOCK behavior remain;
there is no new cache/heap capacity or claimed startup/CPU improvement.

Warm indirect get_data_run requests can borrow the current cached leaf instead
of copying its whole block into scratch. Current parent/leaf bounds are checked;
only a run resolved within that leaf returns directly. Missing records and
cross-leaf contiguous prefixes keep the old scanner/error-prefix behavior.
data-run tests use production BlockCache for warm single/double/fragmented runs,
dirty parent/leaf changes, scratch preservation, misses and invalid cached
bounds; storage tests compare borrowed-view CLOCK behavior with ordinary loads.
No new allocation/cache capacity or launch-wide speed claim.

Ext2 cached ranges of2..64 blocks capture slot indices during presence proof and
copy through those request-local slots, removing a second BTreeMap traversal.
Missing blocks leave output and CLOCK bits untouched; exclusive borrowing keeps
slots stable. The512-byte64-bit stack array covers current64-KiB transfers with
1-KiB blocks; single-block and larger-range paths retain their behavior, with no
cache/transfer-capacity change. Production storage tests cover63/64/65 and129
blocks, partial edges, dirty bytes, misses and eviction equivalence to scalar
loads or untouched controls. No startup/CPU improvement is claimed.

Physical allocate_any retains its checked select-then-allocate_at route. The
direct-carve revival was withdrawn after discovering the existing cycle18
rejection. Buddy block return now combines buddy and duplicate detection into
one scan, preserving first-buddy priority, duplicate refusal, swap-remove and
upward merge order. Real-pool tests compare duplicate/misalignment/high-page
cases and6000 fragmented range returns with the original two-pass merge.
Public allocation/partial-free tests still check exact free-list order and an
independent page model. No cache/index/budget/policy or claimed launch gain.

Immediate process copies and compare-exchange can reuse an exact current Ready
initialization alias through map_process_transfer_frame. Copy resolves both
Frames first; later destination selection only changes CopyWindow on a miss.
The stable map_process_copy_frame contract is unchanged: COW detach obtains a
source before changing initialization slots and must retain a CopyWindow alias.
alpha-copy-window combines both production windows to check reuse, pinned
mixed aliases, owner invalidation, slot replacement, pending MAP/UNMAP fallback
and retained-source stability. Neither cache budget nor guest authority changes;
per-hit avoided MAP does not establish startup-wide or CPU savings.

Private-snapshot destination checks now resolve the memory owner and scan lazy
mapping records once, rather than repeating the same lookup for every target
page. The range helper preserves the original page-grid predicate, half-open
ends, unaligned keys, checked overflow and malformed-entry cutoff. Differential
ProcessManager tests compare3000 ranges for owners/peers/other/unknown PIDs;
production snapshot tests reject first/middle/last conflicts over513 pages before
any canonicalization/copy/PTE change and permit an adjacent lazy range. No lazy
page is materialized or persistent state added; hardware calls are unchanged.

Physical-reference retirement resolves its partition/slot/count once, then
commits directly to that entry after successful release. Last-owner deletion
retains the existing backward-shift algorithm. Failed release keeps its record
and suffix; each next alias resolves a fresh entry after the preceding commit.
No entry survives a reference-index mutation. Scalar-oracle tests cover257
repeated aliases, another live owner, Active/Deferred states, failure prefixes
and successful retry, alongside existing collision/churn and Alpha cleanup tests.
This removes redundant hash lookups; it does not establish launch-wide savings.

Sealed exec retirement now visits current Frame metadata directly in VA order
instead of allocating a backed-page snapshot and looking up each Frame again.
Confirmed PROT_NONE backing is skipped; uncertain entries still receive UNMAP.
The read-only visitor returns the failed VA/error and stops further callbacks;
its structural tree traversal may finish without additional hardware actions.
The handler marks that VA uncertain and returns before cap/RAM cleanup. All
leaf UNMAPs must still succeed before chunk revoke and deferred RAM retirement.
`vm-index` checks order/presence/error prefixes and allocation-free traversal;
production exec tests check zero allocations before first UNMAP over513 pages
and before/after-removal failures at0/1/256/512. These test modeled PTEs and
ownership, not hardware shootdowns or a startup/CPU percentage. User UNMAP
count/order is unchanged; metadata work and temporary storage are removed.

Alpha protection requests now avoid the page rollback Vec and repeated point
lookups when a current metadata range query proves every page is backed,
certain and already has the exact requested rights. COW write markers decline
this path. Copy-window invalidation still runs and its failure propagates.
Changes, holes and uncertain ranges retain the original handler/rollback.
`vm-index` compares exact-range proofs with point queries, including default7,
PROT_NONE, sparse/high/unaligned metadata and allocation-free visits; existing
readonly snapshot queries use the same exact-rights helper. Production handler
tests verify zero allocations over64 pages for rights0..7, unchanged PTEs,
cleanup failures and COW/uncertain guards. Equal-rights pages already avoided
user MAP/UNMAP; the improvement is metadata/temporary storage work, not a claim
of fewer user mapping syscalls or measured startup/CPU savings.

Private-page initialization now also probes ready initialization aliases for
current permanent-pool source Frames, including shared POSIX read buffers. When
preflight resolves only pool Frames, it skips the optional child-ownership
snapshot; every Frame is still resolved again during copying. Private/mixed
sources retain the old snapshot/child validation. Destination-slot collisions
use the existing CopyWindow role swap even without a snapshot, and fallback
source mappings still pin that destination. No capacity, rights, publication,
tail-zeroing or rollback change. `alpha-private-pages` checks pool-only/mixed
hits, zero snapshot calls behind4000 newer records, current-Frame replacement,
collision pinning and failure rollback. Reuse/role-swap optouts retain coverage.
The alias cache is modeled here; actual cache transitions remain covered by
initialization-window tests. Work counts do not establish launch/CPU gains.

Alpha's static image/record staging reader now probes an existing ready
initialization alias by its freshly resolved Frame before using CopyWindow.
The copy into independent static storage remains; no source view survives it.
The lossy Frame hint occupies padding in the existing initialization rows:
4096 aliases, their physical-slot policy and64-KiB metadata bound stay unchanged
(the compact `NANAMI_SMALL_INIT_WINDOW=1` configuration uses2048/32KiB).
Empty/Pending/Ready states preserve failed MAP/UNMAP tracking. A collision,
stale Frame or pending entry declines to the original copy window without scans
or ownership inference. Initialization-window tests exercise hints/collisions/
failure cleanup; staging tests model ready aliases and check fresh data, current
Frame replacement, unaligned spans and committed prefixes. They establish the
conditional path, not actual launch-wide hit/MAP counts or a CPU percentage.

ext2 inode-cache misses now decode the100 bytes of fields in use directly from
a borrowed block-cache record, avoiding a full-block scratch copy. The borrow
cannot outlive a cache mutation; no pointer/view persists. Cold misses decode
the existing block-read scratch in place. `ext2-inode-read` compiles the actual
reader/decoder and block cache with modeled transport/decoded-cache access,
checking block/group boundaries, inode strides, all fields, dirty values,
failed publication and invalid record ranges. Storage tests cover record-view
bounds and CLOCK equivalence. Existing decoded-cache hit/replacement policy is
unchanged. These tests establish contents/work behavior, not CPU or startup gains.

ext2 scalar indirect-pointer lookups now fetch four bytes from the current block
cache instead of copying a whole cached table into scratch. Cold misses retain
ordinary block validation/read/retry/cache-fill behavior. Run scans explicitly
load their leaf table when inspecting multiple pointers; double-table parent
lookups and single-block runs need only the scalar value. No scratch pointer or
new metadata cache persists. Single-block `load_range` also uses one exact index
lookup, retaining range-overflow checks and CLOCK reference marking. Storage
tests check cold fill, warm scratch preservation, dirty values, invalid ranges
and eviction equivalence; data-run tests model warm word reads without touching
scratch and still compare direct/single/double boundaries and error prefixes.
Copy/query work is reduced; device-I/O and launch/CPU gains are not inferred.

UNIX socket reachability now uses a128-byte bounded worklist instead of repeated
whole-graph passes. Each marked endpoint is processed once; existing exact-ID
lookups still resolve its accept/SCM_RIGHTS edges. Peers stay weak. A request-local
sweep cursor skips the unchanged marked prefix while rechecking swap-removed
slots, preserving the original removal/release order. No persistent state,
allocation, capacity or bulk-close lifecycle is added. `linux-unix` checks reverse
chains, full roots/frontiers, independent graph reachability after ID churn,
weak/unrooted cycles and scalar sweep order, plus existing rights/resource tests.
Cycle67's limited startup GC contribution remains evidence; this change does
not claim GC is the current launch bottleneck or establish a CPU percentage.

Alpha's ELF page filler now initializes a page with its first file copy when
that copy covers all4096 bytes, avoiding a preceding zero pass. Partial first
copies still zero the page; BSS/no-copy pages and failures before the first copy
also clear every byte. Segment overlay order, failure prefixes and TLS patching
remain unchanged. `alpha-page-fill` runs the production filler over owned RAM
against a zero-and-overlay oracle, including2000 randomized segment layouts,
overlap, partial pages, BSS, TLS, source/range errors and neighbor sentinels.
It tests page contents and errors, not hardware mapping or a startup percentage.

The ext2 name cache now uses128 one-byte slot hints to avoid scanning all128
entries on an exact hint hit. Parent, length, full name and validity are still
checked; collisions and stale hints fall back to the existing FIFO cache.
Capacity, negative entries, update position and invalidation semantics stay the
same. `ext2-dentry-cache` compares20,000 mixed operations and periodic complete
lookups against an independent model of the old FIFO, plus wraparound/stale
slots, parent/name distinctions and boundary-length cases. These checks prove
cache behavior, not a launch-time or CPU percentage.

Checked-copy permission preflight now queries current VM permission exceptions
and uncertain mappings by range. Single-page requests retain direct lookup;
backing, COW and copy-window preparation remain in the existing transfer path.
Production VM tests compare the range result with page-wise queries, including
partial endpoints, high addresses, dense rights, sparse exceptions and uncertain
updates. The Alpha fixture checks a late denial before a16MiB mocked transfer.
This fixture proves preflight/call behavior, not actual16MiB copying or CPU gains.

Snapshot source validation likewise uses current range metadata: explicit
rights1 must cover every page and unique aligned Frame keys must cover the
whole extent. Missing/default/RX/uncertain pages decline. Two VM tests compare
point checks over513 pages, holes, unaligned metadata and high addresses;
existing production Alpha tests still check rights outside the requested alias.
This changes lookup work only, retaining ownership/capability/refcount checks.

Shared RAM creation and opaque service-buffer zeroing now reuse the existing
permanent-pool initialization window. Every byte is still cleared before any
participant mapping is published, even on a cached alias hit. `alpha-shared-zero`
executes the production zeroing body over owned host buffers, including reuse,
neighbor sentinels, mapping failure prefixes and physical-range overflow. Window
selection/eviction/failure behavior is tested separately by the existing
`alpha-initialization-window` fixture; these models do not prove hardware TLBs.
No additional mapping-cache capacity is introduced.

Private mapping release can now defer ordered page references in one guarded
list pass after preflight. Every allocation field and reverse-snapshot order
must match; otherwise it uses the original scalar deferral. References, existing
deferred-tail order and single-page/whole-extent paths remain unchanged.
Production ownership tests compare both implementations behind512 newer records,
with shared references, duplicate VAs, sparse holes and stale/unordered snapshots.
Canonical shared-page retirement now uses the same order/field preflight, then
calls fallible physical release before unlinking each reference. Failure keeps
the failed record/suffix; callbacks are never retried through scalar fallback.
Tests compare scalar callback order, last-owner decisions, committed prefixes,
remaining lists/refcounts and recycled-node reuse, including failures at either
end and after an owner-head update.

`ext2-sector-count` compiles production inode sector accounting with destructive
block scratch and an independent pointer-graph oracle. Dense/sparse direct,
single/double tables, partial64-pointer batches, repeated child pointers, inline
symlinks and read errors preserve accounting. Double-table accounting captures
64 child pointers before reading leaves, avoiding repeated leaf loads per entry.
With256 entries and all children present, double-table reads fall from65792 to260
(plus one read for a separate single table). This counts helper reads, not device
I/O or measured CPU time. No heap allocation or block-size ceiling is added.

`ext2-inode-write` compiles the actual inode publisher. Size growth within
existing blocks preserves the inode's current sector count, but still writes
the inode, invalidates file versions and updates caches only after success.
Allocation-changing writes and other metadata operations still recount.
The storage suite checks2065 one-byte writes with2065 inode commits and only
three recounts, plus payload readback. The guest file-mapping fixture checks128
small appends across a single-indirect block boundary and fsync/reopen/readback.
Regular-file inode publication now preserves directory continuation generations;
generic metadata and all other inode types retain conservative invalidation.
Production writer tests cover success/failure by inode kind, while the actual
block cache tests verify unchanged metadata writeback ordering. The guest fixture
enumerates64 entries in small batches with unrelated appends between batches.

Temporary image retention: after completing cycle N, delete generated boot
images and their backing rootfs from cycle N-2 and earlier. Retain only the
newest validated candidate and the latest accepted working fallback (or the
two latest accepted images when no candidate is retained), without
duplicate copies under older cycle directories. Keep small measurement logs
and artifact hashes; never delete user input images or files used by a live VM.
When intervening cycles are rejected or diagnostic-only, the predecessor means
the last accepted working image, not an unverified newer directory number.
Retaining an experimental candidate does not make it the default or establish
its performance claim; never discard the accepted fallback for an unverified one.

Also review task-owned temporary files under `/private/tmp` at each cycle
closure and after long build/test batches. Wait for measured VMs and their
owning jobs to finish before inventory or deletion. Remove completed disposable
media and obsolete scratch outputs, not unknown applications' files, official
inputs, active files, retained runtime generations or small reproducibility
evidence. This is housekeeping during ongoing work, not an unattended timer.

Optimization follows Rob Pike's measurement rules: first measure the actual
workload and establish that a component dominates its critical path; only then
tune that component. Source-level suspicion, call counts, translated-block
weights, smaller binaries and favorable microbenchmarks are not elapsed-time
attribution. A five-percent cycle target never authorizes speculative tuning.
Keep failed observations and validate any later change against the original
end-to-end workload, including paint and input readiness.

Cycle165 rejects that profiler for normal-runtime cost attribution. Eight
matched boots preserve every original gate, but count/time recording shifts
click-to-ping mean by+46.24%/+51.15%; loaded-off also fails RTT stability.
Executed-prefix guarding and decoded-ELF checks improve coverage correctness,
not timing validity. No syscall-cost ranking, speed change or Kernel change
is admitted. All eight initial images remain visibly incomplete after ping;
later actual keyboard navigation works, not evidence of100ms full readiness.
Diagnostic media and duplicate VM dirs are removed after verified backups.
See [cycle165 evidence](optimization-cycles.md#cycle-165-reject-the-syscall-profiler-after-matched-perturbation-controls).

Cycle164 validates a host-only syscall-pairing prototype with known Linux
scope calls, then observes7770 paired returns in Files' current image. Separate
exec address spaces and194 ambiguous replacements in another AS remain explicit.
Unsupported-prefix coverage and matched perturbation admission are still
required: no syscall-cost/dominance or speedup claim is accepted. Production
runtime/Kernel unchanged. See
[cycle164 evidence](optimization-cycles.md#cycle-164-syscall-pairing-prototype-validates-observed-coverage-not-runtime-costs).

Cycle163 records the missing visual endpoint with four fresh normal-runtime
boots. Both timeline runs remain visibly incomplete after first ping; their
500ms-request frames (actual apertures about2.67--2.79s after press) show the
menus/tree/list/status, and later real keyboard navigation reaches/usr/local.
These are observations, not exact full-readiness times or a speedup. No new
optimization is selected. See
[cycle163 evidence](optimization-cycles.md#cycle-163-first-ping-precedes-visible-completion-and-later-real-input-works).

Cycle162 withdraws the proposed scalar write-input trial before a runtime build
or VM test. Its measured source half is only about3.3% of whole launch, not a
dominant component. Restore only the new call-site/test edits; retain earlier
work. Normal Alter remains953f. No performance claim or new accepted candidate;
identify a dominant real launch interval before selecting another optimization.

Cycle161 identifies the cold Root POSIX writes: all1228calls/18686bytes in
each of six profiled cohort boots use one observed description and OPEN path
`/alter/linux/tinycore/rootfs/root/.config/xfe/xferc_42`. No unknown destinations,
errors or accounting gaps. All original/FD gates pass9/9 plus the separate pilot.
But count-only raw median differs-10.75%, failing the predeclared global5%
measurement gate: retain destination evidence, NOT newly accepted timing or
speedup. No filename-specific bypass/preseed/buffering; production/Kernel stay
unchanged. The proposed cold-write input trial is withdrawn in162: prior157's
valid costs do not establish whole-launch dominance, and161's failed timing
cannot strengthen the case. Diagnostic pair/ten duplicate VM dirs removed;
accepted118/107 retained. See
[cycle161 evidence](optimization-cycles.md#cycle-161-root-writes-target-one-configuration-file-but-the-global-timing-gate-fails).

Cycle160 uses ONE identical diagnostic ELF/image, unarmed/count/timed across
nine fresh boots. All original/report gates and predeclared5% measurement
gates pass. Scoped lookup-copy time supplies at most4.543%/4.272% upper bounds
against observed raw/Xlib client intervals, including some out-of-interval
clock-copy work. It does not dominate; do NOT tune this communication path.
No view grants/releases in36 complete phases. This neither explains159's
normal/private difference nor revives rejected candidate158. Production/Kernel
unchanged. Next identify FD/destination lifetimes in the already-measured cold
Root POSIX-write path instead of guessing from descriptor kind or optimizing
small warm-copy costs. Diagnostic pair and nine duplicate VM dirs removed;
accepted118/107 retained. See
[cycle160 evidence](optimization-cycles.md#cycle-160-identical-elf-control-validates-timing-and-rejects-lookup-copy-tuning).

Cycle159 checks that candidate's communication phase with12 fresh boots and
scoped view counters. All original/report gates pass. No view grants/releases
occur in any of48 raw/Xlib phases, so mapping churn INSIDE those intervals is
not supported. However normal-before count-only raw median differs-6.01%,
outside the PREDECLARED absolute5% probe gate: do not use the timing profile
to select a speed hack, or call that negative difference a gain. Normal versus
private code-layout/inactive instrumentation remains confounded. Production
and Kernel unchanged; candidate158 remains rejected. Preserve original gates;
isolate measurement effects with an identical diagnostic ELF unarmed/armed
before relying on elapsed attribution. Eight diagnostic images and12 duplicate
VM directories removed after verified backup; accepted118/107 retained. See
[cycle159 evidence](optimization-cycles.md#cycle-159-communication-view-churn-is-absent-but-the-probe-control-fails).

Cycle158 tests the measured write-input read-view candidate in eight balanced
before/after real Files runs. All original launch/RTT and file-mapping gates
pass, but mean launch improves4.91% while pooled raw128 median regresses6.37%,
outside the PREDECLARED5% communication gate. Root improves14.82%, which does
not justify accepting the whole-system tradeoff. Reject the candidate, restore
both production functions/tests and reproduce the exact normal Alter953f hash.
No new optimization is retained; Kernel stays clean/unbuilt. Next measure the
raw phase's actual view-grant/release work and initial-paint interference before
choosing a change; cache pressure is only a hypothesis. Diagnostic media and
duplicate VM directories removed; accepted118/107 retained. See
[cycle158 evidence](optimization-cycles.md#cycle-158-write-input-view-reuse-improves-launch-but-fails-the-communication-gate).

Cycle157 lowers hot-loop probe disturbance with same-actor counter/timer rate
brackets and classifies Root writes. Both whole/local5% measurement gates pass.
All1228 writes are POSIX,18686bytes: source-read73/72ms, provider75/75ms, neither
dominates the other. No pure-IPC or memcpy claim. This selects a bounded
existing-helper trial, not a promise of5% whole-system gain. See
[cycle157 evidence](optimization-cycles.md#cycle-157-low-disturbance-write-decomposition-before-selecting-a-change).

Cycle156 correlates that window-tree scope with all Alter events. Six original
Files launch/RTT runs pass after rejecting an earlier UART-interleaved cohort.
Read-mode Root write handling takes156--159ms: about68% of Root syscall
handling, but only28% of the scoped wall interval and roughly7% of whole launch.
Receive/handling each account for about47/49%; receive is NOT pure IPC and
handling is NOT exclusive CPU. The whole-launch read mean differs+0.97% from
normal, but Root itself grows6.60%: the whole-workload gate hides material
local measurement disturbance. Do not tune from those raw costs yet. Lower
probe disturbance, then measure write descriptor kinds/source-read/provider
stages before choosing a change. No production/Kernel change. Eight diagnostic
images, one initramfs and11 duplicate VM log directories removed after verified
backup; accepted118/107 retained. See the
[cycle156 evidence](optimization-cycles.md#cycle-156-root-scoped-write-handling-is-a-candidate-but-probe-disturbance-matters).

Cycle155 partitions actual resource creation with nested bookkeeping. Six
original Files runs and all four actual hook-coverage/partition gates pass.
The disjoint union covers almost all create. Visual takes2.2ms, fonts27--29ms,
cursors41ms; window-tree work excluding those selected children takes510ms.
That is87.4--87.7% OF CREATE, about23.1% of observed whole launch, NOT88% of
startup or exclusive CPU. Counts alone mislead:1122 cursor calls cost much
less elapsed time than one root call. Loaded-control/read means differ
-1.30/+0.44% from coarse baseline, not exact tax/significance with two runs
per mode. Next correlate this window-tree scope with Alter syscall/service
paths before tuning. No production/Kernel change; two diagnostic images and
one initramfs removed,118/107 retained. See the
[cycle155 evidence](optimization-cycles.md#cycle-155-window-tree-work-dominates-resource-creation-not-whole-startup).

Cycle154 measures interposable Xlib synchronous reply handling INSIDE the
actual create phase, using same-process clock/counter brackets. Six original
Files runs pass. Both timed boots see36 create calls out of84 pre-run calls,
about98ms inclusive elapsed, only16.55--16.82% of create. Roughly485--496ms is
NOT IN THIS HOOK; that complement is not pure CPU, and other XCB/send/wait
paths remain unclassified. No speculative IPC tuning follows. Added loaded-
control/read means differ+0.61/+2.51% from coarse-only baseline, not exact tax
or significance with two runs per mode. No production/Kernel changes; two
diagnostic images and one initramfs removed,118/107 retained. See the
[cycle154 evidence](optimization-cycles.md#cycle-154-interposable-synchronous-replies-are-a-minority-of-gui-resource-creation).

Cycle153 partitions the later Files startup with five clock samples around
FXApp init/create/run entry. Six original cold-launch/RTT runs pass, with real
wrapper coverage verified. Init takes288--301ms, intervening main work293ms,
create581--591ms (about26.5% of observed launch), then154--164ms to ping. This
is distributed inclusive elapsed time, NOT exclusive client CPU or IPC.
Loaded-control/read means differ+1.26/+0.50% from none; only two boots per mode,
not exact tax or zero-overhead proof. Next separate client work from waiting
inside create before tuning. No production/Kernel change; two diagnostic
images and one initramfs removed,118/107 retained. See the
[cycle153 evidence](optimization-cycles.md#cycle-153-files-startup-is-distributed-and-window-creation-is-the-largest-coarse-body).

Cycle152 measures first post-relocation audit consistency to pre-main with a
dependency-free GNU audit module that requests no binding/PLT callbacks.
An official-Linux fixture detects a known150ms constructor; native/model
sanitizer checks pass. Five real Files boots preserve all original gates.
Post-linker cleanup/constructors/startup take34--35ms, about1.6% of observed
launch: do not tune this small phase. Pre-main to guest-observed ping takes
1.329--1.339s, a larger NEXT measurement scope, not exclusive main CPU or proof
of an IPC/Kernel bottleneck. Loaded-control/read means differ+0.21/-2.14% from
none, not an optimization or zero-overhead proof with so few observations.
No production/Kernel changes; two diagnostic images and one initramfs removed,
accepted118/107 preserved. See the
[cycle152 evidence](optimization-cycles.md#cycle-152-post-linker-startup-is-small-and-the-later-inclusive-scope-is-large).

Cycle151 fixes lost measurement records: the native GUI fixture previously
printed only the last nonempty line per poll. It now forwards every newly
completed appended line without replaying unchanged content;11 native tests
and four real Files runs pass. Stock glibc2.42 loader statistics, calibrated
after all original RTT gates, measure about40--41ms, only1.88--1.93% of the
observed launch. Library loading is93% of that small loader scope, NOT93% of
application startup. No loader/runtime optimization follows. Kernel unchanged;
four temporary media removed, accepted118/107 retained. See the
[cycle151 evidence](optimization-cycles.md#cycle-151-lossless-measurement-records-and-a-small-runtime-linker-startup-span).

The log-bridge regression can be run with `cargo test --manifest-path
tests/native-performance/Cargo.toml --offline --test appended-lines --
--test-threads=1` (as one command).

Frozen118/107 images retain the old native fixture. Multi-line diagnostics in
a private clone must install the rebuilt `negoto-smoke` as well as any observer;
rebuilding only a host runner does not update a rootfs-loaded test executable.

Cycle150 validates an external host-stack sampler before considering its
observations. Four original Files runs pass; requested2s/7ms sampling has
mean launch2.192s versus2.111s controls (+3.82%). This is not zero-overhead or
statistical proof. Both reports include the post-ping screenshot, so reject
whole-launch attribution despite the sub5% observed mean difference. CPU2's
233 observations per report are distributed across host waits, MMU/TB work,
locks and unresolved JIT code; none identifies a dominating guest component.
No production/Kernel changes. Two temporary media deleted after hashes,
accepted118/107 retained. See the
[cycle150 evidence](optimization-cycles.md#cycle-150-coarse-host-stack-sampling-does-not-isolate-the-launch-critical-path).

Cycle149 runs the same frozen Tiny Core Files/Xfbdev/FLWM/wbar binaries and
observer under both normal Nanami and an official Linux kernel. Four fresh
boots in N/L/L/N order pass the original launch/ping/raw/Xlib gates. Mean
click-to-observed-ping is2.104s on Nanami and0.668s on Linux, a3.152x whole-OS
ratio, NOT an optimization gain or component attribution. Linux's primary
800x600 framebuffer, PS/2 input and ext4-backed disk access differ from
Nanami's nested display, USB input and user-level storage stack. Only two
observations per OS, with100ms relay/10ms host polling. These are not full
paint/input-ready measurements or a universal QEMU floor. No runtime tuning
follows. Failed setup pilots remain excluded; all temporary media/initramfs
deleted after hashes, accepted118/107 retained. See the
[cycle149 evidence](optimization-cycles.md#cycle-149-same-application-linux-gui-reference-shows-a-whole-os-gap-not-a-bottleneck).

Cycle148 rejects fine-grained host boundary tracing as a basis for five-percent
optimization decisions. Across ten original Files runs, both a register-aware
prototype and a no-register coarse revision show mean measured launch times
about5.4/5.3% above their own no-plugin controls. These small samples do not
establish exact overhead, but low perturbation is unproven. Coarse IRET targets
remain explicitly unknown. No Kernel/runtime optimization follows from these
biased profiles. All original gates pass, two diagnostic media deleted,
118/107 retained. See the
[cycle148 evidence](optimization-cycles.md#cycle-148-cpu-boundary-tracing-perturbs-launch-at-the-scale-of-target-gains).

Cycle147 uses two passive host CPU snapshots around the original Files
click-to-ping workload, without guest hooks or stopping QEMU. Five same-image
runs take2.101--2.162s. Two counter runs consume2.357/2.327s across four TCG
threads, averaging1.095/1.106 host cores; CPU2 occupancy is67.04/67.26%.
CPU2 also hosts Files, Alter and POSIX: these counters do not distinguish
their execution, guest Kernel time, or the cause of unscheduled time.
The counter window extends only3.513/3.538ms beyond the original scope;
controls show no large perturbation, not proof of zero overhead.
No runtime tuning follows. Kernel unchanged/unbuilt, two diagnostic media
deleted,118/107 retained. See the
[cycle147 evidence](optimization-cycles.md#cycle-147-passive-host-cpu-time-is-not-component-attribution).

Cycle146 records the whole Alpha actor, not another selected helper. Repeated
Files traces place399.843/390.750ms in all Alpha handler bodies during the
launch interval, only18.24/17.66% of clone-to-ping. Actual Files-owned native18
requests account for338.402/329.129ms; other guests/pump/native clients remain
separate. These are wall spans, not exclusive CPU or pure IPC. Preserve the
first unusably interleaved output; revised post-timing open then mmap barriers
serialize the dumps without new protocol. No runtime optimization follows.
Both sources and normal Alpha/init restored; all temporary media deleted,
118/107 retained. See the
[cycle146 evidence](optimization-cycles.md#cycle-146-all-alpha-request-bodies-cover-only-about-eighteen-percent-of-launch).

Cycle145 adds VFS-side attribution and repeats the actual Files workload.
Matched VFS bodies take309.415/311.907ms, about14% of clone-to-ping; selected
block-device waits147.210/145.023ms are only6--7% of that whole interval.
Residual is not exclusive CPU. Strictly demultiplex one complete VFS dump
that interrupted an Alter line; preserve the raw parse failure. Separate50
direct mapping/statfs requests that were outside144's selected gateway hooks,
requiring their exact ABI rather than relaxing time/count bounds. All original
tests pass; no optimization follows. Five source snapshots/archive and normal
binaries restored, four media removed,118/107 retained. See the
[cycle145 evidence](optimization-cycles.md#cycle-145-vfs-service-and-backend-spans-are-not-the-overwhelming-launch-cost).

Cycle144 correlates POSIX provider records with Files' actual serialized
Alter intervals, rather than assigning Alter's native badge to a Linux PID.
Selected VFS calls account for401.337/375.924ms out of442.453/416.356ms in
matched POSIX handlers. That is about91% of those handlers, but only17--18%
of clone-to-ping, and includes backend/transport/scheduling/probe time.
Neither VFS CPU nor one overwhelming whole-launch bottleneck is established.
No optimization follows. Restore all26 source snapshots and normal binaries,
remove four diagnostic media, retain118/107. The current normal POSIX binary
differs from the retained seed, so these are not cross-cycle gain comparisons.
See the [cycle144 evidence](optimization-cycles.md#cycle-144-posix-provider-time-mostly-lies-inside-vfs-calls-not-one-proven-whole-launch-bottleneck).

Cycle143 covers the missing namespace gateway. Repeated complete Files traces
place142.262/137.172ms inside posix_path_request and only7.871/6.702ms in the
outer helper. The gateway is95% of that helper, but the entire helper is only
6.14/6.64% of clone-to-ping. Do not tune outer string/descriptor staging or
call the gateway pure IPC/provider CPU. Preserve the slower first profile.
Restore all12 files and normal953f; delete four media, retain118/107. Next
correlate actual caller intervals with service-side processing/backend waits,
without treating Alter's native badge as a Linux guest PID. See the
[cycle143 evidence](optimization-cycles.md#cycle-143-namespace-staging-is-small-gateway-time-is-not-yet-provider-cpu-time).

Cycle142 partitions selected helper/provider spans inside Files' syscall
bodies. Two traces show copyin/out195--199ms, selected POSIX313--342ms and
selected standalone Alpha235--236ms, with312--319ms still residual. Nested
Alpha105--108ms is already inside copy spans. Source review identifies an
unmeasured namespace::request_path / posix_path_request boundary; residual
is not pure local CPU and selected POSIX is not all provider traffic. No
tuning follows. Restore all11 files and normal953f, delete four media, retain
118/107. Next time that actual path-resolution boundary before deciding on a
cache/transport change. See the
[cycle142 evidence](optimization-cycles.md#cycle-142-selected-guest-memory-and-provider-spans-do-not-identify-one-dominant-helper).

Cycle141 expands timing to all Alter actor events. Two complete traces
partition clone-to-ping2.023/2.020s into dispatch57.29/57.67%, receive38.58/
38.29%, readiness/signal pump2.20% and between-probe bookkeeping. Files'
syscalls account for50.37/50.90%; Xfbdev's syscall bodies are only2.67%.
These are inclusive wall spans, not CPU shares or pure IPC cost. Preserve
the first driver's130 exit caused by fixed-width log-label truncation and
the unchanged complete raw trace; corrected predicates and repeated run pass.
No tuning follows yet. Restore sole main.rs and normal953f, remove four media,
retain118/107. Next measure common frontend/provider spans within Files'
dispatches. See the
[cycle141 evidence](optimization-cycles.md#cycle-141-all-alter-events-account-for-the-launch-interval).

Cycle140 measures Alpha's initialized-page constructor without self IPC or
Kernel changes. Two complete Files profiles record241.478/237.242ms across
378 calls, split among allocation52ms, destination aliases64ms, source alias/
initialization69--71ms and target publication38--41ms. That is about11% of
first-constructor-to-ping, with non-negligible probe overhead, not a dominant
whole-launch cause. No tuning follows. Restore all three diagnostic files and
exact normal Alpha96bc/initba776, remove four media, retain118/107. Next
broaden critical-path attribution instead of subdividing a small fraction
indefinitely. See the
[cycle140 evidence](optimization-cycles.md#cycle-140-alpha-initialized-placement-is-distributed-not-a-dominant-launch-cause).

Cycle139 splits actual private file mmap into disjoint wall phases. Repeated
profiles place237.698/242.595ms in placement/stream initialization, about63%
of the143 completely marked private mappings but only11% of clone-to-ping.
Source preparation is61.933/63.950ms and protection34.215/33.816ms. The
2.883--3.462ms copy subtotal covers only Alter's snapshot memcpy, not Alpha's
page initialization. The old74.496ms pcre2 spike does not recur; do not invent
its cause or an11x gain. No tuning follows yet. Restore all seven files and
normal953f, delete four diagnostic media, retain118/107. Next measure the
manager-side placement work with a calibrated non-recursive clock. See the
[cycle139 evidence](optimization-cycles.md#cycle-139-private-file-mmap-phase-time-placement-is-largest-but-not-the-whole-launch).

Cycle138 associates existing, already-copied path bytes and syscall results
with selected dispatch wall spans. Two actual Files runs pass without lost
records. Shared-object-related dispatch takes558.672/433.343ms; font paths
101.560/105.297ms, and all1,228 config writes total only176.129/165.534ms.
Do not tune fonts or buffering from counts. One identical pcre2 mmap takes
74.496/6.598ms with no code change; this unexplained variation narrows the
next measurement to file-source preparation versus mapping/protection, not
an optimization claim or a proven storage/cache/IPC cause. Restore all four
temporary files and normal953f; retain only118/107 media. See the
[cycle138 evidence](optimization-cycles.md#cycle-138-startup-path-identities-and-a-variable-mmap-not-a-speedup).

Cycle137 measures selected Alter handler wall spans during real Files startup.
Three profiles cover parent clone to first ping in 2.286/2.277/2.447s. The
actual creating clone takes 43.597/46.507/58.541ms; the largest child syscall
group, mmap, takes 413.394/435.610/434.435ms (17.75--19.13% of that interval).
Another 1.037--1.125s lies outside the selected handler union. These gaps mix
guest execution, other processes, waits, transport and probe overhead; neither
the gaps nor the handler spans are exclusive CPU time. No single dominant
cause is established, so no tuning follows. Preserve the third run's original
clock-line parsing failure and explicit whitespace-only recovery. Restore
the sole temporary production file and exact normal Alter ELF; remove all
four diagnostic media and retain118/107. See the
[cycle137 evidence](optimization-cycles.md#cycle-137-selected-alter-handler-time-does-not-establish-a-dominant-launch-cause).

Cycle136 obtains repeated coarse user-level time evidence without VM stops.
From actual native input receipt to the observer's ping timestamp, Honoka
spends90.29%/90.35% inside receive/reply-receive; fbdev capture is5.98%/6.02%,
composition2.60%/2.45%, present0.74%/0.66%. This is inclusive wall time, not
CPU accounting or pure IPC cost. The compositor mostly waits for events; its
large execution-count weights did not identify a dominant launch-time cause.
Do not tune it from those weights. Restore all four temporary files and exact
normal ELF, retain raw records/failures, and next trace other-process startup
and observer discovery. See the
[cycle136 evidence](optimization-cycles.md#cycle-136-coarse-user-level-time-rules-out-honoka-as-the-dominant-launch-phase).

Cycle135 rejects repeated-QMP-stop residency sampling for the launch workload.
Both ordinary controls pass at2.269/2.226s click-to-ping, but the diagnostic
hits its512-sample budget, spends2.879s stopped/inspecting in9.675s, and never
observes Files. The observer exits9; the timeout screenshot still shows the
desktop and wbar, not desktop termination. The cause is unresolved. Preserve
the failure, withdraw the sampler/QMP changes exactly, and retain only small
evidence. No time-share attribution or optimization follows from this run.
Next use coarse, bounded user-level timing without repetitive VM stops. See
the [cycle135 evidence](optimization-cycles.md#cycle-135-random-time-residency-sampler-rejected).

Cycle134 adds host-only QEMU TCG diagnostic tooling. The controlled Files run
completes, and exact whole-block byte matching attributes large execution
weights to Honoka's fbdev capture and color blending. These are not CPU-time
percentages: metadata is bounded and translation weights are not retired
instructions or critical-path latency. No dominant time bottleneck or speedup
is established, and no production optimization is made. See the
[cycle134 evidence](optimization-cycles.md#cycle-134-bounded-launch-execution-diagnosis-not-time-attribution).

The optional diagnostic sources are `tcg-profile.c` / `tcg-profile-test.c`
(installed QEMU 11 API 7 and GLib headers) and `tcg_profile.py` /
`test_tcg_profile.py` (Python standard library). The host plugin uses bounded
per-vCPU inline counters and a private control socket. Its controller requires
QMP stop acknowledgement and confirmed paused status before every snapshot.
It is not enabled by the ordinary runner, and instrumented timings must not
be compared with ordinary timings. Native API-model tests also run with
ASan/UBSan; the cycle record preserves exact commands and a real VM validation.

Cycle133 rejects direct final-attribute publication in fresh private forks.
Despite fewer target-page calls and21 successful normal VMs, Files launch rises
7.49% pooled and fork gains do not repeat across cohorts. Restore all eight
prototype files exactly, then isolate only constructor uncertainty/default-
permission correctness fixes and four actual-handler regressions. Three fail
against the old constructor; final native1,377/17 ignored, selected ASan224/5
ignored, x86/ARM checks and three final safety boots pass. Remove all four
temporary media; retain118/107 and small evidence. No speedup or runtime image
promotion is claimed. See the
[cycle133 evidence](optimization-cycles.md#cycle-133-unpublished-fork-final-permissions-rejected-constructor-safety-retained).

Cycle132 retains a user-level safety prerequisite: failed MAP/UNMAP can leave
an uncertain PTE after a fallible SMP shootdown. Track that state separately
from permissions and require acknowledged cleanup before backing reuse. Three
actual-handler regressions fail before the fix; native1,373/17 ignored,
selected ASan220/5 ignored, x86/ARM checks and all21 normal VM gates pass.
Performance is not equivalent: opposite launch cohorts disagree, one candidate
boot has a large RTT increase, and three cleanup submeasurements regress in
both independent controls. Retain the correctness fix in source without
promoting a runtime image or claiming a speedup. Delete the three comparison
media; accepted118/fallback107 remain. See the
[cycle132 evidence](optimization-cycles.md#cycle-132-uncertain-pte-accounting-safety-prerequisite).

Cycle131 does not adopt range-preserving page leases. Twelve new regression
tests, native1,370/17 ignored, selected ASan113/2 ignored and x86/ARM checks
pass, but the uninterrupted launch cohort is1.29% slower. Manual review
interrupts the second cohort; after explicit resume, run only the previously
unrun final sample, preserving the multi-hour gap and original interrupted
driver status. Favorable pooled launch numbers are descriptive, not repeated
improvement proof. Restore all five authored files and exact normal ELF;
native1,358/17 and selected ASan101/2 pass. Delete the four comparison media;
retain accepted118/fallback107 and small evidence. See the
[cycle131 evidence](optimization-cycles.md#cycle-131-range-preserving-private-page-leases-not-adopted).

Cycle130 rejects bounded readiness writeback after ten successful VM runs.
Although the executed pre-scan 616-byte copy is removed, Files click-to-ping
increases 1.02% pooled and raw X11 elapsed time increases 8.28%; the smaller
copy is not a demonstrated system gain. Restore all three authored files,
the exact normal ELF, 1,358 native tests and 133 selected ASan tests. Delete
all four comparison image/rootfs files after terminal/open-file/hash checks;
accepted118/fallback107 remain. See the
[cycle130 evidence](optimization-cycles.md#cycle-130-bounded-readiness-writeback-rejected).

Cycle129 adds optional, bounded observer failure evidence and full-loop mocked
tests. All36 Tiny Core host tests plus native ASan/UBSan pass; default RTT and
no-RTT observer ELFs remain byte-identical. Four diagnostic ABBA boots complete
without reproducing128's failure, which remains unresolved, not cured or
replaced by these results. Diagnostic timings are excluded from performance
claims; no runtime optimization is adopted. Four temporary image/rootfs files
are deleted; accepted118/fallback107 remain unchanged. See the
[cycle129 evidence](optimization-cycles.md#cycle-129-bounded-observer-failure-evidence).

Cycle128 finds an actual signal-pump copy before empty/exited-task checks:
generated code loses a 560-byte pre-check copy and stack use falls 1816->824
bytes. Three new regression tests pass, along with 1,361 native and 99 ASan
tests, but the runtime comparison remains inconclusive: after two mapping
pilots and four complete launch boots, the fifth launch observer exits9
without a completion marker despite a painted Files window. Preserve this
failure rather than replacing it or advertising the favorable first cohort.
Both source files and normal ELF are restored; all four temporary image/rootfs
files are deleted. Accepted118/fallback107 remain unchanged. See the
[cycle128 evidence](optimization-cycles.md#cycle-128-signal-pump-snapshot-removal-inconclusive).

Cycle127 rejects the unflagged, local POSIX-buffer adapter after 18 successful
VM runs. Files click-to-ping improves 3.73% across eight launch boots, but raw
X11 elapsed time increases 5.97% pooled; the reversed cohorts and independent
protocol/process controls remain mixed, with repeated slower native cleanup
submeasurements. This is not a demonstrated overall gain or a causal diagnosis.
The candidate's seven adapter tests pass, then all authored production changes
are restored exactly; the original ELF, 1,358 native tests and 80 ASan tests are
verified again. All four comparison image/rootfs files are deleted, preserving
only accepted118/fallback107 and small evidence. See the
[cycle127 evidence](optimization-cycles.md#cycle-127-unflagged-local-posix-buffer-adapter-rejected).

Cycle126 keeps the original2s response workload and adds an explicit10s
observer settle for16 interleaved/reversed, uninstrumented policy-comparison
boots. Cold launch improves in all four comparisons (pooled4.04%), but this is
not an enabled production policy or a5%/100ms result. The prior raw penalty
does not reproduce at2s; waiting10s does not uniformly improve response.
Default RTT and no-RTT observer ELFs stay byte-identical; the two sleep-constant
bytes are the only early/late ELF differences. All eight temporary disk/rootfs
files are deleted; only accepted118/107 and small evidence remain. See the
[cycle126 evidence](optimization-cycles.md#cycle-126-original-and-later-post-startup-response-controls).

Cycle125 uses temporary phase counters in four same-ELF diagnostic boots.
Every raw phase has zero software-lease misses, evictions and invalidations;
the five POSIX file-copy adapter sites are inactive in both raw/Xlib phases.
One Xlib phase has one successful first write-page grant. These counters do
not identify124's latency cause or establish a performance gain. Restore the
normal sources/ELF and delete all four diagnostic image/rootfs files; keep
only validated118 and fallback107 plus small evidence. See the
[cycle125 evidence](optimization-cycles.md#cycle-125-rawxlib-software-copy-cache-phase-counters).

Cycle124 compares the two POSIX I/O policies in one identical Alter ELF,
selected before the Tiny Core root context. Complete disks differ by one env
byte only. Two reversed cohorts improve Files click-to-ping by5.32%/6.82%,
but raw X11 medians regress5.50%/12.17%; Xlib's earlier regression does not
reproduce. No runtime policy is retained. Eight ordinary boots and a separate
default-mode mapping pilot finish0. Production sources/ELF are restored and
all four comparison image/rootfs files are deleted; keep validated118 and
fallback107 plus small evidence. See the
[cycle124 evidence](optimization-cycles.md#cycle-124-same-elf-early-posix-io-policy-diagnostic-only).

Cycle123 rejects a local typed POSIX-buffer adapter as well: Files launch
improves4.73%, but raw/Xlib128-query medians regress5.99%/9.08%. Five actual
VMs pass. Six common helper bodies match under strict symbolic instruction
comparison; their inlining loss is not demonstrated. Restore all prototype
files and the exact normal ELF, and delete all four comparison image/rootfs
files. A one-ELF runtime-policy comparison is the next diagnostic to audit,
not a verified optimization. See the
[cycle123 evidence](optimization-cycles.md#cycle-123-local-typed-posix-buffers-and-generated-code-rejected).

Cycle122 finds missing small-copy reuse in POSIX file I/O, but rejects global
workspace widening. V1 Files launch improves6.62%/4.19% in two reversed cohorts,
while Xlib round trips regress6.75%/5.41%; a reordered V2 improves launch only
0.79% and still regresses Xlib6.45%. Fourteen actual VMs pass. Restore all six
prototype/profiler files and the exact normal Alter ELF; retain source/ELF/log
evidence, not the eight generated image/rootfs files. The next local typed-I/O
boundary remains a hypothesis, not a performance claim. See the
[cycle122 evidence](optimization-cycles.md#cycle-122-posix-workspace-copy-coverage-rejected).

Cycle121 rejects manager-owned pool Frames for sealed private mmap pages.
Seven actual VM runs pass, including mapping/protection and read-only-fork
isolation, but ordinary ABBA Files click-to-ping rises from2167.036ms to
2227.844ms (+2.81%). The lower raw X11 median is not a launch gain.
Restore all ten prototype source/test files and the exact pre-cycle Alpha/init
ELFs; keep the rejected patch and small evidence, not its image/rootfs files.
The five diagnostic/comparison files are deleted; validated118 and fallback107
remain. See the [cycle121 evidence](optimization-cycles.md#cycle-121-sealed-private-pool-frames-rejected).

Cycle120 separates post-startup X11 request/reply work from the first-window
ping. The test-only observer's `APP_OBSERVER_STEADY_RTT` build compares raw
QueryPointer with the same request through unchanged Xlib, after a two-second
settle. Default observer builds remain byte-identical. Four ordinary ABBA boots
give128-request medians29.868/61.854ms for Nanami raw/Xlib and10.625/15.470ms
for ordinary Linux. A separate client-filtered diagnostic counts approximately
two versus six client syscalls per request; it is not a pure IPC measurement,
launch-speed improvement, or100ms full-readiness result. Framing/partial-I/O
host tests are included in the Tiny Core test suite. All diagnostic image pairs
and generated initrds are deleted; only validated118 and fallback107 remain.
See the [cycle120 evidence](optimization-cycles.md#cycle-120-separate-raw-and-xlib-x11-round-trips).

Cycle115 isolates reusable RAM file-cache backing with guest aliases disabled.
Two reversed ordinary comparisons reduce fork/exec medians5.68%/5.60%, but actual
Files launch changes only-0.66% and native Xfe/BusyBox lifecycle controls regress
in both cohorts. Reject this default-policy change and restore the exact control
ELF; preserve all21 metrics, raw samples, cross-backend checks and ten actual
boots, including two GUI sessions. Delete both rejected comparison image/rootfs
pairs after VM closure; retain only114's validated default-off full-alias candidate
and107's accepted fallback. See the
[cycle115 evidence](optimization-cycles.md#cycle-115-isolate-reusable-ram-cache-backing-without-guest-aliases).

Cycle116 rejects a more precise shared-RAM child-cap copy-alias invalidator.
All ten actual boots pass, but Files improves only2.64%, while ordinary fork
and Negoto creation regress in both process cohorts. Restore the exact Alpha
runtime and retain one general child-alias cleanup-failure/retry regression;
restored native1358/17 ignored and serial selected ASan186/2 ignored pass.
No Kernel/IPC or runtime policy change is accepted. See the
[cycle116 evidence](optimization-cycles.md#cycle-116-invalidate-only-retired-child-cap-copy-aliases-rejected)
for all21 controls, artifact identities and deletion of the rejected image pair.

The cycle97 pause is historical: the user subsequently committed the work as
`acaa38a` (Nanami0.7.2) and resumed optimization. Its interrupted ordinary timing
comparison still does not establish a cycle97 speedup. Cycle96 remains the
frozen performance reference for that comparison.

Cycle98 measures actual dock-to-viewable-window latency, not only fork/exec
loops. Build the desktop image with `--with-tests`, then use
`tests/negoto/qemu-smoke.py --tinycore-app-launch-perf files --normal-devices
--smp 4 --image PATH`. The event-driven Xlib observer is test-only. Repeat in
both orders; its VFS/host polling cannot resolve a single small percentage
difference. See the cycle notes for observer overhead, normal images and the
separate AF_UNIX receive comparison. Kernel and IPC fastpath remain unchanged.

Cycles99--100 reuse bounded read-only RAM leases for small syscall inputs,
first in owned scratch and then in typed owned UNIX payload buffers. The latter
uses `Vec::spare_capacity_mut()` without scratch staging or publishing failed
input. Current permission checks, large checked transfers, ordered faults and
transactional socket queues remain intact. Cycle100's matched protocol medians
fall34.50% for plain UNIX and31.35% for recvmsg; unchanged controls include
regressions. Files' coarse event-loop gate improves only3.45% on average and
still takes about2.87s. Neither that gate nor these protocol gains establish
the requested100ms fully painted, input-responsive launch target. See the
[cycle100 evidence](optimization-cycles.md#cycle-100-copy-small-unix-inputs-into-typed-owned-buffers).

Cycle101 rejects range-only read-lease invalidation: a model's129→1 grants
does not translate into a repeatable launch gain, and several ordinary controls
regress. Restore the exact cycle100 executable and retain general protection,
replacement, VM-peer and stack-reset cache regressions. Restored native1292 and
selected ASan122 pass; Kernel/IPC and the accepted runtime remain unchanged.

Cycle102 rejects byte-verified reuse of existing private read-only file pages.
Two ordinary comparisons pass all functional probes, but Files' elapsed mean
changes by+0.28%/+7.92%; avoiding construction work does not establish a launch
gain. Remove the experimental user-level API and restore the exact cycle100
Alpha/init/Alter executables. Restored native1292/17 ignored, ASan172/2 ignored
and ARM checks pass. Delete diagnostic and rejected image/rootfs files after
VM closure; retain only the working baseline and small evidence, not another
large copy of identical accepted bytes. The100ms paint/input goal remains unmet.

Cycle103 isolates the current Xfe launch before choosing another memory change.
One opt-in diagnostic captures7,327 syscalls from the first Xfe syscall to the
observer's nonce-validated ping: time/clock_gettime account for1,479 calls
(20.19% of calls, not latency). Cached file initialization spans only4.36% of
that local-counter interval. Handler spans include descheduling; the residual
is not an IPC-cost estimate. Restore all diagnostic sources and the exact
cycle100 Alter ELF. Next assess an optional timer-owner-provided native counter
reader with fresh RPC fallback, not stale clock caching or guest MMIO authority.
No runtime optimization or new5% gain is claimed for this diagnostic cycle.

Cycle104 adds an optional native timer-owner counter reader. On a running wide
x86 HPET, Alter validates a separate Uncached mapping and the service's clock
epoch/units, then reads fresh hardware for each clock query. Legacy, narrow,
PIT and unsupported providers retain fresh RPC fallback. No guest MMIO,
device-programming authority, cached NOW value, TSC interpolation or Kernel
change is introduced; alarms and notifications remain service-owned.

Both independent ordinary protocol comparisons lower all eight pooled medians;
large-directory enumeration falls6.32% and11.55%. Files' coarse host gate falls
6.71% in the first cohort but is flat (+0.006%) in the reversed default-build
comparison, so its apparent launch gain is not reproducible. The ping interval
also includes the observer's clock-query overhead, not only Xfe responsiveness.
The process controls retain regressions, including copyout128-VMA+5.15%.
Thirty normal-device boots, two raw Linux time-ABI boots, native1307/17 ignored,
selected ASan71/1 ignored and cross-backend checks pass. Retain this measured
clock-path reduction without claiming a uniform5% gain or the100ms paint/input
target. See the [cycle104 evidence](optimization-cycles.md#cycle-104-read-fresh-native-timer-counters-with-service-fallback).

Cycle105 reuses bounded writable private-RAM views for small Linux copyout,
while retaining current permission checks, generation invalidation, owned
sources, ordered faults and the checked-copy fallback. Shared/COW/device
backing is excluded; no writable authority is added to sealed Linux guests.
FreeBSD translation does not issue the new view requests. Read/write windows
share lease management without sharing their native VA ranges.

Both V2 and final default comparisons pass: copyout falls about74--77%, plain
UNIX about56--60%, small-directory enumeration39--45%, and stat38--65% (the
larger stat figure includes an unusually slow before median). Keep all21
process metrics, eight protocol metrics and six frame controls: recvmsg gains
do not repeat, and Negoto creation regresses5.25%/9.18%. Final native1319/17
ignored, selected ASan247/4 ignored, cross-backend checks and both two-session
GUI runs pass. All46 normal-device runs across this cycle's variants pass.
Kernel and IPC fastpath remain unchanged.

Files' coarse host gate falls10.16%/14.23%, but its ping-endpoint screenshots
still show incomplete painting. This is not the100ms fully painted,
input-responsive launch target. See the [cycle105 evidence](optimization-cycles.md#cycle-105-reuse-bounded-writable-private-ram-views-for-checked-copyout)
for every control, raw sample, artifact and retention decision.

Cycle106 fixes a real Honoka notification-coalescing bug: OR-ed scalar IDs
could route two surface updates to a third window. Opaque handles now carry
disjoint live low-bit hints plus a non-wrapping generation. Existing producers
still work, owner/exact-handle control checks remain, and unrelated fbdev
surfaces are not captured. The old real-Compositor pixel regression fails;
native1323/17 ignored, selected ASan46/2 ignored, both architecture checks,
four actual GUI sessions across SMP1/SMP4 and all14 normal-device boots pass.

Keep this as a correctness fix, not a measured5% speed gain. Files' coarse
gate is flat (-0.17%), ping worsens6.43%, and its second after-image screenshot
still has unfinished panels. All process/protocol/frame controls and contrary
evidence remain in the [cycle106 notes](optimization-cycles.md#cycle-106-make-honoka-present-hints-safe-under-notification-coalescing).
The100ms fully painted, input-responsive launch goal is not achieved.

Cycle107 keeps recvmsg payload/result sources typed through a bounded owned
write vector. Prepare/revalidate all leases before any store; an eviction or
failed preparation keeps the original ordered checked batch, field/padding
boundaries and queue-commit behavior. No Kernel/user ABI or guest authority
changes. Native1330/17 ignored, selected ASan101/2 ignored, cross-build checks,
all27 ordinary boots and six actual GUI sessions pass.

Two independent comparisons lower recvmsg56.08%/56.96% and EOF writeback
73.55%/74.23%. General copyout instead regresses about2--5%; plain UNIX is
flat/4.10% slower and repeat Negoto creation8.11% slower. Files' coarse host
gate stays flat (+0.37%/-0.41%), with unfinished ping-endpoint images in both
cohorts. Keep every raw control in the [cycle107 notes](optimization-cycles.md#cycle-107-retain-typed-owned-vectors-through-small-recvmsg-writeback).
This is a repeated communication-path gain, not a uniform system speedup or
the100ms paint/input target.

Cycle108 tests pure cache lookup/effectful installation separation without
compiler attributes. Although generated cache-hit calls disappear, eight
matched boots show normal copyout effectively flat (-0.08%), fragmented
copyout only1.2--1.5% shorter and recvmsg2.44% shorter, with contrary directory
and process controls. Reject the extraction, restore/rebuild the exact107
ELF and remove108's generated images. Keep all measurements in the
[cycle108 notes](optimization-cycles.md#cycle-108-reject-lookupinstall-extraction-after-a-matched-comparison).
No five-percent system gain or completed paint/input launch is established.

Cycle109 adds opt-in launch observation, not a speedup or a new ready gate:

```sh
python3 -B tests/negoto/qemu-smoke.py --image IMAGE --normal-devices --smp 4 \
  --tinycore-app-launch-perf files --app-launch-ready ping --app-launch-timeline
```

The original press-to-ping measurement stays intact; release-command bounds
are additional fields. Post-ping PNGs use actual capture timestamps, followed
by a real keyboard directory-change challenge. Inspect the images: a fixed
wait, stable picture or sent keys must never stand in for a painted/input PASS.
Both observed boots still have incomplete ping frames, populated listings by
the200ms capture, and visible /usr/local navigation after keyboard input.
Temporary framebuffer timing is removed and production rebuilds identically
to107. Ten runner tests and diagnostic-native1330/17 ignored pass. Details and
all twelve inspected frames are in the
[cycle109 notes](optimization-cycles.md#cycle-109-observe-post-ping-paint-and-real-keyboard-response).
The100ms fully painted/input-ready launch goal remains unmet.

The current timeline runner also writes `app-launch-paint-reference.json`
after QEMU has exited and been reaped. `tests/negoto/files_paint.py` strictly
decodes bounded opaque RGB/RGBA PNGs and compares the complete fixed Files
client rectangle with audited `/root` and `/usr/local` fixture hashes. Missing,
malformed, unsupported or different images remain unmatched. Run an existing
capture directory offline with `python3 tests/negoto/files_paint.py RUN_DIRECTORY`.
These are captured-state matches, not a generic GUI oracle or a launch/input
latency PASS. The original ping measurement and unverified readiness fields
remain unchanged. Reference evidence is in
`session-handoff/logs/linux-native-scope-20261106/paint-oracle-audit/`.
The timeline also sends one Ctrl-L chord (`clear_location` in the installed
Xfe configuration), with captures requested at +50/+500ms and actual command
and capture bounds in `app-launch-single-input.json`. Its audited empty-Location
state is a separate exact reference. It runs after the launch samples, so it
cannot establish launch input readiness. The existing multi-key `/usr/local`
challenge follows it; its timing differs from older timeline cohorts. See
`session-handoff/logs/linux-native-scope-20261106/single-input/` for the first
KVM result. A visually normal caret variant can remain unmatched: do not relax
the region/hash or report this as a latency PASS.

Cycle110 rejects pre-generated Fontconfig caches after eight ordinary matched
boots. A version-matched2.16.2/cache-9 generator is reproducible, and the target
loads all97 cached fonts before GUI startup, but Files is effectively flat
(-0.20%), cold desktop startup regresses2.93%, and warm startup regresses0.45%.
Keep the11-tested HMP period-key correction, not a new cache build dependency.
All candidate/diagnostic images and extracted tool trees are removed after
closure; working106/107 pairs and small evidence remain. See the
[cycle110 notes](optimization-cycles.md#cycle-110-reject-pre-generated-fontconfig-caches-after-cold-startup-comparisons).

Cycle111 rules out two more suspected startup costs in one diagnostic Files
launch: all 10,720 observed syscall faults carry inline register context
(zero legacy fallbacks), and post-handler readiness plus signal processing
accounts for about 1.47% of the observed local elapsed interval. This is not
a speedup or an IPC-cost estimate. Remove the temporary instrumentation and
its image/rootfs after recording the raw counters; normal Alter rebuilds
byte-identically to107, and working106/107 remain the only retained pairs.
See the [cycle111 notes](optimization-cycles.md#cycle-111-check-inline-fault-context-and-event-tail-cost).

Cycle112 repeats the detailed Xfe profile on the current accepted code, after
the clock and recvmsg changes. mmap now contributes41.10% of captured dispatch
ticks; cached/streamed page initialization contributes11.57% of the entire
observed interval. Clock calls remain frequent but take only0.55% of that
interval. These are diagnostic elapsed spans, not CPU fractions or a new
speed comparison. Restore all probes exactly and remove their image pair;
see the [cycle112 notes](optimization-cycles.md#cycle-112-refresh-the-current-xfe-workload-and-audit-snapshot-lifetime).

Cycle113 prototypes reclaimable, read-only file-snapshot storage behind the
then-default-off `NANAMI_IMMUTABLE_SNAPSHOTS=1` switch. Publication follows complete
I/O, version validation and read-only protection; eviction retires rather than
overwrites backing. Failed construction/retirement retains ownership without
serving a cache hit. Default and enabled native1338/17 ignored, enabled ASan175/2
ignored, cross-backend checks and four QEMU boots pass. No guest private aliases
or copy elision are connected yet, and no speedup is claimed. Delete the
validation image/rootfs after closure; accepted106/107 remain the working pairs.
See the [cycle113 notes](optimization-cycles.md#cycle-113-prototype-reclaimable-read-only-snapshot-storage).

Cycle114 connects experimental R/RX private file aliases to those RAM snapshots,
with unique-extent/page accounting, explicit RAM reference units, detach-before-
write and whole-object release validation. It was default-off at that checkpoint. Final native
1357/17 ignored, selected ASan191/2 ignored, cross-backend checks and all34 QEMU
boots across three variants pass. Final Files timing improves only0.79% with
10ms host observation; earlier cohorts improve4.31--6.72%, while the unchanged
200ms observer is flat/slightly worse. Do not claim a repeatable5% launch win.
Fork/exec improves about27%, but fragmented-fork controls retain regressions.
Keep the final validated experimental image and107's stable fallback, deleting
the two-generations-old106 pair and all superseded comparison/diagnostic images.
The next comparison isolates RAM backing from aliasing: Alpha's permanent-pool
copy-window ownership differs from PID-scoped child aliases. This is a code-
based lead, not an isolated causal result. See the
[cycle114 notes](optimization-cycles.md#cycle-114-connect-private-file-snapshot-aliases-and-isolate-their-cost).

Cycle96 enables opt-in private read-only fork sharing for sealed x86 Linux
guests. Legacy Native/FreeBSD forks and non-x86 backends retain eager copying.
RW and PROT_NONE pages still copy. A VM-wide supervisor barrier protects fork
and detachment without replaying handled waits; normal checked copyout adds no
prepare RPC. The real SMP1 syscall-free-thread regression also corrects the
CLONE_VM priority20/ordinary-task priority4 mismatch: both now use priority4.

Final native1255, core/quiescence ASan145, ARM checks, SMP1/SMP4 isolation and
source-first-reap tests, and two complete GUI sessions pass. Three real pairs
per SMP mode save exactly4MiB for a4MiB RO fork extent. Both independent12-boot
ordinary comparisons pass all30 metrics and six controls: fork5.16% /13.03%
faster, fragmented128-VMA fork10.42% /17.00%, startup6.58% /4.35%. Negoto-create
does not repeat its gain and copyout-normal regresses2.09% /0.43%. Full results
and failed untimed attempts remain in the cycle notes; this is neither a uniform
5% system improvement nor writable-page COW. Kernel/IPC/FPU are unchanged.

The production sharing/detach implementation is included by `core-process`;
`linux-private-memory` includes the real quiescence guard. The functional RAM
probe checks actual allocation savings, proxy-write/protect rejection, detached
byte isolation and a child surviving source-root reap:

```sh
python3 -B tests/negoto/qemu-smoke.py --image PATH_TO_TEST_IMAGE \
  --private-readonly-fork --normal-devices --smp 4
```

Repeat with `--smp 1` and require the `source-reap=true` footer from the latest
functional fixture. This untimed probe is not substituted into frozen ordinary
timing rootfs images. See `tests/alter-linux/fork-readonly.c` for the complementary
Linux write-upgrade, PROT_NONE, descendant and parked/running CLONE_VM checks.

Cycle95 fixes last-owner RAM retirement when a process holds several aliases.
The old real-image regression loses exactly4MiB over four retirements; final
SMP1/SMP4 runs reclaim it, and two complete GUI sessions pass. Native1239,
core ASan129, ARM and all24 ordinary ABBA/BAAB boots pass. Fork improves
6.20% / 12.05% and fragmented128-VMA fork8.82% / 12.26%, but startup does not
repeat its improvement and some Alpha lifecycle/copyout results regress.
Keep all30 metrics and six controls: retain the correctness fix without claiming
a uniform5% system gain, enabled RO sharing or COW. Kernel/IPC remain unchanged.

Cycle94 rejects direct final-permission construction for fresh private forks.
Both independent ordinary comparisons pass all 30 metrics and six controls,
but ordinary fork is about 14% slower and Negoto creation also regresses.
Archive the prototype and restore all seven source/test files plus the exact
cycle93 Alpha/init/Alter bytes. Restored native tests and three real Tiny Core
startup/stop/reuse sessions pass; no optimization from this experiment ships.

Cycle93 repairs active/deferred RAM ownership and rejects an initial contiguous
index that exceeded Alpha's fixed32MiB heap during the full GUI workflow. The
retained partitioned index passes native1227, core ASan117, ARM checks, real
SMP1/SMP4 page-reuse and capability-isolation tests, and two complete GUI
sessions. Twelve ordinary boots pass all30 metrics and six frame controls.
Performance is mixed, including slower startup/constructor controls: retain
this as a correctness prerequisite, not a claimed5% speed win or enabled COW.

Physical-allocation regressions count active and deferred owners together.
They cover reuse, peer release/reap, exec, two deferred owners and an independent
mixed-operation model. Moving an active record to deferred must preserve its
reference count and reuse the same metadata node. An untimed QEMU regression
checks actual peer data through eight release/remap cycles:

```sh
python3 -B tests/negoto/qemu-smoke.py --image PATH_TO_TEST_IMAGE \
  --ram-sharing-reuse --normal-devices --smp 4
```

The Tiny Core builder's `--with-tests` option installs the native
`/alter/linux/bin/ram-share-smoke` fixture. Repeat with `--smp 1`; this is a
RAM-lifetime correctness check, not a performance or COW benchmark.

The final-owner alias regression uses the same fixture:

```sh
python3 -B tests/negoto/qemu-smoke.py --image PATH_TO_TEST_IMAGE \
  --ram-alias-retirement --normal-devices --smp 4
```

It gives a suspended peer two aliases to one MiB of RAM, retires the original
owner, then kills/reaps the final peer. After two warmups, four retirements must
not leak that RAM; a 256KiB allowance covers small late metadata growth. The
old snapshot leaks exactly four MiB. Repeat with `--smp 1`; this untimed test
neither enables COW nor establishes a system speedup. Native tests also cover
active/deferred aliases, registered peer owners, shared-VM owner isolation, all local
capability removals and a failed retirement after a successfully freed prefix.

Cycle92 implements the user-approved Linux capability boundary without Kernel
changes. Alpha retains each management root while guests execute with one
shared empty capability root; fork/shared-VM children inherit it and exec keeps
it. Native1217, core ASan110, ARM checks and builder32 tests pass. A real native
CapabilityCall probe fails on the old image and passes on sealed SMP1/SMP4
guests through fork/shared-VM/exec. Full ordinary comparisons are recorded in
the cycle notes. This is a prerequisite for safe sharing, not COW or a claimed
speedup by itself; native and ordinary FreeBSD launch behavior is retained.

Cycle91 tests the existing priority4 placement preference on CPU2 against
accepted87 in twelve normal-device ABBA boots. All30 metrics and six controls
pass, but startup is+4.16%, ordinary fork only-3.42%, and several controls are
slower. Reject this broad worker pinning; the shipped scheduling policy is
restored. A2.7x isolated cross-core getpid difference is not a whole-system gain.

Cycle90 removes two actual184-byte context copies using borrowed owned-event
snapshots and borrowed pending replies, without changing fault payloads, ABI or
Kernel code. The normal four-boot getpid control gives only-0.43% on the same
core and+0.84%/+0.97% across cores, so reject the candidate and restore the
exact normal Linux/FreeBSD ELFs. Native1212, ASan30 and both ARM checks pass.
Do not repeat byte-copy tuning as the missing order-of-magnitude improvement.
Its sharing-authority question was subsequently explained and explicitly
approved by the user; cycle92 records that separate implementation.

Cycle89 audits a major measurement boundary rather than retaining another
small runtime prototype. A calibrated integer-only getpid diagnostic finds
about19.3us per same-core round trip versus52.0--52.3us across cores (Alter
on CPU2), while dispatch spans average about2us/2.6us. The existing handler
profiler excludes the surrounding fault/receive/reply/resume path. Keep this
as an instrumented placement diagnostic, not an isolated IPI measurement,
proof of a QEMU defect, real-hardware result or10x application speedup.
The host is arm64 and this x86_64 QEMU binary supports only TCG. Restore the
exact normal Alter ELF after profiling; Kernel/Alpha/user-rootfs and accepted
cycle87 remain unchanged. Old-image preparation has not yet produced a valid
matched cumulative comparison.

Cycle88 rejects a one-pass physical-RAM/reference retirement prototype after
24 ordinary boots and four fixed-gap control boots. Fragmented-128/256-VMA
fork and Negoto creation improve by at least5% in both ordinary comparisons,
but the pure Alpha constructor/reaper loop is +3.86%/+5.76%, and the independent
137ms-gap control is +3.14%. Keep all30 metrics and six controls, including the
favorable results; less bookkeeping is not a uniform performance win. Archive
the six new ownership/retry tests with the candidate, restore the exact cycle87
Alpha/init bytes, and retain no new runtime API. Candidate native1218 / core
ASan111 and restored native1212 / core ASan105 plus ARM checks pass. Kernel,
IPC, Honoka, Alter and user input images stay unchanged; cycle87 remains the
accepted baseline.

Cycle87 accepts two compact synchronous creation-log literals, keeping every
event and PID/image/CPU/priority field with no new service, queue or API.
All 24 ordinary ABBA/independent-BAAB boots pass all 30 metrics and six controls.
Linux fork improves 9.53% / 12.21%, fragmented-256-VMA fork 6.42% / 7.75% and Alpha
spawn 7.49% / 5.40%; other lifecycle/Negoto controls remain mixed, and startup
+4.40% does not repeat (-0.08%). Do not claim a uniform system/boot speedup.
A separate fixed-gap control improves only 1.74%, despite 28% fewer output bytes.
Verify 480 complete creation records plus 824 raw-fork records; native 1,212,
rendering ASan 42, ARM and SMP=1 cleanup pass. Kernel/IPC and user inputs remain
unchanged. The accepted baseline is now the cycle 87 image / Alpha 50ad2578.

Cycle86 implements and rejects a bounded user-level debug-log consumer. It
preserves output, has no drain-time Alpha RPC dependency, and passes native
1226, transport/router ASan13, ARM checks, 384 controlled lifecycles and a
160-child single-core full-buffer fallback. A phase-distributed control is
11.14% faster even through complete drain, but twelve ordinary boots show
Alpha spawn -29.64% followed by reap +184.10%, fork +25.15%, and only -2.60%
for the adjacent eight-child loop / -0.21% startup. Keep all30 metrics and six
controls; a favorable stage is not a system win. Archive the implementation,
restore the exact prior runtime, and next reduce actual byte-wise output work
without suppressing events or adding a process/Kernel API. Restored native1212,
rendering ASan42, ARM and three real startup/cleanup/reuse sessions pass;
Alpha/init/Kernel/Honoka/user-rootfs/accepted-image identities are exact.

Cycle85 tests the smaller, format-independent mask-cache alternative: real
Honoka updates improve17--20%, but the adjacent-burst Alpha control remains
unsettled. Six three-way normal process boots retain every21-metric result.
An eight-boot independent137ms-gap control instead finds about10% lower Alpha
latency for both rendering candidates; a byte-identical rewritten original
is within0.1% pooled. These different workloads do not erase the earlier hot
control results. A separate24-spawn diagnostic finds forced character-at-a-time
logging at35.8% of the measured spawn-function span, not CPU/system time.
Restore source and all normal ELF/init identities, rebuild normally and pass
native1212, rendering ASan42, ARM and three startup/reuse sessions. Both shader
candidates remain unaccepted. Next address the measured logging path without
silencing output, changing Kernel/IPC, or adding unsafe service dependencies.

Cycle84 verifies a Honoka rendering candidate with exact RGB8 rounding and
cached immutable channel masks. Real synchronous window updates improve
24--31% across ABBA and independent BAAB comparisons. Generic formats remain
exact; the framebuffer grows by8bytes. However, the ordinary Alpha-only
constructor/reaper control is +5.62% initially and +10.98% on repeat. This is
not a uniform system speedup: retain all30 normal metrics and six controls,
hold the runtime candidate for a tradeoff decision, and restore exact normal
production/ELF bytes. Keep two general pixel/format regression tests. Native
1212, rendering ASan42, ARM and two real desktop sessions pass. Candidate
source, normal test image and all measurements are retained; Kernel/IPC,
user rootfs and the accepted cycle73 image are unchanged.

Cycle83 broadens read-only QEMU attribution across Kernel/Alpha and eight
full-text-verified native images. Honoka capture/blending is a work hotspot,
but the observer extends runs and increases periodic rendering; weighted TB
shares are not CPU-time or latency shares. A separate low-overhead idle/resume
trace finds core0 halted about36%, core1 about85--87% and core2 about66--71%
in steady session/cleanup intervals, not per-syscall waiting. Three diagnostic
boots and a plugin-free three-session control pass. Production/test sources,
Kernel/IPC, all normal ELF/init/rootfs/image identities and accepted cycle73
remain exact. No speedup is claimed; distinguish rendering and startup gates
and obtain lower-observer timing before selecting another representation.

Cycle82 rejects a smaller allocation-free bootstrap frame hash index after12
normal boots: startup +1.00%, fork +12.06%, BusyBox lifecycle +11.97% and
Negoto create +17.24%, with all30 metrics and six controls retained. A separate
validated CR3/full-text QEMU profile shows about28% fewer Alpha weighted TB
entries, not CPU-time shares, but ordinary application time does not improve.
Restore exact production/ELF/init bytes and the original layout assertion;
keep two general allocation-free key-churn/copy-isolation tests. Restored
native1210, ASan144, ARM and three startup/reuse sessions pass. Record and
repair a private objcopy snapshot rewrite from untouched init CPIO evidence.
Kernel/capability policy and accepted cycle73 stay unchanged. Next broaden
whole-path attribution instead of choosing another Alpha-only metadata layout.

Cycle81 evaluates request-local batch removal of page references without a
permanent index or wider per-page Nodes. Native1214, core ASan108 and ARM
checks pass, as do24 normal comparison boots. Initial fork -12.45% reverses
to +2.07%; independent BusyBox lifecycle is +5.04% and Negoto create +9.47%,
while startup stays +1.11%/+0.17%. Retain all30 metrics and six controls in
both independent comparisons; reject the runtime prototype. Restore exact
production/ELF/init bytes and keep two general ownership/UNMAP retry tests.
Restored native1208, core ASan105, ARM and three startup/reuse checks pass.
Remove only closed owned trial images. Kernel/capability policy and the
accepted cycle73 runtime remain unchanged; no new5% gain is claimed.

Cycle80 distinguishes active/deferred allocation-list searches: failed
many-page requests visit5.21million mostly single-page records in a valid
three-session diagnostic. Evaluate an exact per-owner extent count in both
plain and size-neutral NonZero head layouts. All36 ordinary comparison boots
pass, but the compact independent repeat gives startup +0.06%, fork +15.18%
and no repeat of the initial pure-constructor gain. Preserve favorable
BusyBox lifecycle -7.28% and stat -5.40% too; reject both runtime layouts.
Restore exact production bytes/ELFs and keep two baseline-compatible mixed-
size/reference regressions. Native1206, core ASan103, ARM and three actual
startup/reuse checks pass. Successful one-page unlink scans remain a lead,
not proof of CPU share. Kernel/capability policy/accepted cycle73 are unchanged.

Cycle79 repairs the read-only QEMU register-buffer reuse and obtains valid
CR3/full-text-tagged Alpha execution counts. These are instruction-weighted
TB entries, not CPU-time fractions. Reject an allocation-free static-AVL
height guard:16 ordinary comparison boots reproduce Linux fork +12.24%/
+13.27%; the initial favorable pure constructor -11.12% becomes +4.13% in
the independent process repeat, while startup is only -1.32%.
Keep all30 primary metrics, six frame controls and the21-metric repeat,
restore exact runtime/ELFs and retain three general AVL invariant/oracle
regressions. Restored native1202/ASan128, ARM and three actual startup/reuse
sessions pass. Kernel/capability policy and accepted cycle73 stay unchanged;
no new5% end-to-end gain is claimed.

Cycle78 separates eight private-file mmap intervals without a production
change. Captured initialization dominates the local elapsed spans, while
preflight plus metadata commit is0.37% of the outer mmap aggregate; these
include service waits and are not exclusive CPU shares. A separate host QEMU
plugin produces no valid CR3-tagged counts and is explicitly rejected.
Restore both diagnostic sources and exact normal executables; native1199,
ASan124, ARM checking and three X11-ready/cleanup/reuse sessions pass.
At the user's request remove only the obsolete Tiny Core build/build-desktop
trees and closed owned diagnostic disks, retaining the current build tree,
rootfs, pinned downloads and compressed diagnostic evidence. Kernel and the
accepted cycle73 runtime remain unchanged; no new5% gain is claimed.

Cycle77 rejects eight-page permission metadata blocks: dense host allocation
layouts shrink44.88%, but sparse layouts grow4.42x. Twelve normal comparisons
give startup -1.93%, stock ls +0.58%, fork-exec +1.29% and Negoto create +9.56%;
keep favorable ordinary fork -8.34% too, without selecting it as overall gain.
Restore exact production bytes/ELFs. Retain three baseline-compatible exact-
key, allocation-free removal and mixed-operation regressions plus an ignored
density/drop probe. Restored native1199/ASan124, ARM checking and three actual
startup/reuse sessions pass. Corrected measurements-final.json checks every
existing control metric. Kernel/capability policy and accepted cycle73 runtime
remain unchanged; no new5% end-to-end gain is claimed.

Cycle76 separates15 fork/exec stages over three real diagnostic startup/reuse
sessions: Alpha construction/VM cloning is98.74% of the captured fork spans,
FD inheritance0.57%; exec image replacement65.16%, argv/env capture3.34%.
These include waits, not exclusive CPU time. A separate same-stream cache
replay saves only5.13% of misses with same-capacity4-way, not5% of application
time. No speculative cache/FD change is retained. Verify installed rootfs ELF
identity and padded profile labels before accepting counters. Restore all five
sources exactly; native1196/ASan113, ARM check and three normal startups pass.
Readonly sharing requires a still-pending explicit optional capability-policy
decision; default runtime, Kernel and accepted cycle73 remain unchanged.

Cycle75 rejects both a512-byte index over the existing128-slot name cache
and its combination with cycle74's final error-safe lookup hint. Separate
normal12-boot comparisons give stock ls -3.70%/-2.56%, startup -2.07%/+3.23%,
with all contrary process controls retained. Native1203/1211 and ASan69/77
pass, but neither candidate meets acceptance. Restore exact accepted bytes;
native1196/ASan62, ARM checking and three startup/reuse sessions pass.
IPC source review confirms short register-sized calls and cycle5's existing
one-word Linux replies; no new IPC/Kernel change is justified by that audit.
Remove closed trial disks and unused Tiny Core probe-build caches, not sources,
saved rootfs/boot images or evidence. Accepted cycle73 remains unchanged.

Cycle74 rejects a last-successful-block lookup hint: actual stock-ls record
visits fall about88%, but normal ls/startup improve only2.55%/0.86%, with
contrary process controls. All12 paired boots pass. Restore all source/test
bytes and remove probes; native1196/ASan62, ARM checking and three restored
startup/reuse sessions pass. Phase-separated counts retain about3.3million
name-cache slot checks per ls8 as a new workload lead, not a CPU-share claim.
The accepted cycle73 runtime remains unchanged; no new gain is claimed here.

Cycle73 retains only an existing priority22 CPU2 placement preference, joining
VFS with POSIX/Alter without changing executable bytes or Kernel IPC. Paired
normal runs reduce stock ls7.62%, stat16.54% and enumeration20.59%/23.14%;
startup is only2.99% shorter. Native bulk-read/process controls show tradeoffs,
preserved in full with a separate reversed-order repeat. Native1196/ASan62,
all18 boots, two real GUI sessions and one-CPU fallback pass. See the cycle
record for all timings and limits; this is not a universal system speedup.

Cycle72 rejects a block-local directory restart despite75.56% fewer actual
cached reads: normal small/large enumeration is+4.27%/+8.35%. All12 comparisons
pass, but work counts are not speed evidence. Restore exact production bytes;
retain3general seek/error/oracle tests. Native1196/ASan53 and three restored
startup/reuse sessions pass. A read-only audit finds POSIXcore2/VFScore3 cannot
use the same-core direct-return branch; assess locality without Kernel edits.

Cycle 71 separates filesystem work with 17 existing-sync boundaries. Warm
stat512 performs no block copy/device read; warm stock ls8 still scans about
904k lookup records without device I/O. Small directory scans copy17.7MB
versus1.25MB for large buffers; source review finds read-ahead rollback cannot
reuse Ext2's exact-position-only cursor. These are work counts, not CPU/speed
claims. Remove every probe exactly; native1193 / ASan41 and three normal
startup/cleanup/reuse sessions pass. Normal runtime and Kernel are unchanged.

Cycle 70 evaluates scoped immutable reads of cached Ext2 directory/inode blocks.
Candidate native 1,201 / ASan 49 and all 12 normal comparison boots pass, but
stock BusyBox ls improves only 2.20%, large enumeration worsens 5.90% and
startup worsens 4.27%. Preserve the favorable stat -6.42% result too; do not
select it alone. Reject the candidate and restore all seven files exactly;
restored native 1,193 / ASan 41 and three startup/cleanup/reuse sessions pass.
Kernel and accepted runtime stay unchanged.

Cycle 69 rejects readdir-driven cache prefill: stock BusyBox ls improves only
2.77%, while small/large raw directory scans regress 19.52%/74.13%. All 12
normal comparison boots pass. Restore the exact cycle-68 runtime, retaining
only a real-filesystem unlink/recreate/rename cache-freshness regression.
Restored native 1,193 / ASan 14 and three startup/cleanup/reuse sessions pass.
No new runtime speedup or Kernel change is claimed; full evidence is retained.

Cycle 68 retains capacity-aware directory batches and safe forward dirent
compaction, without new buffers, caches or service operations. Matched normal
Tiny Core runs reduce 64 large-buffer directory scans 552.585 -> 105.824ms
(-80.85%), with the same 474 entries/checksum and 2,560 -> 384 getdents calls.
Small-buffer time is +1.66%, stock BusyBox ls -4.01%, startup -0.60%; do not
claim a whole-system 81% gain. All 12 comparison boots, 1,193 native tests,
14 focused ASan tests, ARM checking and two real GUI sessions pass. Full
results and contrary controls are preserved in the cycle notes.

Cycle 67 measures UNIX GC before considering bulk FD cleanup. Three real
startup/reuse sessions flush 50 collections, 9,792 root slots and no exec-time
collections. Do not add batch-lifecycle complexity for this startup trace.
Restore all probes exactly; native 1,188 / ASan 86 and three normal startup/
cleanup/reuse sessions pass. The accepted cycle 66 runtime is unchanged.

Cycle 66 retains direction-aware UNIX readiness without new retained state:
general state changes still wake every interested waiter, while data arrival
and released capacity wake only matching interests. A nonempty recipient
queue needs no further readable notification. Matched startup diagnostics give
4,409 -> 2,449 scans and 3,600 -> 1,632 unready scans, with comparable ready
counts. These are work counts, not CPU or speed percentages. Normal startup
-1.97% does not establish a 5% gain; all contrary controls/outliers are kept.
Native 1,188 / ASan 86, ARM check, 24 comparison boots across two candidates
and two real GUI sessions pass. Probes are removed; Kernel is unchanged.

Cycle 65 measures actual readiness wakeups in two separate three-session
Tiny Core diagnostics. All observed retries are poll/pselect, not parked
read/write: do not optimize an unobserved direct-retry path. About 67% of
wakeup calls are UNIX-only; 85% of UNIX-only scans remain unready. Audit send
notifications for already-readable queues next, while retaining capacity/rights
release notifications on receive. This is work-count evidence, not CPU share
or a speedup. Remove both probes exactly; all 1,180 native tests (16 ignored),
97 ASan tests and three normal startup/reuse sessions pass. Kernel is unchanged.

Cycle 64 evaluates and rejects borrowed stop checks and a readiness writeback
reduction. Optimized code really loses a PID lookup and a 616-byte pre-scan
copy, but a fresh awake ABBA comparison establishes no 5% gain. Restore exact
cycle62 runtime/ELFs/rootfs; retain four stop/continue/deadline regressions and
make the clock/readiness mocks represent stopped tasks. All 1,180 native tests
(16 ignored), 69 focused ASan tests and three restored startup/reuse sessions
pass. No Kernel change.

Cycle 63 evaluates and rejects a single bounded UNIX payload spare. Real
diagnostics find 96.59% of observed packets <=4KiB; a native 256-round check
reduces allocations 256 -> 0. That is not a speedup claim. Preserve a complete
sleep-contaminated first comparison separately after confirming macOS power
events. A fresh awake reversed-order comparison gives write/read -3.33%,
recvmsg +0.09%, with movement in unchanged controls too. Restore exact runtime
and retain only two payload/PEEK/fault/short-length semantic regressions.
Restored native1,176/16ignored, UNIX ASan28 and three actual startup/reuse
sessions pass. Kernel and accepted cycle62 Alpha stay unchanged.

Cycle 62 retains an eight-region bootstrap table-readiness history (+64 bytes
per Alpha, no extra mapped pages). All three native suspended construction/
reap boot medians beat every baseline median; pooled time falls 7.97%. This is
not Linux program execution. Eighteen normal comparison boots also preserve
contrary fork +4.25%, UNIX write/read +4.20% and no established startup gain,
including one slow recvmsg boot. No whole-system 5% claim is made. Native
1,174 / ASan 55, ARM compile, two GUI sessions and one-core process checks pass.
Kernel, ordinary VmSpace policy, ABI and IPC fastpaths remain unchanged.

Cycle 61 measures file-mmap work in three actual startup/reuse sessions, then
removes its probes completely. Snapshot hits serve 1,624 of 1,783 source
requests; the only 24 bypasses are large-file streams. Partial-page staging
copies total just 4,944 bytes. No region cache or lazy-tail complexity is
adopted. Initialization RPC elapsed ranges include service waits, not exclusive
CPU or IPC cost. Restored native 1,170 / 16 ignored, focused ASan 33 and three
further actual startup/cleanup/reuse sessions pass. Kernel is unchanged.

Cycle 60 proves two namespace reply copies disappear in optimized code, then
rejects the candidate because balanced real startup/stat results do not earn
adoption. It rebuilds non-core applications into rootfs and compares all 3,890
paths, finding exactly the intended two changed ELFs. Three retained
baseline-compatible `linux-path-lookup` regressions check maximum readlink,
request bounds/nonoutput replies and canonical metadata decoding boundaries.
Restored native 1,170 / 16 ignored, focused ASan 15 and three actual startup/
cleanup/reuse sessions pass. No new runtime layout or Kernel change remains.

Cycle 59 separately tests two request-scoped ext2 inode-kind reuse candidates.
Fewer actual modeled inode reads and passing production-source regressions do
not establish a real startup gain: the final normal QEMU screen gives stat
-2.18% and startup +0.02%. Reject both entire candidates and preserve their
source/ELF/measurement archives. Restored native 1,167 / 16 ignored and ASan 48
pass, as do three additional actual startup/cleanup/reuse sessions. The cycle
notes identify existing result-buffer bounces as a new, unimplemented lead;
Kernel remains unchanged.

Cycle 58 evaluates and rejects a bounded ext2 namespace-root binding cache:
native mutation/SHM regressions pass, but balanced real stat gains of 3.31%
do not translate into faster X11-ready startup. The entire candidate is
archived rather than retained as new runtime/test-module complexity. The
restored cycle57 runtime passes 1,167 native tests / 16 ignored and 48 focused
ASan tests, plus three actual startup/cleanup/same-ID-reuse sessions. Kernel
remains unchanged; the cycle notes preserve all contrary observations and a
separate, not-yet-implemented request-scoped inode-reuse lead.

`alter-dynamic` exercises the production PT_INTERP preparation/installation
against an independent zero-span/ordered-file-write model. Six regressions cover
unaligned heads, distinct file offsets, BSS, holes, shared boundary pages,
overlapping/unsorted segments, generated layouts, and failed requests. Its
opt-in work-count probe reports request and zeroing counts, not wall-time gains.
Cycle 33 records why a single-initialization candidate was rejected despite
reducing zero writes: real fork/exec and X11-ready startup did not improve.

`vm-index` checks the production VM page index against an independent model,
including permission-only entries, descriptor replacement, sparse range walks,
and full-word descriptor/permission values. Its opt-in benchmark records both
allocation footprints and timings; source overrides allow matched comparisons.
Additional AVL checks cover 20,000 mixed full-Word-key updates, sorted/reverse
deletion and reuse, empty-tree storage release, cross-thread ownership/shared
reads, and insertion when larger contiguous allocations are deliberately denied.
The ignored churn probe reports partially populated retention as well as dense
construction. Cycle 34 records two rejected pooling representations; fewer
allocator calls alone did not improve real fork/exec or desktop startup.

The Tiny Core fbdev, AF_UNIX and repeated process-lookup follow-up is documented
in [Tiny Core desktop performance](tinycore-desktop.md).

The current `--tinycore-perf` frame comparison uses a frozen accepted capture
snapshot from 2026-11-01, four balanced implementation orders, and independent
expected damage-area assertions. It times 120 idle/sparse/full-update frames
inside the native guest, not X11/compositor/scanout FPS. `fbdev-damage` also
compares exact rectangle order, captured bytes and padding on surfaces wider
than 1,024 pixels and both 64-bit alignment parities. Cycle 28 records why a
candidate that looked faster in desktop wall-time profiling was rejected after
a reproducible full-update regression in this controlled workload.

The same notes cover process-creation measurements and the validated file
snapshot cache. Replacement first applies the existing protected-main and
rounded-byte-budget constraints, then uses empty capacity. Under pressure from
another version of the same inode, eligible older versions and failed owners
share an age-ordered victim tier before unrelated images. Without that pressure,
ordinary LRU applies. Identity affinity is only an eviction hint; cache hits
still require the complete nonreused VFS version identity and size.
`alter-image-cache` runs the production loader with mocked
service requests, including cache hits, mutable files, partial I/O, buffer reuse
and mmap segment reuse. Its current cache admits up to 48 small-file snapshots
within the original 16-MiB retained-data budget, rather than allocating 48
maximum-size buffers. Admission includes page-rounded/geometric capacity and
falls back to the existing uncached buffers when necessary. PT_INTERP loading
explicitly protects the outstanding main ELF's address from replacement.
Tests also cover a near-ceiling whole-file snapshot, immutable old-version
bytes, warm segment reuse and over-ceiling no-prefetch/no-allocation fallback.
[Cycle 40](optimization-cycles.md#cycle-40-admit-medium-dsos-within-the-existing-snapshot-byte-budget)
records the rejected general 4MiB per-file candidate. ELF and cold writable
mappings retain the 2MiB ceiling. Whole-file readonly mappings may now admit
larger immutable snapshots within the same 16MiB total bound; partial cold
mappings never prefetch. x86_64 defaults to immutable backing, with
`NANAMI_IMMUTABLE_SNAPSHOTS=0` restoring mutable storage and the old ceiling.
`linux-file-snapshot` tests separate rounded file aliases from private zero
suffixes, publication errors and legacy/final zero-page permissions. Alpha
tests cover 513-page aliases, source retirement, COW and uncertain MAP failure.
KVM evidence and limits are recorded in the handoff's `large-file-snapshots`
checkpoint; this change does not establish a repeatable startup speedup.
Tests cover a 32-file resident set, large-file churn at the byte limit and an
interpreter that must bypass the cache to preserve its live main image.
`ext2-file-versions` checks bounded token eviction and
exhaustion; storage/truncate tests also compile the real invalidation hooks.
`alpha-initialization-window` checks stable pool-frame mappings, bounded collision
eviction, and MAP/UNMAP failure ownership. Its default 4,096-entry cache uses a
16-MiB alias range and64 KiB of metadata; the compact optout uses2,048 entries,
8MiB of alias space and32KiB of metadata. Tests also cover
resident reuse above the old capacity and failed MAP followed by failed cleanup.
The cycle notes report both measured fork/exec gains and slower controls.
Real ELF updates and complete Linux
fork/exec timings are tested by `qemu-smoke.py --tinycore-process-perf`.
This fixture also measures getpid/getuid controls and checked credential copyout
before forking. Its native image-lifecycle rows include VFS input loading,
owned-RAM preparation, suspended child creation/kill/reap, and final input-buffer
release. Preparation and source cleanup are reported separately; faster
construction alone must not hide deferred alias cleanup. These rows do not
resume the Linux ELF or include interpreter/application execution.
`alpha-memory-staging` (in `core-process`) compiles the production input reader
and checks current-Frame reads without ownership scans/snapshot allocation,
whole multi-page extent inputs, stale metadata, byte freshness, shared-VM
sources, boundaries, overflow and mapping errors. Existing `alpha-copy-window`
and shared-memory lifecycle tests check the reused alias cache's cleanup rules.
`qemu-smoke.py --tinycore-startup-perf` measures the real OCI
create/start path until an aterm child has connected to X11 and its fresh
`desktop-ready` marker is read back. It repeats three sessions, deleting each
execution context before reusing the same container ID. The 100-ms observer
interval bounds polling resolution. Session deletion and the observer's sample
printing are outside the measured interval; guest-critical-path work, including
synchronous diagnostic output, remains part of elapsed startup time.

The QEMU runner accepts `--accel kvm` (CPU `host`) or `--accel tcg`
(CPU `max`); `--cpu` overrides that model. `--tcg-threads` requires TCG.
Each run saves its exact `command.json` and keeps temporary disk overlays
in that run's log directory. Keep different accelerators/hosts separate.

For post-launch communication diagnostics, build the desktop image with
`--desktop --image --with-tests --with-launch-rtt`, then run the QEMU fixture
with `--accel kvm --tinycore-app-launch-perf files --app-launch-ready ping
--app-launch-rtt`. It requires a complete report containing three raw128 and
three Xlib128 samples. These measure the whole X11 stack, including scheduling;
they do not isolate IPC. RTT mode excludes `--app-launch-timeline` so screenshots
and keyboard challenges do not overlap timed RTT work. A ping response still
does not prove completed painting or input readiness. Performance comparisons
need frozen inputs and complete, predeclared matched cohorts.

`--host-process-stats` optionally records Linux QEMU process CPU counters around
the launch interval, with PID/start identity, actual read bounds and tick rate.
User ticks already include guest ticks; these totals cannot isolate guest IPC
or application CPU work. Contradictory counters are rejected. The 2026-11-06
nested-KVM admission trial failed its 5% perturbation gates; its CPU breakdown
must not be used as normal-runtime attribution. The separate identical-command
control also failed stability. `--host-cpus 0,2,4,6,8` can explicitly constrain
Linux KVM via taskset and verifies existing QEMU threads before timed work, but
that distinct baseline also failed stability; it is not a demonstrated fix.
Choose masks from the actual host topology, keep these trials separate, and do
not relax gates or rerun unchanged conditions until a favorable result appears.
Per-run `host-manifest.json` records harness source hashes and kernel identity
before starting QEMU, for future provenance.

`--host-poll-ms` explicitly selects the host serial observation interval
(1..1000ms, default200ms) and records it in the launch JSON and console. The
native relay still polls at100ms. Nanosecond timestamps do not remove either
observation interval, and a ping is not full-paint/input readiness. Different
host polling intervals need separate disturbance controls; do not interpret
a shorter observation delay as a guest speedup.

`alter-syscall-scope` tests a bounded ledger/controller and the native packet
decoder. Alter's non-default `syscall-scope` feature connects it around Linux
dispatch handlers. Count mode needs no per-handler clocks; timed mode
rejects missing/reversed/overlapping counters. Nonreused scope epochs reject
stale records, owner checks protect stop/cancel, overflow invalidates rather
than partially commits, and out-of-table syscall numbers retain a separate row.
Feature-only PR_SET_NAME values `nscope.count`, `nscope.time`, `nscope.stop`
start/stop a caller-owned scope. Owner exit/removal/committed exec cancels it.
Private diagnostic PRCTL option `0x4e530001` copies a stopped, owner-checked
8272-byte LE report using ordinary typed owned copyout; it retains the report
on EFAULT and does not grant native capabilities. Header words are magic
`0x00315043534e414e`, epoch, mode(0=count/1=time), start, end, ledger-valid,
row count513, and1=local counter unverified;513 call/tick pairs follow.
Row512 retains all out-of-table numbers. No native pointers are serialized.
This is not an A9N Kernel ABI or default Linux behavior. Legacy
`NANAMI_ALTER_PROFILE=1` cannot be combined with this feature.

`tests/alter-linux/syscall-scope.c` and the native Negoto probe validate known
counts, non-owner refusal, EFAULT retry, packet fields and cancellation before
owner reaping/while an exec child stays alive. `--syscall-scope-fixture active`
or `off` runs this functional check on KVM with the test bundle. The tracked
`package-scope-fixture.py` creates separate paired images from frozen inputs,
verifying readback, filesystem health and unchanged boot/ESP/trailer bytes.
Serial rows are not an oracle: one real run lost a row to interleaved output.
The current native fixture independently validates both binary packets before
its cleanup footer; the strict legacy parser and failed evidence are retained.

The opt-in `--scope-disk-readout` additionally retains a private qcow2 overlay,
waits for the native fixture's post-workload VFS sync and strict cleanup footer,
stops/reaps QEMU, and extracts both files with read-only debugfs. Schema3 results
retain full 8272-byte packets, SHA-256 hashes, independent host validation of all
rows/headers, and extraction commands. Disabled controls require both files
absent. KVM SMP1/SMP4 passed this storage path; backing images stayed unchanged.
Its diagnostic bootstrap waits for initial compositor render plus a fixed10s
settle because a font-log tag interleaved in the first attempt. This provides no
boot/readiness timing assertion and does not change ordinary performance gates.
Raw serial packet export was rejected after actual interleaving; it is removed.
See `session-handoff/logs/linux-native-scope-20261106/DISK-EXPORT-PLAN.md` and its
result for retained failures and the exact diagnostic conditions.

For real app diagnostics, `desktop.build_window_observer(..., syscall_scope=True)`
builds a separate `/bin/aw-scope`; `package-scope-fixture.py --observer ELF`
installs it into both paired images. `qemu-smoke.py --tinycore-app-launch-perf
files --app-launch-ready ping --app-syscall-scope off|count|time` uses that observer
as the live owner. It starts before observer READY and stops after the first
verified ping reply. Serialization/fsync follow the endpoint and any requested
RTT samples. The host requires APP-SCOPE-SAVED, reaps QEMU, and independently
decodes the full workload packet from the retained overlay. Disabled mode must
produce no packet. `app-syscall-scope-result.json` explicitly denies performance
admission and records coverage/boundaries; these diagnostic runs use the same
fixed-settle bootstrap as disk fixture diagnostics.

Normal/off SMP1, count SMP1, and time SMP1/SMP4 functional runs passed. They are
single samples, not an overhead/stability cohort. Scope includes pre-click delay,
observer work and background Linux dispatch in this Alter; it is not per-app
CPU time or an exact click interval. Stop/control overhead can affect observed
ping latency. First ping still does not prove full paint or input readiness.
See the APP-SCOPE plan/result under the same handoff log directory. No cost
ranking or optimization is admitted from these packets before clock and matched
normal/off/count/time disturbance validation.

Normal cached private file mmap initializes fresh backing directly with
final R/W/X rights through trusted Nanami request0x1035, avoiding a subsequent
permission-remapping pass. `NANAMI_MAP_FINAL_PERMISSIONS=0` restores the old
two-step path; `=1` explicitly enables the default. It requires matching Alpha;
update Alpha and Alter together. The
legacy two-word initializer contract remains unchanged. Model/failure tests and
real KVM file-mapping regression pass. SMP4 ABBA observed6.15s vs4.78s mean Files
first-ping latency, but control and candidate repeatability both failed5%; no
22% speedup was established by that experiment. See the retained plan and
`session-handoff/logs/linux-native-scope-20261106/final-permissions/RESULT.md`.
An extension to private anonymous mappings was implemented and tested, then
withdrawn:its KVM SMP4 ABBA observed launch+10.51%,raw+5.54%,Xlib+8.11%, with failed
repeatability. The live source enables only the file-backed path;
see `session-handoff/logs/linux-native-scope-20261106/anonymous-final/RESULT.md`.

Normal Alpha initialization reuses
only already-ready initialization aliases after current Frame/private ownership
checks, with a bounded request-local ownership walk and original CopyWindow
fallback. `NANAMI_INIT_SOURCE_REUSE=0` restores the original mapping path;
`=1` explicitly enables the default. No cache growth or guest mapping limit.
Historical host tests and real KVM file
mapping pass, but its ABBA stopped at a candidate boot timeout and observed QEMU
SIGSEGV; no performance result/adoption. The HMP sender's separate EOF-spin bug
was fixed with EOF/exited-process checks and a total reply deadline. See
`session-handoff/logs/linux-native-scope-20261106/source-reuse/RESULT.md`.
The pending reuse shortcut now gets only the owner's scalar root, eliminating
an actual528B snapshot-copy call confirmed in native assembly. Diagnostic-only
KVM SMP1 verification reached a reuse hit near Linux context fork and passed the
file-mapping regression; it does not prove Files mmap hit coverage or speed.
`NANAMI_INIT_SOURCE_REUSE_DIAG=1` emits a one-time forced record only for functional
reachability builds; do not use those builds for timing. See
`session-handoff/logs/linux-native-scope-20261106/source-reuse-reachability/RESULT.md`.
The matching non-diagnostic scalar revision completed SMP4 ABBA:observed launch
mean5.71s vs5.08s, but within-control launch shifted28.21% and within-candidate
11.31%; raw/Xlib repeatability also failed. No11% gain or default adoption is
claimed. Full records:
`session-handoff/logs/linux-native-scope-20261106/source-reuse-scalar-perf/RESULT.md`.

Normal Alpha initialization also enables collision-aware role selection with
source reuse. `NANAMI_INIT_SOURCE_ROLE_SWAP=0` restores the previous placement;
`=1` explicitly enables the default. Only on an initialization-slot/source collision, it places
the fresh destination in existing CopyWindow and pins it through any source
fallback; cache sizes stay unchanged. Partial-page/pinning tests and KVM mapping
regression pass. SMP4 ABBA observed6.17s vs4.88s mean launch, but controls and
candidate raw RTT failed repeatability; no20.9% speedup/default adoption claimed.
Evidence:`session-handoff/logs/linux-native-scope-20261106/source-role/RESULT.md`.
Those comparisons are historical and do not establish a precise gain. Current
normal-path integration and combined safety/runtime checks are recorded in
`session-handoff/logs/linux-native-scope-20261107/mapping-default/`.
The anonymous final-permissions extension remains withdrawn. Continue removing
demonstrated work on the current KVM host per user instruction; timing variance
alone does not block all implementation, and it must still be reported honestly.

KVM SMP1/SMP4 functional success does not establish normal-workload coverage,
runtime overhead, calibrated clock/CPU identity, a cost ranking or a gain.
Only handler dispatch is observed; page faults, register access, replies and
unobserved work are not inferred from the remainder. Restarted dispatches can
count more than one invocation per logical syscall. Inclusive spans include
RPC waits/descheduling. Actual application boundaries, durable real-app cost data
and matched off/count/time disturbance gates remain required before attribution.

Cycle 36 records a rejected user-level deferred-fork implementation: immutable
snapshots and bounded prefault/address hints passed functional checks, but real
startup was 22.2% slower. The accepted runtime keeps eager private cloning; no
deferred-page protocol, fault resolver, working-set hints or Kernel changes are
retained. Native constructor/model tests are not substitutes for this end-to-end
acceptance check.
Cycle 30 moves eight fixed menu/picture link helpers from guest startup to
graphical image assembly. A matched four-boot ABBA comparison measures actual
X11-ready startup at 6,665 -> 5,643 ms (15.3% lower), with unchanged native
runtime binaries. This is not a claim about every application or display FPS.
The desktop fixture also checks the default aliases and preservation of a
custom menu alias and picture file across Exit/restart; see the cycle notes
for individual samples, image provenance and validation logs.
`linux-mappings` also checks independent ordinary-fork VMA storage, shared
CLONE_VM aliases, permission/order preservation and recoverable allocation
failure. Its opt-in fork-table benchmark compares bulk inheritance with the
former repeated `add_mapping` loop; it excludes IPC and physical-page copying.
`alpha-memory-clone` (in `core-process`) exercises the production VM-copy
traversal: fresh private children batch only contiguous backed-page runs,
while sparse holes, per-page protections, independent bytes, rollback ownership
and generic overlap order are preserved. A differential fixture compares sparse
mixed-permission snapshots with the original page-at-a-time path. Page
construction is modeled here; `alpha-private-pages` checks the real constructor's
multi-page partial-failure cleanup, and QEMU checks guest-visible protection.
`physical-allocator` checks the real allocator against an independent extent
model and benchmarks newest-first, oldest-first and interleaved page releases.
It also checks page-at-a-time release of bulk-backed physical extents, including
interior splits, double-free rejection and complete pool reuse. Its opt-in
batch-allocation diagnostic separates allocation from subsequent page release;
cycle 37 records why fewer physical allocation calls did not justify retaining
the constructor candidate without improved real startup.
`physical-allocation-index` separately checks the compact lookup index's chain
and swap-remove invariants, partial splits, reserved duplicates, allocation
failure fallback, encoding boundary, and bounded bucket storage. This separate
target does not change the public-API baseline source override benchmark.

`alpha-copy-window` checks the bounded four-way process-copy mapping cache,
including resident working-set reuse, pinned-source eviction, canonical shared-VM
owners, PID-bucket collisions, failed MAP/UNMAP recovery, and alias-window bounds.
The owner-count buckets only skip provably empty invalidations; they impose no
PID limit and never replace current-Frame or permission validation.
Owned/shared RAM buffers record permanent Alpha pool Frame aliases, allowing
the bounded copy cache to reuse those aliases after guest mappings are released.
Ordinary private RAM and MMIO/framebuffer child aliases retain PID-scoped cleanup.
`linux-read-window` separately tests Alter's bounded64-page supervisor read
leases: fresh content, VM-owner/generation changes, boundaries, eviction and
uncertain-install/failed-release quarantine. `linux-guest-copy` checks current
permission preflight and checked fallbacks. Alpha lifecycle tests exercise the
real grant handler's authority, pinning and partial-failure cleanup; reservation
tests ensure high fixed-VA leases do not pollute ordinary heap allocation holes.
Actual protection, replacement and fork isolation are checked by the80-page
Tiny Core mapping probe, with1/4-core read-only-fork/source-first-reaping tests.
See [cycle99](optimization-cycles.md#cycle-99-bounded-supervisor-read-leases-for-small-syscall-inputs)
for matched timings and the distinction between event-loop and fully painted,
input-responsive application readiness.
Namespace checks classify already-resolved internal descriptors only; they do
not establish capability validity, rights or object identity. Real-memory tests
cover bootstrap/normal namespaces and process-arena reset. The shared-memory
lifecycle harness also keeps copy aliases resident through current permission
changes and checks that denied copies/foreign supervisors never reach its
modeled byte-transfer boundary. Its native alias model is not a hardware TLB
proof; actual Tiny Core process/startup and GUI checks are recorded in
[cycle 38](optimization-cycles.md#cycle-38-copy-aliases-backed-by-alpha-owned-pool-frames).

The Tiny Core profile's one-process Xlib readiness helper is measured separately
in [cycle 39](optimization-cycles.md#cycle-39-eliminate-repeated-x-readiness-subprocesses).
Its host bounded-retry/liveness model is checked by the builder's C test, while
actual X11 readiness and the standard libc entry are exercised in normal QEMU
startup and two-session GUI regressions. First-start and later-session timing
results are reported separately; this profile optimization does not change
ordinary Linux fork, Alter's handlers or Kernel IPC.

[Cycle 41](optimization-cycles.md#cycle-41-measure-current-file-mmap-read-and-initialization-work)
separates current snapshot reads/copies from private-page initialization using
temporary user-level aggregates. It records both Alter-side request spans and
Alpha-side phases, including their overlapping coverage and exit/reap flush
limitations. Those diagnostic counters are not exclusive CPU time or a
measured speedup; all instrumentation is restored before normal validation.
Source validation, private backing and rollback checks remain intact.

[Cycle 42](optimization-cycles.md#cycle-42-separate-private-page-content-work-and-source-alias-reuse)
splits the content path further and rejects an eight-way cache experiment after
normal process/startup comparisons. The copy-window harness retains an explicit
2,048-alias/metadata-budget guard; production stays with four ways. A source
audit records the write-permission, capability-domain and backing-lifetime
requirements that must precede any read-only private-fork sharing experiment.

[Cycle 43](optimization-cycles.md#cycle-43-measure-private-fork-permission-inventory-before-sharing)
measures the current permission mix for actual desktop forks, without treating
non-writable PTEs as proof of safe sharing. Clone traversal regressions cover
later write upgrades/supervisor copies and source replacement with descendant
forks. Their backing/protection are modeled; they are not hardware or future
shared-Frame lifetime/capability-isolation proof.

[Cycle 44](optimization-cycles.md#cycle-44-eliminate-repeated-immutable-posix-identity-queries)
retains a connection-owned identity snapshot. `alter-identity` compiles the
production cache and Linux query adapter with a modeled POSIX transport, covering
distinct real/effective user/group IDs, failure retry, connection changes,
explicit same-slot reset, independent runtimes and zero IDs/metadata bounds.
Normal guest tests also check UID/getresid results. The getresuid-based workload
labelled `copyout` includes identity lookup: its improvement here is not a new
Alpha byte-copy primitive. Both-order process comparisons and two real desktop
sessions are recorded separately from unchanged startup results. Future POSIX
credential mutation/session rebinding must update or clear this snapshot;
permission decisions remain with POSIX/VFS.

[Cycle 45](optimization-cycles.md#cycle-45-exact-short-product-hpet-conversion-evaluation)
rejects a short-product HPET arithmetic fast path after normal Tiny Core
comparisons show no startup improvement. `hpet` retains independent exact
floor/ceiling and saturation oracles across small/large timestamps, product
overflow boundaries, valid/random periods and bounded clock state. The timer
runtime is restored byte-for-byte; no Kernel or clock-sampling policy changes.

[Cycle 46](optimization-cycles.md#cycle-46-avoid-an-image-rendering-client-for-a-solid-background)
measures repeated VMA gap scans and leaves their algorithm unchanged because
their observed cost is small. It instead retains the unchanged upstream
`xsetroot` for the profile's solid background, selected from the existing pinned
package. Initial startup improves in both measurement orders; its combined
median is 5.18% lower. All-session startup improves 3.22%, not 5%.
The two-session real desktop, pinned-package assembly and builder regressions
cover input, launcher, Information, background/dock and restart behavior.

[Cycle 47](optimization-cycles.md#cycle-47-coalesce-adjacent-unix-recvmsg-result-fields)
coalesces the adjacent control-length/flags output fields in UNIX recvmsg,
with ordered whole-field fallback on a bad pointer. `linux-unix` checks exact
copy counts, no allocation, untouched inputs/padding, queue preservation and
SCM_RIGHTS rollback. The integer-only guest fixture checks real read-only page
boundaries before its send/recvmsg and EOF loops. The QEMU runner also guards
against ending UART capture at a report's prefix; three host tests exercise
its actual completion predicate. See the cycle record for separately scoped
socket and whole-desktop measurements.

[Cycle 48](optimization-cycles.md#cycle-48-decode-unix-io-descriptions-in-place)
folds a private, single-caller message decoder into UNIX I/O dispatch. The
normal x86_64 image eliminates two 300-byte copies used to return and unwrap
the description; its I/O stack frame is 128 bytes smaller and total text is
208 bytes smaller. No unsafe
initialization, new state or wire change is added. Boundary regressions cover
zero/maximum/over-limit vectors and late input faults without partial packets.
Matched normal-image measurements do not establish a 5% application gain;
this is a small copy/storage cleanup, not an IPC-speed claim.

[Cycle 49](optimization-cycles.md#cycle-49-classify-unix-requests-before-descriptor-lookup-rejected)
evaluates moving UNIX operation decoding before FD lookup. Generated code and
instrumented native tests confirm that unrelated syscalls avoid the lookup,
but normal-image comparisons show no getter/startup improvement and slower
fragmented-fork controls. The candidate is rejected and runtime binaries are
restored exactly. General regressions retain unrelated-syscall fallthrough,
foreign/invalid descriptors and live FD-reuse checks, not a requirement to
retain the rejected decoder or its lookup instrumentation.

[Cycle 50](optimization-cycles.md#cycle-50-measure-socket-state-searches-in-real-startup)
measures socket-state searches during three real Tiny Core startups. The
observed table peaks at eight live sockets; syscall-side get/get_mut searches
average 4.87 comparisons and account for 0.084% of instrumented handler elapsed
ticks. Event-side searches are reported separately. This does not justify a
new lookup cache as the next major startup optimization. All temporary probes
are removed and normal runtime binaries are restored exactly; profile times
are not used as normal-image speed comparisons.

[Cycle 51](optimization-cycles.md#cycle-51-ordered-checked-memory-transfer-vectors)
adds a bounded, ordered checked-copy vector to Alpha and uses it for recvmsg
result fields. The descriptor list is snapshotted before output; each range
keeps current permission preflight and prefix-on-error semantics. Kernel and
IPC fastpath are unchanged. Matched socket loops improve 11.57% / 18.87% for
write/recvmsg and EOF recvmsg; startup is neutral. Ordinary fork remains an
unresolved control: the initial pooled increase does not repeat in the reverse
extension, but the complete pool is still slower. Do not claim overall 5%
startup gain or fork equivalence. See the complete measurements and protocol
contract before expanding use of the new API.

[Cycle 52](optimization-cycles.md#cycle-52-isolate-fork-controls-without-kernel-changes)
separates Alpha-only profiling from normal-image Alter-only comparisons, with
eight sequential QEMU boots. The latter gives ordinary fork -1.58%, fork/exec
+0.37%, and mixed fragmented-fork controls; an unrelated native constructor
control also moves. These measurements do not erase cycle 51's earlier slower
pool or establish performance equivalence. Temporary probes/build overrides
are removed and the exact normal runtime is restored; Kernel stays unchanged.

[Cycle 53](optimization-cycles.md#cycle-53-request-local-path-walk-buffer-reuse-rejected)
rejects request-local ext2 pathname scratch/output buffer reuse. A deep-path
host probe reduces allocation calls 151 -> 80 and matched host elapsed time
25.93%, but real Tiny Core stat is 3.22% slower and startup is neutral. All
twelve comparison boots finish, and the normal runtime is restored exactly.
Three retained namespace regressions cover leaf/parent results through deep
paths, symlinks/dot-dot/missing targets, and the canonical-path length boundary;
the temporary host allocator counter and its performance gate are removed.

[Cycle 54](optimization-cycles.md#cycle-54-fixed-slot-indexed-dentry-lookup-rejected)
measures 46.74 visited slots per ext2 name-cache query during real startup and
tests a fixed-slot hash index preserving capacity, FIFO order and invalidation.
Six oracle/collision/budget tests pass, but twelve normal-image comparison boots
give stat -1.82%, missing stat +2.30% and neutral startup. Reject and archive the
entire candidate; the exact normal runtime is restored. Search counts alone are
not CPU shares or proof of an end-to-end gain. The historical cycle50 image is
now preserved losslessly as a verified gzip archive to recover working space.

[Cycle 55](optimization-cycles.md#cycle-55-stat-latency-attribution-and-borrowed-owner-lookup-rejected)
separately profiles stat stages and Alpha checked-copy stages, without Kernel
changes. Nested waits are not exclusive CPU/IPC costs. Optimized-code review
disproves a proposed 528-byte copy saving on the relevant owner lookup paths.
A small borrowed-owner candidate passes tests but yields neutral stat, mixed
controls and no startup gain, so it is rejected. One owner/snapshot regression
remains; all production probes and binaries are restored. A same-core condition
in the existing Reply-Receive direct switch motivates investigating user-level
service placement, not a Kernel change or an unmeasured performance claim.

[Cycle 56](optimization-cycles.md#cycle-56-isolate-ipc-locality-without-changing-kernel)
confirms that placement is a large performance lever without modifying Kernel.
A broker-only diagnostic preserves other processes' round-robin placement and
reduces matched QEMU startup 14.40%, stat 37.32% and recvmsg 32.81%. However,
getpid/getuid become about 2.8 times slower when the guest and Alter separate.
Do not adopt this probe or hard-code its image/priority selectors. Preserve
all controls, restore source, and investigate guest/supervisor locality while
retaining worker/device parallelism. Global single-core pinning is also rejected.

[Cycle 57](optimization-cycles.md#cycle-57-configurable-broker-locality-without-kernel-changes)
adds optional boot-configured priority-class placement, not built-in service
names or a new IPC operation. Explicit affinity wins; unavailable CPUs keep
automatic placement and unrelated tasks retain their round-robin sequence.
Keep only priority21 near Alter; leave VFS near storage after a wider grouping
regresses large-ELF input. Final balanced QEMU results are stat -12.89%, missing
stat -15.04%, startup -4.05%, with basic syscalls near baseline and recovered
large-ELF controls. Native1167, focused ASan149, ARM compile, one-CPU process
and two-session GUI checks pass. Kernel stays unchanged; whole-startup 5% is
not claimed. The older cycle51 image is losslessly archived; current fixture
image is cycle57-normal.img.

`nanami_runtime` now uses the no-std TLSF allocator from pinned `rlsf` 0.2.3.
This affects native Rust applications/services, including the native Alter
process. It does not replace a Linux guest's libc malloc, Alpha's allocator,
or anything in A9N.

The subsequent background-cache, scrollback and terminal-transfer work is
documented in [Native hot-path follow-up](hotpaths.md).

The Alpha mapping and Alter vectored-I/O follow-up is documented in
[Alpha / Alter transfer paths](alpha-alter.md).

The subsequent copy-window, clock-query and readiness review is documented
in [Alter/Linux syscall hot paths](syscall-hotpaths.md).

The module split and POSIX delegated-write follow-up is documented in
[Alter / POSIX I/O](posix-io.md).

The lookup, timer, storage and input follow-up is documented in
[Core and server hot-path review](core-servers.md).

Negoto's rootfs-context/path and startup-argument regressions are in `negoto`;
OCI configuration/lifecycle tests are in the root workspace's `negoto-oci`
crate. `linux-readiness` covers blocking pipe reads/writes and EOF wakeups;
`linux-pipe` checks circular I/O, atomic writes and copy-failure rollback.
`linux-console` checks the production window-local mediumraw/keymap and PS/2
codecs, including wheel negotiation, short reads, large movements and Linux
mousedev response replacement. `linux-directory` checks that legacy raw console/mouse nodes
require a graphics grant. See the [Tiny Core recipe](../../thirdparty/tinycore/README.md)
for real Xfbdev/FLWM/aterm keyboard, mouse and lifecycle tests in QEMU.
The [Negoto application guide](../../nanami/docs/application.md#negoto-oci-bundles)
documents the unmodified BusyBox fixture and real QEMU lifecycle tests.
`linux-job-control` exercises the production session/process-group model and
tty ioctl marshaling, including fork inheritance, exec restrictions, foreground
group transfer, controlling-terminal ownership, invalid groups and bad pointers.

The HTTP ARP, TCP receive-flow-control, and NIC backlog regression tests are documented in
[HTTP / network regressions](../network/README.md).

The deferred-notification service receive regressions are documented in
[Deferred IPC notifications](ipc/README.md).

USB root discovery and late-attachment regressions are documented in
[USB input and root storage](../usb/README.md).

Cursor/drag damage and compositor RAM writes are documented in
[Motion rendering](motion.md).

`display-selection` exercises the manager's common native/fallback policy:
unknown GPUs, native spawn failure, explicit probe rejection, caller validation,
duplicate reports, fallback startup failure and absent boot framebuffer.
Once native activation succeeds it cannot start a competing framebuffer writer.
`tests/alter-linux/qemu-smoke.py --expect-framebuffer` checks a single fallback
startup and publication after framebuffer initialization in a real guest.

PCI inventory/BAR transaction checks are in `pci-server`; `usb-pci` exercises
the shared PCI client and USB setup together. `pci-server` also resolves the
published `pci-ids-rs` database and checks its name chunks through the production
client, including unknown IDs and malformed replies. `pci-info` tests CLI
formatting and buffered terminal output with partial writes/backpressure.
`acpi` also checks MCFG parsing
and ECAM address calculation. `intel-graphics` includes the production Gen9
scanout logic with a host MMIO/DMA model, including damaged back-buffer repair,
unsupported-mode refusal, GGTT allocation above the CPU aperture and timeout
ownership. It also exercises real GMBUS/AUX transaction sequencing, bounded
DEFER/NAK/busy handling, VBT/OpRegion parsing, EDID detailed timings and the
production display-discovery client wire format. `honoka-motion` checks that
native scanout receives one present per frame, with pixels matching sparse
framebuffer-fallback composition. These are not Intel hardware
tests. See [Device Drivers](../../nanami/docs/device-drivers.md) for the backend
scope and QEMU limitations. `tests/alter-linux/qemu-smoke.py --storage
ahci|virtio|usb` can regression-test each root-disk transport in a snapshot.
Add `--native-info --network` to capture the three PCI listing modes and the
renamed `net-info` builtin for visual inspection; this does not claim automated
text recognition of those screenshots.

The host runner's optional `--host-process-stats` also retains raw per-thread
stat/schedstat snapshots around the click/ping interval. It checks HMP ownership,
start ticks, unchanged thread membership, counter monotonicity and the existing
guest/inclusive-user accounting rule. The [Linux scheduler counters](https://docs.kernel.org/scheduler/sched-stats.html)
report runtime/runqueue nanoseconds and timeslices; these are not independently
validated physical CPU or Hyper-V wait measurements. Unproven runqueue enablement
omits wait deltas; any accounting inconsistency rejects thread attribution.
Raw values remain available for diagnosis, with CPU-cost admission false.

The optional `honoka-frame-events` feature in Honoka and this test crate counts
notification batches, fbdev scans, emitted damage rectangles and dirty render
passes without adding clock RPCs. It disables the separate wall-time profiler
and reports at most64 groups after existing clock notifications; groups are not
elapsed seconds or FPS. Native tests cover merged bits, reset and overflow.
Default builds omit these hooks. Interleaved rows are not reconstructable timing
evidence, and diagnostic counts alone cannot establish a stage's CPU cost.

`linux-unix` also checks the normal coalesced receive path's bounded owned
gathering (`NANAMI_UNIX_COALESCED_RECVMSG=0` restores the original path):64/256-byte
receives allocate nothing,257 bytes retain the Vec fallback, including a partial
first-packet offset. It preserves ordered whole-payload/metadata
copyout, queue rollback, PEEK and ancillary barriers. Explicit `=1` enables it;
native allocation reduction does not prove an end-to-end speedup. The raw Linux
file-mapping fixture adds16 real protected-header fault/retry cases; a dedicated
probe built with `NANAMI_REQUIRE_UNIX_RECVMSG_FAULTS=1` requires that marker within
the same collected terminal buffer as the overall file-mapping completion.

`gpu` exercises the common GPU client/provider protocol, ownership, padding,
quotas and dead-client reclamation. `gpu-blt` runs production Gen9 command
encoding, PPGTT and Execlists submission through a checked host MMIO/DMA model,
including incomplete completion and CSB wrap. `honoka-motion` also compares
GPU-protocol composition to CPU pixels and checks that a failed copy poisons
both operands. The Alpha shared-memory transaction tests cover sharing existing
owned RAM without reallocating/zeroing, release order and failure rollback.
These models are not physical Intel validation; see [GPU service](../../nanami/docs/gpu.md).

`virtio-gpu` covers the real modern PCI capability parser, split control queue,
wire commands and common GPU provider. Tests check row padding, resource
switching, CPU copy, display ownership and the legacy API; ring-index wrap and
bad fences/used entries cannot release DMA before acknowledged reset. Failed
sharing detaches resources before releasing RAM. QEMU `--gpu virtio
--expect-virtio-gpu` exercises the actual device; `--gpu virtio
--virtio-gpu-iommu --expect-framebuffer` deliberately declines unsupported
ACCESS_PLATFORM and checks framebuffer fallback. The default smoke-test display
remains `std` so existing framebuffer regressions retain their original scope.

## Choice and boundaries

The previous implicit-list allocator scanned occupied blocks on every
allocation, locked again to validate the result, and disabled both directions
of free-block coalescing. TLSF uses size-class bitmaps and free lists instead.
Its allocation/free bookkeeping is constant-time, excluding contention and
Alpha growth requests. Realloc can resize in place; a relocating realloc still
has to copy data. See the [rlsf documentation](https://docs.rs/rlsf/0.2.3/rlsf/).

[mimalloc](https://github.com/microsoft/mimalloc) and
[jemalloc](https://jemalloc.net/jemalloc.3.html) are not drop-in allocators for
Nanami's `no_std`, `target_os = "none"` applications. A native port would need
their OS memory/thread/TLS interfaces adapted to Nanami, without routing the
native allocator through Alter/Linux. That port is not part of this change.
`rlsf::Tlsf` accepts RAM pools directly, so it can use the existing Alpha heap
request without a new kernel or service ABI. Although Cargo.lock includes
rlsf's conditional libc dependency, libc is not built for either native target.

- One allocator lock protects ordinary allocation/free; an additional growth
  lock serializes misses. Alpha IPC runs without the allocator lock held.
- A miss rechecks the pool after taking the growth lock, avoiding redundant
  mappings when another thread has already grown or freed memory.
- Adding a region preserves live allocations. Regions are retained until
  process exit, with the previous limit of 16 and minimum growth of 4 MiB.
- Mapping ranges are checked for overflow/overlap before metadata writes.
  TLSF uses only the returned mapped range. Alpha's unmapped guard gap stays
  untouched; the old additional 64-KiB in-mapping tail reservation is removed.
- `heap_stats` remains an explicitly diagnostic pool walk. It reports block
  bytes including allocator headers/padding, excluding TLSF sentinels; free
  bytes do not guarantee one allocation of that size. The unstable inspection
  API is why rlsf is pinned. Allocation/free never call the inspection API.
- The x86_64 allocator control object is 8,600 bytes of BSS, including the
  size-class tables and region bookkeeping. This is separate from heap stats.

Honoka/Shell also release the parsed Font after prerasterizing their fixed
glyph caches, render through glyph references instead of copying bitmaps, and
initialize the owned caches directly on the heap. Rendering geometry is unchanged.

## Host tests

The suite also includes the production Alpha launch/environment/partition
code (`alpha-launch`, `alpha-environment`, `physical-memory`, `alpha-domains`),
external image-policy parsing (`boot-policy`), and arbitrary-name registry/wire
enumeration (`services`). These are mechanism tests, not proof of live kernel
revocation. Real child-domain, namespace, and caller-identity checks are described
in [Architecture migration gates](../../nanami/docs/architecture.md#architecture-migration-gates).

Run from the repository root, using its pinned Rust toolchain:

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --lib -- --test-threads=1
cargo test --release --manifest-path tests/native-performance/Cargo.toml --lib -- --test-threads=1
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --lib heap_benchmark -- --ignored --nocapture --test-threads=1
```

The harness includes the production allocator, substituting only the Alpha
mapping request and disabling its registration as the host global allocator.
Tests cover alignment/zeroing, 20,000 randomized operations with simultaneous
live allocations, coalescing, in-place and cross-mapping realloc, allocation
failure preserving bytes, mapping bounds, size-class growth, region limits,
and concurrent allocation/growth. Use one test thread because the mapping mock
is shared; concurrency tests create their own worker threads.

AddressSanitizer on an Apple Silicon host:

```sh
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --lib -- --test-threads=1
```

To compare with the original allocator without changing the working tree:

```sh
baseline_dir=$(mktemp -d)
git show 9e43161d0f42c6abdc2e2b7a892a24065cb66384:nanami/servers/libs/nanami-runtime/src/heap.rs \
  | sed '/^#\[global_allocator\]/d' > "$baseline_dir/heap.rs"
NANAMI_HEAP_SOURCE="$baseline_dir/heap.rs" RUSTFLAGS='--cfg legacy_heap' \
  cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --lib heap_benchmark -- --ignored --nocapture --test-threads=1
```

The timing workload retains N allocations of 64 bytes/alignment 16, then
measures 10,000 alloc/free pairs five times and reports the median ns/pair.
It excludes pool setup/growth and does not measure application throughput.
An aarch64-macOS release run on 2026-09-15 (rustc 1.96 nightly-2026-03-25):

| Retained allocations | Original ns/pair | TLSF ns/pair |
| ---: | ---: | ---: |
| 128 | 688 | 14 |
| 1,024 | 2,780 | 15 |
| 4,096 | 11,051 | 14 |

These host microbenchmarks demonstrate removal of allocation-count-dependent
scanning, not an end-to-end speed multiplier or bare-metal measurement. An
earlier run gave 378/2,798/11,421 ns for the original and 15 ns for TLSF;
short timings vary with host scheduling/cache state.

## Native validation (2026-09-15)

All target-selected Rust apps/services built in release for x86_64 and AArch64.
Nine host correctness tests passed in debug/ASan and release configurations.
The benchmark is separately ignored by default.

The updated x86_64 image passed all three existing Alter/Linux QEMU fixtures
(`linux-syscall-smoke`, `glibc-true`, `glibc-regression`) with both one and four
CPUs. Shell/Honoka text and the desktop were checked in captured framebuffers.
Tests used snapshot disks, temporary firmware variables and no networking.
AArch64 runtime and physical hardware performance were not tested.

Heap usage immediately after font initialization, before other app state:

| App | Original used bytes | Updated used bytes |
| --- | ---: | ---: |
| Honoka | `0x3c5c40` | `0xe720` |
| Shell | `0x3adc40` | `0xa520` |

These bytes are now reusable within the process, not returned to Alpha. The
saved pre-change image supplied the original font statistics; its standalone
glibc-true test timed out, so it was not used for guest timing comparisons.

The A9N source diff was identical before/after the work. Its kernel ELF was
also byte-identical (SHA-256
`bcf6a93fb2c80ea6a70d4080143fcc57546376fc8c1209d16689fea347a5cfe9`).

## Native software work reduction (2026-10-15)

- System Manager now shares one ELF staging buffer between boot, ordinary exec,
  and argument-bearing suspended launches. It requests another mapping only
  when an image exceeds the current page-rounded capacity. Alpha copies image
  bytes and materializes child image pages before replying; no child retains
  the caller's scratch mapping. This is buffer reuse, not an executable cache:
  every launch still reads the current file. Superseded smaller mappings remain
  allocated under the existing Alpha heap lifetime rules.
- The production-loader test loads the eleven image sizes from the reported
  boot sequence with three heap requests instead of eleven. Another test loads
  101 images fitting one mapping with just one heap request. Both launch paths
  use the same reader, which closes handles on failures and rejects zero or
  oversized read replies before copying.
- Shell reads up to 4096 output bytes per terminal IPC instead of 256: a full
  4096-byte parser budget needs one data read instead of sixteen. The parser's
  work budget is unchanged. Its private pending buffer grows by 3840 bytes;
  retaining that copy is necessary because cursor-report replies reuse the same
  IPC shared memory while output is being parsed. Partial reads, split escape
  sequences, errors, and session cleanup retain their behavior.
- Honoka's static information panel measures each character once during line
  wrapping instead of repeatedly measuring growing prefixes. Width work is
  linear in text length. Tests compare the old layout with the real font and
  fallback advances, including UTF-8 boundaries, and retain pixel/clipping tests.

These are request/work counts, not end-to-end speed multipliers. No kernel ABI,
timer/notification synchronization, WC write barrier, or framebuffer ownership
rule changes are involved.

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --offline \
  --test image-loader --test shell-output --test honoka-info-panel -- --test-threads=1
```

All 18 tests in these targets pass in debug, release, and host AddressSanitizer
builds. The complete native suite passes 527 tests (7 timing tests ignored).
All target-selected Rust applications/services build for x86_64 and AArch64.
The fresh x86_64 image boots to Honoka without input using the snapshot launcher.
The SMP4 / USB2 BOT / high-MMIO / HPET terminal regression also passes native
editing/history, repeated launches, BusyBox vi file contents, USB arrow keys,
termios/TIOCGWINSZ, and minimum-size shrink/grow. Tests use private snapshots and
firmware variables with no NIC; existing VMs/images are not modified.

```sh
cargo xtask build --linux-bin out/alter-glibc-root/bin/terminal-test \
  --rootfs out/software-optimized-terminal-rootfs.img \
  --output-image out/software-optimized-terminal.img
python3 tests/usb/qemu-hid.py --image out/software-optimized-terminal.img \
  --smp 4 --usb-storage --usb2 --high-mmio --hpet on \
  --bash-smoke --no-network --terminal-smoke
```

## Further complexity reductions (2026-10-15)

- Shell's VT screen now rotates a logical row origin for full-screen scrolling.
  One-line scrolling clears/copies only O(columns) cells instead of moving
  O(rows × columns) cells. Partial scroll regions retain their copy behavior.
  Resize, erase, history and alternate-screen access all use logical rows.
- The screen tracks changed spans and cursor cells. Active-terminal output
  reconstructs/presents merged dirty row bands rather than the whole window;
  parser-only operations such as cursor reports need no redraw. Surface resize,
  entering/leaving foreground mode and explicit full invalidations still repaint
  the entire surface. Row bands include actual font overhang, and clip background,
  glyph and underline writes to avoid double-blending antialiased pixels.
- ext2 keeps a continuation position per file-handle slot, validated by inode,
  requested entry index and metadata generation. Sequential fixed-batch directory
  enumeration is O(N), instead of repeated prefix scans totaling O(N²). Rewind
  and other seeks fall back to a fresh scan. Every metadata write attempt
  invalidates positions, including failed writes and unchanged-size mutations.
  Existing VFS cookies and external interfaces are unchanged.
- Alpha's physical-allocation bookkeeping uses a per-PID ownership chain and
  exact-range reference-count indexes. Reap/exec reference bookkeeping visits
  that PID's K entries with expected O(K) hash-table work instead of O(K × A)
  rescans of all A entries. Individual VA lookup/removal still scans that PID's
  chain. Active/deferred counts and exact (base page, page count) identity retain
  the previous rules; this does not add kernel alias enforcement or change
  physical-memory reclamation policy.
- Alpha's indexes use recycled arena nodes and backward-shift hash deletion,
  not drop/reallocate tree nodes. Metadata capacity
  grows geometrically with the high-water live set and is reused across churn;
  this is extra indexing metadata, not a claim of lower peak metadata usage.

Alpha's fixed 32-MiB mapped heap also uses TLSF, replacing its no-op bump
deallocation. `alpha-heap` includes the production allocator and tests repeated
20-KiB fork-scratch reuse, alignment, realloc preservation/failure, concurrent
live allocations and coalescing. The bootstrap heap mapping, Kernel APIs and
IPC fastpath are unchanged.

Regression coverage includes a saved pre-change terminal/directory oracle,
6,000 mixed terminal operations, real-font pixel comparisons, directory mutation
and seek/error recovery, randomized active/deferred ownership transitions,
shared-memory failure/release-order checks, forced hash collisions and 10,000
PID/range churn cycles without metadata capacity growth. A single-character
update reconstructs less than one tenth of the default window's area; a 1,024-slot
directory fixture performs over 32 times fewer block-buffer reads. These are
work-count checks, not physical hardware speed measurements.

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --offline \
  --test shell-terminal --test shell-render --test ext2-directory \
  --test core-process --test storage -- --test-threads=1
```

The full native suite passes 545 tests (7 timing tests ignored). The changed
terminal/render/directory/ownership targets also pass release and host ASan runs.
x86_64's full image build succeeds; AArch64's servers/apps, Alpha and kernel code
builds succeed, but its U-Boot image packaging requires a running Docker daemon.
The new `out/complexity-reduced.img` passes no-input USB snapshot boot and the
SMP4 / USB2 BOT / high-MMIO / HPET terminal regression, including BusyBox vi
file-content checks and shrink/grow liveness. The shared initramfs was restored
to x86_64 after the ARM build. Existing user VMs, disks and physical USB devices
were not modified.

## Alpha imports and managed receiver fairness (2026-10-16)

`alpha-environment`, `alpha-domains` and `alpha-launch` cover the common boot
contract's owned import records, alias/limit validation, ambiguous child labels
and receiver-local capability installation. `service-bindings` distinguishes
process-owned and domain-owned Sessions and retains per-import charges after
close. `physical-memory` rejects allocation/mapping of partially occupied boot
pages and checks that following free pages remain usable.
It also mixes 100 process capability arenas with frame metadata, verifies
buddy reservations exclude arena RAM from frame reuse, and checks root-slot
recycling does not allocate new backing. `ext2-mapped-pages` covers VFS-owned
read-page aliases, write coherence, EOF padding, ownership and retryable lease
release; the real Linux mapping lifecycle is exercised by the Tiny Core GUI probe.

`service-receiver` exercises the production sequential receiver, including
uncertain replies, quiesce acknowledgement and a continuously queued 32-request
batch. A bounded Notification poll prevents management starvation without an
Alpha request on each data-path call. Cross-domain QEMU fixtures exercise the
same path against the unchanged kernel; see the [architecture test commands](../../nanami/docs/architecture.md#architecture-migration-gates).

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test alpha-environment --test alpha-domains --test alpha-launch \
  --test physical-memory --test service-bindings --test service-receiver \
  -- --test-threads=1
```

The full native suite passes 662 tests (7 timing tests ignored). These six
targets pass 50 host AddressSanitizer tests; xtask passes 24 tests. The C++
registry/managed-connection wire test passes AddressSanitizer/UBSan. Explicit
import, alias isolation, shared upstream Session identity, two-connection quota
and existing-handle Provider exchange pass QEMU on x86_64 and AArch64, each with
1 and 4 cores. Attenuated alias/container reclamation remains quarantined, not
claimed as completed by these tests.

## Opaque session-buffer handover (2026-10-16)

`service-buffers` executes the production mapping transaction with substituted
page effects. It checks retained contents/client VA, exact abort restoration,
all-candidate rollback after partial MAP/receiver failure, no removal of an
unrelated pre-existing mapping, uncertain UNMAP retention, bounded allocation
and quiesced close. `service-bindings` also checks matching data profiles and
that a partially failed withdrawal cannot be committed or automatically resumed.

The full native suite passes 676 tests (7 timing tests ignored). The buffer,
binding and receiver targets pass 25 AddressSanitizer tests. The optional QEMU
fixture receives the actual stale-access fault through a separate guest resolver,
checks the old sender/address, withholds its reply, stops that blocked task and
verifies that its uncleaned Cspace cannot be reaped. It is not just a check of
Alpha's mapping table. Reproduce with `build-domain.py --session-buffers` and
`boot-smoke.py --expect-buffers`; see the architecture's profile/command details.
The later event profile extends this coverage; safe backing/quota reuse remains
a separate gate.

## Recursive Alpha delegation and imports (2026-10-16)

The real three-level fixture exposed an authority-fragmentation bug: both
512-MiB Generic sources of a 1-GiB child were partially occupied by bootstrap,
so it could not delegate 512 MiB onward despite enough aggregate free RAM.
Delegation now supplies two 256-MiB bootstrap extents, followed by successively
larger untyped extents. They cover exactly the requested budget. The minimum
512-MiB layout and common boot ABI are unchanged.

`physical-memory` now bootstraps a child against actual checked Generic
conversions and delegates a grandchild, verifying that neither ancestor can
allocate/free its live extent. `alpha-domains` validates the shipped recursive
configuration and its explicit service-readiness dependency. These targets and
the buffer/binding/receiver targets pass 48 host AddressSanitizer tests.

The `--recursive-imports` / `--expect-recursive-imports` guest fixtures add a
1-GiB child and 512-MiB grandchild, two local alias names, an intermediate
domain-owned Session charged against the single upstream allowance, and actual
leaf CALLs through root Provider exchange. Normal calls use the root endpoint
directly, without either Alpha relaying them. The intermediate memory report
excludes the grandchild grant. This is a three-level integration check, not
arbitrary-depth isolation or a live-domain reclamation claim. Commands are in
[Architecture migration gates](../../nanami/docs/architecture.md#architecture-migration-gates).

## Event handover, capability provenance and boot regressions (2026-10-16)

The latest full native suite passes 699 tests (7 timing tests ignored).
The following production-source targets pass 135 AddressSanitizer tests
(1 timing test ignored):

```sh
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml --target aarch64-apple-darwin \
  --test physical-memory --test alpha-domains --test core-process \
  --test service-bindings --test service-buffers --test service-events \
  --test service-receiver --test ipc-notifications --test timer-wait \
  --test virtio-lifecycle -- --test-threads=1
```

`service-events` tests bounded event ownership, visible/PCB-held alias
withdrawal, exact abort installation, failed-transfer freeze, idempotent grants
and charged close. Receiver tests cover management/work coalescing and direct
event delivery without a per-work Alpha RPC. The guest profile additionally
retains queue cursors through pending work and an accepted non-idempotent CALL;
ACK cannot precede its old-Provider reply. These event/permission checks pass
x86_64 and AArch64 QEMU, each with 1 and 4 cores, using
`build-domain.py --service-events` and `boot-smoke.py --expect-events`.

The guest rejects copying Alpha's administrative endpoint, copying an IPC/PCB as
a Notification, overwriting managed handles, and acquiring undelegated I/O/IRQ
authority. It verifies that denied transfers preserve a usable destination.
`core-process` also tests empty slots left by foreign revocation: COPY can fill
them without a failing REMOVE, while an occupied IPC/unknown object is never
deleted by the notification-transfer path. This distinction preserves window
notification replacement after another application exits.

`timer-wait` delivers a completion during the scheduling RPC while bound through
an IRQ alias, verifying the exact cached descriptor and preserved unrelated
flags. `virtio-lifecycle` checks PCI INTx masking in polling mode and device-only
completion cleanup without an unowned IRQ ACK. Real virtio boots now pass on
1 and 4 cores; USB 4-core boot and the full AHCI desktop workflow on 1/4 cores
also pass. Desktop tests include monitor pause/refresh/tabs and Saran
draw/undo/save/resize/close/reopen, followed by independent persisted-BMP decoding.
xtask passes 24 tests; the C++ registry/managed-client wire test passes
AddressSanitizer/UBSan. These checks do not establish safe Cspace/backing or
quota reuse; the pinned-Kernel requests remain separate acceptance gates.

`linux-procfs` checks the production process-backed procfs implementation:
execution-context isolation, numeric PID and thread views, stable directory
cookies during process removal, Linux stat field positions and units, process
states, short reads/EOF, live NUL-separated arguments and proc directory paths.

`linux-task-control` tests production clone3 structure decoding, stack/TLS/TID
translation for the host architecture, size/tail validation, prctl comm updates
and copy faults, irreversible no_new_privs and root-only credential validation.
`linux-sysv-shm` checks actual namespace/key/lifetime decisions: cross-context
access rejection, deferred removal/key recreation, per-context limits including
removed-but-attached segments, IPC_STAT ABI and last-context cleanup. VM IPC is
mocked in these unit tests; the integer-only Tiny Core guest fixture separately
tests real shared aliases, fork, guard pages, protection, partial unmap and
reclamation on QEMU with one and four cores.

Normal builds reject unrelated syscall numbers before AF_UNIX descriptor
lookup. `NANAMI_UNIX_DISPATCH_FILTER=0` restores the original lookup-before-reject
path for comparison or rollback; `=1` explicitly enables the normal guard.
`linux-unix` verifies that unrelated syscalls
and poll/select completion retries perform no FD lookup, guest copy or signal
delivery; the optout performs the original lookups. Supported operations still
resolve the live descriptor, including after reuse. Run this configuration with
`--test-threads=1`. Lookup counts prove eliminated work, not a wall-time gain.

The default readlink path uses a request-local negative
result: the existing rooted no-follow lookup already returns a file
type. After preserving output argument validation, confirmed regular/directory/
socket types return EINVAL without a second READLINK request. Symlink, unknown
type and virtual paths retain the original fallback. No pathname classification
survives this request. A concurrent mutation can linearize a negative readlink
at the observed non-link; later calls always perform a fresh lookup.
`NANAMI_READLINK_KIND_FASTPATH=0` restores the original second-request path for
comparison or rollback; `=1` explicitly enables the default optimization.
`linux-path-lookup` runs the production wrapper/rooted transport with a
backend model to check query counts, no-follow/dirfd/truncation/type replacement,
unknown/virtual delegation and lookup/output error order. Real virtual output
and protected pages require the strict KVM fixture described in alter-linux.
The real Files initialization window showed READLINK requests fall from 67 to
21 with the same 697 syscall entries. This establishes eliminated work, not an
exact wall-time improvement percentage. Fine timing variance alone is not a
reason to keep the functionally verified reduction disabled.

Normal builds use bounded position hints for UNIX socket
lookup. Exact IDs are checked before using a hint; collisions fall back to the
original scan. `linux-unix` covers capacity/churn, stale IDs, both swap-removal
paths, mutable access and accept/SCM_RIGHTS ownership roots. Storage is 128
words per socket collection, with
no additional socket limit or allocation. Functional success does not establish
an application speedup. `NANAMI_UNIX_SOCKET_LOOKUP=0` restores the original
scan with no hint storage; `=1` explicitly enables the default hints. The normal
hint and coalesced paths are also tested together with the default dispatch
guard and readlink reduction.

Normal compatible read-only opens use `VFS_PATH_OPEN_READONLY` to resolve and
acquire a handle in one rooted request. POSIX translates that handle into its
owned FD, then Linux creates the local FD. This removes the second successful
pathname walk and POSIX open request. `NANAMI_ROOTED_READONLY_OPEN=0` restores
the two-request route; FD cleanup on local allocation failure remains fixed.
Create/truncate/write/other flags retain the old route, as do declined/missing
cases. Virtual mount priority is preserved even after a disk symlink resolves
into a virtual path. At POSIX capacity the proxy resolves only, acquiring no
handle; it uses the same3-based slot rule as ordinary allocation, while dup
retains its0-based rule. No persistent path/authority cache is introduced.

The separate `POSIX_REQUEST_OPEN_ROOTED` entry lets older POSIX servers reject
before forwarding an operation whose owned handle they cannot translate. Only
legacy rejection/explicit unsupported may fall back; other errors are retained.
Update ext2 and POSIX together for the optimized route. An old stack remains
functional but can pay an unsupported probe; explicit optout avoids it.
`NANAMI_ROOTED_OPEN_DIAG=1` emits one general and one xfe-specific reachability
line for functional testing; normal builds omit these logs. Native tests run
the production Linux opening/namespace code, real POSIX state/FD allocator/
proxy, ext2's transactional completion helper and the actual client wire code.
The real C fixture/probe adds `ROOTED-OPEN-PASS`/`NANAMI_REQUIRE_ROOTED_OPEN=1`.
Evidence: `session-handoff/logs/linux-native-scope-20261107/rooted-open/`.
This proves eliminated requests and functional reachability, not a precise
launch speedup. Historical wait-shape step counters select RESOLVE/READLINK;
they do not include the new OPEN operation and are not total request counts.

Optional Alter `wait-shape` is a structural diagnostic for the first executable
named xfe. It buffers at most64 wait events and reports overflow, using existing
timeout/deadline/readiness values with no additional clock reads. Phase0 is an
immediate poll scan (before copyout),1 a parked poll wait,2 a wake still pending,
3 a handler return value,4 a handler error. Wake rows also include blocking I/O;
these may have no recorded poll start. Cached ticks are not elapsed durations.
Rows are reported once after existing timer notifications advance two backend
seconds, so serial printing does not interrupt every recorded wait. This does
not establish measurement overhead, CPU cost, wall-time attribution or readiness.
The pure `wait-shape` test covers bounded storage/PID isolation/report scheduling;
native clocks/readiness harnesses stub the serial adapter. Cross-build and KVM
are required to test actual image selection and reporting. Default builds omit
the diagnostic entirely.

The wait diagnostic also records exact socket/peer IDs per row and one owned
peer-address/live-FD-holder snapshot. It matches current open-description IDs,
counts every matching live descriptor, saves up to4 executable basenames, and
flags name/address truncation. Creator credentials do not establish ownership.
If an accepted endpoint is still pending without a holder, identity capture is
retried only on later buffered events of the first connection. A reused FD or
another PID/connection cannot replace that identity. This identifies an endpoint;
it does not establish its processing time or CPU share.

The diagnostic can also bracket recorded event points using successful existing
`refresh_clock` samples. Each row keeps the preceding sample and the first later
sample; later work never widens that upper bound. Rate changes, regressions and
cache/sample mismatches mark the window inconsistent; missing bounds stay
missing. `clock-consistent=1` means observed sample consistency, not independent
counter accuracy or instrumentation admission. With same-epoch fresh samples,
adjacent event separation lies in `[max(0,next.lower-prev.upper),
next.upper-prev.lower]` provider ticks. Phase1 follows scan/alarm setup, phase3
precedes register write/resume: neither is an exact total blocked-time boundary.
No additional clock read/RPC/alarm is introduced. The model tests first-sample
closure, missing/equal samples and permanent rejection after inconsistencies.

A fixed diagnostic census starts only if buffered row14 is a handler-result
record, and stops at the selected PID's next poll-scan record. It counts Linux
dispatch entries0..511 plus an outside bucket, including the closing poll.
The window is fixed prospectively from earlier evidence, not moved to whichever
interval looks largest. Other PIDs and out-of-window calls are excluded. Output
includes total entries, nonzero kinds and overflow; require all rows/totals to
match before treating the census as complete. Missing/incomplete windows stay
incomplete. Counts do not classify page faults, native resume, successful
results, user CPU or handler time.

Inside that fixed gap, successful `read_c_string` inputs can be snapshotted
before SHM translation/reuse. Up to128 distinct raw strings retain192 bytes
each; truncated prefixes never merge, and hit/drop/allocation/overflow totals
are explicit. Hits aggregate operations; `first-call` is only the first observed
dispatch. Only the last string in a dispatch receives a matched immediate handler
result, once. Park/unknown remains unknown. Text uses escaped printable ASCII,
otherwise raw bytes. Paths are not canonicalized; relative names and successful
open results alone do not prove file existence, open mode or cache validation.

The production `MemoryManager` tests also cover large GUI frame-node metadata:
lazy overflow beyond the primary process arena, repeated root-slot reuse without
additional backing allocation, isolation between processes and failed-revoke
retry. This is reserved capability metadata, not anonymous RAM returned to the
buddy allocator. The independent `--tinycore-apps` QEMU test validates the
installed upstream applications' viewable X11 windows; see the
[Tiny Core recipe](../../thirdparty/tinycore/README.md).

Bounded same-pool-Frame COW fault batches now reserve replacement RAM and slots
once and prepend all deferred page owners once. They retain actual source bytes,
per-VA references, final-owner reuse and all leaf acknowledgements. Fragmented
RAM/slots fall back to point allocation; conversion/copy/MAP failure retains the
unpublished suffix until acknowledged cleanup. `NANAMI_PRIVATE_BATCH_BACKING=0`
uses the point path. Tests cover64-to1 allocation requests, nonzero source bytes,
last-owner reuse, fragmentation, fatal allocation rollback, failed suffix
retirement/retry, and shared-owner slot holes/overflow. The preceding checkpoint
is `cow-batch-backing`; its Release/Debug each2654/0/20, optout522/0/2, and six
KVM runs pass. See its source guards and raw evidence under `session-handoff`.

The follow-up COW Frame ceiling and private-write proof cache trials were
implemented and reverted. Frame counts were unchanged39328/launch1536 while
conversion ranges increased59->74. The final proof-cache candidate passed
Release/Debug2663/0/20, related default/optout148/0/2, disabled views1, and eight
KVM runs, but saved only7 preparations in the Files PID42 flow and showed no
whole-launch benefit in two comparisons. Accepted source/binaries were restored
exactly; those candidate tests do not replace the accepted2654/0/20 result.
Runtime bracketed tags are padded to20 columns: diagnostic parsers must allow
whitespace after `[`. Small source/patch/raw records and the parser correction
remain under `cow-batch-backing/rejected-preparation-hints-20261113`; no additional
image, snapshot or PNG generation was retained for those rejected trials.

The preceding `page-table-ready` checkpoint also reuses the existing confirmed
4K table hint inside `ensure_page_tables`, including direct page-view callers.
Unknown regions and huge-leaf preparation keep the Kernel query; table/huge
invalidation and every leaf MAP/UNMAP acknowledgement remain unchanged. No new
cache, ABI or dependency. Release/Debug each2656/0/20; new cases cover512 repeated
preparations, separate address spaces, huge queries, leaf UNMAP, invalidation,
and query/conversion failures. Four normal KVM runs and two census runs pass;
strict/GUI SMP4, timing/census SMP1 on CPU0, genuine huge16 and five apps pass.
Launch view work: control472 requests/472 MAP/472 table queries; new473/473/4.
These request sets differ and cover selected handler wrappers, not all Kernel
work. Normal ping1.518105662s versus fresh control1.316091133s does not establish
a whole-launch benefit.100ms paint/input readiness remains unachieved. Three
earlier observer-input timeouts, the corrected fixture failure, exact probe
sources and pre-launch helper differences remain separate in the raw evidence.
Keep only current/previous generations and remove tmpfs after producers finish.

Current `page-view-vectors` batches up to17 exact supervisor page grants in one
Nanami RPC. Scalar authority, RAM pins, COW, rights and leaf acknowledgements
remain unchanged. Protect all prepared hits/slots before victim selection; wait
for victim-release ACKs before granting. Preserve a known successful prefix,
quarantine uncertain entries and retain every requested VA on a lost/malformed
reply. No output stores precede complete preparation. `NANAMI_BATCH_PAGE_VIEWS=0`
retains scalar grants and the initial checked-copy admission. Update Alpha and
both Alters together; no Kernel/target/FPU/priority/dependency changes.

Final Release/Debug each2672/0/20, related optout414/0/2; eight KVM actual0 include
normal/diagnostic strict, three normal timing conditions, two diagnostic timing
conditions and five GUI apps/pointer/cleanup. Genuine guest huge16 in both strict
runs. Real guest17page batches complete twice; native tests verify bytes, pins,
prefix failures, failed cleanup, protected hits and lost/corrupt replies. A final
host fixture correction imports actual UAPI status values and checks FATAL5;
the old VM fixture hash is preserved separately from final native proof. Product
sources and canonical bytes are unchanged after VMs, and cross-build reproduces
the tested binaries. See `post-vm-test-change.json` for that one host-only path.

Launch vector work is34 requests/92pages, including Files PID42=24/53. This is
not a complete RPC/Kernel census or an exact delta from cold checked copies.
Normal1.315686178s, same-source optout1.315418358s, fresh preceding image1.315855592s
are single100ms guest/200ms host poll observations; no whole-launch gain or100ms
paint/input readiness is established. Keep current/previous only and retire
scratch after actual producers terminate. Evidence:page-view-vectors in the
session-handoff log tree.
