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
initializes at most eight supported functions and exposes their inventory and
live carrier state through PORT_COUNT/PORT_INFO/SELECT_PORT. The network server
chooses a connected port during DHCP, rechecks late carrier during offer waits,
and tries another connected LAN after an offer timeout. A selection also updates
the DHCP MAC and transaction ID. There is still one active IP interface, not
simultaneous multi-interface routing or migration of established connections.
No QEMU-specific static IP is assigned after a DHCP timeout.

## Hardware ownership and data path

Configuration reads/writes and BAR probing use `pci-server`. BAR0 registers are
mapped UC through Alpha; packet buffers and descriptors use one pinned coherent
x86 DMA allocation per function with full 64-bit addresses. PCI PM transitions use D0,
MSI and MSI-X are disabled, and only the selected device enables RX/TX.
Deselected ports retain their own DMA memory; switching never recycles descriptors
that the old controller might still access. They never compete for the registry name.
The service is registered only after the rings and wakeup source are ready.
PCI Bus Master Enable is set before queue programming/enabling: I210/I211
master-disable holds/resets the DMA queues, so enabling BME afterwards is too
late. Global RX/TX stays off until selection. A failed function's queues are
disabled and its DMA retained while the remaining functions are initialized;
one failed LAN port must not prevent publishing a working port.

e1000e/PCH descriptor and packet no-snoop controls are cleared before BME:
the DMA allocation relies on x86 cache coherence, not merely volatile access.
I219 initializes the required CTRL_EXT, TXDCTL and transmit-arbitration bits
after reset. Queue threshold changes retain required bit 22 and program both
legacy TXDCTL registers identically; they do not invent an igb queue-enable bit
on legacy hardware. Legacy RX disables extended
descriptors without erasing the PCH NFS parsing workaround. PCH filter setup
only clears its 32-word multicast table, not the larger discrete MAC table or
the discrete-only VLAN filter register range.

I219 performs a MAC-only reset, preserves firmware/ME PHY configuration and
applies the PCH reset-hang workaround. It never issues a combined PHY reset or
uses the discrete controller's EEPROM protocol against PCH flash. Discrete
devices with no existing carrier receive a bounded CTRL.PHY_RST hardware reset,
then leave PHY power-down/isolate using MDIC. I210/I211 packet-buffer sizes
are restored for warm-reset startup. Waits are bounded and yield via timers;
NVM, flash and firmware images are never programmed. Platforms needing an
additional ME/ULP exit or PHY-specific firmware workaround need separate
validation; a missing/invalid MAC or timed-out operation declines that function.

