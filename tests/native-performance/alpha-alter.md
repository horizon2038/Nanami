# Alpha / Alter transfer paths

This records the initial transfer optimization. The later
[poll/clock_gettime review](syscall-hotpaths.md) supersedes its per-transfer
MAP/UNMAP counts with a lifetime-managed copy-window cache.

This follow-up changes Nanami user-space only. A9N sources, syscall register
writeback, and the fault/reply ABI are unchanged.

## Alpha

`VmSpace` and `BootstrapVmSpace` each remember the last successfully mapped
leaf-table region, using one additional word (8 bytes on both targets).
The supported x86_64 and AArch64 configurations both use 4-KiB pages and
512-entry leaf tables. A subsequent map in that 2-MiB region skips
`GET_UNSET_DEPTH` and the `ensure_page_tables` function call. A miss uses the
original table construction path. No speculative failing MAP or new HAL
diagnostic is needed on first use.

This is a table-existence hint, not a cached frame or a retained mapping.
Unmapping frames leaves page tables intact. Exec replaces `VmSpace`, and reap
or failed-spawn cleanup removes it; the hint cannot follow a recycled process
address-space descriptor. Recording an intermediate page table alone never
marks a region ready. If table removal is added in the future, it must also
invalidate this hint.

For a warm process-memory-copy chunk using different frames, the two temporary
maps and two unmaps now need four kernel calls instead of six (the two depth
queries are gone). First use and lazy-page materialization are excluded from
that count. Source/destination mappings are still removed before return.

Other bounded changes:

- Resident frame lookup does not first query the process entry; that lookup
  is needed only for lazy materialization.
- Heap mapping resolves its VM tracker once per batch, not once per page.
- Frame allocation vectors reserve their known size once. This also avoids
  abandoning successive growth buffers in Alpha's existing bump allocator.
- Same-frame copying uses overlap-safe `ptr::copy` directly. The two copies
  through a 4-KiB static bounce buffer and that buffer are removed. Cross-page
  forward/backward traversal and temporary-map cleanup are unchanged.

## Alter

POSIX-file and terminal `writev` gather directly into their existing service
shared-memory buffer and commit once per filled buffer, or at the end of the
vectors. There is no additional staging allocation or memcpy. The normal
`write` transfer limit is retained, including the lower VFS scratch offset.
Other file kinds retain the existing per-vector handling; in particular this
does not merge datagrams or change pipe/device semantics.

A production-helper test with sixteen 256-byte vectors and a 4-KiB buffer
observes sixteen guest-memory transfers and **one** service write, instead of
sixteen service writes. This is a call-count result, not a 16x speed claim.
Vectors crossing buffer boundaries may need additional guest-memory transfers.
Short/zero writes stop at the committed byte count. If a later guest copy
fails, only successfully staged bytes are committed; partially copied bytes
from the failing vector are not sent.

The enlarged guest regression exposed an existing ext2 indirect-block bug:
`alloc_block` overwrites the shared scratch buffer with allocation metadata.
`ensure_data_block` now reloads the affected indirect block before updating its
pointer, for both single and double indirection. This prevents large file
writes from corrupting the block pointer table. It is a correctness fix, not
a claimed performance improvement.

## Verification

On 2026-09-15, all 22 host correctness tests passed in Release and ASan, both
native architecture builds passed, and all three guest fixtures passed on the
final x86_64 image with both one and four CPUs. This includes the single/double
indirection write/readback checks. `llvm-objdump` of both Alpha binaries also
confirms that a matching table-region hint bypasses `ensure_page_tables` and
goes straight to the MAP syscall/SVC.

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  -- --test-threads=1
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --lib --tests -- --test-threads=1
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 1
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 4
```

The host tests include the production VM trackers, AVL implementations and
vectored-write helper. They cover table-region boundaries, frame removal,
address-space separation/replacement, zero vectors, buffer splits, guest-copy
failures (including partial scratch writes), short writes, service failures,
invalid lengths, and exact copy/service-call counts. Alongside the earlier
heap/GUI tests there are 22 correctness tests, with timing workloads ignored.

The static guest smoke additionally verifies a 77,001-byte unaligned `writev`,
`/dev/null`, zero-length vectors, an unmapped later vector, read-only descriptor
rejection, terminal `writev`, readback, and a 280-KiB file reaching double
indirection on the fixture's 1-KiB ext2 filesystem. Existing dynamic fixtures
cover glibc/DSO loading, private/fixed mappings, fork and exec.

Runtime testing uses QEMU snapshot disks and a separate temporary rootfs;
the user's `out/ext2.img` is not rewritten. Alpha and the target-selected
Rust apps/services are also release-built for both architectures. AArch64
runtime and physical-hardware latency/throughput are not measured here.

The pre-task image contained A9N v0.3.1, whereas the already-current source
checkout builds v0.3.2. No A9N source was edited by this work, but rebuilding
that source changes the kernel artifact, so those images must not be used as
a controlled before/after timing comparison.
