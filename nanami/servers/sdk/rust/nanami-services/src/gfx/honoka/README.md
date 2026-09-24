# Window resize

Drag the right/bottom border or the lower-right corner. Honoka previews the
new outline; releasing the left button proposes a **content** width/height,
excluding the title bar and borders. Minimum content size is 72x32 pixels.

GUI clients use `WindowEventQueue`, not the raw `InputEventQueue`. Resize is
`INPUT_EVENT_KIND_WINDOW_RESIZE` (6), with width in `value0` and height in
`value1`, followed by the usual `HONOKA_NOTIFICATION_INPUT` wakeup. Dimensions
are positive, at most `i16::MAX`, and bounded by the desktop. The latest resize
is held in an atomic coalescing mailbox after the normal input ring (word 1029
of the existing 16-KiB mapping), so a full mouse/key ring cannot lose it. The
normal input ring layout is unchanged. Rebuild the compositor and clients
together; old clients do not consume resize proposals.

Native clients own a `WindowSurface`. Upon resize, stop using its drawing
pointer, call `surface.resize(port, window, width, height)`, and recreate drawing
targets from the resulting `pixels()`, `width`, and `height`. Redraw and submit
full damage. `resize_with` runs a preservation callback while both old and new
client mappings remain valid (used by Saran).

The synchronous resize RPC validates ownership and allocates a replacement
before committing dimensions and the compositor's mapping together. Allocation
failure leaves the old surface intact. Each side releases only its own old
mapping; a cleanup failure retains that mapping and must be resolved before
another replacement can be allocated. The frame may temporarily be blank until
the client repaints; this is not a double-buffered presentation protocol.

Alter/Linux fbdev uses `honoka_resize_viewport` instead: existing `/dev/fb0`
mmaps, resolution, and stride remain valid. A smaller window clips and a larger
window pads the fixed-resolution canvas; neither silently stretches it nor
changes the guest's display mode. Select the guest mode with `--fb-size`.

# Shell terminal

The command editor supports Up/Down history (including the unfinished draft),
Left/Right, Home/End, Delete and insertion at the cursor. USB extended scancodes
and PS/2 navigation keys use the same path.

Foreground output uses a streaming cell-based VT screen: cursor addressing,
erase, insert/delete characters and lines, scroll regions, deferred wrapping,
alternate screen, SGR colors/background/reverse/underline, and device/cursor
queries. Escape sequences may cross IPC boundaries. Navigation keys are sent
as CSI/SS3 sequences, with Ctrl and Alt handling. The default `TERM` is `vt100`
instead of the unregistered `nanami`. This is not a complete xterm emulator:
UTF-8/wide characters, programmable tab stops and DEC character sets remain
unsupported. Normal-screen scrollback is bounded to 128 rows; alternate-screen
output does not enter that history.

Shell updates terminal-service rows/columns on resize; Alter exposes them via
`TIOCGWINSZ`/`TIOCSWINSZ`. Termios settings are retained across queries and fork,
including canonical/echo state and output `OPOST|ONLCR` conversion. General
signal-handler delivery (including `SIGWINCH`) is still unimplemented in Alter;
running Linux applications that rely on it are not automatically told to
redraw on resize. This change does not add full POSIX job control or a PTY.
The BusyBox vi test covers display, navigation, editing, named saves, shortening
an existing file and `:x`. Alter's `ftruncate(2)` request reaches ext2 through
POSIX/VFS; shrinking releases blocks, and extending exposes zero-filled holes
without changing the shared file offset. Persistence follows the existing
dirty-cache policy (`fsync`, `O_SYNC` or delayed writeback).
