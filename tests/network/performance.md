# HTTP throughput follow-up

This follows the receive-loss fixes in [HTTP / network regressions](README.md).
It changes user-space net-server, virtio-net and their SDK transport only;
A9N, interrupt/register writeback and the HTTP response format are unchanged.

## Changes

- **Pipelined TX:** previously each SEND copied into one DMA buffer, kicked
  the NIC and spun for that packet's completion before replying. Up to 64
  descriptor chains now have their own header and payload storage. SEND
  replies after copying/publishing the packet. Only a full queue waits for
  completion. Used descriptor IDs, not submission order, decide which buffer
  can be reused. A timeout or notification error does not release in-flight
  storage. TX-only completion interrupts are suppressed; RX IRQ rearming is
  unchanged. Smaller negotiated queues use fewer chains.
- **Batched RX:** feature negotiation enables RECV_BATCH with up to 32
  independent frame records per IPC; unsupported backends retain the old
  single-frame API. The driver drains the software queue first, then copies
  NIC buffers directly into SHM and kicks RX once per batch. RX DMA capacity
  grows from 8 to at most 64 buffers. Batch bounds are checked before consuming
  packets. RX batch storage is disjoint from the TX workspace, so constructing
  an ACK cannot overwrite unprocessed frames.
- **Indexed TCP:** inbound tuples use a fixed chained hash index instead of
  scanning all 1,024 connections. A free bitmap replaces ordinary allocation
  scans. Nonzero connection IDs encode slot and generation; SEND directly
  checks that slot, its owner and exact ID. Reuse invalidates the previous ID.
  IDs remain nonzero 32-bit values for existing metadata consumers. The
  default-ID compatibility path and full-table FIN_WAIT2 reclamation still
  scan; neither is the ordinary explicit-ID HTTP send/allocate path.

Queue ordering follows the [Virtio split-ring specification](https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html):
payload/descriptors precede available-index publication, and used completion
is acquired before reading/reusing device-owned buffers. This is still the
legacy PCI driver, not a modern PCI/MMIO virtio implementation.

The extra bounded memory is deliberate: the DMA request grows from 48 KiB
to 228 KiB (before the allocator's rounding), and TCP indices occupy 10,368
bytes. This is not additional per-packet allocation. RX batching uses the
already allocated 128-KiB backend SHM. The rebuilt x86_64 net-server main
stack frame is 102,616 bytes, below its 256-KiB stack; TCP payload buffers
remain in static storage.

## Measurements (2026-09-15)

Same host, QEMU 11.1.1 x86_64 software emulation, 4 guest CPUs, `-cpu max`,
the same HTTP response and `ab -n 10000 -c 1000` workload, no Keep-Alive
and no injected traffic. Every run below completed all 10,000 requests with
zero failures. Runs were sequential, with a fresh VM per sample. Darwin's
test-only `--fix-slirp-backlog` was enabled for every image, including baseline.

| Cumulative change | RPS | Time (s) |
| --- | ---: | ---: |
| Completed-request baseline, before this follow-up | 347.46 | 28.780 |
| Pipelined TX / suppress TX-only interrupts | 591.04 | 16.919 |
| Also batched RX / more RX buffers | 1,019.53 | 9.808 |
| Also TCP indices | 1,059.57 | 9.438 |

These are end-to-end QEMU timings, not bare-metal results or CPU-profile
percentages. The staged RX row includes both batching and buffer capacity;
it does not isolate their individual contributions. Likewise, the relatively
small final increment is not proof of an exact isolated hash-index speedup.

A second baseline/final pair measured **313.25 / 907.42 RPS** (31.923 / 11.020 s),
again 10,000 completions and zero failures in each run. Across the two pairs,
the measured improvement is approximately **2.9–3.0 times**. Host scheduling
and emulation variance remain visible; no bare-metal speedup is claimed.

Reproduce after building with `scripts/build-image.sh`:

```sh
python3 tests/network/qemu-http.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --fix-slirp-backlog -n 10000 -c 1000
```

On non-Darwin hosts omit `--fix-slirp-backlog`. Do not compare the RPS of
`--multicast --evict-arp` stress runs with this non-injected baseline.

## Validation and limits

- Release and AddressSanitizer: 104 host correctness tests passed, including
  40 production-code network tests. Four unrelated timing tests stay ignored.
- x86_64 and AArch64 target-selected native services build successfully.
  AArch64 is compile coverage only, not a NIC hardware validation.
- 4-CPU multicast / neighbor-cache-pressure load: 10,000 / 10,000 completed,
  zero failures. No next-hop ARP query occurred in this sample; it does not
  independently prove the ARP retry path was exercised (host tests cover it).
- Final image, 1 CPU and multicast, n=1000/c=1000: 1,000 / 1,000 completed,
  zero failures. With Keep-Alive and neighbor-cache pressure, n=10000/c=1000:
  10,000 / 10,000 completed, zero failures. Neither sample generated a
  next-hop ARP query.
- The normal explicit-ID SEND lookup is inlined into the release binary:
  slot masking plus active/owner/exact-ID checks, with no 1,024-entry fallback.
- A9N kernel ELF SHA-256 remains
  `bb98956fa845b877fc3067cbd7c23c9995f2d30f848b78e99455bc488a50bb5a`.

NIC SEND still uses one IPC per transmitted frame; this work batches RX, not
TX IPC. Full TX queues retain bounded waiting rather than inventing a
zero-progress retry status after a partially consumed TCP send. Outbound TCP
retransmission timers, half-open expiry, full TIME_WAIT and the limitations
described in the receive-fix report are not implemented by this optimization.
