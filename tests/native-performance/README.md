# Native application heap performance

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
- Alpha has a bump allocator, so the new indexes use recycled arena nodes and
  backward-shift hash deletion, not drop/reallocate tree nodes. Metadata capacity
  grows geometrically with the high-water live set and is reused across churn;
  this is extra indexing metadata, not a claim of lower peak metadata usage.

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
