# Motion rendering

Honoka now keeps the first and latest cursor/drag-outline bounds within each
unrendered run of mouse motion. Intermediate positions were never drawn, so
they need neither erasing nor publication to fb-server. The two bounds remain
separate instead of becoming a potentially screen-sized bounding rectangle.
Non-motion input flushes these bounds before processing button/key transitions;
rendering flushes them before consuming ordinary damage.

Pointer deltas still apply sequentially with the original edge clamping.
Client events, queue sizes, per-frame input budgets and drag geometry are not
changed. Motion clipped to the same cursor position does not request repaint.

For 64 interior mouse movements without other damage, this needs two cursor
rectangles (968 pixels total), independent of the distance travelled. A drag
needs at most ten rectangles (two cursors and two four-edge outlines), rather
than overflowing the individual-damage limit and repainting the window-sized
bounding rectangle. Ordinary window-content updates are still rendered.

Honoka's canvas is shared RAM, not the hardware framebuffer aperture. Its fills
use ordinary row stores instead of per-pixel volatile stores; pixel/blend stores
are likewise ordinary RAM accesses. The release fence and synchronous present
IPC are unchanged. fb-server still owns hardware publication. Hardware cache
attributes and hardware framebuffer copies are **not** changed by this work.

## Tests

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test honoka-motion --test hotpaths
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml --target aarch64-apple-darwin \
  --test honoka-motion --test hotpaths
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test hotpaths ram_fill_benchmark -- --ignored --nocapture
```

`honoka-motion` compiles the actual compositor/framebuffer with IPC and font
stubs. Five tests compare final pixels after batching against rendering after
each individual event: cursor bursts, edge clipping/reversal, sparse drag
outlines, release followed by motion, and clicks between movements. Damage
rectangle counts/areas are also checked. `hotpaths` adds first/latest bound
tracking, reset and clipped row-fill/padding checks.

All 198 release host tests pass (five timing tests ignored); the five compositor
tests and eight hot-path tests also pass AddressSanitizer. A host RAM-only
2560x1080 fill sample measured 849,220 ns with volatile pixel stores versus
171,258 ns with row fills. This is an Apple Silicon host microbenchmark, **not**
an x86_64 hardware framebuffer throughput or desktop-latency measurement.

See [USB tests](../usb/README.md) for sustained motion/drag QEMU checks.
Real-machine responsiveness and the reported whole-system hang still require
hardware validation; reduced redraw traffic alone is not a hang fix.
