# Desktop tools validation

```text
cargo test --manifest-path tests/native-performance/Cargo.toml --test desktop-tools --test honoka-motion -- --test-threads=1
cargo xtask build --rootfs out/desktop-tools-rootfs.img --output-image out/desktop-tools.img
python3 tests/desktop-tools/qemu-smoke.py --image out/desktop-tools.img --smp 4
cargo xtask run --image out/desktop-tools.img --snapshot
```

The smoke test clones the supplied image into a temporary directory, uses private
UEFI variables, virtual USB input, AHCI storage and no network. It never mounts
or writes the supplied disk. `debugfs` from e2fsprogs is needed for the independent
post-exit BMP check (`--debugfs /absolute/path/to/debugfs` if needed).

For the two-phase boot image, pass `--expect-two-phase` to additionally require
Booter preparation, handoff and launcher suspension in order before the normal
service/application workflow. This is not a child-domain or Provider handover
test; those have separate acceptance requirements.

It checks monitor tabs, paused pixel stability and manual refresh, then draws
with Saran, changes colors, checks crop/rotate/flip and undo/redo, saves,
shrinks/grows the viewport, cancels a dirty close, reopens the image and compares
document pixels. The saved
file is independently checked for BMP dimensions, row orientation and expected
background/brush/fill/line colors. Screenshots and logs remain in the reported
temporary directory for visual inspection, including the standard arrow cursor.

The host tests exercise the production ring history and missing samples,
unit formatting, clipping, BMP parsing/round-trips/fuzz input, painting primitives,
flood fill, preview rollback, crop/rotate/flip, revision-aware undo, resize/zoom,
selection clipboard, file failures and unsaved/overwrite guards. Cursor tests
check the hotspot/outline/tail, clip equivalence and framebuffer stride guards;
compositor tests check pixel-exact cursor movement and damage coalescing.

The complete native suite shares global heap-test fixtures; run it with
`-- --test-threads=1`. AddressSanitizer and release-mode runs cover the new model
and pixel tests as well. Existing GUI resize-test dimensions have been updated.
