# Shared mapping reservations

`shared_memory.rs` owns reusable VA ranges and frame-capability slots for
`OS_REQUEST_SHARED_MEMORY_CREATE`. The physical pages remain tracked by
ProcessManager's existing allocation references. Each peer has its own
reservation; releasing one peer must not release the other's physical backing.

- The Alpha transaction in `alpha/shared_memory.rs` reserves both peers,
  allocates backing pages, copies capabilities, zeros the backing, and maps it.
- Release unmaps a peer, removes only its capability copies, drops its physical
  reference, and finally recycles its reservation. Alpha's canonical physical
  frame capabilities remain cached for reuse. The last peer frees the RAM.
- Allocation/copy/map errors roll back installed mappings and capabilities.
  A failed cleanup retains ownership rather than reusing an uncertain mapping.
- Free-range indexes use address order for coalescing and size order for
  best-fit lookup. They grow with live reservations and holes, not cumulative
  allocations or an array sized for all possible frame slots.
- A fixed-address anonymous allocation excludes its VA range from shared holes.
  Its capability slots are separate. Heap guard gaps keep their existing
  allocation semantics; they are not drawn from the shared-mapping hole pool.
- Exec/reap discard the process's reservations and holes along with its other
  mapping state. Shared backing still referenced by another process survives.

This specifically fixes shared-memory lifetime/reuse. Anonymous and DMA
allocations retain their existing release policies. Frame nodes are retained
for reuse; this is not an increase or removal of per-process metadata limits.

The host `core-process` tests compile the production reservation and Alpha
transaction implementations with checked syscall mocks. QEMU fbdev tests in
`tests/alter-linux/framebuffer.md` cover actual map/unmap and process lifetimes.
