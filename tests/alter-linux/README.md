# Alter/Linux dynamic-loader regression

## Shared memory, namespace and thread regressions

The integer-only fixtures `memory-events.c`, `namespace.c` and `threads.c`
exercise the actual Linux ABI without libc or FPU. Build each with the matching
x86_64 entry stubs; the thread fixture also needs `threads-x86_64.S`:

```sh
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -ffreestanding -fno-builtin -fno-stack-protector -mno-sse -mno-sse2 \
  -mno-red-zone -O2 -Wl,-e,_start,--image-base=0x400000 \
  tests/alter-linux/memory-events.c tests/alter-linux/signals-x86_64.S \
  -o out/nanami-memory-events
# Repeat for namespace.c -> out/nanami-namespace.
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -ffreestanding -fno-builtin -fno-stack-protector -mno-sse -mno-sse2 \
  -mno-red-zone -O2 -Wl,-e,_start,--image-base=0x400000 \
  tests/alter-linux/threads.c tests/alter-linux/threads-x86_64.S \
  tests/alter-linux/signals-x86_64.S -o out/nanami-threads
cargo xtask build --no-linux-bins --linux-bin out/nanami-memory-events \
  --linux-bin out/nanami-namespace --linux-bin out/nanami-threads \
  --rootfs out/alter-memory-rootfs.img --output-image out/alter-memory-boot.img
python3 tests/alter-linux/qemu-smoke.py --image out/alter-memory-boot.img \
  --program nanami-memory-events --program nanami-namespace \
  --program nanami-threads --diagnostics --smp 4
```

Use `--rebuild-rootfs` only when intentionally replacing that test rootfs. The
runner uses disk snapshots; it requires each fixture's successful exit status.
Repeat with `--smp 1` to cover same-core scheduling as well.

- Memory/events: physical sharing across aliases/fork/SCM_RIGHTS, protection
  faults, retained anonymous PROT_NONE through fork/remap, checked copyin/out,
  memfd EOF SIGBUS/truncate/regrowth/seals, futex wake, timerfd/epoll and signalfd.
- Namespace: rooted relative/absolute links, link-before-`..` semantics,
  `O_NOFOLLOW`, inline/block-backed links, loops, dirfds after rename,
  mkdirat/symlinkat/readlinkat/linkat/renameat/fchdir, and root-escape rejection.
- Threads: real CLONE_VM, distinct TIDs/shared PID, TLS, shared mmap/FD/cwd,
  clear-child-tid joins, local/group signal queues via signalfd, leader exit
  before its worker, and worker-initiated exit_group.

The native suite also exercises VM ownership/reclamation, namespace walking,
mapping-table split capacity, event/FD lifetime, signal masks and robust futex
owner-death behavior:

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml -- --test-threads=1
```

Validated with one and four QEMU CPUs: all three fixtures, the existing UNIX
socket and eventfd/epoll fixtures, Neovim headless startup, and interactive
edit/save/quit followed by independent BusyBox readback. The native suite passes
877 tests (7 timing tests ignored); all 25 xtask tests pass. Release builds for
x86_64 and AArch64 checks for Alter, POSIX, ext2 and Alpha pass. A9N sources and
the kernel IPC fastpath are unchanged. The raw guest fixtures are x86_64-only.

## UNIX domain socket regression

Build the freestanding fixture, then install it in a fresh image:

```sh
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -fno-builtin -mno-sse -mno-sse2 -O2 -Wall -Wextra -Werror \
  -Wl,--image-base=0x400000,-z,norelro,-z,separate-loadable-segments \
  tests/alter-linux/unix.c tests/alter-linux/signals-x86_64.S -o out/unix-test
cargo xtask build --no-linux-bins --linux-bin out/unix-test --linux-bin bin/busybox \
  --size-mib 64 --rootfs out/unix-rootfs.img --output-image out/unix.img
python3 tests/alter-linux/qemu-smoke.py --image out/unix.img \
  --program unix-test --diagnostics --smp 1
