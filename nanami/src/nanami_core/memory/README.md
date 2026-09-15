# Physical capability management

RAM allocation and capability derivation are separate operations. The buddy
allocator owns the allocation state; deriving a generic does not reserve all
of the RAM described by it.

- Before Alpha's heap exists, only the heap's frames are materialized. One
  inline range record describes them; no dynamic index is needed.
- After heap initialization, initial generic remainders enter a B-tree range
  index. Its initial size depends on the boot memory-map entries, not RAM size.
- A frame request materializes an aligned range of at most 8 MiB containing
  that address. Short and unaligned initial regions use smaller power-of-two
  ranges. Frame descriptors are retained for reuse after physical pages are
  freed.
- Untouched RAM and MMIO prefixes stay as large generics. Splitting to a distant
  address takes a bounded number of power-of-two prefixes, rather than visiting
  every intervening 8-MiB chunk. Prefixes remain searchable for later requests
  at lower addresses.
- Range lookups are logarithmic in recorded ranges. A separate generic index
  in the key keeps overlapping firmware device descriptions from replacing RAM
  sources.

## Capability storage

`directory.rs` implements an append-only, four-level radix-256 arena. Branches
are created only as their first slot is used. Only the current allocation spine
is retained in software; existing descriptors do not change during growth.

The four levels are constrained by the capability encoding, not a RAM-size
estimate: a 12-bit root, 32 directory bits and at most 11 frame-leaf bits use
55 of the 56 payload bits. Exhaustion is checked before descriptor construction.
There is no 1024/2048-entry physical-range directory.

Directory and frame-node backing memory is obtained in 8-MiB pools. Refilling
reserves still-untyped, non-device RAM through the buddy allocator. Headroom
for the worst-case source split and directory growth is kept in the previous
pool, avoiding recursive allocation from an empty pool. Previously published
nodes retain their backing memory.

## Failure and ownership rules

Each successful prefix conversion commits its new source watermark immediately.
Failed later conversions cannot discard that prefix or re-convert an occupied
slot. A failed frame batch is quarantined because a kernel batch can have
partially installed frames.

Boot reservations and generic-delegation watermarks are recorded separately:
kernel metadata is reserved RAM, while a derived generic's unused pages remain
available to the buddy allocator. No kernel changes are needed.

This removes the physical-range table's RAM-size ceiling. It does not remove
independent OS resource limits such as Alpha's heap size or the process/page-table
capability pools.

## Tests

Run the production implementation with checked syscall mocks:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test physical-memory -- --test-threads=1
```

Coverage includes constant bootstrap cost through 16-TiB synthetic memory maps,
sparse RAM/high MMIO, unaligned tails, 1100 materialized ranges, metadata-pool
growth, stable descriptors, failed directory/prefix allocations, and quarantine
after a failed frame batch. Mocks do not validate hardware page tables or DMA;
USB/AHCI QEMU boot tests exercise those integration paths.
