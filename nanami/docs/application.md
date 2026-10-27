# Nanami Application Guide

This guide describes how to add native Nanami applications, services, and
drivers. The Rust application ABI supports x86_64 and AArch64; the freestanding
C++ SDK remains x86_64-only.

This guide follows Nanami v0.6.2 and `AlphaEnvironment` ABI 2. Public wire types
are defined in `nanami-uapi`; generic Alpha requests and managed-Service helpers
are exposed through `nanami-runtime`.

## Component Placement

```text
nanami/servers/
  core-services/        # Boot services and manager-selected hardware drivers
  apps/                 # Services and applications installed into rootfs /bin
  uapi/                 # Pure ABI/protocol definitions and C++ headers
  targets/              # Native target JSON and linker scripts
  libs/nanami-runtime/  # Entry, allocator, OS requests, IPC and C++ implementation
  libs/nanami-client/   # Typed service protocol wrappers
  libs/                 # Other reusable implementation crates
```

Use `core-services` for boot services and hardware drivers selected by
Driver Manager. Upper services, desktop components, tests and applications
belong in `apps`. Hardware drivers must not duplicate PCI configuration I/O:
use `nanami-client::pci` and add architecture/platform selection in
`core-services/driver-manager/src/arch/`. See [Device Drivers](device-drivers.md)
for current display and PCI contracts.

The build discovers Rust components through `Cargo.toml` and C++ components
through declarative `nanami-app.toml` manifests. A disabled source directory may retain `_Cargo.toml`, but it
is not built or installed.

## Startup Manifests

Choose one startup level:

- Add boot-critical components to `nanami/servers/boot-list`.
- Add rootfs system services to `nanami/servers/system-list`.
- Add default user-session applications to `nanami/servers/session-list`.
- Do not add applications that should be launched manually.

Manifest entries use:

```text
<name> <priority> <path> [arguments...]
```

