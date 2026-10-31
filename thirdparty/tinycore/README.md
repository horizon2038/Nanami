# Tiny Core userspace on Alter/Linux

Two explicit profiles are provided: a console and an optional X11 desktop.
`--desktop` installs the official **Xfbdev, FLWM Topside, wbar and aterm**, together
with their pinned libraries and fonts. The desktop also includes **Xfe** (files),
**FlPicSee** (images), **mtPaint**, **Galculator** and the **Flit clock**.
The real X server draws into Alter's
framebuffer inside a Honoka window; this is not a native shell made to look
like Tiny Core. No Linux kernel VM or Kernel modification is involved.

This builds Tiny Core **17.1 x86_64 userspace**, not a Linux kernel VM. Negoto
asks Alter/Linux to load the official dynamically linked `/bin/busybox`, which
reads this port's `/init` using its `sh` applet. The resulting shell, applets,
dynamic linker and libc come from Tiny Core's official rootfs, not Nanami's
standalone BusyBox payload.

Alpha allocates process capability arenas from its refillable, buddy-reserved
RAM pool. Starting desktop clients is not limited by the fixed 128-MiB
bootstrap kernel-object pool; reaped root slots reuse their existing arenas.
This is a Nanami-side allocator change, not an A9N Kernel change.

## Build and run

From the repository root:

```sh
python3 thirdparty/tinycore/build.py --desktop --image
cargo xtask run --image thirdparty/tinycore/build-desktop/nanami-tinycore.img --storage ahci
```

In the Nanami shell:

```text
negoto run --bundle /alter/linux/tinycore-desktop tinycore
```

Use the aterm shell normally. Click/drag the nested title bar to move its
window. Clicking the blue desktop opens FLWM's menu; `Terminal` starts another
aterm. The bottom dock provides Terminal, Files, Editor, Images, Paint,
Calculator, Tiny Core information and Exit desktop. The blue desktop's
menu also offers Clock. Menu entries link directly to the upstream ELF programs;
dock buttons run `nanami-app` through BusyBox explicitly for their arguments and
log files. Neither path relies on the unsupported FLRun spawn path or on Kernel
script-interpreter loading.
Closing any terminal leaves the desktop running. Choose **Exit desktop** to
stop the session and remove the Negoto execution context. The console profile is also included in the
desktop disk:

```text
negoto run --bundle /alter/linux/tinycore tinycore-console
```

Stock wbar is a desktop launch bar rather than an always-on-top panel. If an
application covers it, move or minimize that window to expose the dock.

Documents, Pictures and Downloads are under `/root`. A welcome document and a
sample picture are supplied. The clock profile disables audio/battery
controls because those host devices are not supplied by Alter.

For a smaller, console-only disk, omit `--desktop`:

```sh
python3 thirdparty/tinycore/build.py --image
cargo xtask run --image thirdparty/tinycore/build/nanami-tinycore.img --storage ahci
```

In the Nanami shell:

```text
negoto run --bundle /alter/linux/tinycore tinycore
```

Inside Tiny Core, try `cat /etc/os-release`, `ls /`, `uname -a`, `ps`, or
`echo hello | cat`. `exit` returns to Nanami and removes the Negoto runtime
context. Files written inside the bundle remain on its rootfs; Negoto does not
delete the bundle. `uname` truthfully reports the Alter host, not a Linux kernel.

`ps` uses Alter's process-backed `/proc`, not a prebuilt process list. Only
processes in the same Negoto execution context are visible. Command arguments,
process state and IDs are available; CPU-time and RSS fields are zero because
the host does not supply that accounting.
The stock 1.36.1 BusyBox in this rootfs has no `ps -T` option; Alter's task
directories are tested directly rather than modifying that BusyBox binary.

