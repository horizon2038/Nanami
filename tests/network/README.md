# HTTP / network regressions

The subsequent TX queue, RX batch and TCP lookup changes are documented in
[HTTP throughput follow-up](performance.md).

## ARP-resolution loss

The bridged-network failure combined a one-entry ARP cache, learning from
IPv4 packets before destination filtering, and treating an ARP miss as a
disconnected HTTP client. GETs were acknowledged but their responses were
discarded when unrelated multicast displaced the client's neighbor entry.

The fix keeps 32 neighbors, ignores unrelated IPv4 traffic for neighbor
learning, and returns `NET_SERVICE_RESPONSE_WOULD_BLOCK` from TCP_SEND on an
ARP miss. Neither payload nor FIN is consumed. HTTP retains the response/FIN
and retries; only an idle reactor with pending TX uses a 10-ms timer and
backend poll. A full HTTP response table stops dequeuing new requests.
Normal idle still waits on RX notifications. Alter maps this status to EAGAIN.

## TCP receive and NIC backlog

The remaining load failures had separate receive-side problems:

- All TCP connections shared 32 packet slots, but the advertised window was
  always 65,535 bytes. A backend pump can receive 128 frames before returning
  to the application. Overflowed GETs stayed unacknowledged and had to compete
  with new traffic on every retransmission.
- The virtio driver's software queue overwrote its oldest packets when full.
  This discarded unprocessed SYNs and ACKs, including during retransmission
  bursts. Increasing only the TCP connection table did not fix it.

Each of the 1,024 TCP slots now owns a bounded 1,460-byte stream buffer.
Only backed receive credit is advertised. Consumed bytes reopen the window
once at least half a buffer is available (receiver SWS avoidance); a window
update ACK wakes a zero-window sender without waiting for another application
send. Duplicate/overlapping data and zero-window probes are acknowledged
using the current receive sequence and window.
See [RFC 9293 receiver window management](https://www.rfc-editor.org/rfc/rfc9293.html#section-3.8.6).

The ready queue rotates connection indices, not packet-sized entries.
Partial reads preserve the remainder, SHM bounds are checked before consuming
data, and reset removes stale queued bytes before slot reuse. Buffers use
roughly 1.5 MiB of static storage including metadata; they are not placed on
the 256-KiB user stack. No allocation or packet-sized clearing is needed on
receive/read/reset. A slow reader cannot consume another connection's credit,
and a full TCP buffer does not stop the shared NIC/ARP/UDP pump.

Retransmitted SYNs retain the live connection ID/state. FIN is accepted only
after preceding data is received, including data+FIN packets. Connections
with unacknowledged outgoing data/FIN are no longer reclaimed for new SYNs.
A pure ACK in CLOSE_WAIT no longer triggers an ACK loop.

The virtio driver now limits each software-queue fill to its free slots.
Excess completions remain in the NIC used ring until the client drains the
queue; already queued packets are not overwritten. This initial backlog fix
did not change DMA buffer counts or IRQ handling. The throughput follow-up
increases DMA buffering while retaining the lossless backlog/rearm behavior.
Neither change modifies A9N.

This is not a complete TCP implementation: outbound data retransmission
timers, half-open connection expiry and full TIME_WAIT handling are still
outside this change. No bare-metal retest is claimed.

## Host regression tests

From the repository root:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml --test network
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test network
```

The network tests include production ARP/IP, TCP wire/state/receive/send,
HTTP reactor, TCP indices, backend batch pump and virtio queue code, with
IPC/device I/O mocked.
They cover ARP retry, all 1,024 connections receiving before any app reads,
partial reads, window closing/reopening, sequence wrap/overlap, duplicate SYN,
FIN ordering, invalid SHM requests, connection cleanup/isolation, and a full
software queue leaving NIC completions pending. The throughput follow-up adds
slot-generation reuse/wrap, hash-collision cleanup, TX buffer ownership and
out-of-order completion, used/available index wrap, full TX queues, batch bounds,
packet ordering across software/DMA queues, and the legacy backend fallback.

For the entire native suite, add `-- --test-threads=1`: some allocator tests
share a process-global mapping mock.
Release and AddressSanitizer runs passed all 104 correctness tests, including
40 network tests (4 separate timing tests ignored).

## QEMU integration

Build an x86_64 image first. The runner boots a disk snapshot, launches HTTP
through the shell, and runs host ApacheBench. It needs QEMU, Python 3, and ab.
It uses ephemeral loopback ports, restricted QEMU user networking, and a
private virtual hub for injected packets. It neither writes the image nor
sends injected traffic onto the host LAN. Serial logs, ab output and a packet
capture are retained in the printed temporary directory.

```sh
python3 tests/network/qemu-http.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --multicast --evict-arp -n 10000 -c 100
python3 tests/network/qemu-http.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --multicast --evict-arp -n 10000 -c 1000
```

On Darwin, add `--fix-slirp-backlog` for these high-concurrency tests (requires
a C compiler). Upstream [libslirp uses listen(s, 1) for host forwarding](https://github.com/qemu/libslirp/blob/master/src/socket.c).
This caused host-side timeouts even after every GET observed at the guest
had received a response. The option builds a temporary interposer that raises
that backlog to SOMAXCONN **only in the test QEMU process**. It does not change
host sysctls, installed libraries, Nanami or A9N. The application-side workload
is still unmodified `ab -n ... -c ...`; the correction is logged in qemu.log.
Without the option, the runner preserves normal QEMU behavior.

`--multicast` injects unrelated IPv4/UDP multicast every 10 ms.
`--evict-arp` also advertises 40 neighbors every 500 ms, exceeding cache
capacity. The runner counts guest ARP requests for the HTTP next hop.
It fails unless ab reports all requested responses complete with zero failures.
`--keep-alive` and `--smp 1` exercise repeated stream reads and single-CPU
scheduling.

## Validation (2026-09-15)

- Original ARP failure: multicast n=1000/c=100 stopped at 782 completions.
  The ARP fix alone completed that short test.
- With TCP receive fixes and 1,024 connections, but before the NIC fix:
  n=10000/c=1000 stopped at 9,558. All observed GETs had responses; 442 SYNs
  never received a SYN-ACK despite repeated retries. This used the backlog
  correction, so host listener overflow was not the sole remaining problem.
- With both receive fixes, 4 CPUs, multicast plus forced ARP eviction:
  n=10000/c=1000 completed **10,000/10,000, zero failures**. The capture had
  exactly 10,000 SYNs and SYN-ACKs, and no GET without a response. 55 next-hop
  ARP queries exercised resolution/retry.
- Same final image, 4 CPUs, multicast plus eviction: n=10000/c=100 completed
  **10,000/10,000, zero failures**, with 12 next-hop ARP queries.
- 1 CPU, multicast, n=1000/c=1000: **1,000/1,000, zero failures**.
- 1 CPU, Keep-Alive, multicast plus eviction, n=10000/c=1000:
  **10,000/10,000, zero failures**, with 23 next-hop ARP queries.
- The QEMU load results above use the explicit Darwin backlog correction.
  Their timings include software CPU emulation and deliberately heavy ARP
  churn; they are not bare-metal throughput measurements.
- x86_64 and AArch64 native services built. The x86_64 boot image was rebuilt;
  A9N's kernel ELF SHA-256 remained
  `bb98956fa845b877fc3067cbd7c23c9995f2d30f848b78e99455bc488a50bb5a`.
