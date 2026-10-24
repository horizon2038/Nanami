# Nanami Architecture

Nanami is a user-space operating system built on the A9N Microkernel and the
Nun runtime. A9N provides capabilities, address spaces, scheduling, IPC,
interrupt delivery, and architecture-specific mechanisms. Nanami implements OS
policy in isolated user processes.

This document describes the current implementation. It is not an ABI stability
guarantee.

The current baseline is Nanami v0.6.0, `AlphaEnvironment` ABI 2 and `a9n_abi`
0.8.0. The checked-out Spencer revision includes A9N v0.3.11-smp. Historical
QEMU results below retain their original scope; they are not evidence that
every fixture was rerun after a kernel revision change.

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
  -> Alpha Booter (init, suspended after handoff)
       -> First Alpha (separate PCB, capability space and address space)
            -> core services from supplied initramfs
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

Nun provides the Rust init entry environment used by Booter and A9N ABI
integration shared with Alpha. Alpha's entry uses its own boot contract, not
Nun's init-only entry macro.
Native applications use `nanami_runtime`, which builds on the same A9N ABI but
exposes Nanami OS requests and process startup conventions.

### Alpha

`alpha_booter` is the A9N init process. It adapts init metadata and capability
layout, constructs an independent `alpha` ELF execution environment, installs
`nanami_uapi::alpha::AlphaEnvironment`, transfers authority and resumes Alpha.
Booter and Alpha are distinct processes with distinct address spaces, capability
spaces, stacks and IPC buffers. First Alpha requires no parent server connection.

`nanami_core::alpha_launch` is the allocation-free execution-environment builder
for launchers. It takes actual Generic authority, ELF/module/configuration bytes,
and caller-reserved capability slots. It does not create an OS allocator,
registry or device driver. Alpha initializes its own managers after entry.

Both Alpha linker scripts explicitly define their loadable segments, leaving
ELF headers outside the user mapping. This avoids AArch64 LLD's implicit
address-zero header segment. The build checks the Alpha ELF's machine, load
ranges and executable entry before embedding it, and tracks linker scripts as
Cargo inputs so script changes actually relink Alpha.

Alpha is responsible for:

- management of the capability space supplied by the boot adapter;
- physical-memory allocation;
- process creation, status, termination, and reaping;
- address-space and mapping metadata;
- service registration and name resolution;
- distribution of IPC, notification, IRQ, I/O-port, DMA, MMIO, and shared-memory
  capabilities;
- initial framebuffer metadata;
- loading boot-critical ELF images from the supplied module archive;
- diagnostic memory and process information exposed through `nanami-info`.

Alpha provides generic mechanisms. It does not know how ext2, TCP, Honoka,
POSIX file descriptors, Linux syscalls, or FreeBSD syscalls work.

### Alpha Resource Input

`nanami/servers/uapi/src/alpha.rs` defines the current resource contract. It
contains an ABI version/size, receiver-local self-capability descriptors,
platform/version/display metadata, an optional parent-control descriptor, and
up to 128 memory resources. Each resource includes its actual descriptor,
physical extent, device flag and already consumed prefix. All metadata is owned
inline; it does not retain pointers to `InitInfo` or the adapter's stack.
`boot_archive` and `boot_configuration` identify copied, receiver-owned mappings,
not pointers into Booter. The Alpha executable itself does not embed a service
archive or choose the boot configuration's location.

`alpha_booter/capability_space.rs` alone expands the A9N init root and interprets
its reserved slots. `alpha_booter.rs` converts the resulting descriptors and
reservations. No `InitInfo`, `InitSlotOffset`, or init generic-slot calculation
remains in `nanami_core`. Memory management uses the descriptors supplied for
each resource, not its index in an assumed init capability node. Alpha creates
its own allocator, kernel-object pools and OS endpoint after validation.

Metadata is not authority: the real capabilities must already exist in the
receiving space. Validation checks bounds, overflow, resource flags, consumed
prefixes, duplicate descriptors and required self-capabilities; it cannot prove
that a sender has installed a capability or that claimed metadata matches it.
Consumed prefixes enter both the capability-conversion watermark and the
physical allocator's reserved set, preventing their reallocation. Alpha's own
process record now retains its actual PCB descriptor instead of zero.
Sub-page final conversions reserve their entire partially occupied page in
both the page allocator and the untyped frame-source index; exact byte
watermarks are not treated as page-aligned free ranges.

The launcher records whole Generic reservation units before converting their
contents. Image/BSS, private IPC/TLS, stack, archive, configuration, page tables,
nodes and PCB backing are consequently excluded from later physical allocation.
At commit it moves Generic authority (A9N forbids Generic COPY) and optional
device authority into the recipient. All resource descriptors in the final
record address the recipient's own capability space. Preparation never resumes
the child. Failure leaves successful reservations accounted/quarantined rather
than making uncertain backing memory available for reuse; a partially completed
MOVE handoff is not a reusable/rolled-back launch input.

Booter resumes First Alpha and suspends itself. Alpha also suspends the launcher
before initializing managers, through the receiver-local `retired_launcher`
PCB reference. Both run on core 0 during this handoff; this is not a claim that
remote-core Suspend return proves completion. References to Booter's old root,
address space, frames, page tables and IPC frame are retained in the contract.
Their backing remains reserved. This implementation does **not** revoke or
recycle stopped-Booter resources, which could revoke shared derivation branches.

### Child Alpha Startup and Memory Ownership

The same Alpha ELF, `AlphaEnvironment`, launcher and manager initialization now
start a device-free Child Alpha. A trusted external boot manifest can contain:

```text
@alpha <label> <memory-radix> <alpha-image> <archive-or-> <configuration> [after=<service-name>]
```

The budget is `2^memory-radix` bytes, currently at least 512 MiB because of the
existing manager bootstrap pools. The parent selects a free aligned untyped RAM
extent, reserves it in its physical allocator, retains its outer Generic and
moves inner Generics into the child. Two initial 256-MiB extents cover the
bootstrap pools and heap; larger budgets add successively doubled extents
(512 MiB, 1 GiB, etc.) without changing the total grant. This preserves whole
untyped sources for recursive delegation instead of partially occupying both
halves of a 1-GiB child. Node, PCB, page-table, frame, image,
stack, module and allocator backing all come from that real partition; there is
no upstream allocation fallback. The parent records the extent and lifecycle
before any conversion can fail. Normal parent allocation, freeing and mapping
paths exclude a live or quarantined partition. Allocation searches free buddy
blocks inside untyped extents rather than scanning physical pages.

