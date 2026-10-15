# Saran

Saran is a native bitmap paint application. Start `saran` in Shell, or open a BMP
with `saran /alter/linux/tmp/picture.bmp`. Paths use Nanami's native VFS; relative
paths resolve from `/`, not Alter's guest root. Parent directories must exist.

## Painting

The left toolbar selects Brush, Eraser, Line, Rectangle, Ellipse, Fill, Picker
or a rectangular Select tool. Drag to draw; a click also paints a brush dab.
Line and shape previews follow the pointer and commit as one Undo operation on
release. Esc cancels a preview/stroke. Rectangle/Ellipse can be outlined or filled.
Line/brush/eraser width is adjustable from 1 to 32 pixels.

Left button uses the foreground color; right button uses the background color.
The eraser paints with the background. Click a palette color with either button
to set that color; Color opens a labeled `#RRGGBB` field. X swaps foreground and
background. Picker reads the actual document pixel, not the window decoration.

Select a rectangle and copy, cut, paste, delete or crop it. Paste starts at the
selection's upper-left corner, or at the document origin if nothing is selected.
The clipboard is local to this instance; there is no shared OS clipboard service.
Crop uses the selection; Rotate turns the whole image 90 degrees clockwise;
Flip H mirrors it horizontally. All three are undoable.

## Document and view

- New asks for `WIDTH x HEIGHT`; each dimension is bounded to 1..1024 pixels.
  Resize crops/extends the document, preserving its upper-left contents and
  filling the extension with the background color. It is undoable. Clear is also
  undoable and paints the background color over the document.
- Window resizing only changes the viewport. It never changes canvas pixels,
  including when the window is reduced to Honoka's minimum size.
- Zoom: 25%, 50%, 100%, 200%, 400%, 800%, with nearest-neighbor display.
  Use +/− buttons or Ctrl+wheel. The 1:1 button resets zoom and pan.
- Wheel scrolls vertically, Shift+wheel horizontally; arrows pan by document
  pixels. Scrollbars appear when the zoomed image exceeds the viewport.
- History retains at most 32 changes / 16 MiB of pixel payload, including redo.
  Ordinary operations store changed bounding rectangles, not full-frame copies
  for every pointer movement. A stroke's original snapshot is temporary.

| Keyboard | Action |
| --- | --- |
| B / E / L / R / O / F / I / M | Brush / Eraser / Line / Rectangle / Ellipse / Fill / Picker / Select |
| [ / ] | Decrease / increase width |
| X | Swap foreground/background |
| Ctrl+N / Ctrl+O | New / Open |
| Ctrl+S / Ctrl+Shift+S | Save / Save as |
| Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z | Undo / Redo |
| Ctrl+A / Ctrl+C / Ctrl+X / Ctrl+V | Select all / Copy / Cut / Paste |
| Delete | Clear selection, or whole canvas if none |
| Ctrl+Shift+X / Ctrl+R / Ctrl+H | Crop / Rotate clockwise / Flip horizontally |
| Ctrl++ / Ctrl+− / Ctrl+0 | Zoom in / Zoom out / 1:1 |
| Esc | Cancel dialog/stroke, or remove selection |
| Ctrl+W or Ctrl+Q | Close with unsaved-change check |

## Files and failure handling

Opens uncompressed 24/32-bit BMP, bottom-up or top-down. Reserved 32-bit alpha is
ignored; documents are opaque RGB. Save writes a standard uncompressed 24-bit
BMP, including correct row padding and orientation. Save as requires `.bmp` and
asks before overwriting an existing file. PNG/JPEG, transparency and layers are
not part of this bitmap editor.

Writes are chunked through VFS shared memory, truncate the old tail and complete
`fsync` before clearing the modified flag. Failed read/decode/save operations keep
the current image; canceling New/Open/Close never silently discards it. Undoing to
the saved revision clears `*`. Saving is an in-place operation, **not atomic**:
the existing VFS cannot replace an existing destination with rename, so I/O errors
or power loss can leave a partial disk file. The in-memory image is retained on a
reported save error, but it is not a persistent backup.

The compact file toolbar, text-labeled tools, functional palette, plain workspace
and status line follow `ai-deodrization.md`. There are no decorative cards,
illustrations, gradients or animation. Font/colors follow the other native apps;
Hack's license is in [Honoka](../honoka/licenses/hack.md). Input currently uses
the OS's raw US-layout keyboard events; there is no IME.

See [desktop validation](../../../../tests/desktop-tools/README.md) for tests and a
separate QEMU image that does not overwrite existing guest data.
