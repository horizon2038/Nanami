# Text editor

A native Honoka application, installed as `/bin/text-editor` by the existing
application discovery/build workflow. It does not run through Alter or require
a browser, terminal emulator, timer or new service.

```text
text-editor
text-editor /nanami/session-list
```

Paths in the Open / Save as field are native VFS paths. Relative paths are
resolved from `/`; there is no inherited Shell working directory. For example,
`/alter/linux/tmp/note.txt` is also visible to Linux guests as `/tmp/note.txt`.
The parent directory must already exist. A missing or invalid input file leaves
the current document intact and displays an error.

## Interface

Following the repository's `ai-deodrization.md`, the interface is a compact text
toolbar, labeled path/search field, large paper-colored editing surface and
quiet position/status bar. Line numbers, selection and the scrollbar communicate
document state; there are no cards, decorative icons, gradients or animation.
It uses Honoka's window decoration and existing Hack font, with one warm accent.

| Operation | Keyboard |
| --- | --- |
| New / Open | Ctrl+N / Ctrl+O |
| Save / Save as | Ctrl+S / Ctrl+Shift+S |
| Close | Ctrl+W or Ctrl+Q |
| Undo / Redo | Ctrl+Z / Ctrl+Y or Ctrl+Shift+Z |
| Select all | Ctrl+A |
| Copy / Cut / Paste | Ctrl+C / Ctrl+X / Ctrl+V |
| Find / next / previous | Ctrl+F / Enter or F3 / Shift+F3 |
| Leave a field or cancel a dialog | Esc |
| Move cursor / extend selection | Arrows, Home/End, Page Up/Down / hold Shift |
| Start / end of document | Ctrl+Home / Ctrl+End |

Click and drag to select text. The wheel scrolls vertically; click the scrollbar
to move within the document. Toolbar actions are also clickable. In a path or
search field, Enter or the right-hand text action submits it. In confirmation
dialogs, click an action or press Enter (Save/Replace), D (Discard), Esc (Cancel).
The title's `*` and the status bar show unsaved changes. Undoing back to the saved
revision clears that state. Resizing preserves the document and selection.

## Text and storage

- UTF-8 text, limited to 256 KiB. Invalid UTF-8 and NUL-containing files are
  rejected instead of silently converted. Existing LF/CRLF bytes are preserved;
  Enter uses CRLF when the opened file contains CRLF. Tabs are retained and
  displayed at four-column stops. Lines scroll horizontally rather than wrap.
- The current input protocol provides raw US-layout scancodes, not text/IME
  events. Clipboard operations are internal to this editor instance: the OS has
  no shared clipboard service. Supported Unicode glyphs are cached on demand;
  characters missing from Hack (including Japanese) appear as outlined boxes,
  with their original UTF-8 bytes preserved. Grapheme clusters, combining marks
  and East Asian display widths are not implemented; navigation is by scalar
  value (CRLF is treated as a single line break).
- A gap buffer avoids copying the whole document on each edit. A line-start
  index permits binary-search line lookup. Undo is bounded to 128 edits / 512 KiB
  of payload, redo shares that budget. Painting visits visible rows/columns;
  idle operation blocks on Honoka notifications, without polling or timers.
- Reads and writes use the existing VFS shared-memory channel in bounded
  chunks. All handles are closed on success and failure. Saving writes the
  contents, truncates the old tail and calls `fsync` before marking the document
  saved. A failed operation retains the editor buffer and its modified state;
  closing or replacing an unsaved document requires confirmation. Save as asks
  before replacing an existing path.
- **Saving is not atomic.** The current ext2 rename operation does not support
  replacing an existing destination. Saves therefore update files in place; an
  I/O error or power loss can leave a partial disk file. The in-memory buffer is
  kept on a reported error, but is not a persistent backup. Concurrent external
  modifications are not detected; Save overwrites the currently named file.

Hack is reused from Honoka's assets; see its [license](../honoka/licenses/hack.md).

## Validation

Host-side tests execute the production document, input, controller and renderer:

```text
cargo test --manifest-path tests/native-performance/Cargo.toml --test text-editor --test text-editor-storage
```

From the repository root, create a separate image and run the QEMU workflow:

```text
cargo xtask build --rootfs out/text-editor-rootfs.img --output-image out/text-editor.img
python3 tests/text-editor/qemu-smoke.py --image out/text-editor.img --smp 4
cargo xtask run --image out/text-editor.img --snapshot
```

The smoke test uses a private cloned disk and firmware variables, USB input and
no network. It checks file operations, editing, search, undo/redo, the dirty-close
guard, reopening, resizing and command-line paths, retaining screenshots/logs in
a temporary directory. It independently checks the saved file bytes using
`debugfs` after stopping QEMU. Set `--debugfs` if e2fsprogs is not on PATH.
