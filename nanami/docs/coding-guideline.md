# Nanami Rust Coding Guidelines

These guidelines apply to Rust code under `nanami`, including Alpha, core
services, applications, SDK crates, and reusable libraries.

## Goals

- Preserve readability, maintainability, correctness, and performance.
- Respect the responsibility boundaries of a capability-based microkernel.
- Keep control, data, and event paths explicit.
- Make ownership and lifetime visible at capability, mapping, and protocol
  boundaries.

## Source Layout

- Prefer `<module>.rs` with a matching `<module>/` directory over new
  `mod.rs` files.
- Split files when they contain distinct responsibilities. File length is a
  signal, not a rule; consider a split once a file becomes difficult to scan or
  exceeds roughly 300 to 500 lines.
- Keep the entry function and the primary event loop visible in `main.rs`.
  Do not create an almost empty `main.rs` solely to move all behavior into an
  `app.rs` wrapper.
- Put architecture-specific implementation behind an explicit module such as
  `arch/x86_64.rs`.
- Move genuinely reusable protocol logic, checksums, parsers, and wire formats
  into a shared crate. Do not create a shared abstraction for code used once.

## Crate Boundaries

- `libnanami` contains low-level Nanami core calls, process startup, IPC
  primitives, heap support, and architecture entry code.
- `nanami-services` contains typed service names, request constants, message
  layouts, and client wrappers.
- A service implementation owns its policy and server state.
- Alpha must not depend on application, filesystem, network, desktop, POSIX, or
  foreign-ABI semantics.
- Cross-process functionality is exposed as a named service, not through a
  hard-coded provider capability.

## Application Entry

- Native applications use `libnanami::nanami_entry!`.
- The entry function returns `libnanami::NanamiResult`.
- Initialize IPC TLS before issuing Nanami or service requests.
- Initialize the heap before using allocation.
- Return the final result and let `nanami_entry!` report exit status.
- Panic handlers should log concise context, request exit when transport state
  permits it, and then stop without performing unsafe recovery.

## Function Organization

- Arrange functions in reading order. If function A calls function B as its
  main next step, place B below A unless a stronger grouping improves clarity.
- Split a function when a block has an independent invariant, error policy, or
  testable responsibility.
- Keep fast paths short. Move diagnostics, table scans, formatting, and fallback
  policy out of hot loops.
- Use early returns for invalid input and terminal states.

## Naming and Types

- Use meaningful names and avoid unexplained abbreviations. `buffer` is usually
  preferable to `buf`; established protocol terms such as `fd`, `pid`, `irq`,
  and `dma` are acceptable.
- Represent units in names or types: `size_bytes`, `timeout_ms`,
  `physical_address`, and `virtual_address`.
- Use newtypes or enums when raw integers from different domains could be
  confused.
- Do not duplicate numeric protocol constants at call sites.
- Match service and manifest names exactly, including current legacy names such
  as `display_service`.

## API Design

- Define request, response, control, flag, and record-layout constants in one
  protocol module.
- Leave room for explicit protocol versioning when layouts may evolve.
- Return `Result` and preserve `RequestError` where possible.
- Distinguish invalid arguments, unsupported operations, transport failures,
  protocol violations, and remote status errors.
- Document ownership transfer and whether a successful call consumes,
  replaces, or aliases a capability or mapping.
- Validate all shared-memory offsets, lengths, counts, and arithmetic before
  dereferencing.

## IPC, Data, and Events

- Use IPC messages for bounded control metadata.
- Use shared memory or DMA for high-frequency or large data.
- Use notifications to signal available work; drain the associated queue or
  state after wakeup.
- Assume notifications can coalesce.
- Avoid synchronous service dependency cycles.
- Prefer receive/reply-receive server loops.
- Never busy-poll without a documented bound and a reason that an event-driven
  path cannot be used.

## Capability and Memory Safety

- Treat capability slots as owned resources. Do not overwrite an occupied slot
  accidentally.
- Track mapping lifetime separately from virtual addresses returned to clients.
- Revoke/remove process roots before returning Generic-backed memory to the
  allocator.
- Register references for both endpoints of shared memory.
- Keep MMIO and device reservations distinct from reclaimable RAM.
- Use overflow-safe range checks for `base + size` and `offset + length`.
- Release temporary mappings on every success and error path.

## Unsafe Code

- Localize `unsafe` behind the smallest practical interface.
- State the caller-visible invariant when safety is not obvious.
- Use volatile access only for MMIO, device rings, and memory explicitly shared
  with hardware or another process.
- Do not create references to unvalidated guest or shared-memory addresses.
- Prefer unaligned reads and writes only when the protocol layout requires
  them.

## Performance

- Minimize cross-process calls in hot paths.
- Batch queue draining and damage updates.
- Avoid repeated service resolution, capability copies, and shared-memory
  attachment.
- Avoid formatting and per-packet logging in release paths.
- Bound caches and queues, and define eviction or replacement behavior.
- Measure before introducing complex caching or a new abstraction.

## Logging

- Default high-frequency tracing to off.
- Log state transitions, failures, and bounded summaries.
- Keep packet dumps, syscall traces, allocation traces, and OS-request traces
  behind explicit runtime or build-time controls.
- Do not log every request in normal operation.
- Ensure disabled logging does not retain expensive formatting or scans in a
  fast path.

## Change Checklist

- The affected release crates build successfully.
- Service names and protocol constants remain synchronized.
- Shared-memory bounds and capability-slot ownership are checked.
- Exit, close, failure, and process-reap paths were exercised.
- IRQ-driven and fallback paths were tested when both exist.
- No high-frequency debug logging was left enabled.
- User-visible behavior was verified in QEMU, not only with `cargo check`.
