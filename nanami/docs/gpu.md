# GPU service and 2D acceleration

The GPU protocol is independent of Intel registers and the boot framebuffer.
`nanami-uapi::gpu` defines ABI version 1, `nanami-client::gpu` implements the
client, and `nanami-gpu` validates provider requests. `intel-graphics-server`,
`virtio-gpu-server` and the CPU-only `fb-server` implement it. A9N and its
IPC fastpath are unchanged.

## Connection and messages

Providers register `gpu-service` as an alias of their display endpoint. Honoka
uses the already selected `display_service` connection when its
`DISPLAY_CAP_GPU_INTERFACE` flag is set. This keeps presentation and drawing on
the same provider; Honoka does not resolve a second, potentially different GPU.
The existing display protocol remains available to older clients.

All requests use native service IPC: code followed by at most four words, with
an authenticated caller PID supplied by Alpha. Replies contain status and up to
two result words. Reserved inputs must be zero. The current native targets have
64-bit words. No pointers to client command lists, physical addresses, GPU
addresses or vendor-specific command packets cross this interface.

| Request | Arguments after code | Results after status |
| --- | --- | --- |
| GET_INFO (`0x6001`) | none | version in bits 15:0 and supported flags above bit 15; accelerated flags in bits 15:0 and pixel format above bit 15 |
| CREATE (`0x6002`) | width / height packed as two 32-bit values, byte stride, pixel format | buffer ID, caller's CPU mapping address |
| DESTROY (`0x6003`) | buffer ID | zero, zero |
| FILL (`0x6004`) | buffer ID, packed x/y, packed width/height, pixel value | zero, zero |
| COPY (`0x6005`) | source ID, destination ID, source x/y and destination x/y packed as four 16-bit values, packed width/height | zero, zero |
| PRESENT (`0x6006`) | buffer ID, packed x/y, packed width/height | zero, zero |

For two-value packing the first value occupies the low bits. Capability flags
are FILL=1, COPY=2 and PRESENT=4. `supported` means the operation is available;
`accelerated` is its hardware-backed subset. A CPU provider therefore reports
supported operations with **zero accelerated flags**, not a fabricated GPU.
The current format is linear XRGB8888 (format 1, little-endian B/G/R/X bytes).
Presentation additionally requires the screen's exact width, height and stride.
Non-XRGB boot screens keep the legacy display path.

## Ownership and synchronization

- IDs belong to one caller and provider. Cross-owner operations are rejected;
  destroyed IDs are never reused during the service's lifetime.
- Width/height must be 1..8192, stride must be a multiple of 64 bytes, at least
  width times four, and at most 32704. A zero CREATE stride selects the minimum.
  Allocations are page-rounded, at most 64 MiB each, 16 live buffers and 128 MiB
  total per provider. Bounds/overflow are validated before memory or hardware
  access. COPY rejects overlapping rectangles in the same buffer.
- Commands are synchronous: success means writes are complete and CPU-visible.
  Do not access participating buffers concurrently with a request. PRESENT also
  waits for the native display flip to latch. There is no asynchronous fence,
  shader, command-buffer or Linux DRM API in this protocol.
- CPU mappings are Normal/WB RAM, not MMIO. `Surface::destroy` first retires the
  provider handle, then releases the caller's mapping. A caller must stop using
  the mapping after destruction. Alpha references prevent either side's release
  from freeing pages still mapped by the other side.
- Transport/device failure can leave work in flight. Such buffers must not be
  accessed, freed or recycled as though a successful completion occurred.
  Honoka poisons both operands of a failed copy, stops CPU writes/presentation
  and retains their mappings. The native driver pins DMA and address mappings
  after an uncertain completion; it does not silently reuse them through a CPU
  fallback. DESTROY retires a handle even on cleanup failure and retains its
  quota in that case.
- Allocation pressure can reclaim buffers of confirmed-dead owners. Failed
  liveness checks retain them. Alpha is queried only under allocation pressure,
  not on FILL, COPY or PRESENT.

The additive Alpha request `OS_REQUEST_MAPPING_SHARE` (`0x1029`) takes peer PID,
owned base VA and byte count. It shares an exact, active RAM allocation after
checking the caller's frame capabilities and VM records; it cannot map an
arbitrary physical address or a foreign/partial allocation. It preserves RAM
contents, adds a reference, and rolls back partial capability/mapping installs.
GPU drivers use it to share their zeroed DMA allocation with a client without
creating CPU WB/WC aliases. This is a userspace OS-service operation, not a new
A9N kernel ABI.

## Honoka

