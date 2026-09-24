# Alter/Linux poll and clock_gettime review (2026-09-17)

This changes user space only. A9N, the fault/reply ABI, and syscall register
writeback are unchanged. It does not expose HPET, TSC, or a virtual timer to
guests. The follow-up also updates the supplied `doomgeneric/` input backend.

## Transfer path

Even a 16-byte timespec previously passed through Alpha's per-chunk temporary
MAP/COPY/UNMAP sequence. `poll` did this twice, to read and write the pollfd
array. The framebuffer itself is not being remapped by these syscalls.

For resident, distinct source/destination frames and buffers within one page:

| Guest call | Previous MAP / UNMAP count | Cache-hit MAP / UNMAP count |
| --- | ---: | ---: |
| clock_gettime | 2 / 2 | 0 / 0 |
| poll | 4 / 4 | 0 / 0 |

Alpha now keeps a bounded 16-page copy window at `0x66000000`, separate from
image staging and zero-fill windows. Source and destination share the cache;
reversing a poll transfer does not force remapping. Replacement cannot evict
the current copy's source. This is a working-set cache, not a page/process
limit; misses still use the original mapping machinery. Cold page faults,
lazy materialization, eviction, and invalidation are not zero-cost.

Each transfer still checks authority and resolves the current guest mapping.
`munmap`, shared-memory release/rollback, exec, reap, and failed-spawn cleanup
invalidate that process's entries before caps or backing memory are recycled.
Retained-window UNMAP errors are propagated, including IllegalOperation;
they must stop the destructive operation. Failed MAPs remain tracked for
cleanup but cannot produce cache hits. Other staging windows keep their old
cleanup behavior. Adding another frame-cap destruction path requires the
same invalidation rule.

The BootstrapVmSpace AVL allocator also scanned its node array from the start
on each insertion, including every temporary mapping. A free-list plus an
unused-tail cursor now makes node allocation/recycling O(1); AVL lookup and
tree maintenance remain O(log n). No extra heap allocation is introduced.

One aarch64-macOS Release host run, 100,000 insert/remove pairs after filling
the indicated number of nodes (excluding MAP/UNMAP and IPC):

| Retained nodes | Before, ns/pair | After, ns/pair |
| --- | ---: | ---: |
| 1,024 | 743 | 102 |
| 4,096 | 2,213 | 124 |
| 9,216 | 4,859 | 132 |
| 14,000 | 7,286 | 131 |

These are host bookkeeping timings, not guest syscall latencies or an FPS
speedup. The source can be compared without reverting the worktree using
`NANAMI_STATIC_AVL_SOURCE` and the ignored
`bootstrap_temporary_mapping_churn_benchmark` test.

## Timer and readiness

- Every clock request still samples the hardware. Expiry processing reuses
  that event's sample, never a previous event's value.
- HPET's unchanged-comparator path uses that same observation. The ordinary
  time-query path with an unexpired unchanged alarm goes from three counter
  samples to one (nine to three MMIO reads on a stable 64-bit counter).
  The high/low/high consistency check, 32-bit wrap maintenance, and fresh
  before/after samples when programming a comparator remain intact.
- The nanosecond-frequency path avoids generic 128-bit timespec scaling.
  `llvm-objdump` confirms it bypasses `__udivti3`. The compiler still merges
  the seconds division with the generic path; this is not a claim that all
  divisions disappeared.
- `poll` and the readable `select` scan drain input once per scan, on the
  first interested evdev FD, rather than once per input FD. No-input and
  zero-event scans do not drain queues. Negative poll FDs are ignored,
  regular-file readiness combines requested read/write flags, and oversized
  arrays are rejected instead of silently truncated.

## Event-driven readiness follow-up

- poll/ppoll/select/pselect6 share the FD scanner and wait state. Zero timeout
  returns immediately without timer IPC. Finite waits use the existing one-shot
  alarm, shared with nanosleep and framebuffer deadlines; infinite waits do not
  create a polling timer.
- Input, terminal and network notifications and pipe mutations wake only waiters
  interested in that source. Guest interest arrays are snapshotted before parking
  and kept separate from the reusable IPC scratch buffer.
