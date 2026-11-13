# Intel Ethernet net-device

`intel-net` is a userspace PCI Ethernet provider for the existing `net-device`
protocol. It contains e1000e-class and igb-class backends, not Linux driver
wrappers. PCI identity policy is shared with Driver Manager in
`nanami-uapi::intel_net`; matching the Intel vendor or Ethernet class alone is
not enough to bind an unknown controller.

## Device coverage

- e1000e: 82574L/82583V copper and the listed I219-LM/V revisions, including
  `8086:15bb` (I219-LM 7).
- igb: 82576/82580/I350 copper and I210/I211, including `8086:1539` (I211).
- I225, VFs, SerDes/SFP variants and unidentified PCI IDs are not claimed.

The bounded PCI table is the exact authority for binding. Family membership
does not mean every board/firmware/PHY combination has been hardware-tested.

The existing network stack has one `net-device` connection. Driver Manager
prefers a detected virtio NIC, otherwise starts **one** Intel provider. Intel
initializes at most eight supported functions, waits up to three seconds for
carrier, then selects the first link-up function in PCI inventory order. If
none has carrier, it selects the first initialized function. The selected BDF,
device ID, MAC and receive mode are logged. Unselected functions keep bus
mastering and interrupts disabled; they never compete for the registry name.
There is no multi-interface routing or automatic post-publication NIC failover.

## Hardware ownership and data path

Configuration reads/writes and BAR probing use `pci-server`. BAR0 registers are
mapped UC through Alpha; packet buffers and descriptors use one pinned coherent
x86 DMA allocation with full 64-bit addresses. PCI PM transitions use D0, MSI
and MSI-X are disabled, and only the selected device enables bus mastering.
The service is registered only after the rings and wakeup source are ready.

I219 performs a MAC-only reset, preserves firmware/ME PHY configuration and
applies the PCH reset-hang workaround. It never issues a combined PHY reset or
uses the discrete controller's EEPROM protocol against PCH flash. Discrete
devices leave PHY power-down/isolate using MDIC. I210/I211 packet-buffer sizes
are restored for warm-reset startup. Waits are bounded and yield via timers;
NVM, flash and firmware images are never programmed. Platforms needing an
additional ME/ULP exit or PHY-specific firmware workaround need separate
validation; a missing/invalid MAC or timed-out operation declines that function.

Queue zero has 128 RX and 128 TX descriptors with 2 KiB buffers. e1000e uses
legacy descriptors; igb uses advanced one-buffer RX and advanced TX. There is
no TSO, checksum offload, jumbo frame, RSS or multi-queue setup. TX completes
asynchronously, leaves one ring entry empty and never overwrites an in-flight
buffer. Short frames are explicitly zero-padded. RX discards malformed,
oversize, error and multi-descriptor packets, restores advanced descriptors'
buffer addresses after writeback, then returns ownership with a release fence.
Each packet is copied directly between DMA and the network server's SHM once;
there is no per-packet allocation or intermediate software receive queue.

The provider implements SEND, RECV, RECV_BATCH (up to 32 records), GET_MAC,
GET_FEATURES, SHM attachment, RX notification attachment and administrative
LINK_UP/DOWN. Batch ranges are checked before consuming descriptors. A single
nonzero client owns mutable operations after SHM attachment. Notification slots
are distinct from PCI/timer/ACPI/service slots. An error after allocating DMA
parks the driver rather than returning live DMA pages to Alpha's allocator.

INTx routing first uses the ACPI service and otherwise a usable firmware PCI
Interrupt Line. Only GSI routes supported by the current A9N HAL are accepted;
there is no guessed PCH/PIIX register programming. A busy/shared or unresolved
IRQ falls back to a 2 ms timer notification and notifies `net-server` on pending
RX. IRQ mode has a one-second maintenance wakeup, not continuous polling.
Reading ICR deasserts the device before IO-APIC rearm; RX is checked again after
rearm. Existing ACPI root-bus routing does not yet resolve bridge-local `_PRT`
tables, so an I211 behind a bridge may use the timer fallback. This is not an
MSI/MSI-X implementation.

## Validation and commands

Host tests execute production reset/queue/ring/protocol logic with fake MMIO,
including high DMA addresses, advanced writeback, wraparound, TX backpressure,
invalid batches and client ownership:

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml --test intel-net -- --test-threads=1
```

QEMU emulates an 82574L with `e1000e` and an 82576 with `igb` (not an I219 or
I211 PHY). Select the model without changing the host network backend:

```sh
cargo xtask run --image out/hardware/nanami.img --nic e1000e --storage ahci --gpu std
cargo xtask run --image out/hardware/nanami.img --nic igb --storage ahci --gpu std
python3 tests/network/qemu-http.py --image out/hardware/nanami.img --nic e1000e
python3 tests/network/qemu-http.py --image out/hardware/nanami.img --nic igb
python3 tests/network/qemu-http.py --image out/hardware/nanami.img --nic e1000e --shared-intx
```

The HTTP test uses read-only disk snapshots, restricted user networking and
loopback-only forwarding. Emulation and register-model tests cannot validate
the physical machine's PHY/ME behavior, cable, ACPI wiring or I211 silicon.
Check the logged BDF and perform DHCP/HTTP tests on each physical port.

Validation for this implementation: x86_64 Release image, AArch64 Driver Manager
build and 185 host tests passed. QEMU DHCP/HTTP passed with e1000e and igb INTx
(100 requests each), shared-INTx timer fallback (50 each, including one CPU),
and the existing virtio backend (20). All HTTP runs had zero failed requests.
The Intel INTx runs also exercised multicast during DHCP bootstrap. USB-input
startup without i8042 and storage-flushed ACPI S5 shutdown passed as well.

## Hardware references

- [Intel I210 datasheet, revision 3.7](https://cdrdv2-public.intel.com/333016/333016%20-%20I210_Datasheet_v_3_7.pdf):
  DMA descriptors, queue registers and I210/I211 programming interface.
- [Intel I219 datasheet](https://cdrdv2-public.intel.com/612523/ethernet-connection-i219-datasheet.pdf):
  PCH/PHY initialization and firmware ownership.
- [Intel 82574 documentation](https://www.intel.com/content/www/us/en/collections/products/ethernet/gigabit-controllers/82574-controllers.html).
- [iPXE Intel hardware notes](https://github.com/ipxe/ipxe/blob/master/src/drivers/net/intel.c):
  PCH MAC-only reset, I219 FEXTNVM11 reset-hang workaround and warm-reset buffer sizes.
- [QEMU igb model](https://github.com/qemu/qemu/blob/master/hw/net/igb_core.c):
  validation model; its legacy descriptor implementation is incomplete.