An existing generated `build/` or `build-desktop/` is not overwritten automatically. To discard
that directory **including its test disks and any saved guest data** and build
again, add `--rebuild` (only the selected profile directory is replaced).
The normal `out/ext2*.img` images are never replaced.
Use `--output-dir /path/to/new-build` to create a separate tree and disk without
discarding an existing build or its saved data.
Use `--archive /path/to/rootfs64.gz` to import a previously downloaded archive;
the same SHA-256 check applies. `--offline` requires a local/cached archive and
cached Cargo dependencies. Desktop builds also need the cached TCZ packages
listed in `desktop.lock.json`. This recipe assembles the distribution rootfs; it
does not compile Tiny Core's kernel or recompile BusyBox/libc.
Desktop disks default to a 256-MiB ext2 rootfs (console disks remain 128 MiB).
Use `--size-mib 512` for additional writable space. This changes filesystem
capacity, not guest RAM.

## Layout

```text
thirdparty/tinycore/
  build.py                   acquisition and reproducible assembly
  desktop.py                 pinned TCZ extraction and merging
  desktop.lock.json          official desktop packages, SHA-256 and dependencies
  profile/init               Alter console/desktop init
  profile/usr/local/bin/nanami-desktop
  profile/usr/local/bin/nanami-app
  profile/etc/init.d/rcS     userspace startup
  downloads/                 ignored, checksum-pinned archive cache
  build/                     ignored, disposable generated files
    manifest.json            input checksum, overlays, omitted device nodes
    upstream/                extracted official userspace, original init scripts
    overlay/tinycore/        OCI config and private rootfs for xtask
    overlay/tinycore-smoke/  same rootfs, /init --smoke
    rootfs.img               Nanami ext2 staging filesystem
    nanami-tinycore.img      complete boot disk
  build-desktop/             separate generated desktop tree and boot disk
    overlay/tinycore-desktop/  graphics-enabled OCI profile, same private rootfs
```

