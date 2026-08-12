# Nanami Architecture

Nanami is a user-space operating system built on the A9N Microkernel and the
Nun runtime. A9N provides capabilities, address spaces, scheduling, IPC,
interrupt delivery, and architecture-specific mechanisms. Nanami implements OS
policy in isolated user processes.

This document describes the current implementation. It is not an ABI stability
guarantee.

## Design Principles

- Keep policy out of the microkernel.
- Implement drivers and subsystems as independent user processes.
- Resolve services by name instead of embedding provider-specific knowledge in
  Alpha or applications.
- Use synchronous IPC for control operations.
- Use shared memory, DMA, ring buffers, and notifications for data and event
  paths.
- Reclaim process capabilities and physical memory through explicit lifecycle
  operations.
- Keep Linux, FreeBSD, POSIX, desktop, and application policy outside Alpha.

## System Layers

```text
UEFI
  -> A9NLoader-rs
  -> A9N Microkernel
  -> Nanami Alpha
       -> core services from initramfs
       -> system services from ext2 rootfs
       -> session applications from ext2 rootfs
```

### A9N Microkernel

A9N owns kernel objects and the mechanisms required to use them. Relevant
objects include capability nodes, generic memory, frames, page tables, address
spaces, process control blocks, IPC ports, notification ports, interrupt ports,
and I/O ports.

Nanami does not add filesystem, network, window-system, or POSIX policy to the
kernel.

### Nun

Nun provides the Rust entry environment and A9N ABI integration used by Alpha.
Native applications use `libnanami`, which builds on the same A9N ABI but
exposes Nanami OS requests and process startup conventions.

### Alpha

Alpha is the initial Nanami process. It receives the boot capabilities and
constructs the user-space OS environment.

Alpha is responsible for:

- root capability-space bootstrap;
- physical-memory allocation;
- process creation, status, termination, and reaping;
- address-space and mapping metadata;
- service registration and name resolution;
- distribution of IPC, notification, IRQ, I/O-port, DMA, MMIO, and shared-memory
  capabilities;
- initial framebuffer metadata;
- loading boot-critical ELF images from the embedded initramfs;
- diagnostic memory and process information exposed through `nanami-info`.

Alpha provides generic mechanisms. It does not know how ext2, TCP, Honoka,
POSIX file descriptors, Linux syscalls, or FreeBSD syscalls work.

## Process Classes

Nanami processes fall into four practical classes.

### Core Services

Core services are required before the root filesystem is usable. They live in
`nanami/servers/core-services`, are packaged into `initramfs.cpio`, and are
listed in `nanami/servers/boot-list`.

The current boot list starts:

- `timer-server`;
- `rtc-server`;
- `virtio-blk-server`;
- `ext2-server`;
- `system-manager`.

### System Services

System services live in the ext2 root filesystem under `/bin`. The
`system-manager` reads `/nanami/system-list` and starts terminal, POSIX, Alter,
input, display, desktop, and network services.

### Session Applications

After system services have been spawned, the same `system-manager` reads
`/nanami/session-list`. The default session starts the graphical shell. There
is no separate session-manager process in the current implementation.

### ABI Personalities

Alter/Linux and Alter/FreeBSD are native Nanami services that manage foreign
processes. A foreign syscall reaches A9N as an invalid-kernel-call fault. A9N
forwards the hardware context to the registered fault resolver, and Alter
returns the updated context in the fault reply.

This keeps foreign ABI policy in user space while avoiding a separate register
read and write kernel call for every emulated syscall.

## Boot Policy

Nanami has three manifest levels:

| Manifest | Reader | Contents |
| --- | --- | --- |
| `/nanami/boot-list` in initramfs | Alpha | Boot-critical core services |
| `/nanami/system-list` in rootfs | `system-manager` | Upper system services |
| `/nanami/session-list` in rootfs | `system-manager` | User session applications |

All manifests use this format:

```text
<name> <priority> <path> [arguments...]
```

Alpha reads only the embedded boot list. If the rootfs manifests cannot be
read, `system-manager` uses the copies embedded into its binary at build time.

## Capability and Memory Management

### Physical Memory

Alpha receives Generic capabilities from A9N and maintains physical-memory
allocation metadata in user space. Memory is converted to more specific kernel
objects only when needed.

Nanami tracks ownership and references for process mappings, DMA allocations,
and shared memory. Shared backing memory remains allocated while any registered
mapping reference exists.

### Per-Process Capability Space

Each managed process has a root capability node containing its PCB, address
space, OS port, service ports, notification ports, device capabilities, and
frame/page-table capability hierarchy. Alpha reserves only the slots required
by its layout and allocates process roots dynamically.

### Virtual Memory

Alpha tracks mappings separately from capability descriptors. The mapping
metadata records virtual ranges, backing allocations, frame slots, and mapping
kind. This supports anonymous mappings, fixed mappings, process memory copies,
memory cloning, shared memory, MMIO, DMA, and explicit mapping release.

Native heaps request anonymous regions from Alpha. `libnanami` manages multiple
heap regions and can grow the allocator without assuming a single fixed heap.

