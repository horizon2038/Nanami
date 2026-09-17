# Configurable Alter fbdev

Host tests compile the production CLI, dimension validation/packing, and fbdev
screeninfo serializers:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml --test alter-framebuffer
RUSTFLAGS=-Zsanitizer=address cargo test --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test alter-framebuffer
```

Build the Linux fixture (from the repository root, with Zig installed):

```sh
ZIG_GLOBAL_CACHE_DIR="$PWD/doomgeneric/build/zig-cache" \
ZIG_LOCAL_CACHE_DIR="$PWD/doomgeneric/build/zig-local" \
zig cc -target x86_64-linux-musl -O2 -static -Wl,--image-base=0x400000 \
  tests/alter-linux/framebuffer.c -o out/fb-size-test
```

Add `./out/fb-size-test` to the existing `EXTRA_LINUX_BINS` list and rebuild with
`ROOTFS_REBUILD=1`. Retain the usual Doom, WAD, bash and busybox files. Then run:

```sh
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img \
  --smp 4 --hpet on --usb-storage --bash-smoke --no-network --framebuffer-smoke
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img \
  --smp 4 --hpet on --usb-storage --bash-smoke --no-network \
  --doom-first --doom-fb-size 640x400 --bash-input-stress 1
```

Each run uses a private disk copy/snapshot and no physical devices or network.
The fixture checks 640x400, non-page-aligned 641x401, the legacy/default 800x600,
and a 4096x2160 request clamped to a smaller desktop (the default test firmware
uses 1920x1080). For each case it verifies stat before open, fstat, screeninfo,
stride, byte length, SEEK_END, oversized mmap rejection, mapped first/last pixels,
read/write/EOF, and munmap/remap. Children fork and exec both before a session
exists and after it has been created. All parent and child checks must exit 0.

The Doom test captures the display and then checks normal quit, subsequent bash
input/storage writes, cursor motion and clock updates. This is functional
coverage, not a physical-machine FPS measurement.

## Validation and shared-memory regression

The 6 new host tests pass normally and under ASan; the full host suite passes
301 tests (6 opt-in benchmarks ignored). Alter, Alter/Linux and Alter/FreeBSD
build for x86_64 and AArch64. QEMU's 640x400 and 641x401 cases pass, including
fork/exec and remapping. The 640x400 Doom display was visually checked, followed
by normal quit and successful bash/USB-input/storage/clock checks.

The initial sequential fixture exposed an Alpha shared-memory lifetime bug:
after the custom-size cases, the final parent mmap in the default case failed
with EIO. A failure-only diagnostic identified Honoka (pid 11), frame node 27,
slots 13731..14200: its process metadata arena was exhausted, not physical RAM.
Every allocation had consumed fresh frame slots, while release only unmapped
the pages and deferred their ownership until process exit.

Alpha now removes each released shared mapping's capability copies and reuses
its slot/VA ranges, coalescing adjacent holes. Physical pages are freed only
after the last peer reference is gone. This does not increase arena sizes or
modify the kernel. The complete sequential QEMU fixture now passes all four
sizes, **including fork/exec and remapping in the clamped case**.

To isolate a case, append `--framebuffer-case default` or
`--framebuffer-case clamped`; the latter intentionally runs only mapping/remapping
without fork. To stress one long-lived window owner with **128 mmap/munmap pairs**,
append `--framebuffer-case stress`. Run with `--smp 1` and `--smp 4` for both
single-CPU and SMP coverage.
Both CPU configurations pass the 128-pair stress case with HPET and USB storage.

Production reservation and transaction tests are part of `--test core-process`:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml --test core-process
RUSTFLAGS=-Zsanitizer=address cargo test --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test core-process -- --test-threads=1
```

These cover 50,000 mixed-size reservations, hole splitting/coalescing, fixed-VA
exclusion, both peer release orders, exec/reap, reuse while a peer keeps an old
mapping, and failures during allocation, capability copying, and page mapping.
They check that RAM is never freed while a process capability still owns it,
and that a failed mapping does not unmap an existing fixed-address mapping.
All 16 core-process tests pass normally and under ASan (one benchmark ignored).