`-` copies the launching Alpha's module archive; a named archive copies that
module's bytes instead. Configuration bytes are independently copied. The child
gets no parent root, interrupt region, I/O port or framebuffer authority. The
launcher's temporary mapping page tables are prepared from **parent** backing
before building the child, so successful child preparation does not leave a
parent page table backed by revocable child memory. Child records retain their
parent-local memory ID, root-slot reservations and PCB control capability.

`after=` defers startup until the given name is registered in the **local**
namespace. It is registration readiness, not a health check or a Provider
authorization policy. Alpha checks the pending entries on registration, without
polling, recursively launching a configuration server, or interpreting that
name's purpose. The current creation input is trusted boot configuration; an
untrusted caller-facing child-administration API is not provided yet.

Each Alpha has its own registry and local PIDs. `domain_id` is a launcher-local
diagnostic ID, not a global PID or authorization token. Child fixtures register
an arbitrary name, reject lookup of an already registered but unimported parent
name, create a local process/provider, and verify its IPC echo. The parent and
child can use that same public name independently. Child-generated processes
are not included in the parent's ordinary process table; ownership is tracked
in the child-domain records instead.

### Explicit Service Imports

The common `AlphaEnvironment` ABI version 2 includes up to 16 named imported
Services. An external parent configuration specifies:

```text
@alpha laboratory 29 ./nanami/alpha - ./modules/child.conf
@import laboratory child-door upstream-public-name 2
```

The fields are child label, child-local alias, source-local managed Service name,
and downstream connection limit (1..64). Unknown/ambiguous child labels, duplicate
aliases and excess imports fail validation. Startup waits for the specified
managed publications; it does not import other names or fall back to the parent.
Legacy PID-dependent interfaces are not importable through this control profile.

For each import, the source Alpha charges one Session owned by its child-domain
delegation record, distinct from an equal-valued local process PID. The target
Alpha receives a WRITE|COPY alias with its upstream Session identifier fixed;
it receives neither READ/receive nor MODIFY/identity-changing authority. The
launcher MINTs it into a receiver-local import directory (root slot 10). Bootstrap
registers only the supplied local alias, not a parent registry connection.

Local clients connect and close through their own Alpha. MINT gives them
WRITE-only handles in a single operation, without temporarily granting COPY or
identity-changing authority. Each local connection consumes the configured
per-import quota and the Alpha-wide 64-Session quota.
Closed handles remain charged/quarantined, as do failed launch exports. A process
with unclosed managed aliases cannot be reaped and have its Cspace reused.

These downstream handles share **one upstream Session**, not independent parent
Sessions. Their returned SessionIds are local close handles: an upstream Provider
sees the upstream badge, not those local IDs or child PIDs. The child cannot
select the upstream Provider. Re-importing the delegated authority into a further
child preserves that badge and charges another local domain-owned Session.
Service-specific buffer/range/work limits must be enforced by the Provider for
the upstream Session; Alpha does not interpret its protocol. The configured
downstream limit is enforced by the child Alpha's allocation API, not an
unforgeable bound on alias duplication by an administrator with its root Node.
Duplicated aliases do not create additional upstream Session identities.

Normal requests take `child client -> stable endpoint -> upstream Provider`,
without crossing either Alpha event loop. Source-side receiver exchange therefore
changes the Provider behind existing imported handles without reconnecting.
Imports currently use the stateless-control profile only, not PID-based shared
memory, queue migration, device authority, or cross-domain fault recovery.

The boot record is **not yet the complete domain lifecycle ABI**.
Cooperative child shutdown control remains to be implemented. Memory reclamation has an unsafe internal
primitive that revokes the retained Generic before releasing RAM; failure keeps
the partition reserved. Its tests model revocation and reuse, but do **not**
prove that stopping/revoking a live domain on the real kernel is safe. The caller
must establish stopped descendants, removed launcher aliases and quiescent
external IPC/DMA; a remote Suspend return is not that proof. No running child is
automatically reclaimed. CPU-time/affinity isolation, DMA isolation and parent
service-specific allocation limits are not supplied by this memory partition
mechanism; the imported Session/connection accounting above is separate.

## Process Classes

Nanami processes fall into four practical classes.

### Core Services

Core services are required before the root filesystem is usable. They live in
`nanami/servers/core-services`, are packaged into `initramfs.cpio`, and are
listed in `nanami/servers/boot-list`.

The default x86_64 configuration currently starts:

- `driver-manager`;
- `rtc-server`;
- `ext2-server`;
- `system-manager`.

`driver-manager` selects and starts the supported hardware drivers in its own
server, outside Alpha. These names describe the checked-in configuration, not
reserved core names. A component label has no privilege semantics. The
legacy automatic archive scan has been removed: an empty startup configuration
starts no services.

### System Services

System services live in the ext2 root filesystem under `/bin`. The
`system-manager` reads `/nanami/system-list` and starts terminal, POSIX, Alter, Negoto,
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

### Negoto Execution Contexts

`negoto-service` owns OCI configuration validation, container IDs, annotations
and lifecycle state. Alter/Linux exposes a generic prepared-execution context,
not OCI-specific policy: backing root, cwd, executable, argv/env and readonly
mode. Prepare loads a suspended process; start resumes it. Public protocols
are in `nanami-uapi::{alter,negoto}`. Alpha and A9N are unchanged.

