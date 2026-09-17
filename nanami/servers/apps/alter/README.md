# Alter launcher

```text
alter [-t] [-d] [-g [--fb-size WIDTHxHEIGHT]] [-os linux|freebsd] <binary> [args]
```

`-g` / `--graphics` exposes the graphics devices to the guest. For Linux,
`--fb-size` selects the initial `/dev/fb0` content size, for example:

```sh
alter -g --fb-size 640x400 /alter/linux/bin/doomgeneric-fbdev -iwad /bin/doom1.wad
```

Without `--fb-size`, the requested size remains 800x600. Width and height must
be positive decimal integers separated by `x`; `--fb-size` requires `-g` and
must precede the guest executable. It is not passed to the guest's argv or
environment. Zero, malformed, duplicate, and overflowing sizes are rejected.

Honoka adds the title bar/borders and may constrain the content size to its
minimum window size or the desktop bounds. Alter queries the resulting content
size and uses it consistently for fbdev screen information, stride, byte length,
stat, and mappings. Clients must use `FBIOGET_VSCREENINFO` / `FBIOGET_FSCREENINFO`
to discover the actual mode, not assume their requested dimensions were accepted
unchanged. The pixel format remains 32-bit BGR with opaque alpha.

The launch setting is inherited across fork and retained across exec. Processes
in the same graphics session share its dimensions; unmapping/remapping does not
reset them to the default. This option sets the **initial** mode, not a runtime
window-resize API. `FBIOPUT_VSCREENINFO` mode changes remain unimplemented.

The launch IPC remains backward-compatible for callers using the existing
16-byte `argc, envc` header. `ALTER_LAUNCH_FLAG_FB_SIZE` (bit 3) adds an 8-byte
word at offset 16: width in its low 32 bits, height in its high 32 bits. Strings
then start at offset 24. The receiver validates the flag, header extent, and
dimensions before spawning. Both client and server must support the extension
to use an explicit size; legacy launches still use the original wire format.

See `tests/alter-linux/framebuffer.md` in the repository root for regression
tests. No kernel or Honoka protocol extension is required.
