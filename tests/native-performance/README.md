# Native application heap performance

`libnanami` now uses the no-std TLSF allocator from pinned `rlsf` 0.2.3.
This affects native Rust applications/services, including the native Alter
process. It does not replace a Linux guest's libc malloc, Alpha's allocator,
or anything in A9N.

The subsequent background-cache, scrollback and terminal-transfer work is
documented in [Native hot-path follow-up](hotpaths.md).

The Alpha mapping and Alter vectored-I/O follow-up is documented in
[Alpha / Alter transfer paths](alpha-alter.md).

The module split and POSIX delegated-write follow-up is documented in
[Alter / POSIX I/O](posix-io.md).

The lookup, timer, storage and input follow-up is documented in
[Core and server hot-path review](core-servers.md).

The HTTP ARP, TCP receive-flow-control, and NIC backlog regression tests are documented in
[HTTP / network regressions](../network/README.md).

The deferred-notification service receive regressions are documented in
[Deferred IPC notifications](ipc/README.md).

USB root discovery and late-attachment regressions are documented in
[USB input and root storage](../usb/README.md).

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
git show 9e43161d0f42c6abdc2e2b7a892a24065cb66384:nanami/servers/sdk/rust/libnanami/src/heap.rs \
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
