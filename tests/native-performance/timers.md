# Deadline-driven user-space timer

The HPET server separates its clocksource (the free-running main counter) from
its clockevent (timer 0's one-shot comparator). Reading the clock does not create
a periodic timer. The service reports nanosecond **units**, not a claim of 1 ns
hardware resolution; actual resolution is the HPET capability's period.

- Only the earliest pending deadline is programmed. An unchanged deadline does
  not rewrite the comparator. With a 64-bit main counter and an empty queue, the
  comparator is disabled while the clock keeps running.
- Comparator programming rounds up to hardware cycles, includes a 10 us guard,
  and checks the counter after writing. A missed equality comparison is retried
  with more lead time rather than waiting for a counter wrap. Persistent failure
  is reported explicitly.
- Split 64-bit counter reads use high/low/high validation. A 32-bit counter is
  software-extended and needs maintenance interrupts at most 2^30 cycles apart;
  servicing it must not be delayed by a full wrap. A 32-bit comparator uses
  bounded steps for distant deadlines even with a 64-bit clock.
- The bounded 512-entry min-heap inserts and expires in O(log n), and reads its
  minimum in O(1). Alarm replacement searches the active heap; notification
  failure filters/rebuilds it. Neither is a per-tick scan. Expiry no longer uses
  a 512-entry temporary notification array.
- Periodic subscriptions preserve phase and coalesce missed intervals. Existing
  independent sleep/interval requests keep their original semantics.

`TIMER_SERVICE_REQUEST_ALARM_TICKS` (0x4005) adds one replaceable absolute alarm
per client notification slot. `arg0` is the deadline in `monotonic_ticks` units,
`arg1` the slot, and `arg2` is 1 to arm or 0 to cancel. Already-delivered
notifications cannot be recalled, so clients must recheck the clock. Replacing
an alarm does not cancel independent sleep/interval requests on that slot.

Alter/Linux samples the clock for `clock_gettime`, uses absolute deadlines for
`nanosleep`, and arms only the earliest sleeper or mapped-framebuffer presentation
deadline. Reading the clock alone no longer starts a permanent 10 ms interval.
Framebuffer presentation retains its 60 Hz phase, independent of clock queries.
Guest register-return handling is unchanged.

PIT remains the 100 Hz periodic fallback: it has no independent clocksource for
measuring time while stopped. Its resolution/coalesced-IRQ limitations are not
claimed to be fixed here. AArch64 remains unsupported pending a platform device
driver; no EL0 CNTV access is introduced. No kernel/loader changes are required.
This is not whole-system tickless operation: requested intervals (for example,
USB maintenance) and the kernel's own scheduling timer still exist.

## Tests

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml \
  --test timers --test hpet --test linux-clocks -- --test-threads=1
```

The tests include the production heap, timer dispatch, HPET backend with mock
MMIO, and Alter clock/sleep implementation. They cover saturation, capacity,
replacement/cancellation, missed periods, notification errors, 32-bit wrap,
one-shot disable/rearm, fractional clock conversion, stale notifications and
framebuffer deadlines. Mock MMIO cannot emulate a delayed chipset comparator
write or an SMI; QEMU/real hardware testing is still required.

Build the freestanding guest fixture with LLVM/LLD:

```sh
clang --target=x86_64-linux-gnu -O2 -nostdlib -static -fno-builtin \
  -fno-stack-protector -fuse-ld=lld -Wl,-e,_start,--image-base=0x400000 \
  -o out/timer-test tests/alter-linux/timer.c
```

Include `./out/timer-test` (and `./out/writeback-test` for persistence checks) in
`EXTRA_LINUX_BINS` when rebuilding the image. Then:

```sh
python3 tests/usb/qemu-hid.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --smp 4 --hpet on --usb-storage --bash-smoke --no-network \
  --timer-smoke --writeback-smoke
```

Repeat with `--smp 1 --hpet off` for the PIT fallback. The harness uses a private
disk copy and prints its log directory. Run `tests/usb/check_writeback.py` on the
retained `boot.img` to verify persisted bytes without mounting it. Guest timing
checks are functional, not a latency benchmark (the fixture runs with tracing).

Validated with QEMU USB3/4 CPUs/HPET and USB2/1 CPU/PIT: both completed the timer
and writeback fixtures, including offline verification of the persisted
2 MiB + 8 KiB payloads. HPET/4 CPUs also completed Doom Save/Load (8 writes,
2 flushes) and subsequent untraced bash input/write cycles. Host debug/release
suites passed 260 tests (5 benchmarks ignored); the 24 timer-related tests also
passed AddressSanitizer. x86_64 and AArch64 user-space builds passed. Physical
hardware timing/performance has not been measured here.

No journal is introduced. Existing ext2 dirty-cache writeback remains an ordinary
one-shot client; fsync still waits for the block backend's explicit flush.
