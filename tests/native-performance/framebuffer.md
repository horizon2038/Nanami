# Hardware framebuffer publication

Alter's mmap-backed fbdev clients do not have to submit damage or call a present
ioctl. Alter therefore notifies Honoka at 60 Hz while a mapped framebuffer is
active. Previously every notification copied the entire window content to the
hardware aperture, even when the guest had not produced another frame. Increasing
timer precision does not remove that traffic.

fb-server now keeps the last published pixels in private ordinary RAM. It compares
64-byte spans of a requested region and copies only changed spans to the hardware,
merging adjacent changed spans into longer copies. There are no hashes (and thus
no hash collisions), hardware reads, framebuffer cache-attribute changes, or kernel
changes. PRESENT still acknowledges the requested pixel count, including unchanged
pixels. Honoka's synchronous request keeps its shared canvas stable during the
comparison and copy. This does not synchronize the guest's drawing with Honoka;
the existing possibility of tearing in mmap clients is unchanged.

The shadow requires `stride * height` bytes, about 10.55 MiB at 2560x1080x32. It is
initialized to the same white as the hardware startup fill. Allocation failure
retains the original direct-copy path. Screen/mode changes and any future second
writer to the hardware aperture must invalidate/recreate this shadow.

This trades extra RAM reads/comparison and shadow writes for fewer hardware
writes. It is **not** expected to speed up a fully changing framebuffer mapped to
fast RAM. A host 640x400/600-present workload measured roughly 18–31 ms for direct
RAM copies versus 27–60 ms with comparison at 10–60 changing frames per 60 presents.
At 10 changing frames per 60 presents, hardware-destination traffic fell from
614,400,000 to 101,376,000 bytes; completely unchanged frames write zero bytes.
These are algorithm/traffic checks, **not physical-machine FPS measurements**.

## Doom fbdev client

The source in `doomgeneric/` now avoids fractional enlargement: the default
640x400 Doom buffer occupies 640x400 at (80,100) in Alter's 800x600 canvas,
instead of being resampled to 800x500. Larger screens select an integer scale;
smaller screens retain downscaling. Alter's framebuffer mode is unchanged.

The matching 32-bit 1:1 path skips the coordinate and color lookup tables. It
replaces the unused source alpha byte with the framebuffer's opaque value and
stores pixels directly, respecting stride and offsets. `llvm-objdump` of the
x86_64 Zig/musl release build shows SSE2 `movdqu` / `pand` / `por`, with no
per-pixel `memcpy` calls (including the generic packed-store fallback).
This reduces client drawing work, not the compositor's 800x600 damage extent.

Alter now also accepts `-g --fb-size 640x400` before the executable path. With
that option the Honoka content buffer itself is 640x400, reducing composition
and requested presentation area as well as guest drawing. Without the option,
the requested canvas remains 800x600. Honoka may clamp dimensions; fbdev reports
the actual content size. See `tests/alter-linux/framebuffer.md` for size/mapping
and fork/exec regression tests.

`make -C doomgeneric test-fbdev` tests the production rendering helpers;
`doomgeneric/tests/README.md` documents sanitizer and cross-build commands.
The installed binary is `bin/doomgeneric-fbdev`; rebuild rootfs after updating
it. Physical FPS still needs measurement, independently of these pixel and
machine-code checks.

## Physical-machine measurements

Build with `NANAMI_FB_PROFILE=1` in the environment, retaining the normal image
options and setting `ROOTFS_REBUILD=1` so the new fb-server reaches rootfs. For
example, prefix the existing `make image` / `make run` command with
`NANAMI_FB_PROFILE=1 ROOTFS_REBUILD=1`.

After boot, `unchanged-pixel cache bytes=...` confirms whether the shadow exists.
`[fb.perf]` reports a batch approximately every two seconds **when presents arrive**:

- `interval-ms`: wall time covered by the batch, including idle time.
- `presents` / `changed`: requested regions / regions that wrote any hardware bytes.
  These are **not Doom's FPS**: multiple rectangles can belong to one frame, and
  desktop clock/cursor/Shell updates are included.