The Alpha boot-list accepts only the first three fields (no arguments), treats
the name as a label, and reports malformed entries. It also accepts trusted
`@process <initramfs-path> [priority=N] [affinity=N] [platform=0|1] [services=0|1]` defaults.
These apply by explicit image path, not by a built-in Service name; rootfs ELF
buffer spawns do not inherit a grant by claiming a matching image label.
See [Boot Policy](architecture.md#boot-policy). Rootfs system/session entries
continue to support arguments. Priority is 0..31 on both paths.

Child-domain launch configuration also accepts explicit managed Service imports:
`@import <child-label> <local-alias> <source-name> <connection-limit>`.
Only the named stateless-control Service becomes visible under the alias; there
is no parent-name fallback or implicit forwarding of PID-based memory APIs.
See [Explicit Service Imports](architecture.md#explicit-service-imports) for
the shared upstream Session, local close handles, limits and supported lifecycle.

The rootfs image contains extensionless executable names. For example, the
`alter/linux` source directory is installed as `/bin/alter-linux`.

## Minimal Rust Application

Native Rust applications are `no_std`, use `nanami_runtime::nanami_entry!`, and
return `nanami_runtime::NanamiResult` from their entry function.

```rust
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    nanami_runtime::println!("[sample] panic");
    let _ = nanami_runtime::request_exit();
    loop {
        core::hint::spin_loop();
    }
}

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    nanami_runtime::println!("[sample] hello");
    Ok(())
}

nanami_runtime::nanami_entry!(nanami_main);
```

`nanami_entry!` parses the initial process stack, calls the application entry,
and reports its result through the Nanami exit request. The entry function
should not call `request_exit()` after returning `Ok(())`.

## Heap Use

Applications that allocate must initialize the Nanami allocator explicitly:

```rust
#![feature(alloc_error_handler)]
extern crate alloc;

fn nanami_main() -> nanami_runtime::NanamiResult {
    nanami_runtime::ipc::init_ipc_tls()?;
    let (_base, _mapped_size) = nanami_runtime::heap::init_heap(4 * 1024 * 1024)?;
    Ok(())
}
```

The allocator supports multiple regions and may request additional memory from
Alpha. Components with large bounded buffers should still size their initial
heap deliberately and handle allocation failure.

## Cargo Layout

A typical application contains:

```text
nanami/servers/apps/sample-app/
  .cargo/config.toml
  Cargo.toml
  src/main.rs
```

`Cargo.toml`:

```toml
[package]
name = "sample_app"
version = "0.1.0"
edition = "2021"

[dependencies]
nanami-runtime = { path = "../../libs/nanami-runtime" }
nanami-client = { path = "../../libs/nanami-client" }

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"

[workspace]
```

Use an existing neighboring `.cargo/config.toml` as the target configuration.
The top-level build selects the target under `targets/x86_64` or
`targets/aarch64`, enables `build-std`, and passes the matching Nanami user
linker script.

The empty `[workspace]` prevents Cargo from treating each application as a
member of an unrelated parent workspace.

## Build and Install

From the repository root:

```text
cargo xtask servers
cargo xtask build --rebuild-rootfs
cargo xtask run
cargo xtask build --arch aarch64 --no-linux-bins --rebuild-rootfs
cargo xtask run --arch aarch64 --net none
```

`build` assembles the boot disk; an existing staging rootfs is reused unless
`--rebuild-rootfs` is supplied. `run` never builds implicitly. Back up saved
guest data before rebuilding the boot disk, or use `--output-image` to isolate
the output.

For read-only hardware diagnostics in Shell, use `pci-info` (names and numeric
PCI IDs), `pci-info -n` (numeric only), or `pci-info -nn -v` (additional class and
command/status details). The database lives in PCI Server, not in the CLI.
`net-info` is the network-information Shell builtin; the former spelling
`netinfo` has been removed. See [PCI identification](device-drivers.md#identification-and-pci-info).

To package only selected native rootfs applications:

```text
cargo xtask rootfs --rootfs out/minimal.img --no-linux-bins --rootfs-app shell --rootfs-app nanami-info
```

To collect every file in a foreign-binary directory (recursively, by basename):

```text
cargo xtask rootfs --rootfs out/linux.img --linux-bin-dir "path/to/linux binaries"
cargo xtask rootfs --rootfs out/freebsd.img --freebsd-bin "path/to/sh"
```

The default Linux directory is `bin/`; additional files use repeatable
`--linux-bin`. Duplicate names and host links/special files are rejected.
A Linux directory tree can be added with `--linux-rootfs-dir`.

Public protocol constants/types belong in `nanami-uapi`. Runtime code belongs
in `nanami-runtime`; service IPC wrappers belong in `nanami-client`.
Do not put an executable service implementation in UAPI.

### External Boot Configuration

`build` produces the standalone Alpha ELF before packaging initramfs and
building the init Booter. `servers` alone does not rebuild that complete handoff.
After changing the Alpha launch contract, linker scripts or boot configuration,
use `build`, not just `servers` followed by `run`.

To supply a boot list and extra initramfs modules:

```sh
cargo xtask build --boot-list path/to/boot-list \
  --boot-module path/to/child.conf --boot-module path/to/worker.elf \
  --rootfs out/custom-rootfs.img --output-image out/custom.img
```

Each module is a regular file packaged at `./modules/<basename>`; duplicate
basenames are rejected. Non-ELF files can supply child configuration, while ELF
modules must match the selected architecture. Keep the platform's required core
service entries in a custom boot list; supplying a file replaces the packaged
list rather than appending to it.

An optional child declaration uses the same Alpha ELF and launch contract:

```text
@alpha laboratory 29 ./nanami/alpha - ./modules/child.conf
@import laboratory local-door upstream-door 2
```

The radix selects a real Generic RAM partition (`29` means 512 MiB). `-` supplies
no child archive; use a packaged archive path when that child launches modules.
An optional `after=<service-name>` on `@alpha` waits for local registration, not
for a health check or an authorization grant. Imports expose only the named
managed stateless-control Service. They do not grant parent memory/device APIs
or make local PIDs meaningful in another domain. See the
[child-domain contract](architecture.md#explicit-service-imports) and
[test configurations](../../tests/alpha/guest/child.conf).

## Connecting to a Service

Services are resolved through Alpha's registry into a capability slot chosen by
the client.

```rust
const SLOT_TIMER_SERVICE: nanami_runtime::Word = 23;

nanami_client::registry::connect_timer_service(SLOT_TIMER_SERVICE)?;
let timer = nanami_runtime::ipc::process_slot_descriptor(SLOT_TIMER_SERVICE);
nanami_client::timer::timer_service_sleep_milliseconds(timer, 100)?;
```

Rules:

- Treat the destination slot as owned after a successful connection.
- Cache service descriptors in long-lived clients.
- Do not reconnect into an occupied capability slot.
- Reattach shared memory only when the service protocol requires it, and
  invalidate stale pointers after reattachment.
- Retry only startup-dependent failures, with a timer-based bound.

Do not spin or repeatedly issue synchronous discovery requests while the timer
service itself is absent. Report/defer the dependency or retry from an existing
event source. Slot 20 is the conventional legacy server endpoint, 21 the default
notification and 22 the fault resolver; choose an unused application slot.

## Publishing a Service

Generic registration uses a name:

```rust
nanami_client::registry::register_service("sample-service")?;
```

The server port is conventionally stored in process slot 20. Add a typed
registration and connection wrapper to `nanami-client` when a protocol is
shared by multiple clients.

A service loop should receive a request, perform bounded work, reply, and return
to receive. Use `reply_receive` where possible. Do not hold the service loop in
an unbounded poll or wait on another synchronous service without accounting for
dependency cycles.

## Managed Services

The legacy registry API above returns a Provider-local PID and a direct RW
endpoint alias. Managed Services are a separate, opt-in API; they return a
stable `ServiceId`, a local `SessionId` close handle and a WRITE-only endpoint.
Neither ID is a PID. Use `nanami_runtime::service::connect`, not a legacy
registry wrapper, for a managed name.

```rust
let connection = nanami_runtime::service::connect("sample-service", 23)?;
// Issue protocol CALLs directly through connection.endpoint.
// After the protocol's outstanding work has completed:
nanami_runtime::service::close(connection.session)?;
```

Closing withdraws the client's handle. In the current Nanami implementation the
alias, Session slot and quota remain quarantined/charged; do not use repeated
connect/close as an unbounded allocation strategy. A9N v0.3.9-smp supports weak
alias deletion, but Nanami's managed cleanup path has not switched to reuse.

### Provider and Manager Responsibilities

A Provider registers its interface and private slots with `service::Receiver`.
Any process may register itself; publishing a public name and changing its
binding require a manager launched with `@process <image> services=1`.

| Profile | Registration helper | Persistent Service resources |
| --- | --- | --- |
| `STATELESS_CONTROL_PROFILE` (1) | `Receiver::register` | Stateless request/reply endpoint |
| `MANAGED_BUFFER_PROFILE` (2) | `Receiver::register_buffered` | Endpoint plus opaque per-Session buffer |
| `MANAGED_EVENT_PROFILE` (3) | `Receiver::register_buffered_events` | Buffer plus stable pending-work Notification |

Choose distinct unused root slots in 23..223 for the receive capability,
management notification and, for profile 3, work notification. The helpers bind
notifications and return `ProviderEvent::Request`, `Event` or `Retired` from
`receive()`. Reply to each accepted request with `Receiver::reply` before the
next receive. On `Retired`, unregister with `retire_provider(receiver.id)` and
exit. See the [Provider fixture](../../tests/alpha/guest/provider.rs) for the
sequential lifecycle; its test-only rendezvous is not a general discovery API.

An authorized manager uses `publish(name, provider)`, obtains `binding_state`,
and coordinates `prepare`, `quiesce`, then `commit` or `abort`. Refresh state
after transitions and use its generation, rather than inventing an ID or
assuming the first Provider owns the public name. Matching interface and
profile are required. Quiescence must finish accepted CALLs before receive
authority is withdrawn; unknown reply failures are not automatically replayed.
Clients keep their existing endpoint through a successful exchange. Alpha is
not a proxy for ordinary requests.

For profiles 2/3, clients allocate at most one buffer per Session using
`create_buffer(session, bytes)` (maximum 64 KiB, page-rounded). Providers obtain
their local view using `session_buffer(session)` and the request's Session
badge, never a client PID. Compatible Providers own the buffer layout and
validation. Profile 3 clients use `connect_event(session, slot)` for a stable
pending-work handle. Queue contents/cursors are authoritative; notification
counts are not. On resource-rescan hints a Provider enumerates
`buffer_session(provider, ordinal)` rather than querying Alpha on every event.

The x86_64 C++ SDK exposes `connect_managed_service`, `close_managed_session`,
`create_session_buffer`, `session_buffer` and `connect_session_event` in
[`nanami.hpp`](../servers/uapi/include/nanami/nanami.hpp). Cross-domain imports
currently accept profile 1 only. Private heaps, legacy subscriptions and device
DMA do not become migratable by selecting a managed profile.

## Timer Waits and Notification Bindings

Timer sleeps must grant and wait on the thread's currently bound Notification.
The `nanami_client::timer` helpers use the runtime's tracked binding rather than
assuming slot 21, preserve unrelated notification bits, and check the monotonic
deadline after wakeup. A coalesced interval/timer badge is not proof that a
particular sleep has completed.

Use the runtime binding helper when rebinding a thread. Do not substitute another
descriptor alias just because it names the same Notification: deferred badge
state is tracked per descriptor. Dedicated sleep-notification helpers are for
exclusive sequential delays, not a Notification shared with IRQ or interval
users. These helpers still require a working timer provider; the default
headless AArch64 configuration does not supply one.

## Shared Memory and Notifications

Use message registers for control fields, not bulk payloads. Paths, packets,
terminal buffers, framebuffer pixels, and filesystem data belong in shared
memory or DMA buffers.

Use notifications to signal that shared state is ready. A receiver must drain
the queue or state represented by that notification. Notifications may be
coalesced and are not one-message-per-event storage.

Every shared-memory protocol must validate:

- negotiated mapping size;
- offsets and lengths with overflow-safe arithmetic;
- record count and stride;
- ownership and lifetime;
- whether the service replaces an existing per-client attachment.

## VFS Applications

Connect to `vfs-service`, attach shared memory, place paths or payloads into the
mapping, and call the typed wrappers in `nanami_client::vfs`.

Current operations include open, compound open, read, delegated read, write,
create, mkdir, remove, rename, stat, fstat, directory read, and close.

Directory records use the VFS layout defined in
`nanami-client/src/vfs/constants.rs`. Applications must use those constants
instead of duplicating offsets.

## Terminal and Process Launch

`terminal-service` owns terminal sessions and shared terminal state.
`exec-service`, provided by `system-manager`, launches native executables from
the root filesystem with arguments and environment data.

Alter subscribes to terminal input before starting a terminal-bound program,
including programs such as `ping` that never read terminal stdin. With `ISIG`
enabled, the configured `VINTR`, `VQUIT` and `VSUSP` bytes generate SIGINT,
SIGQUIT and SIGTSTP for the terminal's foreground process group in its controlling
session. `ISIG` off and disabled control characters leave the input unchanged;
`NOFLSH` preserves queued input and partial canonical lines.

Alter/Linux supplies Unix98 PTYs independently of these native terminal
sessions. With BusyBox and Neovim packaged into a freshly built Linux rootfs,
launch Neovim normally and run `:terminal /bin/busybox sh -i`. The nested shell
has its own termios, process group, Ctrl-C handling and window size. Leave
Terminal-mode with `Ctrl-\ Ctrl-N`; `exit` ends the shell. The builder adds
`sh` aliases for BusyBox only if an explicit shell is absent. Existing images
need an explicit rootfs rebuild to acquire those aliases. See the
[syscall reference](../servers/apps/alter/linux/linux-supported-syscalls.md#unix98-pseudoterminals)
for supported operations and limits. Negoto's OCI console allocation remains
a separate interface; this does not enable its `terminal: true` profile.

Use these services instead of embedding application-specific launch logic in a
GUI client or Alpha.

## Negoto OCI Bundles

Negoto is a native application and management service using Alter/Linux to run
unmodified Linux ELF binaries. Its `alter-rootfs-v1` profile implements the
[OCI 1.3 bundle format](https://github.com/opencontainers/runtime-spec/blob/v1.3.0/bundle.md)
and create/start/state/kill/delete lifecycle. It is not a full OCI Linux
runtime or a hardened multi-tenant sandbox. It consumes an already unpacked
bundle; it does not pull registry images or unpack OCI layers.

The default system manifest starts `/bin/negoto serve` as `negoto-service`.
State persists across CLI invocations for the service's lifetime, not across
reboots. The checked-in x86_64 BusyBox sample is at
[`samples/negoto/bundles/busybox/config.json`](../../samples/negoto/bundles/busybox/config.json).
It starts `/bin/busybox sh -i` with a readonly rootfs. `process.terminal` stays
false: the invoking Nanami Shell's terminal is used without allocating a PTY.
Alter implements `TIOCGPGRP`, `TIOCSPGRP` and `TIOCGSID` using tracked session
and process-group state. `TIOCSCTTY` and `TIOCNOTTY` manage controlling-terminal
association; another session's terminal cannot be stolen. Fork inherits the
session/group and exec preserves them. Unknown ioctls still return `ENOTTY`.
Alter/Linux implements signal actions, masks, pending signals, `rt_sigreturn`,
`sigaltstack`, `rt_sigsuspend`, `rt_sigtimedwait`, `kill`/`tkill`/`tgkill`, and
`rt_sigqueueinfo`. Standard signals coalesce; realtime signals retain ordered
siginfo records in a bounded queue (`EAGAIN` on overflow). Signal state is
allocated lazily, inherited appropriately by fork, and reset for exec while
preserving ignored dispositions, pending signals, masks and real timers.

Handlers receive Linux ABI signal frames on x86_64 and AArch64. Alternate stacks,
`SA_SIGINFO`, `SA_NODEFER`, `SA_RESETHAND`, `SA_RESTART`, `SA_NOCLDSTOP` and
`SA_NOCLDWAIT` are supported. FPU/SIMD state is deliberately not saved or restored.
`rt_sigreturn` restores general registers and masks without accepting privileged
selectors, execution modes or flags from the user frame. A return trampoline is
mapped read/execute when the application does not supply its own restorer.

Running processes can receive signals without another syscall: Alter waits for
synchronous PCB Suspend to complete before reading their context, installs the
handler frame, and resumes them. This uses the A9N 0.3.11 suspend-continuation
contract. Unaccepted syscall faults are cancelled and replayed after the handler;
syscalls already parked in Alter retain their existing EINTR/SA_RESTART policy.
The currently emulated syscall is excluded from the asynchronous pump until its
reply is sent, so completed work is not replayed. Created OCI processes remain
suspended until start. SIGKILL uses native process termination, including for a
running target. Normal syscall replies and the IPC fastpath are unchanged.
Stop/continue signals are distinct from
termination; `wait4` reports stopped/continued children and signal exit status,
and SIGCHLD notifies the parent. `ITIMER_REAL`, `alarm` and `getitimer` deliver
SIGALRM through the timer service; CPU-time interval timers return `EOPNOTSUPP`.
Synchronous instruction, arithmetic and memory faults can enter a handler, and
broken-pipe writes generate SIGPIPE. Terminal control-character signals use the
same pending-signal and asynchronous delivery mechanism.

From the host repository root, copy the existing static Linux BusyBox into
the bundle. Binary payloads are ignored by Git and must be copied again in a
fresh checkout:

```sh
mkdir -p samples/negoto/bundles/busybox/rootfs/bin
cp bin/busybox samples/negoto/bundles/busybox/rootfs/bin/busybox
chmod 755 samples/negoto/bundles/busybox/rootfs/bin/busybox
```

Import `samples/negoto` under `/alter/linux`. Regeneration replaces the default
rootfs and boot image, discarding saved guest files; back up needed data first.

```sh
cargo xtask build --rebuild-rootfs --linux-rootfs-dir samples/negoto
cargo xtask run
```

For a non-destructive build, choose new paths for both `--rootfs` and
`--output-image`, omit `--rebuild-rootfs`, and use `run --image` for that image.
The guest bundle pathname is `/alter/linux/bundles/busybox`. In Nanami Shell:

```text
negoto run --bundle /alter/linux/bundles/busybox demo
```

Inside the BusyBox shell, for example:

```sh
pwd
/bin/busybox ls /
exit
```

`exit` lets `negoto run` reclaim the container and return to Nanami Shell.
The bundle contains only the BusyBox executable, not applet links; invoke
external applets as `/bin/busybox ls` rather than assuming `/bin/ls` exists.
The sample rootfs is readonly, so commands that modify it fail.

To use the individual lifecycle commands, stop the interactive process before
deleting its container:

```text
negoto create --bundle /alter/linux/bundles/busybox demo
negoto state demo
negoto start demo
negoto kill demo TERM
negoto state demo
negoto delete demo
```

`create` snapshots execution parameters, loads the image and initial stack,
and leaves the process suspended. `start` resumes that process; later edits to
`config.json` do not change those parameters. Rootfs file contents remain
externally mutable. `delete` requires a stopped container and reclaims its
process tree without removing the supplied bundle. The ID can then be reused.
`negoto kill demo KILL` stops a running process; `TERM` and signal `0` are also
accepted. `negoto run --bundle PATH ID` combines create/start/wait/delete using
the timer service. `negoto features` prints supported features. CLI bundle
paths must be absolute.

Supported inputs include argv, environment, absolute cwd, relative/absolute
`root.path`, `root.readonly`, and string annotations. Guest paths are normalized
before the backing-root prefix is added, including host-looking guest paths.
Fork/exec inherit the context and resolve ELF interpreters under the same root.
Existing Alter syscall support determines which Linux applications run. The
tested executable is a static x86_64 BusyBox, copied without ELF rewriting.

Requested namespaces, mount entries, cgroups/resources, seccomp, hooks, nonempty
capability sets, additional groups, nonzero UID/GID, custom rlimits, and PTY
allocation are rejected, not silently ignored. Empty capability sets and
`noNewPrivileges` do not grant privileges; a supplied `umask` must be 022.
Stream I/O can use the invoking Nanami terminal; headless contexts start with
three `/dev/null` descriptors. This is not a separate PTY. Network and virtual
`/proc`, `/sys`, `/dev` behavior comes from Alter, not Linux namespaces. Do not
use this profile for hostile workloads. General rootfs symlink resolution is
not provided by the current ext2/Alter path; the importer rejects host links
and special files. Use real files for this profile.

Limits: 8 containers, 16 simultaneous management clients, 32-KiB configurations,
64 arguments and 64 environment entries with 12 KiB combined string storage,
and 64 annotations with 8 KiB key/value storage. Backing paths must fit Alter's
256-byte buffer and cwd its 128-byte buffer. Native Negoto/Alter code builds
for x86_64 and AArch64; the execution regression exercises x86_64 BusyBox.

Verification on private, disposable images:

```sh
python3 tests/negoto/build-fixture.py --busybox bin/busybox \
  --rootfs out/negoto-rootfs.img --image out/negoto.img
python3 tests/negoto/qemu-smoke.py --image out/negoto.img --smp 1
python3 tests/negoto/qemu-smoke.py --image out/negoto.img --smp 4
cargo test -p negoto-oci -p nanami-uapi
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test negoto --test linux-pipe --test linux-readiness --test linux-sendfile -- --test-threads=1
```

The builder refuses existing output paths and reports the BusyBox SHA-256.
QEMU uses a disk snapshot and private firmware variables. The guest verifies
that create does not execute, argv/env/cwd survive, shell command substitution
and fork/exec work, native-root paths stay inaccessible, readonly writes fail,
kill/delete work, and one ID can be reused 20 times. It also launches the real
native CLI for features/create/state/start/delete and 18 run/wait/delete cycles
to check management-client detach/reuse.
The interactive regression uses the real CLI and a native terminal, verifies
a shell prompt without a job-control-disabled warning, command output distinct
from input echo, external fork/exec, pipelines and foreground transfer, then
exits and checks container cleanup.

## Honoka Applications

A Honoka client:

1. connects to `honoka-service`;
2. binds a notification port;
3. creates a window using drawable content dimensions;
4. attaches the shared logical framebuffer;
5. subscribes to input if needed;
6. writes pixels into the shared framebuffer;
7. submits damage or a present notification;
8. handles close and input events without busy polling.

Window borders, title bars, shadows, alpha composition, cursor, wallpaper, and
themes are compositor responsibilities. Reference clients include
`honoka-client`, `image-viewer`, `eg-test`, `shell`, and
`performance-monitor`.

## Network Applications

`network-service` provides shared-memory protocols for network configuration,
UDP, DNS, TCP, and ICMP. `http-server` is the native TCP server example.

Applications should wait for network progress through the service/event path.
Do not spin on receive state. Device access belongs to `virtio-net`; application
protocols belong above `net-server`.

## Adding a Component

1. Select `core-services` or `apps` based on boot dependency.
2. Copy the closest active component's Cargo and target configuration.
3. Implement a `NanamiResult` entry and bounded event loop.
4. Initialize IPC TLS and heap state as required.
5. Connect to dependencies through `nanami-client`.
6. Register a service only when other processes consume the component.
7. Add a startup-manifest entry only when automatic startup is required.
8. Build the component in release mode.
9. Rebuild the ext2 image and verify the installed extensionless path.
10. Test startup, normal operation, close/exit, and process reaping in QEMU.

## Verification

At minimum:

```bash
cargo xtask build --rebuild-rootfs
```

For user-visible or lifecycle changes, run QEMU and verify the actual workflow.
Native applications can use [GPU service](gpu.md) through `nanami_client::gpu`
for owned XRGB8888 buffers, synchronous fills/copies and presentation. Query
supported versus hardware-accelerated flags; a framebuffer provider implements
the same protocol without claiming a GPU. Honoka window clients can continue
using the existing shared-window framebuffer API.

For graphics changes, inspect the framebuffer at the target resolution. For
network changes, test both QEMU user networking and bridged networking when the
change touches the device path.

## Troubleshooting

### Custom target errors

Build through `cargo xtask servers` or use an existing application
`.cargo/config.toml`. Native applications require the custom target, Rust
`build-std`, and Nanami linker script together.

### Service connection fails

Confirm that the provider appears before the client in the appropriate startup
manifest and that the client slot is empty. A service that starts from rootfs
must not be required by a core service before `system-manager` runs.

### Rootfs executable is missing

Confirm that the component has an active `Cargo.toml` or `nanami-app.toml`, that
its release output exists, and that `--rootfs-app` selection is not filtering it
out. Rebuild the selected rootfs after changing application binaries or
manifests; an existing rootfs is otherwise preserved. Use a disposable path or
back up guest data before passing `--rebuild-rootfs`.