- Terminal readiness checks actual queued raw input or a complete canonical line.
  Polling a canonical terminal retains the line for the later read. Network
  readiness is an owner-scoped, non-consuming service query, not a receive/accept.
  Empty sockets and terminals no longer report unconditional readable status.
- Pipe EOF/error and invalid/negative descriptors, read/write readiness together,
  select exception sets and nfds bounds are handled. select returns the count of
  bits across the output sets. The supported FD/interest limit is 64.
- gettimeofday and REALTIME-family queries use one RTC epoch anchor (RTC assumed
  UTC, initial precision one second), advancing from fresh monotonic samples.
  MONOTONIC-family queries remain fresh as well. CPU-time and TAI IDs return
  EINVAL: the necessary CPU accounting/TAI offset is not available.
- Doom no longer polls during DrawFrame or before removing an already queued key.
  evdev reads contain up to 32 events instead of one event per syscall. Input
  timing remains driven by I_GetEvent; clock samples/game time are not cached.
  Alter also bounds batched evdev output by its actual scratch-buffer size.

## Remaining findings / measurement boundaries

- Signal delivery remains unimplemented. ppoll/pselect6 accept a null or empty
  mask (SIGKILL/SIGSTOP cannot be blocked); an effective nonempty mask now returns
  EOPNOTSUPP instead of silently ignoring it. Signal-driven EINTR/restart semantics
  and full Linux socket behavior are not claimed. The network stack's existing
  active-connect timeout/reset limitations remain. See the
  [poll semantics](https://man7.org/linux/man-pages/man2/poll.2.html) and
  [clock definitions](https://man7.org/linux/man-pages/man3/clock_gettime.3.html).
- Doom uses zero-timeout poll, so blocking support alone is not an FPS fix.
  Its input batching/duplicate-poll removal requires rebuilding the game binary,
  not just Alter. The test build is in
  `out/alter-glibc-root/bin/doomgeneric-fbdev`; the user's `bin/` copy is untouched.
- Alter's syscall trace emits synchronous terminal output per call. It
  identifies call frequency but perturbs the timings being investigated.
  Honoka/fb profiling likewise adds timer IPC and reports wall time including
  preemption; presentation counts are not Doom-generated frame counts.
- Warm copy mappings still require the Alter-to-Alpha request, the copy, and
  mapping/permission lookup. Clock queries still require timer IPC.
- Compare hardware with syscall tracing disabled and `NANAMI_FB_PROFILE=0`.
  Neither these host timings nor a passing QEMU run establish real-machine
  FPS, startup time, USB latency, or framebuffer bandwidth.

## Validation

- Native Release suite after the follow-up: 334 passed, 7 timing tests ignored.
- Follow-up readiness/clock/network tests under AddressSanitizer: 62 passed.
  The initial AVL/copy-window/shared-memory/timer checks passed 57 tests with
  3 timing tests ignored before this follow-up.
- Cache tests cover 10,000 bidirectional transfers after two initial maps,
  bounded eviction with a pinned source, descriptor reuse, and failures both
  during and after installing a mapping. Shared-memory tests use the real
  release transaction and check that mapped caps/backing are not freed.
- x86_64 Release image and AArch64 Alpha/Alter/Linux compile checks; AArch64
  runtime and real hardware have not been tested.
- QEMU with HPET enabled and 1/4 CPUs: real poll/ppoll/select pipe wakeups and
  timeouts, empty terminal readiness, realtime vs monotonic, static syscall smoke,
  glibc startup,
  and glibc regression (including fork/exec and fixed/private mappings).
  Runs use snapshot disks, a temporary rootfs, no networking, and temporary
  firmware variables. The user's `out/ext2.img` is not rewritten.
- The real Doom backend input fixture also passes on 1/4 CPUs: 40 ordered key
  press/release events, two batched reads, three polls including the empty check,
  and no poll/read while dequeuing local keys.
- A9N kernel ELF SHA-256 before/after:
  `4a91bcde0b7618ea705098338a646b92d18890be17dbe1c12bba9b467ff4ac9e`.