The control path is CLI -> Negoto -> Alter -> Alpha. Guest syscalls continue
on the existing fault -> Alter -> native-service path, without a Negoto hop.
Unsupported environment requests fail before process creation. See
[Negoto OCI Bundles](application.md#negoto-oci-bundles) for the restricted
profile, limits and QEMU commands.

Execution-context control is scoped to the native owner PID. Children inherit
root and readonly policy; exec and PT_INTERP use that root. Normalization
precedes host-prefix insertion. Cross-context signals and host keyboard/mouse/
raw-socket access are denied. These checks do not implement Linux namespaces
or make the partial Linux ABI a security sandbox.

Each Alter control client has its own shared-memory view; fork/exec use private
scratch. Prepare snapshots client bytes before parsing offsets. Negoto also
snapshots management JSON and explicitly detaches client views. Context
records survive ordinary Alter zombie cleanup until Negoto deletes them.
Deletion closes resources before native reaping and retains failed records
for retry. CLI exit does not destroy a container.

Alter owns session/process-group identity and controlling-terminal association;
these are not added to Alpha or A9N. `TIOCGPGRP`/`TIOCSPGRP`/`TIOCGSID` read and
update actual foreground-group/session state. Fork inherits group membership,
exec preserves it, and foreign-session/group changes are rejected. Wait targets
and group-directed signals use the same PGIDs. Linux signal state is a lazy
per-process allocation, independent of terminal-group metadata. Terminal input
notifications are installed at launch rather than only on read, so commands
that read sockets instead of stdin still receive Ctrl-C. Alter interprets
ISIG/VINTR/VQUIT/VSUSP and targets only the controlling session's foreground
group. The terminal service filters caller-selected bytes from its input ring
and reports a control-event bitmask; Linux signal numbers and group policy stay
in Alter. READ_INPUT's optional fifth payload word selects this filtering; a
zero word preserves the existing API, and a zero-length filtered read leaves
ordinary input queued. Each unchanged input batch is scanned only once for a
given filter, avoiding repeated scans during canonical byte reads. Alter queues
signals and delivers them at syscall boundaries, while a target is parked in
an Alter fault, or after synchronous PCB Suspend has stopped a running target.
Remote register reads occur only after the stop ACK. A cancelled, unaccepted
syscall replays from the Kernel-prepared context; a syscall already parked in
Alter follows EINTR/SA_RESTART. The event loop excludes its current fault until
the pending reply is sent, and never starts a prepared OCI process while
delivering an ordinary signal. Default stop/continue actions preserve wait state
or a suspended user context, and wait4/SIGCHLD distinguish stops,
continuations and signal termination. Real-time interval alarms share the
existing one-shot deadline scheduler. Handler frames and sigreturn use the
Linux architecture ABI, excluding FPU/SIMD state, with privileged register
fields kept under server control. Normal syscall replies retain their existing
short register payload; only signal delivery/restoration requires a full
context. This uses A9N 0.3.11's suspend-continuation contract without additional
Kernel or IPC fastpath changes.

Empty blocking pipe reads park on existing readiness waiters while a writer
remains; EOF requires the last writer to close. Full writes use the same
wake/retry path. Exit closes FDs before wait/reap, so zombie pipe writers cannot
deadlock shell command substitution. Pipe data uses lazily allocated 4-KiB
buffers, released with the last endpoint, instead of enlarging Alter's
stack-resident Runtime by 64 KiB.

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

Booter locates the packaged boot list and passes its bytes as an independent
input to Alpha; Alpha reads only that supplied configuration. If rootfs manifests cannot be
read, `system-manager` uses the copies embedded into its binary at build time.

Alpha's trusted boot configuration accepts process defaults independently of
component startup entries:

```text
@process ./modules/example priority=27 affinity=0 platform=1 services=1
arbitrary-component-label 19 ./modules/example
```

`@process` matches an explicit initramfs image path, not a basename or known
Service name. Leading `./` or `/` is normalized; a different directory does not
inherit its policy. The optional fields set a default priority (0..31), initial
core affinity, access to raw platform boot metadata, and Service publication/
binding-management authority. An explicit spawn priority overrides its
configured default; unconfigured images use priority 4
and round-robin initial affinity. Policy is parsed before components run;
duplicate image rules, unknown options, and out-of-topology affinity fail closed.
Malformed startup entries are reported, not silently ignored. Alpha's component
entries do not accept argv; rootfs system/session manifests retain argv support.

`cargo xtask build --boot-list <configuration> --boot-module <file>` selects an
external boot configuration and packages a module as `./modules/<basename>`.
Repeat `--boot-module` for additional files, including child configurations;
duplicate basenames are rejected. These inputs are independent of rootfs
system/session manifests. See the [application build guide](application.md#external-boot-configuration).

`services=1` permits a component to publish managed Services and administer
bindings it owns; it is not necessary merely to register its own Provider.
Neither this grant nor `platform=1` is inferred from a public Service name.

Only `platform=1` components may request the RSDP address or use the opt-in saved
register diagnostic. The grant is installed before Resume, independent of the
component label. Caller-supplied ELF buffers cannot select these grants with an
image label. The checked-in x86 configuration places firmware access on core 0
and keeps ACPI/USB above storage probing, without an Alpha image-name branch.
This is initial placement policy, not CPU-time or immutable-affinity isolation:
processes still own configurable PCB capabilities. Device and firmware protocol
decisions remain in the separate Device Manager/driver implementations.
`platform=1` gates metadata/diagnostics only. I/O-port, MMIO and IRQ requests
still use the containing Alpha's actual device authority; this flag is not a
per-driver device-range quota. The device-free child receives none of that
authority, but finer restrictions inside a device-owning Alpha remain separate
policy/accounting work.

## Capability and Memory Management

### Physical Memory

Alpha receives Generic capabilities through its launch contract and maintains physical-memory
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

Native heaps request anonymous regions from Alpha. `nanami_runtime` manages multiple
heap regions and can grow the allocator without assuming a single fixed heap.

### Capability Lifecycle Contract

The current A9N deletion contract separates slot administration from object-use
rights. An RW capability Node can REMOVE a contained WO, RO or NONE-rights
alias. Removing an alias leaves other references to the same object valid.
REVOKE still requires RW on the target: removing a weak alias does not authorize
resetting its shared object. COPY/MINT create peer aliases, not another level of
object derivation. Object identity excludes rights and IPC identifiers;
changing an IPC identifier still requires MODIFY.

The Node layer walks dependency children and manages their slots; each
capability object implements its own `revoke` responsibility. Node destruction
also drains contained slots, including references to objects allocated outside
that Node's Generic. A failed recursive cleanup can have made partial progress;
the remaining ownership must be retained and backing must not be reused.

PCB revocation includes synchronous stop acknowledgement before teardown, even
when the process was running on another core. This is not a device/DMA barrier,
and ordinary process Suspend alone is not a substitute for the complete
reclamation protocol. Nanami must additionally withdraw authority, quiesce device
users and release its own references/accounting. The current managed-Service
close/retire paths still quarantine resources rather than complete that work.
See the [capability reclamation record](kernel-change-request.md) for current
source references and the earlier kernel observations.

### Process Exit and Reaping

Exit and resource destruction are separate operations:

1. A process reports its exit status and is suspended.
2. Its authorized reaper observes the status.
3. The reaper requests process reaping.
4. Alpha removes the process root capability.
5. A9N revokes dependent capabilities.
6. Alpha releases process metadata and physical-memory references.

This ordering is required because Generic-backed kernel memory cannot be reused
until the derived capability tree has been revoked. It describes eligible
ordinary processes, not permission to reclaim a managed Provider, a process
with quarantined managed aliases, a child Alpha or Booter merely because it is
stopped. Those lifetimes require their separate cleanup/accounting protocol.

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
| `negoto-service` | Negoto | OCI bundle/lifecycle management over Alter/Linux |
| `input-service` | `input-server` | Input event distribution |
| `display_service` | `fb-server` | Boot framebuffer sharing |
| `honoka-service` | Honoka | Compositor and window service |
| `net-device` | `virtio-net` | Network-device backend |
| `network-service` | `net-server` | IPv4, ICMP, UDP, DNS, and TCP |

Service slots are capabilities. A long-lived client should retain a successful
connection instead of repeatedly connecting into the same occupied slot.

Registration is append-only: another owner or endpoint cannot replace a name;
an exact owner/endpoint retry is idempotent. A registration request is not
Binding-change authority. Lookup also checks that the original local owner is
still active, so reusing a reaped process's root slot cannot silently substitute
an unrelated provider. The core no longer translates names to a closed
Service-kind list. `OS_REQUEST_SERVICE_LIST` takes `(ordinal, chunk)`: chunk 0
returns the local owner PID and UTF-8 byte length, and chunks 1/2 return two
little-endian words of the name (zero-padded, maximum 24 bytes). Runtime returns
`ServiceInfo`; Shell displays its actual name, including unfamiliar services.
Entries remain append-only during these diagnostic reads.

OS and legacy connected-Service aliases receive READ|WRITE, without COPY or MODIFY,
after Alpha assigns the caller identity. Clients cannot call IDENTIFY to spoof
another caller; child guest fixtures test rejection on the real kernel. READ is
retained by the legacy connection API, not required by current A9N REMOVE.
This does **not** enforce send-only connections or claim that a client cannot
receive on that endpoint. Do not treat these legacy connections as managed
Session handles or use them for cooperative Provider handover.

This is still a direct legacy connection to a Provider endpoint, whose returned
local PID is used by shared-memory and notification setup. Rejecting duplicate
registration prevents name seizure but does not establish independent Service,
Provider, Session or Binding lifetimes, nor redirect an existing connection.
Their migration and live Provider retirement remain separate acceptance gates.

### Managed Services and cooperative receiver exchange

The additive `nanami-runtime::service` API separates four local identities:

| Identity | Ownership / lifetime |
| --- | --- |
| ServiceId | Public name, interface and endpoint owned by Alpha, independently of a Provider arena |
| ProviderId | Registered process, interface token, private receiver slot and management notification |
| SessionId | Client use of a Service, with a WRITE-only endpoint alias and a fixed opaque IPC identifier |
| Binding | Service-to-Provider assignment, generation, phase and optional prepared successor |

The initial explicit `STATELESS_CONTROL_PROFILE` supports stateless request/reply
interfaces without shared application queues, device state or event subscriptions.
It does not automatically migrate existing block/VFS/network/graphics servers.
The separate `MANAGED_BUFFER_PROFILE` additionally supports the opaque persistent
buffers described below. `MANAGED_EVENT_PROFILE` also retains the pending-work
Notification and opaque data queues described below. Device state and private
heap serialization are not part of these profiles. Legacy PID-based
shared-memory/notification APIs remain domain-local APIs, not managed Session
operations or a sandbox between every process in the same Alpha. A SessionId is
not accepted as a PID and no Provider PID is returned by managed connect.

Any local process may register its own Provider; only a component with trusted
`@process <image> services=1` policy may publish or manage bindings. A Service's
creating manager must authorize subsequent changes. Names are ordinary external
registry keys: Provider registration cannot claim a public name or steal an
existing binding. Legacy connect rejects managed names rather than accidentally
granting their clients the legacy RW rights. Service enumeration uses diagnostic
owner 0 for these logical identities, not the current Provider PID.

The normal path remains `Client -- direct CALL --> stable endpoint --> Provider`.
The managed sequential `Receiver` performs one nonblocking kernel Notification
poll per 32 receives, in addition to receiving bound notifications normally.
Queued CALLs can delay delivery of a pending bound Notification; this bounded
poll prevents a busy client stream from indefinitely starving quiescence. It
adds no ordinary Alpha RPC. Management is processed only
after the outstanding reply has completed; an uncertain reply still blocks ACK.
`Receiver` caches its active binding and does not poll Alpha on every receive.
Only control changes and resource grants traverse Alpha. Providers receive RW
aliases without COPY/MODIFY; managed clients receive WRITE only, with a SessionId
badge. The backing endpoint remains in Alpha's pool when the old Provider exits.

The cooperative transaction is:

1. **Prepare** checks manager authority, generation, successor availability and
   interface compatibility without changing the active receiver.
2. **Quiesce** signals the old Provider through a notification created by Alpha.
   The sequential runtime completes each accepted CALL before acknowledging.
   A failed/uncertain reply remains pending; work is not replayed automatically.
3. **Withdraw** removes the old RW receive alias while that Provider is blocked
   in its acknowledgement OS CALL, not waiting on the public endpoint.
4. **Commit** installs the successor's receive alias, then atomically updates
   the binding/generation in Alpha's serialized control loop. Pending normal
   CALLs stay on the same kernel endpoint; the old Provider has no receive alias.
5. **Retire** wakes the old Provider, which unregisters its management state and
   exits. Alpha marks it stopped before notifying the manager. Its Cspace is
   currently quarantined, not reaped/reused.

Abort before quiescence drops the prepared successor; abort after withdrawal
must restore the old receive alias before returning to Active. Failed ordinary
installation leaves the old generation/quiesced state intact. An uncertain
rollback faults the binding instead of installing a second receiver. A failed
publication wake hint does not roll back an already committed public binding.
Private application/device state and non-cooperative Provider recovery are not
covered by this cooperative contract. Opaque managed-buffer state is covered by
the separately opted-in profile below.

New CALLs during the quiesced interval wait for a receiver on the existing
endpoint; there is no timeout/cancellation guarantee and a manager must commit
or abort. Already accepted replies remain the old receiver's responsibility.
The imported-client fixture exercises queued CALLs through exchange. The event
fixture also holds an accepted, non-idempotent CALL open, observes Quiescing
without ACK, releases its old-Provider reply, then verifies quiescence and a
single application of its side effect. Unknown reply failures remain fail-stop,
not a claim of general cancellation or automatic failure recovery.

Tables are bounded per Alpha to 32 Provider records, 32 managed Service records,
and 64 Session records (plus the shared 64-name registry limit). Failed public
endpoint preparation reuses its reserved endpoint slot rather than consuming
unbounded new endpoints. `close` withdraws a WO handle by moving it to bounded
Alpha quarantine; the Session/slot quota stays charged and is not reused.
Unknown connection rollback failures stop and quarantine the caller's resources.
This conservative Nanami cleanup path has not been replaced by quota/backing
reuse after the A9N deletion fixes. Kernel support for weak-alias removal, Node
drainage and synchronous PCB revocation is not proof of complete OS resource
reclamation. See the [current contract and historical kernel requests](kernel-change-request.md).

### Opaque Managed Session Buffers

`Receiver::register_buffered` opts a Provider into `MANAGED_BUFFER_PROFILE` (2).
Both the interface token and profile must match the successor; a stateless
Provider cannot silently receive a buffered Service. A local client calls
`create_buffer(session, bytes)` and the current Provider calls
`session_buffer(session)` using the opaque IPC Session badge, not a Provider or
client PID. Rust and C++ client wrappers return receiver-local address/size pairs.
Imports currently accept profile 1 only, and reject this profile rather than
pretending PID-based shared-memory operations cross domains.

Each Session can own one zero-initialized buffer, rounded to pages, up to 64 KiB.
The 64-Session limit bounds the data backing to 4 MiB per Alpha, excluding
page-table/capability bookkeeping. Exact create retries return the existing VA;
failed construction keeps allocated backing/VA charged. No Frame capabilities
are installed in either participant: Alpha maps its retained backing directly
into their address spaces as RW, without execute permission. These mappings
are not ordinary per-process allocation references that legacy release can free.
Exec is rejected while managed mappings exist or Provider authority is registered;
Provider Cspaces and live managed clients cannot bypass quarantine through reap.
A Provider cannot be its data client's reaper: that role retains separate
memory/PCB authority after its own mapping is removed, so creation/installation
rejects the combination. Other legacy cross-process APIs are not a sandbox
between mutually hostile processes in one Alpha. Legacy notification-copy now
requires a live Native source and tracked Notification provenance; it cannot
copy Alpha's root slots or dispatch a Notification opcode against an IPC/PCB.
A 64-byte per-process flat-root ledger is updated on creation, transfer,
withdrawal and overwrite. A foreign revoke can empty a tracked slot: its type
provenance is not an exact occupancy oracle. Notification replacement first
tries COPY, and only removes a still-occupied, known Notification; an IPC or
unverified object is never deleted. Registration requires a known IPC object.
COPY authority is checked on a private temporary alias before replacing any
existing destination; managed control/event/management slots cannot be replaced
through legacy APIs. IRQ/I/O requests also reject absent environment resources
before invoking the Kernel: optional descriptor 0 denotes no delegation, and
must not be used to dispatch a device opcode against Alpha's root Node.
The device-free event fixture checks these refusals, including an oversized IRQ
number that must never reach a Kernel region operation. This cooperative
profile withdraws its tracked resources, not all authority a process might have
received through unrelated APIs or explicit capability delegation.

Quiesce permits the current Provider's view queries while accepted work is still
draining; new client allocations are rejected in Quiescing. After accepted
replies complete, withdrawal removes the old Provider mappings and
copy-window aliases while it is blocked in the acknowledgement RPC. Only after
that succeeds is its endpoint receive alias withdrawn and the binding marked
Quiesced. Commit maps every live buffer into the successor **before** installing
its receive alias and publishing the new generation. A failed install removes
all candidate mappings, including a page whose MAP may have installed a PTE
before reporting failure. A known no-effect failure never removes a pre-existing
mapping. Uncertain withdrawal/rollback freezes the binding and retains ownership.
Abort restores the old Provider's exact VAs before restoring its receiver.
Client VAs and the backing contents remain stable throughout successful exchange.

The buffer layout, queue indices and validation/interpretation of retained state
belong to the Providers. Alpha neither serializes that state nor resets queues
as if work had not happened. A Provider may cache its VA while active; old views
must not be used after retirement. New Providers obtain their own mapping view.
An attached buffer can currently be closed only while its whole Service is
Quiesced: the client mapping is removed and the WO control handle withdrawn.
Backing and quota remain charged, not recycled. Per-Session cooperative detach,
notification subscriptions, state in private heaps, DMA and non-cooperative
revocation remain separate work; this is not automatic migration of legacy
block/VFS/GUI queues or a claim of complete process isolation.

The optional `--session-buffers` fixture checks zeroing, retained state through
abort and commit, stable handles/client VA, rejection of active-buffer close,
copy-window withdrawal and quiesced closure. The old Provider deliberately reads
its retired VA; an independent guest resolver receives the actual Kernel Fault,
asserts the sender/address, withholds the reply, and stops the blocked old task.
It also checks that the uncleaned Cspace cannot be reaped. This is a real stale-
access check, not just a generation comparison or inspection of Alpha metadata.

```sh
python3 tests/alpha/build-domain.py --arch x86_64 --session-buffers \
  --rootfs out/buffer-rootfs.img --image out/buffer.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/buffer.img \
  --smp 4 --expect-buffers
```

The analogous AArch64 commands require its `--rootfs` argument when running.
`service-buffers` executes the production transaction with injected page effects;
it is not a substitute for the guest's real page-fault assertion.

### Managed pending-work Events and retained queue state

`Receiver::register_buffered_events(interface, receiver, management, event)`
opts into profile 3. The three Native root slots must be distinct and in
23..223. Its registration wire packs `receiver | (event << 8)`; profiles 1/2
retain their original wire representation. This profile includes profile 2's
local opaque buffers and one Alpha-owned Notification object per Service.
It does not import legacy application subscriptions or DMA queues.

`connect_event(SessionId, slot)` / C++ `connect_session_event` grants one stable
WRITE-only client alias. The active Provider holds a removable RW event alias
without COPY/MODIFY. Ordinary NOTIFY delivery is direct and adds no Alpha RPC.
WORK, binding-management and resource-rescan badges are slot-local aliases of
the same object. Notification counts are not queue state: compatible Providers
interpret the retained cursors/layout themselves, outside Alpha/UAPI.

Before acknowledging quiescence, the runtime binds its private management
Notification. Alpha also rebinds the blocked Provider PCB to that private
object, withdrawing the PCB-held event alias as well as the visible root slot.
Event withdrawal precedes old data/copy-window withdrawal and receiver removal.
Commit or abort installs the event and all data views, seeds a resource-rescan
hint, then installs control receive authority. A new Provider may enumerate
`buffer_session(ProviderId, ordinal)` on that rescan; ordinary work uses its
cached views. Rescan rediscovers pending queue entries even if a work badge
coalesced with management before withdrawal. It never resets cursors or replays
completed queue operations.

Client data VA, control handle and event handle remain unchanged through abort
and commit. Close withdraws the event alias into bounded quarantine; backing
and quota stay charged. Uncertain binding/mapping withdrawal freezes the
Service, not a second receiver. This is a cooperative, single-consumer work-hint
contract, not a hostile-process sandbox, two-way event broker or device handoff.

The `--service-events` / `--expect-events` fixture verifies direct queue progress,
pending work during abort/commit, retained cursors/side effects, accepted CALL
reply ownership, old event withdrawal, closed handles and negative capability
transfer cases. Run both 1- and 4-core variants on each architecture:

```sh
python3 tests/alpha/build-domain.py --arch x86_64 --service-events \
  --rootfs out/event-rootfs.img --image out/event.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/event.img \
  --smp 4 --expect-events
```

### Existing stateful Services

Legacy block, VFS, network, input and GUI Services remain fixed-provider for a
Domain's lifetime. Their public registration does not offer the managed
replacement operation. Their reset/reinitialization boundary is a Domain/system
restart, not a registry overwrite or transparent reuse of old PID-based queues.
In particular, open-file/cache state, sockets, compositor/widget state, input
subscriptions and outstanding device descriptors are not silently transferred.
An opted-in Service must define compatible persistent state or its own stable
frontend before using the managed profiles; Alpha does not infer compatibility
from a name. Accepted work in a failed/non-cooperative legacy Provider has no
automatic reply/cancellation/replay guarantee.

## IPC and Data Paths

### Control Path

Service registration, connection, metadata queries, capability transfer, and
operation dispatch use synchronous A9N IPC. Servers normally use a
receive/reply-receive loop so the IPC fast path remains available.

The ownership/launch tree above is not the steady-state IPC route:

```text
setup:   Client -- connect(name) --> Alpha
steady:  Client -- direct CALL --> Provider
data:    Client <--- shared memory / queues ---> Provider
event:   Producer -- Notification --> Consumer
```

Booter serves no post-handoff calls. Alpha is not a common data-path proxy.

### Accepted CALLs and Teardown

A CALL queued on a Port and a CALL already accepted by a Provider have different
lifetimes. Port revoke cancels its queued IPC; it does not search all processes
or cancel an accepted caller's independent reply relationship. That caller may
remain blocked waiting for its reply. PCB teardown handles that process's reply
links separately. Managed quiescence therefore waits for the old Provider to
finish its accepted replies before withdrawing receive authority; it does not
use Port revoke as a general cancellation primitive.

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

- `cargo xtask build` builds native components and invokes SPENCER's Rust
  library with Nanami as the external OS payload;
- `cargo xtask rootfs` uses the reusable Rust `ext2-image` crate;
- the boot initramfs is generated as newc in Rust, without host cpio;
- `xtask` builds the common `alpha` ELF first with `targets/<arch>/alpha.ld`,
  stages the ignored `nanami/alpha.elf`, then builds init Booter through Spencer;
- `cargo xtask run` selects firmware, storage, networking, acceleration and
  launches QEMU directly with a structured argument vector.

ABI definitions are centralized in the dependency-free `nanami-uapi` crate.
`nanami-runtime` provides execution/IPC/allocation; `nanami-client` contains
typed service operations. See [user-space libraries](../servers/README.md).

See the repository [README](../../README.md) for supported commands and runtime
options.

## Architecture Migration Gates

The boot/resource-input implementation is an incremental change, not completion of the
requested architecture. Retain all of the following acceptance gates:

| Gate | Current state / required evidence |
| --- | --- |
| Init ABI boundary | `nanami_core` consumes owned resource input; `alpha-environment`, `physical-memory`, `core-process` and `system-version` tests cover the changed paths |
| Two-phase boot | Implemented: distinct init Booter and First Alpha; x86_64 desktop and AArch64 headless QEMU checks cover handoff/suspension before normal services on 1 and 4 cores |
| Shared First/Child initialization | Both run the same ELF, boot contract, launcher and manager initialization; device-free child fixtures exercise process creation and IPC |
| Booter resource lifetime | Independent copied inputs and stopped-launcher lifetime tested; old Booter backing retained/reserved, not reclaimed |
| Recursive resource delegation | Real three-level Alpha fixture delegates 1 GiB to a child and 512 MiB onward, excludes the grant from intermediate free RAM, charges its reimport Session and preserves upstream handles through Provider exchange. Complete stop/reclaim lifecycle remains open |
| Child process/service creation | Device-free child creates a local process/provider and receives its IPC echo within a 512-MiB partition |
| Namespace and scoped identity | Local same-name providers and rejection of an existing unimported parent name tested; explicit managed imports distinguish domain-owned upstream Sessions from local client close handles |
| External configuration | Arbitrary labels, image policy, startup dependencies, public names, authorized Provider selection and named import aliases/connection limits are inputs |
| Driver boundary | Unused in-process display test driver removed; hardware selection and implementations remain separate servers, not Alpha policy branches |
| Delegation limits and reclamation | Child oversized allocation rejected without parent fallback; reserved/quarantined extents excluded from parent use. Safe live-domain stop/revoke/reclaim on A9N remains pending |
| Registration/binding authority | Separate managed Provider registration, authorized Service publication and owner/generation-checked binding operations implemented; legacy duplicate-name takeover remains rejected |
| Existing connections | Stateless control fixture connects once, receives S1 replies, rejects incompatible prepare, quiesces/commits, observes S1 stopped, and receives S2 replies through the same WO handle; S1/closed-alias backing remains quarantined |
| Handover lifecycle | Cooperative sequential runtime preserves reply responsibility; imported-client fixture keeps CALLs queued during exchange and detects the successor through the original handle. Model/runtime tests cover authorization, failures, bounded management polling, generation and quotas; exhaustive fault scenarios and safe reclamation remain open |
| Data/Event resources | Profiles 2/3 retain opaque backing/client VA; profile 3 also retains direct work handles and queue cursors. Old PCB event bindings and data/copy aliases are withdrawn before exchange. Guest tests cover pending work, accepted CALLs and actual stale-data access faults. Private heap/device state and legacy subscriptions are not implicitly migrated |
| Isolation guarantees | RAM/Generic authority and managed Session/buffer limits are enforced. Native self-PCB can raise priority; a delegated stopped-child PCB can select any online core. Running-target migration has a separate A9N guard. Manifest placement is not a CPU sandbox; no physical Device/DMA isolation is claimed |
| Documentation and regression | Update existing docs with each implemented step and run normal IPC/service/client boot regressions; source review alone is not runtime proof |
| Kernel integration | A9N `d9bd10df9703edae4e110b37fc7e632a35140cae` (v0.3.9-smp), through Spencer `628310ba6d31a4df7a8e7d9fd83b4d21a690f041`; `a9n_abi` remains 0.8.0. Generic deletion fixes do not imply Nanami reclamation/accounting integration |

`alpha-launch` tests execute the production builder with real descriptor-tree
resolution, non-copyable Generic capabilities, checked conversion cursors,
page-table mappings and byte-accurate image/header/module/configuration copies.
They check receiver-local references, independent spaces, no allocation from
unprovided resources, no parent retirement for ordinary preparation, malformed
ELF/slot rejection, and injected failures at every capability operation. These
mocks do not prove kernel revocation or DMA isolation semantics.

The initial audit used Nanami branch `main`, starting from `214e097`, and A9N
v0.3.8. The fixtures exercise actual COPY/MINT attenuation, receiver removal,
queued CALLs and sequential reply ownership for the supported cooperative
profiles. They do not establish general cancellation, selective alias revocation
or remote-core Suspend completion. The original capability probes exposed
attenuated-delete and Node-member reclamation defects; those observations are
historical, not descriptions of A9N v0.3.9-smp. The current probe asserts the
corrected generic contracts, independently of Nanami's still-conservative
managed-resource lifecycle. See [kernel-change-request.md](kernel-change-request.md).

Optional child-domain integration images use test-only guests, not production
service-name branches or an additional resident resource server:

```sh
python3 tests/alpha/build-domain.py --arch x86_64 \
  --rootfs out/domain-rootfs-x86_64.img --image out/domain-x86_64.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/domain-x86_64.img --smp 1 --expect-domain
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/domain-x86_64.img --smp 4 --expect-domain
python3 tests/alpha/build-domain.py --arch aarch64 \
  --rootfs out/domain-rootfs-aarch64.img --image out/domain-aarch64.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/domain-aarch64.img \
  --rootfs out/domain-rootfs-aarch64.img --smp 1 --expect-domain
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/domain-aarch64.img \
  --rootfs out/domain-rootfs-aarch64.img --smp 4 --expect-domain
```

The fixtures wait for registration through external `after=` policy, then test
namespace absence and the child-local echo via actual IPC. A pre-existing parent
name is therefore not confused with a name that simply has not registered yet.
The child tries a 1-GiB DMA-memory request against its 512-MiB partition, then
successfully allocates/touches a small page. This is a RAM-authority test, not
device passthrough or a DMA-isolation claim. Reclamation tests are still host
models, not live-child cleanup proof.

The child fixture additionally runs a managed, stateless receiver exchange. Use
`--expect-handover` on the boot smoke command to require its terminal assertion:
one connect, two S1 replies, rejected incompatible prepare, no commit before
quiescence, S1 withdrawal/stop, two S2 replies through the same handle, rejected
receive/IDENTIFY on the WO client handle, and handle closure. The per-Provider
request counts check that these calls were not replayed. This does not claim old
Provider memory or the closed alias was reclaimed. `service-bindings` and
`service-receiver` test the production control state and sequential runtime.

The explicit-import configuration uses a device-free child and a parent-managed
Provider. It tests a local alias, rejection of two existing but unimported parent
names, distinct local/upstream Session IDs, WRITE-only client handles, a charged
two-connection limit including closed aliases, and source-side Provider exchange
without reconnecting. A continuously queued CALL stream exposed the pinned
kernel's bound-notification starvation; the runtime's bounded poll is exercised
by this guest and an independent 32-request host regression. The following runs
pass on x86_64 and AArch64, each with 1 and 4 cores:

```sh
python3 tests/alpha/build-domain.py --arch x86_64 --service-imports \
  --rootfs out/import-rootfs-x86_64.img --image out/import-x86_64.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/import-x86_64.img \
  --smp 1 --expect-imports
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/import-x86_64.img \
  --smp 4 --expect-imports
python3 tests/alpha/build-domain.py --arch aarch64 --service-imports \
  --rootfs out/import-rootfs-aarch64.img --image out/import-aarch64.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/import-aarch64.img \
  --rootfs out/import-rootfs-aarch64.img --smp 1 --expect-imports
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/import-aarch64.img \
  --rootfs out/import-rootfs-aarch64.img --smp 4 --expect-imports
```

This is one charged upstream Session per import, not per downstream handle,
and not shared-memory/event forwarding or live-domain reclamation.

The `--recursive-imports` configuration adds an intermediate device-free Alpha:
the root delegates 1 GiB to it, and it delegates 512 MiB to a grandchild. External
configuration renames the root publication to `middle-door-5` in the child and
`child-door-9` in the grandchild. The intermediate Alpha's single upstream
connection allowance is consumed by the domain-owned re-export; a native local
process cannot allocate an additional handle or see root/grandchild names. Its
reported free RAM excludes the grandchild partition. The existing leaf client
then checks its two-connection quota, upstream Session badge, WO rights, closure
and root Provider exchange through its original handle. The repeated local
PID 1 at all three levels is not used as cross-domain authority.

```text
resource grants: Root Alpha --1 GiB--> Child Alpha --512 MiB--> Grandchild Alpha
ordinary CALL:   Grandchild client ----root stable endpoint----> Root Provider
```

Reproduce with the optional configuration and corresponding assertions:

```sh
python3 tests/alpha/build-domain.py --arch x86_64 --recursive-imports \
  --rootfs out/recursive-rootfs-x86_64.img --image out/recursive-x86_64.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/recursive-x86_64.img \
  --smp 1 --expect-recursive-imports
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/recursive-x86_64.img \
  --smp 4 --expect-recursive-imports
python3 tests/alpha/build-domain.py --arch aarch64 --recursive-imports \
  --rootfs out/recursive-rootfs-aarch64.img --image out/recursive-aarch64.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/recursive-aarch64.img \
  --rootfs out/recursive-rootfs-aarch64.img --smp 1 --expect-recursive-imports
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/recursive-aarch64.img \
  --rootfs out/recursive-rootfs-aarch64.img --smp 4 --expect-recursive-imports
```

This tests an actual three-level boot and direct reimported IPC, not arbitrary
depth, device/shared-memory imports, child administration or safe reclamation.
The recursive fixture pins its assertion-printing test processes to core 0
through external `@process` rules to reduce diagnostic interleaving. Ordinary
services retain normal placement. The character-at-a-time debug ABI can still
interleave lines even on one CPU; repeated short terminal tokens are emitted
only after all guest assertions pass. The runner requires complete tokens,
not partial or loosely matched diagnostic text.
The four-core run is not a stress test of concurrent test-participant execution;
the separate managed-buffer fixture retains normal multi-core placement.

Add `--kernel-capabilities` to `build-domain.py` and
`--expect-kernel-capabilities` to `boot-smoke.py` for the isolated capability
probe. The current runner requires both `CAPABILITY-PROBE-PASS` and
`CAPABILITY-PROBE-DONE`. Assertions cover peer aliases, weak-alias deletion,
Node drainage, repeated Generic reuse and synchronous process revocation. The
probe retains its outer partition; passing it is not evidence of complete live
child-domain or managed-Session reclamation.

`boot-policy` exercises arbitrary image rules, path separation, checked priority
and affinity, and shipped dependency settings. `services` executes the actual
name directory and wire chunks, including unknown/maximal/UTF-8 names, duplicate
registration, table exhaustion and independent namespaces. Child guests also
enumerate their real registered name, reject identity rewriting and platform
access without a grant, and attempt a denied duplicate-name takeover before
checking the original leaf's echo. The C++ runtime's name packing and request
layout are tested with only kernel IPC substituted:

```sh
clang++ -std=c++20 -Wall -Wextra -Werror -fsanitize=address,undefined \
  -I nanami/servers/uapi/include tests/alpha/registry-cpp.cpp \
  nanami/servers/libs/nanami-runtime/cpp/src/nanami/nanami.cpp \
  -o out/alpha-registry-cpp-test
out/alpha-registry-cpp-test
```

Resource-input regression commands (the image uses separate guest data):

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml -- --test-threads=1
cargo xtask build --rootfs out/architecture-rootfs.img --output-image out/architecture.img
python3 tests/desktop-tools/qemu-smoke.py --image out/architecture.img --smp 4 --expect-two-phase
python3 tests/desktop-tools/qemu-smoke.py --image out/architecture.img --smp 1 --expect-two-phase
python3 tests/alpha/boot-smoke.py --arch x86_64 --storage virtio \
  --image out/architecture.img --smp 4
```

For the headless AArch64 configuration, build a **new** rootfs path or explicitly
regenerate a disposable test rootfs; `build` intentionally preserves existing
rootfs contents and does not replace already installed applications:

```sh
cargo xtask build --arch aarch64 --no-linux-bins \
  --rootfs out/architecture-rootfs-aarch64.img --output-image out/architecture-aarch64.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --smp 1 \
  --image out/architecture-aarch64.img --rootfs out/architecture-rootfs-aarch64.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --smp 4 \
  --image out/architecture-aarch64.img --rootfs out/architecture-rootfs-aarch64.img
```

The boot smoke runner accepts only regular image files, disables networking,
uses QEMU snapshot disks, checks the handoff order, mounted VFS and configured
providers, and always terminates its QEMU process. Failure retains serial and
monitor diagnostics. A previously built U-Boot can be supplied through
`UBOOT_QEMU_BIN` instead of rebuilding it in Docker.

AArch64 still has no supported timer driver in the checked-in configuration.
Alter/Linux now registers without spinning for that missing dependency. Clock
operations attempt the connection on demand, report the real lookup failure if
absent, and can use a provider started later; they do not invent elapsed time.
This is not a claim of working AArch64 sleep/clock support without a provider.
The virtio block DMA layout is derived from the complete 64-KiB transfer buffer,
request header and status byte (rounded to pages), not a fixed 36-KiB allocation.
Host boundary tests and AArch64 rootfs reads exercise the corrected layout.
Virtio completion also acknowledges the device's level interrupt before its
kernel IRQ port, including when polling sees completion before an IRQ wait.
Failure to clear the device leaves the IRQ masked and switches the driver to
polling; it does not unmask a still-asserted device source. These cleanup paths
have ordering/failure tests.

The x86_64 virtio 1-core boot stop was a driver polling-fallback bug, not a
missing Kernel primitive. USB already owned the shared INTx line; virtio could
not obtain it but left device INTx enabled and skipped device ISR cleanup when
no IRQ port was registered. Its asserted source drove the higher-priority USB
loop (about 157,000 empty IRQ events per five-second sample), starving the
storage candidate before publication. PCI INTx is now masked before IRQ setup
and enabled only after a usable port is owned; polling completions clear device
ISR state without acknowledging an unowned kernel IRQ port. Both 1- and 4-core
virtio boots now reach configured services. USB (4 cores) and AHCI desktop
workflows (1/4 cores) also pass.

The timer SDK additionally uses the actual bound root slot for both timer
notification grants and waits. Waiting on default slot 21 after binding alias
18 could leave a badge deferred by an interrupted RPC in the runtime cache,
even when both aliases refer to the same object. The runtime also records
Alpha's initial slot-21 binding before an explicit rebind. Host tests deliver
a timer badge during the scheduling RPC and verify it completes the wait once,
preserving unrelated IRQ flags. No Kernel changes or periodic polling workaround
were introduced for either fix.

The desktop workflow alone is a normal-service regression. The optional
`--expect-domain` fixture adds Child Alpha coverage; neither workflow is a
Provider exchange or live-domain reclamation test.
