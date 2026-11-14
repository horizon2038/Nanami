# HTTP throughput follow-up

## Static settings / file-backed HTTP follow-up (2026-11-14)

See [IPv4 configuration and HTTP serving](../../nanami/docs/network.md) for
the new CLI, GUI, persistence and file/port options. The current changes also
enable per-port Intel INTx with bridge swizzling, select I211's EITR[0] rather
than legacy ITR, reduce pending-response retry from 10 to 1 ms, preserve partial
TCP writes, and remove the redundant empty RECV_BATCH IPC after a short batch.
The latter is checked by a production-code host test: a one-frame batch needs
one receive IPC instead of two, and a later arrival still drains normally.

Same host, QEMU software emulation, 4 CPUs, primary `igb` with an earlier
disconnected `e1000e`, delayed carrier, default 363-byte HTML and no keep-alive:

| Image / sample | `ab -n 500 -c 1` RPS | `ab -n 500 -c 16` RPS |
| --- | ---: | ---: |
| Before these changes | 1,116.69 | 1,189.55 |
| Initial implementation, before short-batch drain optimization | 958.92 | 1,067.52 |
| Final implementation | 1,001.07 | 1,376.67 |

All samples completed 500 requests with zero failures. These are individual,
short samples, not confidence intervals or isolated component speedups. The
final image improves the concurrent sample but not the serial sample; there
is no consistent overall throughput gain demonstrated here. The physical
approximately 50-rps symptom was not reproduced in QEMU, so its resolution is
**not hardware-verified**. Compare a longer, identical workload on that machine,
with the same content, concurrency and keep-alive policy, for example:

```text
ab -n 10000 -c 1 http://192.168.10.101:8080/
ab -n 10000 -c 16 http://192.168.10.101:8080/
```

Logs are under `out/hardware/network-http-before-c*.log` and
`out/hardware/network-http-optimized-c*.log`. Additional integration checks
cover the native GUI, configuration while DHCP is unanswered, cold-restart
persistence, PCI bridge IRQ routing, and a byte-exact 209,160-byte file served
on port 8080. Those are correctness checks, not physical throughput evidence.

## Earlier queue/index work

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

Reproduce after building with `cargo xtask build --rebuild-rootfs`:

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
