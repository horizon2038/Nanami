# Nanami Refactoring Guidelines

Refactoring must preserve observable behavior and capability ownership. In
Nanami, a source-level cleanup can affect process lifecycle, IPC ordering,
shared-memory validity, or device progress even when function signatures remain
unchanged.

## Preserve System Boundaries

- A9N provides kernel mechanisms.
- Alpha Booter adapts init-only metadata and transfers real authority through
  the shared launch contract; it is not a resident resource server.
- Alpha provides generic OS-core mechanisms: process management, memory
  management, capability distribution, name resolution and binding control.
  Ordinary Service requests go directly to the granted endpoint, not through
  Alpha.
- Named services provide subsystem policy.
- Applications consume services and must not acquire provider-specific
  capabilities through hidden shortcuts.

Do not move filesystem, network, GUI, POSIX, Linux, FreeBSD, or
application-specific behavior into Alpha to simplify a caller.

## Preserve the Event Loop

Microkernel components are event-driven. Keep the primary entry and event loop
easy to find in `main.rs`. Extract request handlers, state machines, parsers,
drivers, and protocol helpers when doing so clarifies the loop. Avoid an empty
entry module that adds no useful boundary.

Booter and the independent Alpha ELF intentionally have different entry points.
Preserve that init-ABI boundary; do not merge them merely to remove an entry
module. First and Child Alpha must still share their resource contract,
launcher and manager initialization.

When changing a loop, preserve:

- reply ordering;
- notification binding and draining;
- progress when no request is pending;
- timer and retry bounds;
- shutdown and error transitions;
- fairness between clients.

## Organize for Reading

- Put the public or high-level operation before its implementation helpers.
- Keep related state and operations close together.
- Split by responsibility, not by an arbitrary file-size target.
- Remove dead compatibility paths after confirming they are not part of a
  manifest, build script, protocol, or external ABI.
- Prefer one canonical implementation over copied Linux/FreeBSD/native variants
  when their semantics are genuinely identical.
- Keep named lifecycle steps and masks even when an inline bit expression or
  chained condition would use fewer lines. Share cleanup logic without hiding
  which object owns teardown or which phase can fail.

## Keep `nanami_runtime` Narrow

`nanami_runtime` is the low-level client boundary for Nanami core operations. It
should contain startup, IPC primitives, architecture support, heap support, and
wrappers around Alpha's generic requests.

The generic managed-Service API belongs here because it wraps Alpha's Provider,
Session and binding operations. Its pure wire definitions belong in
`nanami-uapi`; subsystem policy remains outside both layers.

Service-specific APIs belong in `nanami-client` or a dedicated reusable
crate. For example, Honoka window calls, VFS records, terminal protocols, and
network operations do not belong in Alpha-facing low-level code.

## Service Registration

Treat the service registry as a name-to-capability routing mechanism. Alpha
stores names and distributes port capabilities; it does not interpret service
protocols or results.

A refactor must not replace service discovery with fixed process IDs, global
provider slots, or application-specific Alpha requests.

Keep publication/binding authority separate from Provider registration and
client connect rights. Preserve generation checks and WRITE-only managed
client handles. Legacy PID-based connections must not silently become managed
Sessions, or vice versa.

## State and Types

- Give state fields names that explain ownership and units.
- Replace ambiguous integers with enums, bitflags, or newtypes when that blocks
  invalid combinations.
- Keep native PIDs, foreign PIDs, capability descriptors, virtual addresses,
  physical addresses, file descriptors, handles, and byte counts conceptually
  distinct.
- Keep domain IDs, Service IDs, Provider IDs and Session IDs distinct from each
  other and from local PIDs, including across explicit imports.
- Make bounded arrays and queues expose their capacity policy.
- Keep protocol layout types separate from internal state types.

## Capability and Mapping Refactors

Before changing capability allocation or mapping code, identify:

- who owns each slot;
- which capabilities derive from Generic memory;
- which object must be revoked before reuse;
- which processes hold shared references;
- which mappings are RAM, DMA, MMIO, framebuffer, or temporary windows;
- who is authorized to kill, inspect, and reap a process.

Do not release allocator metadata before A9N has removed the dependent
capability tree. Do not infer ownership solely from a virtual address.

Keep resource-contract validation, consumed Generic prefixes, launcher
reservations and capability MOVE semantics intact. A delegated child must not
fall back to the parent's allocator. A failed launch or uncertain rollback
retains its reservation until cleanup is proven, rather than returning it to a
free list.

Current A9N can remove weak aliases and drain Nodes; that does not automatically
make Nanami Session quotas, retired Providers, Booter backing or live child
domains reusable. Preserve the distinction between kernel cleanup and OS/driver
lifecycle integration. Accepted CALL replies remain the old Provider's
responsibility during cooperative handover; Port revoke is not cancellation of
every outstanding reply.

## Shared-Memory Refactors

Protocol refactors must preserve negotiated size, offsets, alignment, producer
and consumer ordering, and attachment replacement behavior. Update the protocol
constants and every producer/consumer together.

Prefer a typed record or constants module over ad hoc pointer arithmetic. Add
explicit bounds checks before optimizing copies.

## Performance Refactors

- Establish a workload and baseline first.
- Optimize cross-process call count, copying, allocation, and wakeups before
  micro-optimizing local arithmetic.
- Keep control paths simple and move bulk traffic to shared memory.
- Batch operations only when latency and fairness remain acceptable.
- Do not suppress errors or lifetime checks to improve benchmark results.
- Compare optimized code generation against a named baseline before changing an
  IPC fast path. Record any added calls, branches or scans; source-level
  factoring alone does not demonstrate an unchanged path.

## Validation

Refactoring is complete only after the relevant end-to-end workflow passes.

- Build affected release targets.
- Run `git diff --check`.
- Boot Nanami when startup, process, memory, driver, or manifest behavior
  changed.
- Exercise normal operation and teardown.
- Verify that process count and memory use return to the expected level after
  closing or reaping components. Explicitly account for documented quarantine;
  a stopped process is not evidence that its backing or quota was reclaimed.
- Capture and inspect the framebuffer after compositor changes.
- Test QEMU user and bridged networking after changes below `net-server`.
- Run foreign binaries after changing POSIX, Alter, process memory, or fault
  handling.