Honoka's composition canvas is a GPU-service buffer with an ordinary CPU
mapping. When advertised as hardware-backed, opaque fills and static-background
copies of at least 4096 pixels use FILL/COPY. The background cache uses a second
GPU buffer. Small primitives, font rasterization, alpha blending, cursor pixels
and client-window composition remain on the CPU. No per-pixel IPC is introduced.

On Gen9, PRESENT copies damaged pixels into a native back buffer with BCS, then
flips at vblank. Alternating-buffer damage history repairs stale regions. The
existing once-per-frame merged presentation remains in use. On the framebuffer
provider, the same PRESENT message uses the existing CPU shadow/damage copy;
Honoka performs fills and copies locally because accelerated flags are zero.
Unsupported protocol/format or unavailable acceleration at initialization keeps
the corresponding software path.

## Virtio GPU 2D backend

`virtio-gpu-server` implements the same buffer, FILL, COPY and PRESENT protocol
on x86_64/pc99. Driver Manager recognizes modern PCI `1af4:1050` display devices,
prefers this virtual display when present, and retains the common framebuffer
fallback. Both VGA-compatible `virtio-vga` and non-VGA modern PCI device classes
are recognized; the tested primary-display configuration uses `virtio-vga`.
The backend is not built on AArch64 while that platform lacks PCI/timer services.

- PCI vendor capabilities supply common configuration, device configuration and
  notification locations. BAR ranges, alignment, capability loops, lengths and
  notification arithmetic are checked. MMIO uses UC mappings; buffer/queue RAM
  remains Normal/WB. PCI MSI/MSI-X and INTx are masked for polling.
- Feature negotiation accepts `VIRTIO_F_VERSION_1`, not virgl, blobs, packed
  queues or optional ring features. An `ACCESS_PLATFORM` requirement is declined
  because this driver has no IOMMU mapper. An incompatible firmware-owned
  device is declined before reset where its feature state is already readable.
- A split control queue runs one command at a time. Every request carries a
  unique fence. Used-ring index, descriptor ID, response length/type, fence and
  reserved header fields are validated before its storage is reused. Polling
  yields through the timer and has a bounded retry count. Timeout poisons the
  queue and pins backing instead of freeing memory still available to DMA.
- CREATE zeroes DMA RAM, creates a BGRX resource, attaches its contiguous backing
  and shares that same allocation with the client through Alpha. Resource width
  includes stride padding; scanout exposes only the visible width. DESTROY
  disables an active scanout, detaches backing and unreferences the resource,
  waiting for each fence before releasing local RAM. Client ownership and
  quotas are still enforced by `nanami-gpu`.
- The standard 2D device has no fill/copy command. Those operations use the
  validated CPU implementation and **accelerated flags are zero**. PRESENT sends
  TRANSFER_TO_HOST_2D and RESOURCE_FLUSH; SET_SCANOUT selects a new resource.
  A resource's entire visible area is transferred before its first scanout so
  uninitialized host pixels cannot become visible. Later updates transfer only
  declared damage, using its byte offset and padded pitch.
- Only one enabled scanout is selected at startup; its dimensions remain fixed.
  No hotplug/mode-change, hardware-cursor, virgl/OpenGL/Vulkan or DRM interface is
  advertised. The existing display protocol also works for older Honoka images.
  Native capability flags request one merged PRESENT per frame, without claiming
  hardware vblank completion.

Service publication follows queue negotiation and display-info validation.
Initialization cleanup acknowledges reset before releasing queue DMA; failed
reset retains the process even if the manager's reply fails. A published native
provider is never replaced by a competing framebuffer writer. On `virtio-vga`,
reset returns the device to VGA compatibility for pre-activation fallback.

After rebuilding, normal `cargo xtask run` selects `virtio-vga` by default.
`cargo xtask run --gpu std` keeps standard VGA/GOP. Existing guest disks and
staging rootfs images are not automatically migrated by this selection.

The `virtio-gpu` host target covers real wire packets, padded pitches, CPU copy,
resource switching/ownership, the legacy display path, u16 ring-index wrap,
delayed/bad completions, failed reset and peer-sharing rollback. In addition,
snapshot QEMU validates the actual virtual device, not just a host model:

```sh
python3 tests/alter-linux/qemu-smoke.py --image out/virtio-gpu.img --gpu virtio --expect-virtio-gpu --storage ahci --smp 4 --program events-test --ui-stress
python3 tests/alter-linux/qemu-smoke.py --image out/virtio-gpu.img --gpu std --expect-framebuffer --storage virtio --smp 1 --program events-test
python3 tests/alter-linux/qemu-smoke.py --image out/virtio-gpu.img --gpu virtio --virtio-gpu-iommu --expect-framebuffer --storage ahci --smp 1 --program events-test
```

