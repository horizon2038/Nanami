# Nanami User-Space Components and SDK

`nanami/servers` contains Nanami's boot services, rootfs applications, reusable
libraries, and native user-space SDKs.

## Layout

```text
servers/
  boot-list              # Alpha/initramfs startup manifest
  system-list            # Rootfs system-service manifest
  session-list           # Rootfs user-session manifest
  core-services/         # Boot-critical services
  apps/                  # Upper services, drivers, and applications
  libs/                  # Reusable implementation crates
  sdk/
    arch/x86_64/          # Target specification and linker scripts
    arch/aarch64/         # AArch64 target specification and linker scripts
    build/                # Shared C++ build rules
    cpp/                  # Freestanding C++ SDK
    rust/libnanami/       # Low-level Nanami core API
    rust/nanami-services/ # Typed service protocols
```

## Component Classes

### Core Services

Only components required to mount and consume the root filesystem belong in
`core-services`. Active Rust and C++ outputs are packaged into
`initramfs.cpio`, and Alpha starts entries from `boot-list`.

The current core path is:

```text
Device Driver Manager -> platform timer + platform storage
                      -> ext2-server -> system-manager
```

The manager queries Alpha for A9N v0.3.0's `init_info` architecture/platform
names and core count before selecting drivers. Driver policies currently
support `x86_64/pc99` and `aarch64/qemu`; unknown platforms are rejected instead
of assuming that all machines of the same architecture have the same devices.

On x86_64 the manager maps `arch_info[0]`'s RSDP on demand, validates and walks
the XSDT/RSDT in user space, and selects HPET when its ACPI table is present.
It selects AHCI through PCI and keeps the existing PIT and virtio-blk
implementations as fallbacks. On AArch64 it currently selects virtio-mmio
storage and deliberately selects no timer: EL0 virtual-timer access is not
exposed while the platform timer policy remains deferred. Architecture-specific
probing lives below `driver-manager/src/arch`.

### Applications and Upper Services

Components under `apps` are built with the same native ABI but are installed
into the ext2 root filesystem under `/bin`. `system-manager` starts entries from
`system-list` and `session-list`; other applications are launched manually.

The rootfs builder uses extensionless executable names. Nested source paths are
joined with `-`, so `apps/alter/linux` becomes `/bin/alter-linux`.

## Rust SDK

`libnanami` provides:

- x86_64 and AArch64 process entry and argument parsing;
- A9N IPC and notification primitives;
- Nanami OS requests for process, memory, capabilities, and services;
- heap allocation and growth;
- process exit and diagnostic information.

`nanami-services` provides named-service registration/connection and typed
protocol wrappers for block, VFS, execution, terminal, POSIX, graphics, input,
timer, RTC, device management, and networking services.

Service-specific protocol code belongs in `nanami-services`, not in
`libnanami`.

## C++ SDK

`sdk/build/cpp_app.mk` supplies the freestanding x86_64 C++ build using Clang,
LLD, the Nanami user linker script, and the C++ Nanami support library.

## Build

From the repository root:

```bash
make servers
make servers ARCH=aarch64
```

Or from this directory:

```bash
make initramfs
```

This builds all active core services and applications, then packages only
`core-services` plus `boot-list` into `initramfs.cpio`.

Build component classes directly:

```bash
make rust
make cpp
make apps
```

Create the ext2 root filesystem from the repository root:

```bash
make fs-image SIZE_MB=64 OUT=out/ext2.img
make fs-image ARCH=aarch64 SIZE_MB=64
```

For x86_64, `make image` embeds this filesystem in the Nanami root partition of
the same GPT `spencer.img` that contains the EFI System Partition. AHCI scans all
active SATA ports and selects the partition by the Nanami root type GUID; block
requests remain relative to that partition. The standalone image remains useful
as a staging artifact and for rebuilding the combined disk.

## Adding a Component

See the [Nanami Application Guide](../docs/application.md) for entry code,
Cargo layout, service use, startup manifests, and verification requirements.
