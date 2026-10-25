# Nanami User-space API and Libraries

`nanami/servers` contains boot services, rootfs applications and reusable
user-space libraries. There is no umbrella `sdk/` directory.

## Layout

```text
servers/
  boot-list, system-list, session-list   # Startup manifests
  core-services/                        # Boot services and manager-selected drivers
  apps/                                 # Upper services and apps
  uapi/                                 # nanami-uapi: dependency-free no_std ABI
    include/                            # Public C++ headers
  targets/{x86_64,aarch64}/              # Target JSON and linker scripts
  libs/
    nanami-runtime/                     # Entry, allocator, OS requests and IPC
      arch/                             # Architecture-specific C++ runtime/ABI
      cpp/                              # C++ implementation
    nanami-client/                      # Typed service clients
    ...                                 # Reusable subsystem implementations
```

## Dependency Direction

```text
nanami-uapi <- nanami-runtime <- nanami-client <- applications/services
      ^              ^
      |              |
    Alpha        C++ runtime uses uapi/include
```

- `nanami-uapi` owns request/response numbers, flags and wire value types.
  It has no allocator, IPC execution or kernel-ABI crate dependency.
  Alpha and native clients share the same OS definitions.
- `nanami-runtime` implements native entry/exit, diagnostics, allocation,
  capabilities, OS requests and IPC. It re-exports UAPI values needed by callers.
- `nanami-client` implements registration/connection and typed wrappers for
  block, VFS, exec, terminal, POSIX, graphics, input, timers, RTC, power,
  device management and networking. Protocol values are re-exported from UAPI,
  not copied into client implementations.
- Other `libs/` crates contain subsystem implementations, not the public ABI.

Rust package names use hyphens; imports use underscores
(`nanami_runtime`, `nanami_client`, `nanami_uapi`).
The wire numbers and Linux/FreeBSD guest ABIs are unchanged by this split.

## Components and Packaging

Boot services and manager-selected hardware drivers live in `core-services/`.
Their binaries and the architecture-specific `boot-list` are packaged in
Rust-generated `initramfs.cpio`. Driver Manager selects platform PCI, timer,
storage, input, display and network-device drivers; ext2-server and
system-manager consume the mounted filesystem. In particular, `fb-server`,
`ps2-server` and `virtio-net` are no longer unconditional rootfs services.
`system-list` starts logical services, including input distribution, the network
stack and Honoka, which connect to those selected hardware providers.

On x86_64/pc99, `pci-server` owns configuration I/O and caches enumeration.
Clients use `nanami-client::pci`; DMA, MMIO and packet/block data paths remain
direct. `intel-graphics-server` selects its backend by PCI ID; the first backend
implements inherited-mode Gen9 scanout with GGTT-backed double buffering,
VBT-aware active-output discovery and monitor EDID over GMBUS/I2C-over-AUX.
Honoka submits one vblank flip per composed frame on this backend. Monitor
detailed-timing queries are read-only, not an implemented modesetting API.
Other display configurations use `fb-server`. See
[device drivers](../docs/device-drivers.md) for the protocol, ownership rules,
supported modes and validation limits.

Normal services/apps under `apps/` are installed into ext2 `/bin`.
Nested paths join with `-`: `apps/alter/linux` becomes `/bin/alter-linux`.
Rust binaries are discovered from Cargo manifests. Freestanding x86_64 C++
apps use declarative `nanami-app.toml` manifests, not Makefiles:

```toml
name = "cpp-hello"
sources = ["src/main.cpp"]
```

xtask invokes Clang/LLD directly, linking the shared runtime and matching target
script. AArch64 currently omits the x86-only PCI, Intel graphics, PS/2, virtio-net,
ACPI/AHCI/HPET/PIT services and C++ sample.

The headless AArch64 configuration has no timer provider yet. Alter/Linux can
register and handle non-clock work without one; clock-dependent operations
resolve the timer on demand and return an error while it is absent, rather than
blocking the boot manager in a high-priority retry loop.

## Build

Run from the repository root:

```text
cargo xtask servers
cargo xtask servers --arch aarch64
cargo xtask rootfs --rootfs out/new-ext2.img
cargo xtask build --rebuild-rootfs
cargo xtask run
```

