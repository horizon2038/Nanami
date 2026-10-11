# Nanami User-space API and Libraries

`nanami/servers` contains boot services, rootfs applications and reusable
user-space libraries. There is no umbrella `sdk/` directory.

## Layout

```text
servers/
  boot-list, system-list, session-list   # Startup manifests
  core-services/                        # Boot-critical binaries
  apps/                                 # Upper services, drivers and apps
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

Only services needed before mounting the rootfs belong in `core-services/`.
Their binaries and the architecture-specific `boot-list` are packaged in
Rust-generated `initramfs.cpio`. Driver Manager selects platform timer/storage
drivers; ext2-server and system-manager consume the mounted filesystem.

Normal services/apps under `apps/` are installed into ext2 `/bin`.
Nested paths join with `-`: `apps/alter/linux` becomes `/bin/alter-linux`.
Rust binaries are discovered from Cargo manifests. Freestanding x86_64 C++
apps use declarative `nanami-app.toml` manifests, not Makefiles:

```toml
name = "cpp-hello"
sources = ["src/main.cpp"]
```

xtask invokes Clang/LLD directly, linking the shared runtime and matching target
script. AArch64 currently omits the x86-only ACPI/AHCI/HPET/PIT services and C++
sample.

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
See the [root README](../../README.md) for payload directories, bridge options,
macOS/Linux prerequisites and image-overwrite rules.

## Adding a Component

See the [Application Guide](../docs/application.md) for entry code, Cargo
layout, startup manifests and verification.
