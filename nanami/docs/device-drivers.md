# Device Drivers

Hardware drivers run in user processes. A9N supplies capabilities and mapping,
DMA, IRQ and IPC mechanisms; this driver reorganization changes no kernel API
or IPC fastpath.

## Startup ownership

`driver-manager` is the existing `device-manager` provider, extended rather than
duplicated. `src/arch/x86_64.rs` and `src/arch/aarch64.rs` select platform drivers.
`boot-list` assigns trusted image policy; hardware-driver binaries are included
in the initramfs even when started after core initialization. `system-list`
contains logical services, not unconditional display/PS2/PCI-network drivers.

On pc99, the manager starts `pci-server`, connects to its registry entry, then
selects storage/timer/USB and optional PS/2, virtio networking and display
providers. The existing storage-root arbitration remains in place. Optional
driver spawn failure is logged without terminating the manager. PCI discovery
failure is a boot error because storage selection cannot safely continue.
Boot-time PCI and timer dependencies use bounded, yielding registration retries;
their providers run at least at the dependent driver's priority. This avoids
PS/2 or ACPI exiting when another CPU has not yet published the timer. Ordinary
service connection and steady-state IPC paths are unchanged.

Display selection and fallback live in the manager's common `display.rs`, not
in an Intel-specific backend. Architecture code returns an optional native
candidate. Unknown/unsupported GPUs, absence of a native platform backend,
failed native-driver spawn or an explicit failed applicability probe select
`fb-server`, provided the boot framebuffer exists. Peripheral rediscovery errors
also take this display fallback instead of terminating the ready manager.

Modern virtio GPU display IDs select `virtio-gpu-server`; supported Gen9 IDs
otherwise select `intel-graphics-server`. The selected process
either publishes `display_service` and reports success, or reports failed
initialization so the manager starts `fb-server`. Probe reports are accepted
only from the selected PID; duplicate success/fallback reports are rejected.
If framebuffer startup fails, a decline report returns an error, not a false
success. Missing boot framebuffer permits headless startup and is logged.

`fb-server` remains a separate backend with its own WB shared composition RAM
and WC hardware framebuffer mapping. It validates the existing 32-bpp layout,
maps/initializes it, then publishes `display_service`; failed setup does not
publish a port clients would block on. It also supplies the common GPU message
protocol with zero hardware-acceleration flags, retaining Honoka's CPU/damage
path without native GPU, EDID, vblank or timer support.

Fallback applies **before native display activation**: a declining driver must
leave the firmware scanout intact. After publication, native readiness-report
failure keeps the existing service alive rather than abandoning its clients.
There is no automatic restart/replacement of an active display owner, crash
watchdog or safe post-modeset handback to the original boot framebuffer.

AArch64 retains its virtio-MMIO storage path and optional boot framebuffer;
the pc99 PCI, Intel and virtio-gpu PCI backends are not built for it.

