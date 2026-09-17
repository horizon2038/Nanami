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
Other existing classification policies, including the exclusion of general
Reserved, runtime-services and boot-services regions, are unchanged.

## Diagnostics

- Loader: `ACPI memory: type=... paddr=... pages=... -> Device` retains the UEFI
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

## Tests

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test acpi --test mmio-requests --test physical-memory -- --test-threads=1
```

Tests use the production loader classification, map builder, driver ACPI parser,
and Alpha MMIO handler. Coverage includes NVS/reclaim discovery, unchanged table
contents, Reserved denial, checksums, page-crossing tables, RSDT compatibility,
the failing child's physical address, failure at each Alpha stage, and unchanged
successful MMIO responses. The existing physical-memory suite checks sparse and
fragmented capability materialization. No kernel change is part of this fix.