`servers` builds all native components and the initramfs; `rootfs` packages
previously built applications using the reusable `crates/ext2-image` library.
The complete `build` task also builds `nanami`'s `alpha` executable using its
own boot linker script, then stages it as the ignored `nanami/alpha.elf` input
to the init Booter build. Use `cargo xtask build` to regenerate both phases;
building servers alone does not update an already embedded Alpha executable.
The complete image also packages the common Alpha ELF as `./nanami/alpha` for
externally configured child launches. `--boot-list PATH` supplies First Alpha's
configuration, and repeatable `--boot-module PATH` adds regular files under
`./modules/<basename>`; duplicate names and wrong-architecture ELF modules are
rejected. Explicit module paths are not restricted to `bin/`, and ambiguous bare
executable names fail instead of silently selecting another directory's file.
See the [root README](../../README.md) for payload directories, bridge options,
macOS/Linux prerequisites and image-overwrite rules.

## Registry API migration

Registration and lookup use externally supplied names (1..24 UTF-8 bytes), not
Service-kind numbers. The core rejects replacement of an existing name by a
different owner/endpoint and accepts exact registration retries. It still
returns local owner PIDs for existing shared-memory APIs; those are not global
domain identities or a replacement for connection authority.

Rust `service_info_by_ordinal` now returns `ServiceInfo` with the actual name;
the former `(pid, fixed-kind)` result and `SERVICE_KIND_*` constants are removed.
Rebuild native clients/rootfs images together with Alpha for the changed list
wire format. Existing images are deliberately not rewritten by `build`.

C++ components can use `register_service(name, service_slot)` and
`connect_service(name, destination_slot, &owner_pid)`. The old net-device
convenience wrapper delegates to name registration rather than sending the
obsolete numeric-kind request. A retained connection is still direct IPC;
these legacy connections do not add an Alpha proxy or implement Provider exchange.

Managed stateless-control Services use the additive Rust
`nanami_runtime::service` API: register a Provider, explicitly publish it from a
`services=1` configuration component, and connect to a `Connection` containing
ServiceId, SessionId and a WRITE-only endpoint (not a Provider PID). `Receiver`
handles cooperative quiescence after replying to accepted calls. The manager
prepares a compatible successor, quiesces, waits for acknowledgement, then
commits. Existing handles keep the same endpoint; ordinary requests do not
traverse Alpha. Legacy connect rejects managed names rather than granting RW
client authority. See the
[managed lifecycle](../docs/architecture.md#managed-services-and-cooperative-receiver-exchange)
for commit/failure behavior, supported scope, limits and quarantined reclamation.
C++ clients have `connect_managed_service` returning `ServiceConnection` and
`close_managed_session`; the endpoint can be used by compatible stateless protocol
calls. Neither SDK presents the Session identifier as a Provider PID.

External `@import <child-label> <local-alias> <source-name> <connection-limit>`
entries delegate only named managed stateless-control Services to a Child Alpha.
Clients keep using `service::connect` / `connect_managed_service` in that child's
namespace. An imported connection's local SessionId is a close handle; the
upstream Provider observes the separately scoped shared import Session badge.
No PID-based shared-memory or notification API is implicitly forwarded.
See [Explicit Service Imports](../docs/architecture.md#explicit-service-imports)
for authority, accounting and reclamation limits. Rebuild the common Alpha ELF
and Booter together for AlphaEnvironment ABI version 2.

`Receiver::register_buffered` opts into the opaque managed-buffer profile (2).
`create_buffer(SessionId, bytes)` grants a client-local view (up to 64 KiB);
`session_buffer(SessionId)` returns the current Provider's view of that same
backing. These are Session-authorized operations, not peer-PID lookup. No Frame
capability is delivered to either process. New Provider mappings are installed
before receiver commit; abort restores the old VAs. An attached buffer currently
requires whole-Service quiescence before closing, and its backing/quota stays
charged. C++ clients use `create_session_buffer` / `session_buffer` with
`SessionBuffer` outputs. Imports, legacy Notification subscriptions, DMA and private
application-state migration are not part of this profile. See
[Opaque Managed Session Buffers](../docs/architecture.md#opaque-managed-session-buffers).

`Receiver::register_buffered_events(interface, receiver, management, event)`
opts into profile 3: the same local buffers plus a stable pending-work
Notification. Rust `connect_event` / C++ `connect_session_event` grants a
WRITE-only client handle. Work NOTIFYs go directly to the current Provider;
Alpha neither proxies events nor interprets queue cursors. The runtime/Alpha
withdraw old visible and PCB-held event aliases before data/control handover,
then seed a rescan before activating the successor. See
[Managed pending-work Events](../docs/architecture.md#managed-pending-work-events-and-retained-queue-state).

## Adding a Component

See the [Application Guide](../docs/application.md) for entry code, Cargo
layout, startup manifests and verification.