python3 tests/alter-linux/qemu-smoke.py --image out/unix.img \
  --program unix-test --diagnostics --smp 4
```

`unix.c` exercises actual Linux syscalls without libc, FPU or networking:
stream/datagram/seqpacket pairs, pathname and binary abstract names, real socket
stat mode, rename/hardlink/unlink/rebind, peer credentials, half-close, fork and
epoll, `SCM_RIGHTS` for regular files and eventfd (including across fork),
`MSG_CMSG_CLOEXEC`, in-flight pipe status changes, record truncation, and 160 ancillary reference-cycle
create/close iterations. Success prints `unix-socket-ok` and exits zero.
The runner requires the exit status and uses a QEMU disk snapshot.

Native tests execute the production local-socket handlers with memory/IPC
mocked, covering backpressure, backlog retry, failed accept/recvmsg copyout,
descriptor exhaustion, FD ownership/truncation/GC, stream barriers, abstract
namespace separation and sockaddr length handling:

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --offline \
  --test linux-unix --test linux-readiness --test posix-io -- --test-threads=1
```

The POSIX/VFS tests also check the socket creation request's fourth word,
socket-vs-directory mode discrimination, and deferred close during filesystem
freeze. Scope and explicit compatibility limits are in
[the syscall support document](../../nanami/servers/apps/alter/linux/linux-supported-syscalls.md#unix-domain-sockets).

Validated on x86_64 QEMU with one and four CPUs, together with the eventfd/epoll
fixture. The Neovim edit/save/quit regression and independent BusyBox readback
also pass on both CPU configurations. The native suite passes 772 tests
(7 timing tests ignored), all 18 UNIX socket tests also pass in release mode,
and all 24 xtask tests pass. Native services build for x86_64 and AArch64;
the raw guest fixture is x86_64-only. The prepared combined test image is
`out/unix-final.img`; it leaves the default guest image/rootfs unchanged.

## Static Neovim and libuv regression

The existing, unmodified `bin/nvim` is an x86_64 static musl executable.
Its original fault at `0x8883bd` is `hlt` inside musl's `abort`, following
failed libuv initialization, not an FPU instruction. Libuv requires
`clock_getres`, epoll, eventfd, nonblocking `pipe2`, `FIONBIO`, and correct
pipe/terminal access modes from `F_GETFL`. Reporting a write end as read-only
prevents TUI RPC, while a read-only terminal mode disables libuv's TTY writes.
Nonblocking terminal reads must also return `EAGAIN`, not park the event loop.

A static binary still needs its **matching Neovim runtime data**. The checked
payload identifies itself as `v0.13.0-dev-1895+g0851e7dabe`; do not substitute
an unrelated host installation's runtime. Prepare a staging tree containing
`usr/local/share/nvim/runtime` from that build, then create a fresh test image:

```sh
mkdir -p out/nvim-runtime-overlay/usr/local/share/nvim
cp -R /path/to/matching/neovim/runtime out/nvim-runtime-overlay/usr/local/share/nvim/
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -fno-builtin -mno-sse -mno-sse2 -O2 -Wall -Wextra -Werror \
  -Wl,--image-base=0x400000,-z,norelro,-z,separate-loadable-segments \
  tests/alter-linux/events.c tests/alter-linux/signals-x86_64.S -o out/events-test
cargo xtask build --no-linux-bins --linux-bin bin/nvim --linux-bin bin/busybox \
  --linux-bin out/events-test --linux-rootfs-dir out/nvim-runtime-overlay \
  --size-mib 128 --rootfs out/nvim-rootfs.img --output-image out/nvim.img
python3 tests/alter-linux/qemu-smoke.py --image out/nvim.img \
  --program events-test --program 'nvim --headless -u /dev/null -i NONE -c q' \
  --nvim-editor --smp 1
python3 tests/alter-linux/qemu-smoke.py --image out/nvim.img \
  --program events-test --nvim-editor --smp 4
```

The event fixture checks counter semantics, packed epoll data, readiness across
fork, final-close cleanup, and pipe nonblocking/access flags through real guest
syscalls. The editor test types text, saves with `:wq`, then requires a separate
BusyBox process to read back the exact saved line. Screenshots alone are not a
passing test. QEMU uses snapshots and does not modify the supplied image.
`--diagnostics` sends explicit `-t -d` traces to serial without corrupting the
curses screen; ordinary `-t` continues to trace to the terminal.

Inside Nanami, launch without terminal tracing:

```sh
alter /alter/linux/bin/nvim -u /dev/null -i NONE --noplugin /tmp/example.txt
```

Use `-i NONE`, not `-i /dev/null`: the latter asks Neovim to write ShaDa
temporary files beside `/dev/null` when quitting.

This is a bounded editor/libuv compatibility target, not complete Linux or
plugin compatibility. The kernel and FPU/signal-frame implementation are
unchanged. See the syscall support document for explicit epoll limits.

Validated on x86_64 QEMU with one and four CPUs: the raw event fixture,
headless startup/exit, and untraced interactive editing, save/quit and independent
BusyBox readback all pass. The native suite passes 772 tests (7 timing tests
ignored), and all 24 xtask tests pass. Native services build for x86_64 and
AArch64; the supplied Neovim executable itself was exercised only on x86_64.
The prepared local image is `out/nvim-verified.img`, with matching runtime data;
the default guest image/rootfs is not replaced.

```sh
cargo xtask run --image out/nvim-verified.img --storage ahci --net none --snapshot
```

## Terminal Ctrl-C regression (2026-10-25)

The Negoto smoke includes a real terminal control byte interrupting a foreground
BusyBox `sleep 100`; the interactive BusyBox shell must survive and execute the
next command. The fixture also installs the unchanged BusyBox at
`/alter/linux/bin/busybox` for standalone Alter networking tests.

```sh
python3 tests/negoto/build-fixture.py \
  --rootfs out/ctrl-c-rootfs.img --image out/ctrl-c.img
python3 tests/negoto/qemu-smoke.py --image out/ctrl-c.img --smp 1
python3 tests/negoto/qemu-smoke.py --image out/ctrl-c.img --smp 4
python3 tests/alter-linux/qemu-smoke.py --image out/ctrl-c.img --network \
  --program 'busybox ping 10.0.2.2' --ctrl-c-after 3 --smp 1
python3 tests/alter-linux/qemu-smoke.py --image out/ctrl-c.img --network \
  --program 'busybox ping 10.0.2.2' --ctrl-c-after 3 --smp 4
```

Use fresh build paths. The ping runner sends an actual Ctrl-C keyboard event,
rejects an early guest exit, and requires status zero from ping's own exit
syscall rather than a forced native kill. Networking is restricted to the
isolated QEMU user-mode network.

## Asynchronous signal delivery (2026-10-25)

Alter uses A9N 0.3.11's synchronous Suspend and continuation contract to deliver
signals to running processes without another syscall. Parked syscall waits keep
their EINTR/SA_RESTART behavior; unaccepted syscall faults replay after the
handler. FPU/SIMD signal-frame state is excluded.

The raw `signals.c` / `signals-x86_64.S` fixture is included in the Negoto smoke
image. It checks SIGALRM during a syscall-free assembly loop, preservation of
RCX/R11 and callee-saved registers, and STOP/CONT plus a pending handler during
a child's busy loop. Existing signal-frame, blocking pipe, timer and interactive
BusyBox tests run alongside these checks.

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test linux-signals --test linux-signal-delivery
python3 tests/negoto/build-fixture.py \
  --rootfs out/async-signals-rootfs.img --image out/async-signals.img
python3 tests/negoto/qemu-smoke.py --image out/async-signals.img --smp 1
python3 tests/negoto/qemu-smoke.py --image out/async-signals.img --smp 4
```

The fixture builder requires new output paths and refuses to replace existing
guest data. QEMU uses snapshot disks. For the complete host suite, use
`cargo test --manifest-path tests/native-performance/Cargo.toml -- --test-threads=1`;
the heap fixtures share mock allocation state between tests.

## time(2), clock_nanosleep and ncurses regression (2026-10-15)

The x86_64 `time` syscall (201) returns Unix seconds and optionally stores the
same 64-bit value at `tloc`; invalid destinations return `EFAULT`. It shares the
existing RTC epoch anchor and fresh timer-service samples with `CLOCK_REALTIME`.
AArch64 has no separate `time` syscall; its libc uses `clock_gettime`.

The real static `bin/sl` reproduced `Error opening terminal: vt100.` because the
launcher selected `TERM=vt100` but the standard filesystem had no terminfo entry.
The rootfs builder now writes a small, deterministic legacy-format entry to
`/alter/linux/usr/share/terminfo/v/vt100`, hard-linked under `76/vt100` for ncurses
builds using hexadecimal directories. The shared launcher sets `TERMINFO` to
the guest path. The FreeBSD guest receives the same terminal data/environment.
No host `tic`, personal terminfo, ncurses install, or new build dependency is
required. The entry describes only Shell's supported controls, not a fictitious
xterm or full DEC graphics implementation.

After terminal initialization succeeded, the same `sl` revealed that glibc
`usleep` uses syscall 230, `clock_nanosleep`, rather than syscall 35. Implementing
relative/absolute REALTIME, MONOTONIC and BOOTTIME waits through the existing
one-shot alarm fixes the missing animation delays as well. The saved original
syscall context is used on resume and in completion tracing. Signals and CPU
accounting are not replaced with success-returning stubs.

References: [time(2)](https://man7.org/linux/man-pages/man2/time.2.html),
[clock_nanosleep(2)](https://man7.org/linux/man-pages/man2/clock_nanosleep.2.html),
[compiled terminfo format](https://invisible-island.net/ncurses/man/term.5.html).

```sh
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -fno-builtin -O2 -Wall -Wextra -Werror \
  -Wl,--image-base=0x400000,-z,norelro,-z,separate-loadable-segments \
  tests/alter-linux/time.c -o out/alter-glibc-root/bin/time-test
cargo xtask build --rootfs out/alter-time-ncurses-rootfs.img \
  --output-image out/alter-time-ncurses.img \
  --linux-bin out/alter-glibc-root/bin/time-test
python3 tests/alter-linux/qemu-smoke.py --image out/alter-time-ncurses.img \
  --program time-test --program 'sl -l' --smp 4
python3 tests/alter-linux/qemu-smoke.py --image out/alter-time-ncurses.img \
  --program time-test --program 'sl -l' --smp 1 --visual-program 'sl -l'
cargo test -p nanami-xtask
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test linux-clocks -- --test-threads=1
```

The build uses `bin/sl` as an existing Linux payload and leaves it unchanged.
Use fresh output filenames or explicitly pass `--rebuild-rootfs` to replace
only this disposable test rootfs. The existing default rootfs is intentionally
not overwritten: rebuilding services without rebuilding that rootfs will not
install the new terminal data.

The freestanding fixture bypasses libc/vDSO to check NULL/unaligned/bad `tloc`,
eight-byte writes, realtime agreement, advancing seconds, and relative/absolute
clock sleeps that do not wake early. Host tests cover 2038+, low/non-divisor
tick rates, error propagation, invalid/unsupported clocks, context preservation,
alarm rollback, and independent parsing of the entry by host `infocmp` when
available. Traced guest runs verify exit status; `--visual-program` separately
captures untraced frames because terminal-directed strace output disrupts curses
screen updates. Every QEMU run uses snapshots, private firmware variables, and
no network or physical disks.

Validation: all 551 native tests pass (7 timing tests ignored), as do 22 xtask
tests including the independent host-ncurses parser check. The 15 clock tests
pass in debug, release and AddressSanitizer builds. x86_64 and AArch64 Alter/Linux
compile successfully. The raw time/clock-sleep fixture and the unchanged static
`sl -l` both exit with status zero on one- and four-CPU x86_64 QEMU; untraced
frames show the train moving with no terminal-definition error. The rebuilt
image is `out/alter-time-ncurses.img`; default images and A9N source are unchanged.

## Dynamic loader fixtures

The fixtures exercise real glibc startup, PIE relocation, executable/DSO TLS,
`malloc`, `dlopen`/`dlsym`/`dlclose`, `fork`/`wait`, and dynamic `execve`.
They also check positioned I/O through duplicate descriptors, append status,
`CLOEXEC` preservation on failed exec, `AT_EMPTY_PATH`, private file mappings,
`MAP_FIXED` replacement across a hole, and `munmap` with holes.

Build in a Linux environment with GCC and glibc development files:

```sh
sh tests/alter-linux/build-glibc-fixtures.sh out/alter-glibc-root
```

On macOS, an isolated amd64 Ubuntu container can build them instead:

```sh
mkdir -p out/alter-glibc-root
docker run --rm --platform linux/amd64 \
  -v "$PWD/tests/alter-linux:/tests:ro" \
  -v "$PWD/out/alter-glibc-root:/export" ubuntu:24.04 sh -c \
  'apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev && sh /tests/build-glibc-fixtures.sh /export'
```

Build the image and test:

```sh
LINUX_ROOTFS_DIR="$PWD/out/alter-glibc-root" cargo xtask build --rebuild-rootfs
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 4
```

The runner uses a temporary OVMF variables file, disables networking, and boots
the disk with `snapshot=on`; guest file tests do not modify the supplied image.
It runs the existing static syscall smoke, glibc `true`, and the dynamic
regression, requires exit status zero, and retains serial logs/screenshots in
the printed temporary directory. Repeat with `--smp 1` to check single-core.
No physical disks are accessed. For interactive testing, run
`alter /alter/linux/bin/glibc-regression` in Nanami's shell.

## BusyBox networking regression

Build the freestanding fixture with the static-fork ELF base used by Alter:

```sh
clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -fno-builtin -O2 \
  -Wl,--image-base=0x400000,-z,norelro,-z,separate-loadable-segments \
  tests/alter-linux/network.c -o out/alter-glibc-root/bin/network-test
```

Include `bin/busybox` through repeatable `--linux-bin` options, seed this fixture through
`LINUX_ROOTFS_DIR`, and rebuild the rootfs (`--rebuild-rootfs`). Run:

```sh
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --network --program network-test --smp 4
```

This uses restricted user-mode networking. It verifies repeated `busybox
--install`, installed hard-link identity and errno, Linux interface ioctls,
DHCP-derived resolver defaults and explicit-file precedence, loopback/gateway
ICMP, the real ARP table, and warning-free `ip a`/`ip addr`/`ip link` output
(including `SIOCGIFTXQLEN`, which returns the software queue length, not the
driver's TX ring capacity). A separate opt-in test
uses outbound NAT (no port forwards) for public DNS/ICMP interoperability:

```sh
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --internet \
  --program 'busybox nslookup google.com' --program 'busybox ping -c 1 google.com'
```

Signal delivery remains outside this change: ordinary repeated ping, alarm
timeouts, and Ctrl-C still depend on the deferred signal implementation.
ARP inspection and interface queries are supported; ARP/interface mutation,
IPv6, and a general Linux routing configuration API are not implemented here.
These tests do not overwrite the supplied image or access physical disks.

Validation (2026-09-24): the network fixture passes with one and four CPUs;
BusyBox `nslookup google.com` and `ping -c 1 google.com` pass with outbound NAT.
The host suite passes 370 tests (7 ignored). Hardware has not been retested.
Separately, read-only `e2fsck -fn` reports pre-existing group-layout, bitmap,
and reference-count inconsistencies in the built-in image writer. These were
reproduced with the unmodified writer as well; this change does not claim to
make generated images pass e2fsck.

## Directory visibility and sendfile regressions

Build the two freestanding fixtures (LLVM with the x86-64 target and LLD):

```sh
for fixture in directory sendfile; do
  clang --target=x86_64-unknown-linux-gnu -fuse-ld=lld -nostdlib -static \
    -fno-stack-protector -fno-builtin -O2 \
    -Wl,--image-base=0x400000,-z,norelro,-z,separate-loadable-segments \
    tests/alter-linux/$fixture.c -o out/alter-glibc-root/bin/$fixture-test
done
```

Seed these through `LINUX_ROOTFS_DIR`, with `bin/bash` and `bin/busybox` in
repeatable `--linux-bin` options. Use a disposable rootfs/image: the fixtures create, rename,
and delete guest files. Run on snapshots without network access:

```sh
python3 tests/alter-linux/qemu-smoke.py --image /path/to/test/spencer.img \
  --program directory-test --program sendfile-test --smp 4
```

`directory-test` reproduces bash-created files readable by `cat` but absent from
`ls /` on the old implementation. It checks merged mountpoints, rename/unlink
visibility, small-buffer pagination, and shared root offsets through dup/seek.
`sendfile-test` checks explicit/implicit offsets, EOF, a 100-KB transfer, append
and access-mode errors, bad pointers, and pipe/terminal output. Host sendfile
tests also inject short output writes and EAGAIN to check input-position
preservation. Repeat the guest tests with `--smp 1`.

Validation (2026-09-24): both fixtures pass on x86_64 QEMU with one and four
CPUs. The host suite passes 375 tests (7 ignored), and Alter/Linux builds for
x86_64 and AArch64. The old image fails the directory fixture specifically at
the assertion that the newly created file appears in `ls`. Hardware and
AArch64 runtime behavior have not been tested.

## Host-only ELF validation and storage startup tests

```sh
rustc --test -A dead_code tests/alter-linux/elf-tests.rs -o out/alter-elf-tests
out/alter-elf-tests
rustc --test tests/alter-linux/block-startup-tests.rs -o out/block-startup-tests
out/block-startup-tests
```

The loader/library fixtures are generated artifacts, not committed binaries.
The rootfs importer rejects symlinks/special files and conflicting files;
copy the intended loader and DSOs as regular files at their guest paths. It
does not implicitly copy host `/lib` contents or follow host symlinks.

Validation after the storage startup and approved FPU/SIMD fixes: the static
syscall smoke, glibc `true`, and the full dynamic regression pass with both one
and four CPUs on x86_64/QEMU. A separate four-CPU boot also passes the standalone
dynamic regression, matching the launch order that previously timed out on
glibc's exit-handler lock via futex. These runs use no temporary diagnostic
instrumentation. Eight host ELF/range tests and seven storage startup tests
also passed. The AArch64 Alter/Linux and ext2 targets compile, but were not
runtime-tested.

Before the FPU/SIMD fix, four-CPU runs were intermittent: one full run passed,
a standalone dynamic regression timed out, and a diagnostic run passed. The
timeout has not reproduced in the post-fix runs above; these finite regression
runs do not establish that every possible SMP interleaving is correct.

Read-only investigation found an independent critical issue in A9N's x86_64
FPU/SIMD context switching: `floating_point.hpp` declared `x_save_mask` as
`inline static`, so the CPU initialization and process-switch translation units
had distinct masks. The latter remained zero and its switch helper returned
without saving/restoring the context. Before the fix, disassembly of
`spencer/out/x86_64-pc99-release/a9n/kernel.elf` contained initialization
`xsave64` instructions but no `xsaveopt64` or `xrstor64`.

With explicit approval, the declaration was changed to `inline uint64_t
x_save_mask = 0;`. The rebuilt kernel has one shared mask symbol and both
`xsaveopt64` and `xrstor64` in its context-switch path. Verify with:

```sh
llvm-objdump -d spencer/out/x86_64-pc99-release/a9n/kernel.elf | rg 'xsave|xrstor'
```

That disassembly observation was specific to the inspected local Release
artifact, not proof that all previously used Nanami kernels lacked these
instructions. The user reported a different observation on their working
kernel; the artifacts have not been matched. The glibc timeout's cause was not
established by these finite tests. Alter's pre-existing futex handler is still
a success-returning stub, not a complete synchronization API.

## IPC fastpath follow-up (2026-09-08)

The revised implementation retains the original priority queues and scheduling
functions. The earlier 32-bit bitmap, separate direct-reply APIs and compound
fastpath eligibility check have been removed. Reply-Receive retains its
`and_then`/`switch` structure:
complete the reply once, then either enqueue the client if the server continues,
or block the server and try to return directly to the client. WAIT and
READY_TO_SEND are both marked likely. Notification/fault handling and remote-core
routing remain on their existing paths.

A same-core normal reply returns directly only when no **strictly higher-priority
ready process** is queued. A higher-priority peer forces the existing
enqueue-and-schedule fallback; an equal-priority peer does not exclude direct
return. This condition is checked at the return, including work made ready while
the server handled the call. The caller/server priority relationship does not
authorize bypassing higher-priority ready work.

`try_direct_schedule` no longer raises the ready-queue search bound to the directly
selected process's priority. If an ordinary dequeue left an empty highest queue,
it trims the cached bound only as far as needed for the target comparison.
Consequently the running server does not force an empty-priority scan on every
round trip. No fixed-width priority bitmap or caller/server comparison is added.

The existing direct-switch implementation is specialized with a compile-time
`IS_REPLY` parameter solely for quantum handling: replenish the client's quantum
without Call-style donation, including when another process is selected. Both
specializations use the same scheduler checks and context-switch code. Call keeps
its existing IPC flow. The server still joins the IPC receive queue. Both payload
and capability transfer must finish successfully before committing the normal
reply.

On x86_64, a syscall which returns to a different process skips the FS/GS base
writes at syscall exit because switch_context already performed them. A
same-process return still writes the saved bases, preserving self-TLS updates.
Boot/interrupt exits, GPR writeback, XSAVEOPT/XRSTOR and their masks are unchanged
by this follow-up. In a same-core two-process round trip, the FS-base writes and
GS-base WRMSRs each go from four executions to two; this is an instruction-path
observation, not a measured latency improvement.

Run host regressions (HAL effects mocked; production scheduling/IPC code linked):

```sh
CXX=clang++ SANITIZE=1 sh tests/alter-linux/test-ipc-fastpath.sh
```

Both UP and SMP variants check 20,000 scheduler operations (including direct
selection) against a FIFO/priority oracle, all supported peer priorities, 1,000
IPC round trips, higher-priority fallback, equal/lower-priority direct returns,
stale bounds from dequeued servers, work queued before/during a call, and quantum
handling when a peer is selected. They also cover buffered/register payloads,
capability moves, nested replies, pending/absent bound notifications, nonblocking
receive, queued senders/receivers, missing reply targets, fault replies, failed
transfers, and remote-core replies (SMP). Host tests do not execute privileged
FS/GS writes. The quantum choice is compile-time, not a runtime branch. Existing
capability/reply validity and notification checks remain.

Validation: the UP/SMP host tests pass with AddressSanitizer and UBSan, both with
the production range 0..31 and with a temporary include overlay changing only
`PRIORITY_MAX` to 256 (range 0..255). `TEST_INCLUDE_ROOT` can point the host runner
at such an overlay. The production priority limit has not been changed. The
x86_64 image builds, and all three guest fixtures (static syscall smoke,
`glibc-true`, `glibc-regression`) pass with both one and four QEMU
CPUs. The AArch64 kernel also compiles; AArch64 execution and bare-metal IPC
latency have not been tested.

Known pre-existing limitation, not changed here: PCB CONFIGURE assigns a new
priority without moving an already queued process to its new priority queue.
This can violate the queue-ordering invariant used by both ordinary scheduling
and the direct-return eligibility check. For example, enqueue a peer at priority
2, change its PCB priority to 4, then directly schedule a priority-3 caller: the
caller is selected. The same reproducer fails with the baseline scheduler logic.
The priority regressions above keep each queued process's priority stable; fixing
live priority changes requires a separate PCB/scheduler update.

## Storage startup ordering

ext2 retries both block-device and timer-service discovery during startup.
Only successful 100-ms timer sleeps consume its 64-wait budget; yield-only
retries cannot exhaust the timeout before the drivers have registered. Storage
is checked first, so an available block-device does not require a timer.
Transport/protocol errors and failed sleeps are reported immediately.

Without a registered timer there is no elapsed-time deadline: ext2 yields and
continues discovery. If neither service ever registers, it remains waiting.
Once timer-service is connected, missing storage times out after 6.4 seconds of
successful sleeps, with a final storage check after the last sleep. This uses
the platform timer service, not a user-visible architecture counter.

Seven host tests exercise the production connection loop with deterministic
service responses, including late timer/storage registration, storage without
a timer, the final retry, timeout accounting, and error propagation.

## Performance changes in this iteration

- Runtime process queries borrow metadata instead of returning the complete
  descriptor/mapping tables by value; snapshots remain on mutating fork paths.
- Mapping release/materialization uses one IPC per contiguous tracked range,
  instead of one per 4-KiB page.
- Short guest paths start with a 128-byte read, growing subsequent reads rather
  than copying a whole page for every path.
- Launch stack buffers are reused, and replaced ELF buffers are released.
- Positioned I/O is one POSIX request, with direct-buffer reads where available;
  it does not emulate offsets using seek/read/seek.

These are structural reductions, not a claimed measured speedup percentage.
The Alter architecture-specific register writeback code is unchanged. A9N changes are
limited to the separately approved `x_save_mask` declaration and the IPC fastpath
follow-up described above. The only register-writeback change in that follow-up
is skipping redundant FS/GS base programming after a process switch.

## Readiness regression fixture

`readiness.c` is a freestanding x86-64 fixture for poll/ppoll/select timeouts,
pipe wakeups/EOF, empty terminal readiness and clock IDs. Build into a **test**
Linux rootfs and run with the existing snapshot-disk smoke runner:

```sh
clang --target=x86_64-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -O2 -Wall -Wextra -Werror -Wl,--image-base=0x400000 \
  tests/alter-linux/readiness.c -o out/alter-glibc-root/bin/linux-readiness
# Rebuild the image using a separate ROOTFS_IMAGE and this LINUX_ROOTFS_DIR.
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 4 --program linux-readiness
```

### File truncation

`truncate.c` exercises real `ftruncate(2)` traps through Alter, POSIX, VFS and
ext2: direct/single/double-indirect shrink boundaries, sparse extension and
zero-filled tails, unchanged dup offsets, sizes visible through another open
descriptor, invalid/read-only descriptors, and fsync/close/reopen contents.
It writes only `/tmp/truncate-test` in the disposable guest disk.

```sh
clang --target=x86_64-linux-gnu -fuse-ld=lld -nostdlib -static \
  -fno-stack-protector -fno-builtin -O2 \
  tests/alter-linux/truncate.c -o out/alter-glibc-root/bin/truncate-test
# Rebuild using a private ROOTFS_IMAGE and this LINUX_ROOTFS_DIR.
python3 tests/alter-linux/qemu-smoke.py \
  --image spencer/out/x86_64-pc99-release/spencer.img --smp 4 --program truncate-test
```

The USB terminal smoke test also saves an explicitly named file from BusyBox vi,
shortens an existing file, exits with `:x`, and checks both files byte-for-byte;
see [USB tests](../usb/README.md#shell-terminal-and-gui-resize).
