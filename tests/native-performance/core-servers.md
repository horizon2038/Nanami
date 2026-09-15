# Core and server hot-path review

Baseline: `f0f077216336fd21c35cca96b45362e37520a2ae` (2026-09-15).
No A9N source, kernel ABI, register-writeback logic or hardware timer interface
was changed. This pass targets repeated request/interrupt work; it does not
claim that every subsystem is now maximally optimized.

## Implemented

### Alpha process and VM lookup

`ProcessManager` keeps its existing process/VM vectors in PID order and uses
binary search: O(log N) lookups without an auxiliary index or cache. Install,
reap and failed-spawn discard preserve ordering. Removal now shifts entries
(O(N)); this favors frequent lookups over less frequent process teardown.
Tests include out-of-order installation and 1,000 install/reap/replacement steps.
Exec resets the existing boxed `VmSpace` rather than allocating a new box.
DMA mapping resolves the process VM once outside the per-page loop.

### Shared HPET/PIT timer service

An earliest-deadline value lets ticks before expiration return without scanning
512 slots or initializing the 512-word expiration buffer. Scheduling, periodic
rearming and failed-notification retirement maintain it. Expiring ticks still
scan the fixed table. Missed periodic intervals advance arithmetically while
preserving phase. Saturation retains the final representable deadline; at
`u64::MAX` it fires once and retires instead of looping indefinitely.
IRQ acknowledgement, lazy hardware start, PIT/HPET selection and clock access
are unchanged. No userspace architectural timer exposure was added.

### ext2 data/block I/O

- Full-block overwrites and zeroing skip old-data reads. Partial blocks retain
  read-modify-write, preserving bytes outside the requested range.
- Physically contiguous **already allocated** full blocks are written in runs
  up to the existing 16-KiB scratch capacity. Block-map traversal completes
  before payload staging because traversal can overwrite the same scratch.
- Existing-allocation overwrites within the old size skip unchanged inode writes
  and block-accounting walks. Growth and allocations still persist metadata,
  including allocations through existing indirect tables inside the old size.
- Each successfully written block caches its own buffer slice. Short/failed
  writes invalidate affected cache entries, return an error and are not retried.
  Writes remain synchronous; no write-back cache or weaker durability was added.

For 1-KiB blocks, a contiguous 64-KiB overwrite now takes **4 data-write IPCs
instead of 64**, without old-data reads or an unchanged inode write. The host
test checks the exact requests and every output byte. Fragmented allocations
and partial edges form smaller requests. AHCI/virtio-blk use their existing
multi-block API; DMA ownership and hardware command semantics are unchanged.

### Input distribution

A driver-queue batch retains event order, subscription masks and the 512-event
budget per driver, but sends one final notification per affected subscriber.
Single-request publication still notifies immediately. A concurrent consumer
draining the queue does not suppress the final wakeup. Shared-queue memory
ordering is unchanged. Tests cover both shared and conventional queues.

Timer state/dispatch, input state/distribution, and ext2 block/data I/O were
extracted into bounded modules rather than enlarging service main files.

## Measurements

Apple Silicon host, release Rust nightly-2026-03-25, medians of five samples.
These exclude kernel IPC, interrupts and devices: they are **not** bare-metal
Nanami throughput measurements. Uniform PID access measures entry+VM lookup;
it does not guarantee faster first-entry lookup or measure insertion/removal.

| Process count | Before, ns / entry+VM lookup | After |
| ---: | ---: | ---: |
| 16 | 6 | 5 |
| 64 | 22 | 8 |
| 256 | 94 | 12 |
| 1,024 | 441 | 19 |

Deadline-before-expiry timer dispatch changed from 286.118 to 0.486 ns/tick.
This isolates a highly predictable guard, not total interrupt-processing cost.

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test core-process --test timers benchmark -- --ignored --nocapture --test-threads=1
```

`NANAMI_PROCESS_SOURCE` and `NANAMI_TIMER_SOURCE` select saved baseline source.
For the old monolithic timer source, extract `schedule_timer` through
`milliseconds_to_ticks`, add `use super::*`, and expose `schedule_timer`,
`ensure_timer_started` and `fire_expired_async_timers` to the harness parent.

## Other reviewed paths / remaining work

| Area | Finding / decision in this pass |
| --- | --- |
| Core physical allocator / heap | Physical allocation/reference bookkeeping still scans vectors. Alpha still uses a bump allocator with no-op deallocation, unlike native apps' TLSF. Replacement needs a separate bootstrap/reclamation test pass; it was not silently swapped. |
| Communication / registry | Bounded service-name lookup is mainly connection/startup work, not the payload path; left intact. |
| Driver-manager / system-manager / RTC | Preserve platform detection, boot-device matching, launch setup and stable RTC sampling; no discovery or privilege changes. |
| AHCI / virtio-blk / RAM disk | Existing synchronous buffers benefit from ext2 batching. Client-buffer DMA would need a separate lifetime/isolation design. |
| Alter / POSIX | Delegated read/write and writev batching remain intact. Core/storage improvements reduce downstream work without new request codes. |
| PS/2 / input / terminal | PS/2 already batches publication and terminal already uses bulk ring transfers. Change only repeated input-server notifications. |
| Honoka / framebuffer / Shell | Existing dirty rectangles, opaque copies, background/glyph caches and scrollback ring remain. No visual tradeoff was introduced. |
| net-server / virtio-net / HTTP | Shared buffers, direct receive and HTTP response caching already exist. Per-packet backend IPC, receive pumping and synchronous transmit remain candidates. Offline QEMU fixtures do not measure network performance. |
| Monitoring / sample apps | Periodic diagnostics/demo rendering are not representative throughput workloads; no feature was removed to improve benchmark numbers. |

## Validation

64 host correctness tests passed in release and AddressSanitizer configurations;
four timing benchmarks are ignored by default. New tests include the production
process manager, timer logic, input distribution, and ext2 transfer/cache loops,
with external services and fixtures mocked.

All selected native Rust apps/services and Alpha built for x86_64 and AArch64.
QEMU 1/4-CPU runs passed Linux syscall smoke, glibc-true and glibc-regression.
The Linux fixture additionally overwrites/rereads 280 KiB across both ext2
indirection levels and verifies a partial-block overwrite. The 4-CPU run also
exercised input, scrollback wrap, page scrolling, clear and command history.
Tests use snapshot disks, temporary firmware variables and no networking;
the user's existing rootfs image was preserved.

A9N's kernel ELF stayed byte-identical (SHA-256
`bb98956fa845b877fc3067cbd7c23c9995f2d30f848b78e99455bc488a50bb5a`).
AArch64 runtime, bare-metal throughput and network throughput remain unmeasured.
