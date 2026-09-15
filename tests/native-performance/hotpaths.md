# Native hot-path follow-up

These changes are confined to Honoka, Shell and terminal-service. A9N, Alpha,
the service protocols, shared-memory ownership, notification conditions and
hardware framebuffer accesses are unchanged.

## Removed work

- Honoka previously parsed/scaled/converted the wallpaper again on every
  background damage update, including behind translucent Shell windows. It now
  renders the static wallpaper/menu once into an owned, display-format cache,
  then copies only damaged rows through the existing framebuffer blitter.
  Clock, windows and cursor remain dynamic. The original renderer is retained
  when allocation fails or the optional cache exceeds its 32-MiB budget.
  Geometry/theme are fixed for the compositor lifetime; future changes to
  either must rebuild/invalidate this cache.
- The cache occupies 7.91 MiB of application heap at 1920x1080, and has an
  initial generation/zeroing cost. It can reuse the heap released by font
  initialization; this is a memory-for-repeated-rendering tradeoff, not zero
  additional memory use. It does not expose the hardware framebuffer to clients.
- Full Shell scrollback formerly shifted 127 rows of 89 characters and their
  u32 colors on each append: 56,515 bytes of old data moved, plus the new line.
  A circular buffer now overwrites only one line (445 bytes) and advances its
  head. Logical indexing is used for rendering, partial-line replacement,
  prompt removal, clear and scroll positioning.
- Cursor blinking no longer rewrites the prompt/colors or calls
  `scroll_to_bottom`. It repaints/presents the prompt's row only if visible,
  so reading older output is not interrupted by the timer.
- Terminal output and non-echo input now copy at most two contiguous spans
  into the private byte ring; reads do the reverse. Ring cursors/length change
  once per transfer, not per byte. Empty/full/short transfers, zero-length
  availability queries, editing/echo and notification rules are retained.
  Mapping bounds are still checked by the handlers before raw-pointer copies.

## Reproduce the host checks

The tests compile the actual new scrollback/ring/cache modules and the existing
Framebuffer implementation. Only the display-service IPC endpoint is mocked;
attempting to present the offscreen cache fails the test. This does not test
hardware or IPC timing.

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --test hotpaths
cargo test --release --manifest-path tests/native-performance/Cargo.toml --test hotpaths
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test hotpaths
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test hotpaths hotpath_benchmark -- --ignored --nocapture --test-threads=1
```

Tests compare 20,000 scrollback operations against a deque, including wrapping,
pop/replace/clear and character/color consistency. Another 20,000 operations
check terminal partial transfers against a deque, with wrap/full/empty cases
and canaries around the read destination. Cache tests compare pixel output
against a direct reference draw for 1,000 damage updates per channel layout,
including padded destination stride and translucent overlays. Oversized,
zero and overflowing cache geometry must fall back without drawing.

Host release timing on aarch64 macOS, nightly-2026-03-25, 2026-09-15:

| Isolated workload | Previous logic | Updated |
| --- | ---: | ---: |
| Ring write + read, 1 byte | 3 ns | 5 ns |
| Ring write + read, 1 KiB | 3,379 ns | 29 ns |
| Ring write + read, 4 KiB | 14,024 ns | 116 ns |
| Append to full scrollback | 937 ns | 14 ns |
| Reference background, 712x396 | 617 us | 29 us |

Each value is the median of five batches. Ring/scrollback batches contain
10,000 operations, background batches 100. The terminal baseline retains the
original push/pop implementation; the scrollback baseline isolates its old
full-buffer shift. The background workload uses a deterministic pixel-color
pattern through production Framebuffer code, **not the actual PNM wallpaper**;
cache initialization is excluded. These are local algorithm comparisons, not
Nanami application throughput or physical-hardware speedups. One-byte ring
transfers show no improvement; no across-the-board speedup is claimed.

## Guest checks

Build an image with the existing Linux fixtures, then run:

```sh
LINUX_ROOTFS_DIR="$PWD/out/alter-glibc-root" make image
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 4 --ui-stress
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 1
```

The optional UI pass fills scrollback beyond 128 lines, writes a recognizable
bottom marker, scrolls up/down, clears, recalls a command, and moves the mouse.
Inspect `scroll-up.png`, `scroll-bottom.png` and `screen.png` in its printed
temporary directory. The marker should be absent when scrolled up, present at
the bottom, and `ring-cache-ok` should appear twice after command recall.
The one-second pause after page movement also exercises cursor blinking while
reading older output. The runner uses snapshot disks and temporary firmware
variables; no physical disks are accessed.

Known pre-existing limitation: PS/2 tags extended navigation keys with bit
0x100, but Shell matches only unextended codes (e.g. 0x48, not 0x148). The UI
test therefore uses keypad navigation codes. That input-code mismatch is not
changed by this performance work.

Validation on 2026-09-15: all target-selected Rust applications/services built
in release for x86_64 and AArch64. The three Linux guest fixtures passed with
one and four QEMU CPUs. The final four-CPU UI screenshots showed the expected
marker visibility, stable scrolled-up view across cursor ticks, and two echoed
`ring-cache-ok` commands after clear/history recall. Desktop, translucency,
text and moved cursor were visually inspected. Physical hardware and AArch64
runtime were not tested. The A9N source diff and kernel ELF SHA-256 remained
identical to the pre-change snapshot.
