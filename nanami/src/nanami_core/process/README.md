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

## Uncertain PTE state

A mapping operation can fail after changing a leaf but before completing its
remote TLB shootdown. VmSpace tracks these exceptional addresses separately
from permission bits and backing ownership. Its permission query returns zero
until cleanup is confirmed; zero is not, by itself, evidence of an absent PTE.
The empty uncertainty set allocates no nodes on successful operations.

Initial private-page construction rejects an already uncertain destination and
marks possible PTE side effects before fallible Frame bookkeeping. Successful
legacy construction records its actual ALL access, replacing stale PROT_NONE
reservation metadata; the subsequent protection request must not mistake that
new accessible leaf for an already absent one. These rules also apply when
construction fails before its ordinary protection step.

Protection retries first unmap an uncertain leaf, including a previously empty
leaf, and require a successful capability response before clearing uncertainty.
Private and shared release retain Frames and physical references on any cleanup
error. They cannot defer, free or recycle backing merely because permissions
read as zero. Acknowledged cleanup permits ordinary release/reuse again.

Fork/copy and private detach reject uncertain source backing; sealed exec also
unmaps uncertain leaves before clearing capabilities or releasing RAM. The
legacy native exec reset rejects a VM that already has unresolved mappings.
The dedicated production-handler tests inject errors before/after PTE changes,
model idempotent empty-leaf UNMAP plus fallible acknowledgement, and exercise
rollback, VM peers, shared release and subsequent confirmed cleanup. They are
not hardware TLB timing tests or a performance claim. Kernel ABI is unchanged.
