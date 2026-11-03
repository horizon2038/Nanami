# Native application heap performance

The ongoing evidence, acceptance rules and rejected experiments are recorded in
[Nanami / Alter optimization cycles](optimization-cycles.md).

Cycle97 is paused at the user's request. The working tree retains a sealed-exec
bulk-retirement candidate and destructive-exec error handling. Native1264,
ASan153, ARM checks, SMP1/SMP4 ownership tests and two complete GUI sessions
pass, but its timing comparison was interrupted. Accepted cycle96 remains the
performance baseline; do not claim a cycle97 speedup or release acceptance.

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
snapshot cache. `alter-image-cache` runs the production loader with mocked
service requests, including cache hits, mutable files, partial I/O, buffer reuse
and mmap segment reuse. Its current cache admits up to 48 small-file snapshots
within the original 16-MiB retained-data budget, rather than allocating 48
maximum-size buffers. Admission includes page-rounded/geometric capacity and
falls back to the existing uncached buffers when necessary. PT_INTERP loading
explicitly protects the outstanding main ELF's address from replacement.
Tests also cover a near-ceiling whole-file snapshot, immutable old-version
bytes, warm segment reuse and over-ceiling no-prefetch/no-allocation fallback.
[Cycle 40](optimization-cycles.md#cycle-40-admit-medium-dsos-within-the-existing-snapshot-byte-budget)
records the rejected 4MiB per-file candidate; production retains the 2MiB
ceiling and original 16MiB total capacity bound.
Tests cover a 32-file resident set, large-file churn at the byte limit and an
interpreter that must bypass the cache to preserve its live main image.
`ext2-file-versions` checks bounded token eviction and
exhaustion; storage/truncate tests also compile the real invalidation hooks.
`alpha-initialization-window` checks stable pool-frame mappings, bounded collision
eviction, and MAP/UNMAP failure ownership. Its 2,048-entry cache uses an 8-MiB
alias range and 32 KiB of metadata, not an 8-MiB data buffer. Tests also cover
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

The production `MemoryManager` tests also cover large GUI frame-node metadata:
lazy overflow beyond the primary process arena, repeated root-slot reuse without
additional backing allocation, isolation between processes and failed-revoke
retry. This is reserved capability metadata, not anonymous RAM returned to the
buddy allocator. The independent `--tinycore-apps` QEMU test validates the
installed upstream applications' viewable X11 windows; see the
[Tiny Core recipe](../../thirdparty/tinycore/README.md).
