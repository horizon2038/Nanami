# User-space ACPI mapping

The kernel finding HPET does not imply that an init/driver capability exists for
the table's physical pages: the kernel walks ACPI through its direct map, while
driver-manager requests individual page mappings from Alpha.

The loader used to classify `EfiACPIMemoryNVS` as `Reserved`, which A9N excludes
from the initial generic capabilities. This makes NVS-resident ACPI tables
unavailable to user-space discovery. Both ACPI reclaim and NVS are now explicitly
classified as `Device`: their contents remain preserved, and Nanami excludes
them from ordinary RAM and capability-metadata allocations. No firmware contents
or table pointers are rewritten, and no physical addresses are hard-coded.

ACPI tables can reside in either of these UEFI types. See the
[UEFI platform requirements](https://uefi.org/specs/UEFI/2.10_A/02_Overview.html).
AML SystemMemory OperationRegions can additionally reference Boot Services
Data or Runtime Services Data. Those allocations and interleaved Boot Services
Code (no longer callable after ExitBootServices) are retained as Device
capabilities, never cleared, executed or returned to the ordinary allocator.
Firmware Reserved ranges also retain mapping authority, but no allocator use.
Kernel/init/trampoline allocations are recorded at allocation time and subtracted
regardless of EFI descriptor boundaries; firmware Reserved does not identify
loader ownership. Runtime code, Unusable/PAL and explicit private allocations
remain excluded. This is a loader policy change, not a kernel mapping exception.
After constructing gaps, Reserved entries are omitted without joining across
their holes. Export produces power-of-two ranges and checks the existing
128-Generic budget, not merely the number of original map entries. This avoids
both overflowing the Kernel ABI and silently dropping the tail of a coalesced
range in the kernel's bounded eight-piece split loop.

## Diagnostics

- Loader: `Firmware descriptor (before private exclusions): type=... paddr=... pages=... -> Device` retains the UEFI
  type before adjacent regions are merged.
- Alpha: `[mmio.err] pid=... paddr=... bytes=... stage=... page=... err=...` names
  the failed operation: validation, virtual reservation, physical allocation,
  frame conversion, frame slots, capability copy or page mapping. `page` is an
  index within the request. Errors are printed even when ordinary `os.log` is off.
- Driver-manager: `ACPI map failed page=... bytes=...` gives the actual failing
  page, which may be a child table rather than the RSDP page.
- A failed scan is no longer followed by a misleading "no ACPI HPET" message.
  PIT remains the fallback; the failure is not ignored or retried with invented
  addresses/capabilities.

The reported physical-machine failure (`status=0x1`, RSDP `0x8c7c8000`) identifies
a failed MMIO request but does not identify its page or UEFI type. NVS exclusion
is fixed and reproduced by the host model; confirmation on that machine requires
a fresh boot with this loader and Nanami. Success should include
`ACPI HPET at ...` and `ready mode=hpet-one-shot`.

The original shutdown failure on page `0x8c3a3000` did not identify its EFI
type. A subsequent raw loader log now identifies it as firmware EFI Reserved
(`0x8c29e000`, `0x466` pages). The exported Device range covers that page, and
the next shutdown reaches `Mid(String)` in AML instead of a mapping panic.
The policy distinguishes firmware reservations from loader-owned
pages without relying on that address or inferred EFI type. Tests cover merged
EFI Reserved descriptors containing both private allocations and firmware data,
including reservation boundaries crossing descriptors and generated MMIO gaps.
There is no capability-synthesis fallback in Alpha: only exported firmware
authority is usable, and unusable/runtime-code/private pages still fail closed.

## Physical-trace follow-up (2026-11-14)

Image: `out/hardware/nanami.img`, SHA-256
`d04059dbce19dbdb59850a5af7eab8d205bcb7018547d3cfd9cb3e0d95f832b3`.
Selected native suites passed 251 tests (one existing ignored), and the AML
suite passed 38. New regressions cover the physical I211 trace's ASDE speed
selection, coherent DMA policy before mastering, current-port DHCP identity,
vacant-GGTT ownership/PAT setup, Mid Local/Arg and aliased targets, empty and
overflowing extraction ranges, and complete string/long-buffer comparisons.
No Kernel or loader source change was needed for this follow-up.

This image passed delayed-carrier DHCP with both emulated Intel NIC orders
and 10 HTTP requests per order, zero failures. Both captures passed the DHCP
wire checks. S5 passed PC/USB-storage and Q35/AHCI, each with four CPUs, HPET,
USB input and no i8042. Logs and guest serial/pcap/QMP artifacts are under
`out/hardware/hardware-20261114-review-*`. These tests do not emulate I211 PHY
speed detection, UHD 630 GT behavior, or the physical machine's firmware AML.
The physical errors explain the changes; the resulting hardware behavior
still needs a physical run.

## Reservation ownership validation (2026-11-14, previous image)

Image: `out/hardware/nanami.img`, SHA-256
`12fe38b838bcb7fb84058cf0e323fd3edf3e48295af09d1f25541953f1d6b7c6`.
The selected native suites passed 245 tests (one existing ignored test), and
the AML suite passed 32. They include coalesced EFI Reserved data/private
allocations, descriptor-crossing exclusions, complete export beyond eight
pieces, I219/I211-specific queue registers, all eight GPU PAT indices, the
all-WB private PAT case, and undefined IDLE-to-ACTIVE CSB fields.
The A9N Kernel worktree is unchanged.

This image passed PC USB-storage/USB-input and Q35 AHCI/USB-input ACPI S5,
both with four CPUs, HPET and no i8042. It also acquired DHCP after delayed
carrier with either e1000e/igb PCI ordering and completed 20 HTTP requests per
ordering with zero failures. Single-e1000e INTx and virtio each completed ten
additional requests: all 60 HTTP requests succeeded, and all four captures
passed the DHCP wire checks. Logs/serial/captures are grouped under
`out/hardware/hardware-20261114-final-*`. These
are emulator/register-model results, not a claim of physical I219/I211, UHD 630
or this machine's AML shutdown success. The later raw loader log confirms the
failed page's EFI Reserved type; there is no address-specific exemption.

## Tests

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test acpi --test mmio-requests --test physical-memory -- --test-threads=1
```

Tests use the production loader classification, map builder, driver ACPI parser,
and Alpha MMIO handler. Coverage includes NVS/reclaim discovery, unchanged table
contents, explicit private reservation denial, checksums, page-crossing tables, RSDT compatibility,
the failing child's physical address, failure at each Alpha stage, and unchanged
successful MMIO responses. The existing physical-memory suite checks sparse and
fragmented capability materialization. No kernel change is part of this fix.

## Firmware handoff validation (2026-11-13)

The rebuilt loader/rootfs image is `out/hardware/nanami.img`, SHA-256
`7b4f6c75a4aee8834343feeaf0637ce99934f87ea2e57f29500d9c0a392b2c9d`.
Selected native suites passed 162 tests (one ignored), and AML suites passed
32. They include the production loader policy/export logic, Gen9 filled-GGTT
takeover and rollback, I211 BME-gated queue setup and DHCP port selection.
This is register-model evidence, not physical Intel hardware validation.

QEMU passed USB-storage/USB-input S5 with four CPUs and no i8042, plus DHCP/HTTP
with e1000e INTx and both e1000e/igb multi-port orders with late carrier. All
three HTTP runs completed 10 requests with zero failures. Logs and guest serial
output are grouped under `out/hardware/firmware-handoff-*`; no extra boot-image
copies were retained in the temporary directory.

An additional `physical-memory` run on this AArch64 host passed 40 tests and
failed 18: these unguarded tests require private x86_64 large-page support,
which production `PrivateLarge::new` deliberately disables on AArch64. The
suite does not import the loader policy and its production sources were not
changed here. Do not report that broad run as passing or change the architecture
guard to force it green. The physical firmware type of `0x8c3a3000`, actual
UHD 630 scanout/BCS, and I219/I211 PHY behavior still require the user's machine.
