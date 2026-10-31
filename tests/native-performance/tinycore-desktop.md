# Tiny Core desktop performance (2026-10-30)

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
Alter now uses eight LRU content snapshots, at most 2 MiB each, with optional
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

Honoka compares eight sampled pixels per aligned group, combining differences
before deciding whether to update the capture. Alignment is checked once per
tile row, not once per pair. Odd alignment, padding and tail pixels retain a
bounded scalar path. All comparisons remain exact, and drawing still uses the
same captured values; the source is never reread after comparison. There is no
new FPU/Kernel support, reduced refresh rate or skipped input sample.

The integration probe's `--tinycore-perf` mode runs the production capture and
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