All three paths reached Honoka/Shell and completed `events-test`. The native
SMP 4 run's scrolling/history/clear screenshots were inspected. The third path
intentionally declines an unsupported feature and verifies live framebuffer
fallback on the same virtio-vga device. The ordinary xtask launcher was also
booted with its default USB root/input and native virtio display in a snapshot.
These checks validate virtio 2D, not the separate physical Intel backend.

The complete serialized native host suite on 2026-10-26 passed 856 tests
(7 existing ignored). Release services built for x86_64 and AArch64; the latter
correctly omits this pc99 PCI driver. The final boot image repeated both the
native virtio path and the deliberately declined-feature fallback successfully.

Device references: [Virtio 1.2 specification, section 5.7](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html),
[QEMU virtio-gpu](https://www.qemu.org/docs/master/system/devices/virtio/virtio-gpu.html).

## Intel Gen9 BCS backend

The existing Skylake/Kaby Lake/Coffee Lake whitelist and conservative inherited
display-mode checks still apply. The backend:

- Claims an idle BLT engine with an owned forcewake request. It does not steal
  an existing request, reset the GPU, reprogram render/media engines or change
  firmware display clocks/links.
- Uses one private logical BCS context and Execlists submission. Typed
  `XY_COLOR_BLT` / `XY_SRC_COPY_BLT` packets implement fills and copies.
- Installs only owned DMA pages in a private four-level PPGTT, and only uses
  empty GGTT entries for context/ring/status/surface storage. GPU page-table
  attributes select an existing UC private-PAT entry; the global GPU PAT and
  CPU device-map policy are not rewritten.
- Explicitly flushes CPU cache lines at CPU/GPU ownership boundaries. A matching
  post-sync memory breadcrumb, context-complete CSB event and idle engine are
  all required before reusing the context/ring. Polling has a bounded timeout
  and yields through the existing timer service; no GPU IRQ path is added.
- Fills and copies private scratch surfaces and verifies their pixels before
  exposing hardware capability flags. Failure leaves native CPU scanout
  available; uncertain engine DMA stays pinned. A subsequent device hang fails
  closed instead of racing CPU rendering against unfinished DMA.

This is **2D BLT acceleration**, not 3D rendering, OpenGL/Vulkan, Mesa, DRM or
full Wayland graphics support. Native display applicability/fallback policy is
documented in [Device Drivers](device-drivers.md).

## Validation

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --test gpu --test gpu-blt --test virtio-gpu --test honoka-motion --test core-process -- --test-threads=1
cargo xtask servers
cargo xtask servers --arch aarch64
```

`gpu` runs the real client/provider protocol, including ownership, bounds,
padding, quotas and dead-client reclamation. `gpu-blt` interprets the production
packets through the production PPGTT/Execlists code in a checked host MMIO/DMA
model, including CSB wrap and incomplete-fence failures. `honoka-motion` compares
GPU-protocol and CPU composition pixel-for-pixel through cursor motion and
window dragging, and checks fail-closed behavior. `core-process` covers mapping
sharing, both release orders and rollback failures.

Validation on 2026-10-25: the complete serialized native host suite passed
846 tests (7 existing ignored), and the root workspace tool/UAPI suites passed
39 tests. x86_64 and AArch64 Release services built successfully, including a
complete x86_64 boot image with the new Alpha sharing operation. Disposable
snapshot QEMU runs passed framebuffer-provider publication and the unmodified
Linux `events-test` at AHCI/SMP 4 and virtio-blk/SMP 1. The SMP 4 run also
exercised Shell scrollback/history/clear; its captured screens were inspected.

QEMU's virtio GPU and framebuffer providers exercise the common protocol in
the guest, not the Intel engine. Neither the model nor QEMU proves physical
Intel operation or a performance gain. Physical validation must observe
`Gen9 BCS accelerated fill/copy/present ready` after the driver self-test and
check repeated presentation on the supported GPU.

Primary register references:

- [Intel Skylake command-stream programming PRM](https://cdrdv2-public.intel.com/685887/intel-gfx-prm-osrc-skl-vol06-command-stream-programming.pdf)
- [Intel Skylake structures PRM](https://cdrdv2-public.intel.com/685883/intel-gfx-prm-osrc-skl-vol02d-commandreference-structures.pdf)
- [Linux v4.19 logical-ring/context definitions](https://github.com/torvalds/linux/blob/v4.19/drivers/gpu/drm/i915/intel_lrc.c)
- [Linux v4.19 command definitions](https://github.com/torvalds/linux/blob/v4.19/drivers/gpu/drm/i915/intel_gpu_commands.h)
