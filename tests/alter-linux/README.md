# Alter/Linux dynamic-loader regression

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
LINUX_ROOTFS_DIR="$PWD/out/alter-glibc-root" make image
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

Host-only ELF validation and storage startup tests:

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
