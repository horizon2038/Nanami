# Nanami Application Guide

This guide describes how to add native Nanami applications, services, and
drivers. The Rust application ABI supports x86_64 and AArch64; the freestanding
C++ SDK remains x86_64-only.

## Component Placement

```text
nanami/servers/
  core-services/        # Boot-critical binaries packaged into initramfs
  apps/                 # Services and applications installed into rootfs /bin
  uapi/                 # Pure ABI/protocol definitions and C++ headers
  targets/              # Native target JSON and linker scripts
  libs/nanami-runtime/  # Entry, allocator, OS requests, IPC and C++ implementation
  libs/nanami-client/   # Typed service protocol wrappers
  libs/                 # Other reusable implementation crates
```

Use `core-services` only when a component must run before the root filesystem
is available. Normal drivers, upper services, desktop components, tests, and
applications belong in `apps`.

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

## Connecting to a Service

Services are resolved through Alpha's registry into a capability slot chosen by
the client.

```rust
const SLOT_TIMER_SERVICE: nanami_runtime::Word = 22;

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

Use these services instead of embedding application-specific launch logic in a
GUI client or Alpha.

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

Confirm that the component has an active `Cargo.toml` or `Makefile`, that its
release output exists, and that `ROOTFS_APPS` is not filtering it out. Rebuild
`out/ext2.img` after changing application binaries or manifests.
