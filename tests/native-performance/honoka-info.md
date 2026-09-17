# Honoka desktop information

Honoka shows the running kernel version, Nanami core version, architecture, and
platform in an opaque white panel with black text below the upper-right clock.
Application windows cover it like the rest of the desktop. Long lines wrap to
fit narrower screens. The panel is included in the existing background cache;
without that cache, only its intersection with the damaged region is redrawn.

Information is queried once at startup. `OS_REQUEST_NANAMI_INFO` selectors 6
(kernel version from `InitInfo`) and 7 (core `CARGO_PKG_VERSION`) return a
NUL-terminated string in two little-endian words per reply. `arg1` selects the
chunk. Exact chunk-sized strings have an additional all-zero chunk. Existing
architecture/platform selectors retain their fixed-size format.

Run the transport and rendering regression tests:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test system-version --test honoka-info-panel --test honoka-motion
```

These cover optional version suffixes, chunk boundaries, invalid replies,
buffer bounds, opaque colors, positioning, wrapping, and clipped redraws using
the real font renderer. The existing cursor/window-damage tests also exercise
the compositor with the panel enabled.

After rebuilding the image, capture the real desktop with:

```sh
python3 tests/usb/qemu-hid.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --smp 4 --hpet on --usb-storage --bash-smoke --no-network --desktop-info-smoke
```

The harness verifies startup and successful information queries, then writes
`desktop-info.png` to its printed log directory for visual inspection. It uses
private disk copies; it does not write to physical storage.
