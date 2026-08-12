# Nanami Refactoring Guidelines

Refactoring must preserve observable behavior and capability ownership. In
Nanami, a source-level cleanup can affect process lifecycle, IPC ordering,
shared-memory validity, or device progress even when function signatures remain
unchanged.

## Preserve System Boundaries

- A9N provides kernel mechanisms.
- Alpha provides generic OS-core mechanisms: process management, memory
  management, capability distribution, and service routing.
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

## Keep `libnanami` Narrow

`libnanami` is the low-level client boundary for Nanami core operations. It
should contain startup, IPC primitives, architecture support, heap support, and
wrappers around Alpha's generic requests.

Service-specific APIs belong in `nanami-services` or a dedicated reusable
crate. For example, Honoka window calls, VFS records, terminal protocols, and
network operations do not belong in Alpha-facing low-level code.

## Service Registration

Treat the service registry as a name-to-capability routing mechanism. Alpha
stores names and distributes port capabilities; it does not interpret service
protocols or results.

A refactor must not replace service discovery with fixed process IDs, global
provider slots, or application-specific Alpha requests.

## State and Types

- Give state fields names that explain ownership and units.
- Replace ambiguous integers with enums, bitflags, or newtypes when that blocks
  invalid combinations.
- Keep native PIDs, foreign PIDs, capability descriptors, virtual addresses,
  physical addresses, file descriptors, handles, and byte counts conceptually
  distinct.
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

## Validation

Refactoring is complete only after the relevant end-to-end workflow passes.

- Build affected release targets.
- Run `git diff --check`.
- Boot Nanami when startup, process, memory, driver, or manifest behavior
  changed.
- Exercise normal operation and teardown.
- Verify that process count and memory use return to the expected level after
  closing or reaping components.
- Capture and inspect the framebuffer after compositor changes.
- Test QEMU user and bridged networking after changes below `net-server`.
- Run foreign binaries after changing POSIX, Alter, process memory, or fault
  handling.
