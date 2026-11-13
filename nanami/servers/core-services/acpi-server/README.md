# ACPI power service (x86_64)

Run `shutdown` in **Nanami Shell** to synchronize storage and enter ACPI S5.
This is a Shell command, not an implementation of Linux's `reboot(2)` syscall.
Rebuild both initramfs and rootfs to update acpi-server and Shell
(`cargo xtask build --rebuild-rootfs`, or `cargo xtask run --build --rebuild-rootfs`). Do not overwrite
an image while another QEMU instance is using it. `--rebuild-rootfs` recreates
the rootfs: back up any files saved inside the old image first.

The x86_64 driver-manager starts this server from initramfs and delegates the
RSDP address only to that process. Alpha places it on the boot processor, as
required for SMI_CMD accesses. No A9N kernel changes are involved. The server
and its initramfs entry are excluded from AArch64 builds.

On the first shutdown request, `acpi` 6.1.1 validates/reads RSDP, RSDT/XSDT,
FADT, DSDT and SSDTs. Its AML interpreter evaluates `\_S5`; the service does not
scan AML bytes for a hard-coded sleep value. MMIO mappings are cached for the
server lifetime, including tables spanning multiple initial physical-memory
generics. The interpreter's hardware accesses remain in userspace.

The UEFI loader retains ACPI reclaim/NVS, boot-service images/data and Runtime
Services Data and firmware Reserved ranges as non-allocator memory with user-space mapping authority. AML
SystemMemory OperationRegions can reference firmware data buffers, not just
ACPI table allocations. These contents are never zeroed or recycled as normal
RAM. Boot-service code (no longer callable after ExitBootServices) is retained
as data alongside interleaved boot allocations; it is not executed or reclaimed.
Kernel/init/trampoline allocations are tracked explicitly at every successful
loader allocation and subtracted before exporting capabilities. Firmware can
coalesce their EFI Reserved descriptors with adjacent firmware reservations;
the EFI type alone cannot distinguish ownership. Unusable/PAL and runtime
firmware code remain inaccessible. No kernel memory-attribute or allocator
exception is added, and no physical failure address is hard-coded. This change
needs the rebuilt loader, not merely a new rootfs.
Reserved entries are omitted only after gap construction and without joining
across their holes. The exported map is checked against the existing 128-Generic
ABI budget, including the kernel's descriptor splitting cost.

The pinned crate includes a [local AML reference-handling patch](../../vendor/acpi/NANAMI.md)
for integer arguments/locals used by firmware methods. The host tests use the
same patched dependency; failures still abort shutdown rather than being ignored.
`Store`/`Acquire` expression results are preserved, including mutex timeout as
a nonzero result. Invalid comparisons report the failing AML scope and byte
offset; an uninitialized local is not silently converted to zero. A new firmware
failure still needs its method/table evidence before changing evaluation rules.

The subsequent physical shutdown reached AML `Mid(String)`, confirming that the
previously denied firmware data page was now accessible. `Mid` now resolves
transparent Local/Arg sources and clamps string/buffer slices without overflow.
Full string/buffer comparisons use unsigned lexicographic order and lengths,
not four-byte integer truncation. Tests execute real AML methods through the
same power path, including `_PTS` using a `Mid` result before PM1 S5 writes.
No firmware method is skipped and no failed transition is reported as success.

Process-local capability slots are assigned in `src/slots.rs`. PCI remains in
slot 23 throughout AML evaluation; shutdown connects VFS in the distinct slot
25. Reusing PCI's occupied slot for VFS fails service connection before storage
freeze or S5 register writes. The host tests check that all assignments are
distinct, and a compile-time assertion guards the PCI/VFS pair.

Shutdown sequence:

1. Resolve and validate the firmware's S5 definition and PM1 registers.
2. Ask ext2-server to flush dirty blocks and the block-device cache, then freeze
   mutations. Only the registered ACPI service may freeze/thaw the filesystem.
3. Enter ACPI mode if necessary, evaluate optional `\_PTS(5)` and `\_GTS(5)`,
   clear WAK_STS, and program each PM1 control block's own SLP_TYP and SLP_EN.
4. If the transition fails, attempt to restore the previous power mode and
   thaw the filesystem, then report failure. Never bypass a failed storage flush.

Scope and limits:

- Supports PC-style, non-hardware-reduced ACPI (16-bit PM1 control and 32-bit
  event blocks), with System I/O or System Memory registers. Missing `_S5`, bad
  tables, unsupported register layouts, and AML errors abort power-off.
- AML System Memory / System I/O accesses and PCI configuration via MCFG/ECAM
  are supported. Shared legacy CF8/CFC accesses are not used: their two-step
  transaction is not SMP-safe without coordination with the other drivers.
- Embedded-controller/SMBus operation regions and AML operations unsupported
  by the crate are not emulated. A host/interpreter panic disables further ACPI
  requests and attempts to thaw storage and report failure to the caller; after
  a partial firmware method failure, reboot before retrying power management.
- This is not yet a complete ACPI OSPM: no SCI/power-button handling, thermal
  policy, suspend/resume, restart, or graceful process termination. Applications
  must save their own userspace-buffered data before shutdown. VFS-accepted writes
  are synchronized and further filesystem mutations are rejected during S5.
- ACPI mode is not changed at boot. On failure after switching from legacy mode,
  restoration is attempted using ACPI_DISABLE (required for that transition).

Validation:

```sh
cargo test --locked --manifest-path tests/acpi/Cargo.toml
cargo test --locked --release --manifest-path tests/native-performance/Cargo.toml
python3 tests/acpi/qemu-shutdown.py --image /path/to/test.img --smp 4
python3 tests/acpi/qemu-shutdown.py --image /path/to/test.img --smp 1 --hpet off
python3 tests/acpi/qemu-shutdown.py --image /path/to/test.img --smp 4 --storage usb
```

The QEMU test uses a disposable disk snapshot, no NIC, and private OVMF variables.
It requires a QMP `SHUTDOWN` event with `guest=true`, `reason=guest-shutdown`,
and a successful QEMU exit after the VFS flush and S5 log records. This does not
substitute for testing the firmware's AML and power-off behavior on real hardware.