The [GPU service guide](gpu.md#virtio-gpu-2d-backend) describes the virtio 2D
backend's capability discovery, fenced queue, shared DMA resources and
pre-activation fallback. `cargo xtask run` defaults to virtio-vga on x86_64;
`--gpu std` explicitly selects standard VGA/GOP. The common message protocol and
legacy display interface are both implemented. CPU fill/copy and virtual
resource transfer must not be confused with virgl or physical-GPU acceleration.

## PCI service

`nanami-uapi::pci` defines the wire protocol and `nanami-client::pci` the client.
The single server owns CF8/CFC. For segment zero below offset 256 it uses legacy
configuration I/O, with actual byte/word/DWORD access widths. ACPI MCFG supplies
UC ECAM mappings for extended or nonzero-segment accesses. MCFG checksums, bounds
and overlapping bus ranges are validated; absence of MCFG permits legacy PCI.

| Request | Inputs | Results |
| --- | --- | --- |
| ENUMERATE (`0xd103`) | ordinal | packed address, device/vendor/class/header; `PCI_ENUM_END` at end |
| READ (`0xd101`) | address, offset, width | value |
| WRITE (`0xd102`) | address, offset OR width shifted 16, value | status |
| PROBE_BAR (`0xd104`) | address, BAR index | physical base, size OR IO/prefetchable flags |
| NAME (`0xd105`) | inventory ordinal, field, byte offset | total UTF-8 byte length, up to 8 bytes packed little-endian |

An address packs segment in bits 47:32, bus in 23:16, device in 12:8 and
function in 2:0. Width is 1, 2 or 4; offset must be naturally aligned and below
4096. Clients retain a connection rather than resolving the registry per read.

Enumeration is cached once, follows segment-zero buses reachable from bus 0
through PCI bridges, handles multifunction devices and rejects more than 512
devices. It is not a hotplug inventory or enumeration of every MCFG segment.
BAR probing is one serialized transaction: preserve command, disable decoding
and bus mastering, size the BAR, restore both halves, then restore command.
Command writes use 16 bits so adjacent W1C status is not cleared. Failed BAR
restoration leaves decoding disabled. Drivers must probe before enabling their
own queues/DMA; this API is not live resource relocation.

AHCI, USB, virtio-blk, virtio-net and AML PCI operations use this service instead
of maintaining separate config-port latches/scanners. Common MSI/MSI-X disabling
preserves unrelated controls. Virtio drivers request only their BAR I/O range,
not the entire port space. ACPI generic I/O excludes CF8/CFC.

The current native authority model grants configuration access through the PCI
service connection. It does not implement per-function leases, driver sandbox
isolation or a DMA IOMMU. Alpha still grants MMIO/DMA/IRQ capabilities. Packet,
block, MMIO and DMA data paths do not traverse the PCI service.

### Identification and `pci-info`

PCI Server embeds [`pci-ids-rs` 1.2026.10](https://docs.rs/pci-ids-rs/1.2026.10/pci_ids_rs/)
(`pci_ids_rs` in Rust), including its packaged PCI ID database. It resolves vendor,
device, class, subclass and programming-interface names once for each enumerated
device and retains static string references. Config-space reads/writes and
ordinary data transfers do not perform ID-database lookups. Native clients do
not link their own copy of the database.

NAME fields 0..4 select vendor, device, class, subclass and interface. Ordinals
refer to the same stable inventory as ENUMERATE. Unknown IDs have empty names,
not enumeration errors. Out-of-range ordinals/fields/offsets are rejected.
`nanami_client::pci::name` assembles and validates the UTF-8 text in a caller's
buffer. Human-readable names are diagnostic metadata, not driver-match rules;
hardware selection still uses numeric IDs/class codes and backend support.

From Nanami Shell:

```text
pci-info
pci-info -n
pci-info -nn -v
net-info
```

`pci-info` is a native `/bin/pci-info` application. Default/`-nn` output includes
address, class, vendor/device names, numeric IDs and revision. `-n` omits name
queries; `-v` adds programming-interface, header and command/status information.
All hardware access is read-only: it never sizes BARs, writes config registers,
enables a device or changes IRQ routing. Output uses the invoking foreground
terminal; non-terminal launches use serial output. Missing PCI service is
reported immediately, including on the current AArch64 configuration.

`net-info` is the renamed Shell builtin, previously `netinfo`; it shows IPv4,
gateway, DNS and MAC information. Shell help uses the new spelling.

## Intel Graphics: initial Gen9 backend

The server/backend boundary dispatches a whitelist of Intel Skylake, Kaby Lake
and Coffee Lake display-function PCI IDs. Other Intel HD generations are not
silently treated as register-compatible.

The backend reads BAR0 registers and GGTT, BAR2's aperture, and GGC's GGTT size.
It does not obtain mode/stride/surface from UEFI framebuffer metadata. It:

- Adopts one already running, unscaled, linear XRGB8888 primary plane, with zero
  plane position/offset and no additional enabled planes or primary pipes.
- Rejects active compression, panel self refresh, display low-power states and
  unsupported layouts before changing any GGTT entry or plane surface.
- Allocates two contiguous, page-aligned scanout buffers, using physical
  addresses below the backend's 39-bit limit. Native DMA mappings are WB;
  explicit cache-line flushes publish CPU writes before scanout.
- Copies the inherited image through the aperture and binds only entirely
  zero GGTT PTEs. New surfaces may use the full GGTT range addressable by
  `PLANE_SURF`, not just BAR2's smaller CPU aperture. Uploads use the DMA's CPU
  mapping; only the inherited image is read through BAR2. Existing
  firmware/scratch mappings are never overwritten.
- Exposes the existing display protocol and the common GPU protocol. GPU-buffer
  presentation uses the BLT engine when its self-test succeeds; the legacy
  shared-framebuffer path retains CPU copies. Damage history repairs the stale
  alternating buffer.
- Arms `PLANE_SURF`, then checks `PLANE_SURFLIVE` using bounded 1-ms timer sleeps.
  Register write readback alone is not considered flip completion. A timeout
  poisons presentation and retains DMA/PTEs because either buffer may be live.

`GET_CAPABILITIES` (`0x5004`) returns `DISPLAY_CAP_NATIVE_SCANOUT` and
`DISPLAY_CAP_VBLANK_FLIP` for this backend. Both providers additionally return
`DISPLAY_CAP_GPU_INTERFACE`; this flag alone does not mean hardware acceleration.
The native flip flags denote
hardware-vblank-latched flips, not a vblank IRQ subscription API. Honoka queries
these capabilities once while connecting. With vblank flips it composes sparse
damage independently, then presents their bounding rectangle once per frame.
Untouched gaps retain the compositor's previous RAM pixels. This avoids waiting
for a separate vblank for each dirty rectangle. The framebuffer fallback keeps
its individual damage submissions.

### Active output and monitor discovery

The Gen9 backend reads transcoder/DDI routing, including the separate eDP
transcoder, to identify the inherited output as HDMI, DVI, DisplayPort or eDP.
It does not identify a connector from a GPU model alone. Unsupported cloned,
MST or FDI routing does not produce invented output information.

For board wiring it reads the PCI ASLS OpRegion and the VBT General Definitions
block. Header, checksum, BDB block bounds and variable child-record lengths are
validated. OpRegion 2.0 absolute and 2.1+ relative extended-VBT addresses are
handled separately. Complementary HDMI/DP entries for one DDI merge; conflicting
explicit routes are rejected. Sunrise/Kaby/Comet-V and Cannon/Comet Point PCH
IDs select different GMBUS pin encodings. An explicit VBT route takes precedence;
unknown PCH/pin combinations are not guessed.

HDMI/DVI EDID reads use GMBUS at 100 kHz; DP/eDP reads use I2C-over-AUX. AUX
handles ACK, NAK, native/I2C DEFER, short replies and MOT release. Transactions
poll with bounded timer sleeps, leave interrupts disabled while polling, and
restore idle channel configuration. An engine busy at entry is not touched.
Still-busy timed-out transactions are not overwritten by cleanup. No panel
power, link-training, PLL, CDCLK or mode change is performed by discovery.
DDC failure leaves native scanout available without advertising EDID.

EDID parsing validates the base header and block checksums. It retains at most
256 raw bytes and extracts progressive, digital separate-sync detailed timings
from the base and first CTA extension. Duplicate detailed timings merge while
retaining the preferred flag. Missing, failed or additional extensions are
reported as truncated; a valid base survives a corrupt extension. Standard
timings, CTA VICs and DisplayID are not expanded into modes.

The additive read-only protocol is available through
`nanami_client::gfx::display`:

| Request | Inputs | Results |
| --- | --- | --- |
| CONNECTOR_INFO (`0x5005`) | connector ordinal | two words: output ID/kind/pipe/flags/active dimensions, EDID bytes/detailed-timing count; `(0, 0)` ends iteration |
| READ_EDID (`0x5006`) | connector ordinal, byte offset | up to 16 EDID bytes, little-endian packed in two words |
| MONITOR_MODE (`0x5007`) | connector ordinal, timing ordinal, part 0/1 | clock/flags and horizontal timing in part 0; vertical timing and reserved zero in part 1; `MODE_END` ends iteration |

`DISPLAY_CAP_CONNECTOR_INFO`, `DISPLAY_CAP_EDID` and
`DISPLAY_CAP_MONITOR_MODES` advertise the discovered information. Only the
single active output is enumerated, not disconnected physical connectors.
Monitor timings are EDID advertisements, not a promise that the driver can
program them. There is deliberately no `SET_MODE` request at this stage.

This initial backend leaves clocks, watermarks, PLLs and HDMI/DisplayPort links
unchanged. It does not provide resolution changes, complete physical-connector
enumeration, hotplug, suspend/resume, 3D rendering, DRM ioctls or Mesa integration.
It is therefore not yet a Linux/Wayland GPU ABI. Firmware that fills all GGTT
entries with scratch mappings will fall back instead of guessing ownership.

QEMU does not emulate the Intel GPU. Host tests exercise production register
logic, DMA copies, damage repair and timeout ownership using an MMIO model;
QEMU regression exercises PCI, selected drivers and the framebuffer fallback.
Neither result proves native scanout on physical Intel hardware. A real-machine
test should retain serial output and require the `[intel-graphics] native
scanout ...` and manager readiness messages before evaluating page flips.

The independent [GPU service guide](gpu.md) describes the shared message ABI,
Honoka integration and Gen9 BCS fill/copy/presentation acceleration. Its scratch
self-test must succeed before the provider reports accelerated capabilities.

Register/reference sources:

- [Intel Gen9 documentation index](https://www.intel.com/content/www/us/en/docs/graphics-for-linux/developer-reference/1-0/skylake.html)
- [Intel Kaby Lake display PRM](https://cdrdv2.intel.com/v1/dl/getContent/723969)
- [Intel Kaby Lake memory views PRM](https://cdrdv2.intel.com/v1/dl/getContent/723948)
- [Linux v6.6 PCI generation IDs](https://github.com/torvalds/linux/blob/v6.6/include/drm/i915_pciids.h)
- [Linux v6.6 Gen9 display registers](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/i915_reg.h)
- [Linux v6.6 GGTT setup](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/gt/intel_ggtt.c)
- [Linux v6.6 GMBUS register definitions](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/display/intel_gmbus_regs.h)
- [Linux v6.6 AUX register definitions](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/display/intel_dp_aux_regs.h)
- [Linux v6.6 VBT record definitions](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/display/intel_vbt_defs.h)
- [Linux v6.6 OpRegion definitions](https://github.com/torvalds/linux/blob/v6.6/drivers/gpu/drm/i915/display/intel_opregion.c)

## Checks

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --test pci-server --test usb-pci --test intel-graphics --test acpi -- --test-threads=1
cargo xtask servers
cargo xtask servers --arch aarch64
```

Use `cargo xtask build` to package driver/initramfs changes into the boot image.
An existing rootfs is not updated implicitly and may still contain the old
`/nanami/system-list` that starts PS/2, framebuffer and network-device drivers.
Migrate that manifest to the checked-in list or create a new rootfs; do not boot
both the old unconditional drivers and the manager-selected copies. For example:

```sh
cargo xtask build --no-linux-bins --rootfs out/gen9-rootfs.img --output-image out/gen9.img
cargo xtask run --image out/gen9.img --snapshot
```

Choose unused output paths. `--rebuild-rootfs` replaces guest data and is only
appropriate for a disposable test image or after backing up the existing disk.

### Validation record (2026-10-25)

- Native host suite: 791 passed, 7 existing ignored tests; xtask: 17 passed.
- Release service builds: x86_64/pc99 and AArch64/QEMU passed.
- Snapshot QEMU with the rebuilt manifests: AHCI and legacy virtio-blk at SMP 4,
  USB storage at SMP 1 reached Honoka/Shell, accepted PS/2 input and executed
  the unmodified Linux `events-test` fixture with exit status zero.
- Restricted loopback HTTP regression through virtio-net: 20 requests, zero
  failures, including DHCP configuration.
- The initial SMP 4 attempt stopped during kernel clock initialization, before
  entering Nanami. A retry exposed the early PS/2/timer registration race;
  after the bounded boot-connection fix both SMP 4 storage configurations
  completed. The initial pre-user-space stop is not declared fixed here.
- Intel hardware scanout remains unverified on a physical GPU; the Gen9 host
  register model is distinct from the QEMU framebuffer-fallback checks.

### PCI diagnostics follow-up (2026-10-25)

- Full native host suite: 801 passed, 7 existing ignored tests; xtask: 17 passed.
- x86_64 and AArch64 Release service builds passed with the new CLI/protocol.
- A fresh snapshot QEMU image at SMP 4 enumerated seven PCI functions. Serial
  output resolved Intel chipset/AHCI and Red Hat virtio names from the database.
- `pci-info`, `pci-info -n`, `pci-info -nn -v` each displayed their inventory and
  exited with status zero. Screenshots were inspected, including numeric
  fallback for the database-unknown `1234:1111` VGA function.
- `net-info` displayed the guest IP, gateway, DNS and MAC; the Linux
  `events-test` fixture also exited successfully in the same boot.

### Gen9 display discovery follow-up (2026-10-25)

- Full native host suite with the documented `--test-threads=1` mapping mock:
  822 passed, 7 existing ignored tests; xtask: 17 passed. The default parallel
  test run failed in shared heap-mapping fixtures; the serialized full run
  passed. Gen9's 29 tests and Honoka's 21 motion tests also passed separately.
- Gen9 tests cover full-GGTT allocation above BAR2, GMBUS/AUX requests and
  cleanup, short replies, repeated DEFER, NAK, busy timeouts, EDID checksums and
  detailed timings, VBT wiring merges/conflicts, OpRegion RVDA semantics and
  the production discovery-client wire format. These use simulated hardware.
- x86_64/pc99 and AArch64 Release service builds passed. A fresh disposable
  `out/gen9-ddc.img` contains the updated Honoka and graphics server; existing
  default/rootfs images were not replaced.
- Snapshot QEMU: AHCI at SMP 4 and virtio-blk at SMP 1 reached Honoka/Shell and
  completed the Linux `events-test` fixture with exit status zero. The AHCI
  screenshot was inspected. These are framebuffer-fallback regressions,
  not Intel DDC or native scanout tests.
- No A9N or IPC fastpath source changes were made. Native Intel hardware
  validation and full modesetting remain separate work.

### Framebuffer fallback follow-up (2026-10-25)

- Display selection is shared by the x86_64 and AArch64 manager paths. The
  independent framebuffer backend remains packaged in the boot initramfs.
- Seven selection-policy tests cover unsupported/no native backend, driver
  spawn failure, declined initialization, readiness, duplicate/unauthorized
  reports, failed fallback startup and absent boot framebuffer.
- Full serialized host suite: 829 passed, 7 existing ignored tests. x86_64 and
  AArch64 Release service builds passed; A9N sources were unchanged.
- Snapshot QEMU at SMP 4 with `out/display-fallback.img` and
  `qemu-smoke.py --expect-framebuffer --program events-test` confirmed one
  framebuffer startup, publication after initialization, Honoka/Shell drawing
  and Linux fixture exit status zero. The final screenshot was inspected.
  Initialization checks use the completed-cache marker because SMP serial
  output may interleave the prefix of multi-part mapping logs.