### Process Exit and Reaping

Exit and resource destruction are separate operations:

1. A process reports its exit status and is suspended.
2. Its authorized reaper observes the status.
3. The reaper requests process reaping.
4. Alpha removes the process root capability.
5. A9N revokes dependent capabilities.
6. Alpha releases process metadata and physical-memory references.

This ordering is required because Generic-backed kernel memory cannot be reused
until the derived capability tree has been revoked.

## Service Registry

Servers register a service name and service-port capability with Alpha. Clients
connect by name and receive a copy of that port in a caller-selected capability
slot.

Current service names include:

| Service | Provider | Role |
| --- | --- | --- |
| `timer-service` | `timer-server` | Sleep and interval timers |
| `rtc-service` | `rtc-server` | RTC time |
| `block-device` | `virtio-blk-server` | Block I/O |
| `vfs-service` | `ext2-server` | ext2-backed VFS |
| `exec-service` | `system-manager` | Rootfs process launch |
| `terminal-service` | `terminal-service` | Terminal sessions |
| `posix-service` | `posix-server` | POSIX-oriented process and FD facade |
| `alter-linux` | Alter/Linux | Linux ABI personality |
| `alter-freebsd` | Alter/FreeBSD | FreeBSD ABI personality |
| `input-service` | `input-server` | Input event distribution |
| `display_service` | `fb-server` | Boot framebuffer sharing |
| `honoka-service` | Honoka | Compositor and window service |
| `net-device` | `virtio-net` | Network-device backend |
| `network-service` | `net-server` | IPv4, ICMP, UDP, DNS, and TCP |

Service slots are capabilities. A long-lived client should retain a successful
connection instead of repeatedly connecting into the same occupied slot.

## IPC and Data Paths

### Control Path

Service registration, connection, metadata queries, capability transfer, and
operation dispatch use synchronous A9N IPC. Servers normally use a
receive/reply-receive loop so the IPC fast path remains available.

### Data Path

Large or frequent data is not copied through message registers:

- virtio drivers use DMA queues;
- block, VFS, POSIX, terminal, and network services use shared memory;
- Honoka clients draw into shared logical framebuffers;
- input and device paths use shared queues.

### Event Path

Notifications represent pending work. IRQ delivery, timer completion, input,
network progress, and compositor damage use notification ports. A notification
handler drains the associated shared state rather than treating the
notification as the data itself.

## Storage Stack

```text
raw ext2 image
  -> virtio-blk PCI device
  -> virtio-blk-server  (block-device)
  -> ext2-server        (vfs-service)
  -> system services and applications
```

`virtio-blk-server` owns the device, virtqueue, DMA buffers, I/O-port range,
and IRQ capability. `ext2-server` implements the filesystem-facing operations
and caches filesystem data above the block service.

The rootfs image builder installs:

- system and session manifests under `/nanami`;
- native applications under `/bin` using extensionless executable names;
- external Linux binaries under `/alter/linux/bin`;
- external FreeBSD binaries under `/alter/freebsd/bin`;
- Honoka configuration and themes under `/.honoka`.

The built-in ext2 writer supports multiple block groups and indirect blocks.
It fails on insufficient capacity rather than silently omitting applications.

## Network Stack

```text
virtio-net PCI device
  -> virtio-net  (net-device)
  -> net-server  (network-service)
  -> native clients and Alter sockets
```

`virtio-net` owns the hardware-facing queues. `net-server` implements DHCP,
ARP, IPv4, ICMP, UDP, DNS, and TCP services. Alter/Linux maps supported socket,
raw ICMP, and route-netlink operations onto this service, allowing BusyBox
tools such as `ip` and `ping` to use the native Nanami network stack.

## Graphics and Input

```text
boot framebuffer
  -> fb-server
  -> Honoka compositor
  -> shared window framebuffers
```

Honoka owns desktop composition, window decorations, transparency, themes,
wallpaper, cursor rendering, focus, and window movement. Clients own only their
drawable content and submit damage after writing shared pixels.

```text
PS/2 hardware
  -> ps2-server
  -> input-server
  -> Honoka and subscribed clients
```

Input events are distributed through per-subscriber shared queues and
notifications.

## POSIX and Alter

`posix-server` provides process metadata, credentials, environment variables,
file descriptors, open-file descriptions, directories, terminal I/O,
filesystem operations, memory operations, process launch, waiting, and socket
plumbing over native services.

Alter reuses this facade but owns ABI-specific structures and behavior. Linux
and FreeBSD support remains partial; a dispatched syscall may intentionally
implement only the semantics needed by currently tested static binaries.
Canonical syscall lists are maintained beside each personality implementation.

## Build Integration

Nanami delegates final image construction to SPENCER:

- `scripts/build-image.sh` builds user-space components and the initramfs, then
  invokes SPENCER with Nanami as an external OS payload;
- `scripts/create-ext2-image.sh` builds the root filesystem image;
- `scripts/run-qemu.sh` configures OVMF, block devices, networking, capture,
  acceleration, and QEMU execution.

See the repository [README](../../README.md) for supported commands and runtime
options.