- `requested-kib` / `written-kib`: full requested region bytes / actual copied bytes.
- `service-us` / `max-us`: total / maximum bracketed presentation wall time.
  These include comparison, copying, preemption and timer IPC overhead; they are
  **not pure MMIO execution time**. Serial output is outside each timed operation.

Compare gameplay over several consecutive batches, excluding initial full-screen
rendering/window creation. Large service times implicate publication or scheduling
delays; small times do not distinguish guest rendering, compositor work and waiting.
For an A/B test, rebuild with `NANAMI_FB_PROFILE=1 NANAMI_FB_CACHE=0` to retain the
same measurement path but disable the shadow. Remove `NANAMI_FB_PROFILE` for a normal
build: it performs no additional timer IPC or per-frame diagnostic output.
Neither the reported approximately 10 FPS nor its primary cause is considered
resolved until physical-machine measurements confirm it.

### Composition versus presentation

The same `NANAMI_FB_PROFILE=1` build now also reports `[honoka.perf]`, using the
already connected timer service (no direct access to a hardware clock):

- `renders` / `rects`: compositor batches / submitted rectangles, not guest FPS.
- `compose-us` / `max-compose-us`: total / maximum time spent composing a batch
  into ordinary shared RAM.
- `present-us` / `max-present-us`: total / maximum synchronous display-request
  time for a batch, including fb-server work and IPC/scheduling delays.
- `interval-ms`: elapsed batch interval including idle time, starting with the
  first measured render rather than system boot.

These are wall times; preemption and the extra timer IPC are included. The
diagnostic itself adds three clock queries per nonempty compositor batch, plus
the existing fb-server measurements. It does not measure time spent blocked
waiting for a render notification or guest-side rendering directly. An empty
render pass does not query the clock. Disabled builds contain neither the new
monotonic clock-query calls nor profile logging; inspect the release binary when
changing these hooks.

Use several consecutive gameplay batches from **both** logs, excluding startup.
High composition time points to compositor work or its scheduling; high
presentation time with high fb-server service time points to publication or
fb-server scheduling. High presentation time with low fb-server service time
points to IPC/queueing or diagnostic overhead outside that bracket. If both are
small, investigate guest execution, syscall service and timer wakeups next.
The two profilers have independent batch boundaries: compare rates over several
batches, not raw subtraction of two adjacent log lines. Neither proves the
framebuffer's memory type or measures hardware bandwidth in isolation.

For example, a four-CPU QEMU HPET/USB-root gameplay batch measured 122 compositor
renders in 2000 ms, with `compose-us=120840` and `present-us=402805`: about 0.99 ms
composition and 3.30 ms synchronous presentation per render. This validates the
measurement path, not the physical machine's performance or Doom's frame rate.

## Regression tests

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test framebuffer --test honoka-motion
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml --target aarch64-apple-darwin \
  --test framebuffer --test honoka-motion
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test framebuffer framebuffer_benchmark -- --ignored --nocapture
NANAMI_FB_PROFILE=1 cargo test --release \
  --manifest-path tests/native-performance/Cargo.toml \
  --test graphics-profile --test honoka-motion
```

The tests compile the real presentation/damage modules, substituting guarded RAM
for the hardware aperture. Coverage includes duplicate frames, coalescing changed
spans, 3,000 random clipped/overlapping rectangles against the direct-copy reference,
padded strides, partial spans, cursor restoration, failed requests and allocation
fallback. Existing real-compositor motion tests check cursor/drag pixel equivalence.
The profile tests check disabled/missing-port behavior, clock frequency conversion,
separate timing totals and maxima, idle-inclusive batching, and failed clock samples.
The compositor tests also cover merging more than 64 dirty rectangles.

Validation: the full release host suite passes 283 tests (6 opt-in benchmarks
ignored). The 8 framebuffer, 6 compositor-motion and 4 enabled-profile cases also
pass AddressSanitizer. fb-server and Honoka build for x86_64 and AArch64. The
diagnostic x86_64 image passes
the four-CPU HPET/USB-root test: Doom gameplay was visually inspected, normal quit
returned to Shell, and subsequent untraced bash input, filesystem writes, mouse
motion and desktop-clock updates passed. A separate run also covered window
dragging. No physical-machine FPS improvement is claimed by these checks.