The build pins the official
[rootfs64.gz](https://distro.ibiblio.org/tinycorelinux/17.x/x86_64/release/distribution_files/)
to SHA-256
`4eff7d1c521a77f00da07c56dd80d2c9ca4d667bf0919bcb7f05f86bf663a6ae`.
Its published MD5 is `d02529ab5ef6c9851f0579d698f1352e`. An upstream replacement
fails verification instead of silently changing the input. Source archives are
available from the [Tiny Core source directory](https://distro.ibiblio.org/tinycorelinux/17.x/x86_64/release/src/);
the individual upstream licenses continue to apply. Generated binaries and
archives are not added to this repository.

The desktop dependency graph is resolved explicitly by `desktop.py
--refresh-lock`; ordinary builds never silently select newer TCZ packages.
`desktop.py --extend-lock` pins additions while preserving existing package
hashes and removes unreachable dependencies from the resulting lock.
Only `xwininfo` is installed from Xorg's binary-tools package; its unrelated GL
diagnostics and Mesa/LLVM dependency tree are not part of this desktop. GTK's
util-linux dependency contributes runtime libraries, not hardware-management
commands or a second copy of Xlibs' `mcookie`. These selections are recorded in
the lock/manifest. FLTK 1.3 and 1.4 runtime libraries coexist: the only omitted
1.3 members are unversioned development symlinks, leaving the existing 1.4 aliases
intact. Flit retains its original `.so.1.3` dependency.
Official MD5 checksums are checked during pinning and every normal installation
checks the committed SHA-256. Extensions are unpacked on the host at build
time, not mounted in the guest, and package post-install scripts are not run.
FOX contributes its unchanged runtime libraries for Xfe, without its unrelated
demo programs or GL/GLU package dependencies: those libraries have no GL/GLU
ELF dependency. Desktop startup does not run extension maintenance tools:
making GTK cache generation mandatory caused the startup test to time out, so
it is not made a prerequisite for launching the session. Galculator's
basic interface works without these caches; optional icon-theme lookup can
report a GTK warning.
If `unsquashfs` is unavailable, the recipe builds pinned squashfs-tools 4.6.1
under `downloads/desktop/host-tools/`; this needs a host C compiler, make,
zlib and liblzma development files (`brew install xz pkg-config` on macOS).
`UNSQUASHFS=/path/to/unsquashfs` selects an existing tool, and `HOST_CC` selects
the host compiler. No host package manager is run automatically.

Extraction validates newc member paths and never creates host device nodes or
writes through an archive symlink. Relative/absolute/dangling guest symlinks are
preserved in ext2, not resolved against the build host. Linux devices are
provided by Alter. Setuid/setgid bits are removed on the build host and owner
read permission is added to execute-only upstream files for packaging. This
profile runs as root; it does not provide Tiny Core multi-user permissions.

## Explicit port changes

The profile overlays `/init`, `/etc/init.d/rcS` and the desktop startup script.
Original rootfs versions remain under the selected build's `upstream/`;
`manifest.json` records the replacements and desktop package lock.

- `/init` initializes the console environment, sources the profile `rcS`, then
  execs the **official Tiny Core BusyBox ash** interactively. `--smoke` instead
  checks private-root paths, symlinks, file writes, pipelines, fork/exec and wait.
- `rcS` prepares writable userspace directories, reports Tiny Core's release and
  the actual host, sets `/tmp` permissions using Alter's persistent `chmod`,
  and sources Tiny Core's `/opt/bootlocal.sh` administrator hook.
- `--desktop` invokes the startup script with the official BusyBox shell,
  starts Xfbdev, waits for a real X11 connection, then starts FLWM and aterm.
  X clients use `DISPLAY=unix/:0`: the X server is started with `-nolisten tcp`,
  so clients must retry the local socket rather than fall back to a TCP
  connection while Xfbdev is still starting. This also applies with normal
  QEMU user networking enabled, not only the no-NIC test configuration.
  The startup loop uses stock `xset -display "$DISPLAY" q` for a real local
  X11 round trip. The packaged `waitforX` hard-codes `:0` and ignores
  `DISPLAY`, so it is retained upstream but not used as this profile's probe.
  Its `io.nanami.graphics="true"` annotation explicitly grants a window-local
  graphics session. Other bundles do not gain raw keyboard/mouse access.
  `/dev/tty0`/`tty1` are a virtual console, not host VT switching; keyboard
  mediumraw/keymap ioctls and PS/2 mouse negotiation translate Honoka events.
  The host cursor and nested pointer use unaccelerated coordinates; FLWM's
  optional pointer warping is disabled using its `NOWARP` setting.
  Legacy mouse motion accumulates independently of the finite evdev event
  ring; slow X readers retain residual displacement. Resized viewports crop
  the fixed guest framebuffer, so input is clamped to that same backing extent.
- Linux's `/sbin/init` remains installed but is **not executed**: Alter does not
  provide PID namespaces/PID 1 or a Linux kernel's init lifecycle here. This is
  a userspace profile, not a full service supervisor.
- No mount/umount, udev, Linux kernel modules, zram/swap, RTC device wait, getty
  or Linux virtual consoles are emulated by scripts. Alter already supplies
  `/dev`, `/proc`, `/sys` and terminal I/O. The unmodified `tc-config` hardware
  boot path is retained on disk but not called.
- This is not full Tiny Core hardware boot, runtime `.tcz`/squashfs mounting,
  a control-panel/extension-manager port or a full Linux compatibility claim.
  The desktop consists of Xfbdev + FLWM + wbar and the installed X11 clients.
  Available tools still depend on Alter's implemented syscalls. FLRun remains
  installed but is not advertised by the dock: its libc process-spawn path needs
  additional Alter clone/vfork/exec compatibility. Do not treat it as a working
  application launcher in this profile.
- X11 TCP listening is disabled. The local X server uses `-ac` within the
  execution context's private UNIX-socket namespace; this is not a multi-user
  X authority/authentication implementation.

These differences are intentional: Tiny Core's
[normal boot sequence](https://wiki.tinycorelinux.net/doku.php?id=wiki:the_boot_process)
depends on a Linux kernel and cannot just be marked successful on Alter.

## Tests

```sh
python3 -m unittest discover -s thirdparty/tinycore -p 'test_*.py' -v
cargo test -p ext2-image -p nanami-xtask

# --rebuild discards only this generated build/, including its guest disks.
python3 thirdparty/tinycore/build.py --rebuild --image --with-tests --offline
python3 tests/negoto/qemu-smoke.py --tinycore --smp 1 \
  --image thirdparty/tinycore/build/nanami-tinycore.img
python3 tests/negoto/qemu-smoke.py --tinycore --smp 4 \
  --image thirdparty/tinycore/build/nanami-tinycore.img

python3 thirdparty/tinycore/build.py --desktop --rebuild --image --with-tests --offline
python3 tests/negoto/qemu-smoke.py --tinycore-desktop --smp 1 --normal-devices \
  --image thirdparty/tinycore/build-desktop/nanami-tinycore.img
python3 tests/negoto/qemu-smoke.py --tinycore-desktop --smp 4 --normal-devices \
  --image thirdparty/tinycore/build-desktop/nanami-tinycore.img
python3 tests/negoto/qemu-smoke.py --tinycore-apps --smp 4 --normal-devices \
  --image thirdparty/tinycore/build-desktop/nanami-tinycore.img
```

`--with-tests` additionally requires Clang and LLD to build integer-only Linux
chmod and shared-file-mapping fixtures (`CC=/path/to/clang` selects Clang). It includes private test
bundles for pathname/FD/dirfd changes, symlink flags, unlink lifetime and readonly
errors. QEMU uses snapshots. The native probe runs `/init --smoke`, independently reads
its result marker, and checks zero exit status. It then runs the interactive
profile with a real terminal-service console, checks the release, executes
commands, exercises foreground jobs/Ctrl-C, exits and checks context cleanup.
Directory checks run the stock glibc-linked BusyBox `ls .`, `ls /`, `ls /bin`
and `ls` after changing directories. Raw syscall checks also cover both
sign-extended and zero-extended `AT_FDCWD` for relative `openat`, `newfstatat`,
`fchmodat` and `fchmodat2(AT_EMPTY_PATH)`.
Private-root directory enumeration overlays only `/dev`, `/proc` and `/sys`;
the legacy personality's `/temp` alias is not invented inside the container.
An actual `/temp` in its rootfs is still listed normally. Native `linux-directory`
tests cover both mount profiles, shadowing, small buffers, dup and seek cookies.
Expected output strings are split in the sent shell commands so input echo
cannot count as a successful test. The QEMU test also starts the normal GUI
console after the probe, saves `tinycore-console.png` in its reported log
directory, and rejects guest panics throughout both phases. This repeated
fork/exec workload exercises Alpha's reusable fixed-size TLSF heap as well.

The desktop test first executes the file-mapping guest regression, then waits
for the shell inside a real X terminal, sends physical USB keyboard/mouse events,
and independently reads the command's output file through VFS. It closes that
terminal, clicks the dock to start a new terminal, verifies another output file,
opens Editor and the information terminal, and clicks Exit desktop.

`--normal-devices` keeps PS/2 enabled and adds xtask's virtio-net user-network
device. This covers cold-start X clients while networking is available, not
just the default isolated no-NIC test setup. `--net none` can override that
network device when isolating a regression. Tests use QEMU snapshots and do
not change the saved disk.

Screenshots retain the drag, launcher, editor and information stages; the probe
checks the information terminal's command output and CLI exit/context removal
independently through VFS.

The `--tinycore-apps` test drives an actual X terminal to run a test-only script,
then closes that terminal so it cannot hide the application windows.
It starts Files, Images, Paint, Calculator and Clock one at a time,
requires a new `IsViewable` X window while the process remains alive, and saves a screenshot
for each. Titles must identify the actual application rather than a startup
error dialog. The native probe reads independent progress/result files and
verifies desktop Exit and context cleanup. It is a launch/render smoke test,
not a claim that every upstream feature has been tested.

Fluff and Dillo were evaluated but are not included in the standard profile:
Fluff requires inotify and otherwise presents an initialization error; Dillo's
file plugin daemon requires TCP loopback, which Alter currently rejects at
bind. Merely showing their warning/empty windows is not a successful application
test. Xfe supplies file management without introducing a fake inotify shim.
The console tests also cover the no-NIC case: an absent network backend must
not leave Alter blocked in a network-service attachment IPC.

The expanded profile was checked with 20 host packaging tests, 958 native
regressions (eight timing benchmarks ignored), real application windows on
one/four-core QEMU, and two complete desktop sessions in one four-core boot.
The latter checks Terminal/Editor/Information launching and fresh second-session
commands rather than accepting stale result files. See the
[desktop test notes](../../tests/native-performance/tinycore-desktop.md) for
the metadata-arena fix, input synchronization and validation boundaries.
