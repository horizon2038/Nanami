# x86_64 mapping attribute and CPU PAT regression tests

Run from the repository root:

```sh
sh tests/kernel-mmio/run.sh
```

The runner uses production inline functions from `memory_types.hpp` and compiles
`memory_manager.cpp` with AddressSanitizer and UndefinedBehaviorSanitizer,
with SMP on and off. The header is included by both translation units; attribute
conversion is checked at compile time and at runtime.
Production page layout and traversal are retained. CPUID, PAT/CR access,
cache/TLB instructions and serial output are host mocks. The architecture
headers are mechanically adapted to translate host addresses without a
higher-half offset. These tests do not emulate caches or real MMIO transactions.

The 20 UP/SMP scenarios cover:

- All three mapping flags at 4-KiB, 2-MiB and 1-GiB leaf sizes, every R/W/X
  combination, and both RAM/Device allocation flags (144 mappings per normal
  scenario). Frame layout remains three words.
- PAT[0]=WB, PAT[1]=WC, PAT[3]=strong UC on each CPU, preserving that CPU's
  other five entries. Already configured PAT is a no-op; repeated setup is
  idempotent and has no shared BSP/AP initialization state.
- Firmware PAT variants, missing PAT support, and unsupported live PCID.
- Cache/TLB invalidation with cache fills disabled, and restoration of CR0/CR4.
- Reserved attributes rejected before leaf publication.
- Mapping without a global ready flag, boot memory map or GOP information.
  Device allocation classification never overrides the user's mapping flag.

C++ attributes live in the existing `kernel/memory/memory_type.hpp`, with
PAT/MAIR encoding in each HAL. `liba9n` remains an architecture-independent
general-purpose library. Rust uses published `a9n_abi 0.8.0` attributes directly.

The kernel no longer scans Device regions, rewrites the direct map, adds
framebuffer exceptions, reserves boundary-table RAM, or shortens the boot
memory map. Its ordinary direct map remains in the boot-created 2-MiB layout.
APIC memory attributes are left to firmware. Neither the loader nor the kernel
adds an APIC/MTRR acceptance check or an attribute-based boot rejection.
User code owns mapping-type
selection and alias consistency, including aliases through the unchanged
kernel direct map. Neither these host tests nor QEMU establishes real-machine
cache behavior or resolves the original USB/Timer IPC stall.