Queue zero has 128 RX and 128 TX descriptors with 2 KiB buffers. e1000e uses
legacy descriptors; igb uses advanced one-buffer RX and advanced TX. There is
no TSO, checksum offload, jumbo frame, RSS or multi-queue setup. TX completes
asynchronously, leaves one ring entry empty and never overwrites an in-flight
buffer. Legacy threshold units are explicitly descriptors (GRAN=1), with the
Intel full-descriptor TX writeback policy (WTHRESH=1). Advanced TX uses RS with
WTHRESH=0. Both families write back a single RX descriptor. A
polling-only port cannot depend on another packet or an enabled device IRQ to
flush its first DHCP reply. Short frames are explicitly zero-padded. RX discards malformed,
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
IRQ, or a multi-port provider, uses one 2 ms timer notification and notifies `net-server` on pending
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
python3 tests/network/qemu-http.py --image out/hardware/nanami.img --nic igb --spare-nic e1000e --late-link
```

The HTTP test uses read-only disk snapshots, restricted user networking and
loopback-only forwarding. Emulation and register-model tests cannot validate
the physical machine's PHY/ME behavior, cable, ACPI wiring or I211 silicon.
Check the logged BDF and perform DHCP/HTTP tests on each physical port.

The HTTP harness also validates captured DISCOVER/REQUEST frames: their BOOTP
payload is at least 300 bytes **inside the IP and UDP lengths**, END padding is
zero, and IPv4 checksum/MAC fields are valid. A strict BOOTP-server host test
silently drops shorter requests and requires an actual OFFER/REQUEST/ACK lease;
this catches the earlier 259-byte DISCOVER that permissive QEMU slirp accepted.

On a DHCP send/offer/ACK failure only, the network server asks the optional
owner-scoped REPORT_STATUS control for an Intel DMA snapshot. This reports
queued/completed TX position, next RX readiness, hardware heads/tails, packet
counters and MAC/queue controls without resetting the device, consuming RX or
reading ICR. GPTC/GPRC are clear-on-read hardware counters (deltas since their
previous read/reset). Successful lease acquisition produces no such snapshot;
there is no periodic diagnostic logging.

Specification-audit validation on 2026-11-14: 245 related host tests passed
(one existing ignored), plus 32 AML tests. The final image acquired DHCP with
both e1000e/igb PCI orderings after delayed carrier and completed 20 HTTP requests
per ordering without failures. Reserved-bit/head/tail models cover I211-specific
semantics; the PCH model checks GRAN, both TXDCTL policies and the no-read reset
delay. The failed hardware DHCP path now reports DD/pending TX, RX availability
and good-packet counters instead of equating service acceptance with wire success.

GPU/DHCP regression validation on 2026-11-13: 210 related host tests passed
(one existing ignored test). QEMU acquired DHCP leases after delayed carrier
with both e1000e/igb PCI orderings and completed 20 HTTP requests per ordering;
single-e1000e and virtio each completed another 10. All 60 requests succeeded,
and all four captures passed DHCP wire checks. The previous guest's saved
capture fails the same length check, providing a negative control independent
of the new packet builder. Physical I219/I211 transmission is not validated
by these emulated runs.

Validation for this implementation: x86_64 Release image, AArch64 Driver Manager
build and 185 host tests passed. QEMU DHCP/HTTP passed with e1000e and igb INTx
(100 requests each), shared-INTx timer fallback (50 each, including one CPU),
and the existing virtio backend (20). All HTTP runs had zero failed requests.
The Intel INTx runs also exercised multicast during DHCP bootstrap. USB-input
startup without i8042 and storage-flushed ACPI S5 shutdown passed as well.

## Hardware references

The I219 PHY datasheet is not a complete PCH MAC register specification. The
300/C240 PCH volume 2, chapter 9 establishes the integrated D31:F6 PCI/D0/BME
interface; its public MMIO list does not describe every DMA register. PCH MAC
thresholds and required bits are cross-checked against Intel's e1000e shared
code, not inferred from I211 or QEMU. The relevant family differences are:

| Property | I219/PCH e1000e | I211 (333017 rev. 3.4) |
| --- | --- | --- |
| RX/TX descriptors used | Legacy 16-byte | Advanced one-buffer RX / advanced data TX |
| Threshold fields | Six-bit; GRAN=1 selects descriptors | Five-bit descriptors; bit 24 reserved |
| TX writeback | GRAN=1, WTHRESH=1; required bit 22 in both queues | WTHRESH=0, RS on each descriptor (8.11.15) |
| Queue enable | Global RCTL/TCTL; DCTL bit 25 is not enable | RXDCTL/TXDCTL bit 25, acknowledge before tail (4.5.9/10) |
| Head initialization | Write only while global RX/TX disabled | RDH/TDH read-only; queue enable initializes them (8.9.7/8.11.13) |
| PHY ownership | Retain ME/OEM configuration; no discrete MDIC/EEPROM/reset sequence | Discrete hardware PHY reset and MDIC path |
| MAC speed source | Preserve PCH/ME PHY policy; no forced MAC speed | Internal PHY SPD_IND: ASDE=0, FRCSPD=0, SPD_BYPS=0 (3.5.3.1.2.2) |
| Coherent WB DMA | Clear GCR descriptor/payload no-snoop bits | CTRL_EXT.NS_DIS=1 overrides per-queue no-snoop (8.2.3); no PCH GCR-bit assumptions |

The physical I211 failure trace showed `STATUS=0x00380703` (10 Mb/s MAC)
with `CTRL=0x081c0261` (ASDE enabled), successful descriptor writeback/GPTC
increments, and no good received packets. PHY auto-negotiation in BMCR is
separate from MAC auto-speed detection. The driver now uses direct internal
PHY speed indication and clears a surviving firmware SPD_BYPS override.
This corrects a documented programming error; local TX counters alone do not
prove delivery to a DHCP server or establish the PHY's negotiated speed.
On a DHCP timeout, owner-only diagnostics report MAC speed, pre-filter TPR,
CRCERRS/RXERRC/MPC deltas and receive routing in addition to GPRC/DMA state.
These clear-on-read statistics are not sampled on the successful packet path.

- [Intel 300/C240 PCH datasheet volume 2, chapter 9](https://www.intel.com/content/dam/www/public/us/en/documents/datasheets/300-series-chipset-pch-datasheet-vol-2.pdf).
- [Intel-authored PCH required MAC initialization and coherent DMA policy](https://github.com/torvalds/linux/blob/v6.12/drivers/net/ethernet/intel/e1000e/ich8lan.c).
- [Intel-authored e1000e transmit setup](https://github.com/torvalds/linux/blob/v6.12/drivers/net/ethernet/intel/e1000e/netdev.c):
  full-descriptor writeback and identical TXDCTL policy for both legacy queues.
- [Intel PCIe GbE software manual](https://www.intel.com/content/dam/www/public/us/en/documents/manuals/pcie-gbe-controllers-open-source-manual.pdf):
  descriptor writeback/prefetch policies, legacy GRAN units and immediate RS completion.
- [RFC 1542, section 2.1](https://www.rfc-editor.org/rfc/rfc1542.html#section-2.1):
  BOOTP length checks use IP/UDP lengths and reject payloads shorter than 300 bytes.

- [Intel I210 datasheet, revision 3.7](https://cdrdv2-public.intel.com/333016/333016%20-%20I210_Datasheet_v_3_7.pdf):
  DMA descriptors, queue registers and I210/I211 programming interface.
- [Intel I211 datasheet, revision 3.4](https://cdrdv2-public.intel.com/333017/333017%20-%20I211_Datasheet_v_3_4.pdf):
  master-disable behavior (4.3.1.1) and queue initialization (4.5.9/4.5.10).
- [Intel I219 datasheet](https://cdrdv2-public.intel.com/612523/ethernet-connection-i219-datasheet.pdf):
  PCH/PHY initialization and firmware ownership.
- [Intel 82574 documentation](https://www.intel.com/content/www/us/en/collections/products/ethernet/gigabit-controllers/82574-controllers.html).
- [iPXE Intel hardware notes](https://github.com/ipxe/ipxe/blob/master/src/drivers/net/intel.c):
  PCH MAC-only reset, I219 FEXTNVM11 reset-hang workaround and warm-reset buffer sizes.
- [QEMU igb model](https://github.com/qemu/qemu/blob/master/hw/net/igb_core.c):
  validation model; its legacy descriptor implementation is incomplete.
