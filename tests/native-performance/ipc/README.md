# Deferred IPC notifications

`nanami_runtime::call_port` preserves a bound notification when it interrupts Call,
then retries the RPC. The kernel has already consumed the notification; it is
now held in the SDK's `PENDING_NOTIFICATION`, not in the kernel notification
port. Previously only `notification_wait`/`notification_poll` consumed this
saved state. Service receive loops could block despite a saved IRQ, never
acknowledging/re-enabling its interrupt line. For the timer server this can
also stop USB polling and the desktop clock.

Service receive now delivers saved notifications before blocking. Reply-Receive
must first complete the outstanding reply: only when a saved notification exists
does it use Reply and return a notification event without receiving. On reply
failure the saved flags are restored. The ordinary path retains the combined
Reply-Receive syscall; checking for saved flags uses an atomic load, with an
exchange only when nonzero. Fault-context payload construction is unchanged.

The tests compile the production `ports.rs`, `service.rs` and event types against
a mock ABI and real `a9n-types::MessageInfo`. They start at the boundary where
Call has saved a notification; they do not simulate kernel IRQ delivery or run
the Call retry loop. An unexpected blocking receive fails immediately.

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test ipc-notifications -- --test-threads=1
```

Coverage: delivery without a second IRQ, reply-before-event ordering, unchanged
combined ordinary/fault replies, unchanged register payload, retaining flags on
reply/validation failure, badge coalescing, unrelated notification descriptors,
no duplicate delivery, and preserving the next incoming kernel message.

Before the fix, five of the initial seven tests failed because service receive
ignored the saved notification. All ten original cases pass after the fix.
This establishes the SDK bug, not that every reported hardware hang has the
same cause. Hardware confirmation with the rebuilt rootfs is still required.

An additional 13 cases cover the runtime's standalone normal Reply and the
production Timer server's opt-in first-reply comparison. The standalone helper
does not receive or consume saved notification flags. The following existing
Receive delivers them before consuming a queued kernel request. Reply errors
retain the flags; Receive errors do not cause a second reply.

The Timer comparison requires both `NANAMI_USB_INIT_TRACE=1` and
`NANAMI_TIMER_SPLIT_REPLY=1`. Only its first selected 10 ms main-slot interval
uses Reply followed by Receive; other/subsequent replies retain the original
combined operation. Tests assert the post-Reply and pre-Receive logs' order,
error propagation, no retries, and unchanged reply words. Run with both flags
enabled, tracing disabled, splitting disabled, and invalid/unset flag values
to exercise the compile-time gates. This tests Nanami control flow against mock
syscalls, not the physical machine's reply delivery or scheduling.
See [the image build and comparison procedure](../../usb/README.md#first-timer-reply-path-comparison-experimental).

## Production-kernel source review: bound-notification dispatch

The hardware experiment returned from standalone Reply but did not show USB's
registration return. Rather than extending serial traces, a separate host
harness now links the actual A9N IPC port, notification port, process manager
and scheduler. Only HAL register/context operations are mocked. Access modifiers
are relaxed in the fixture to prepare queues; production method bodies are not
replaced. These tests do not exercise x86 assembly, IRQ controllers or MMIO.

```sh
sh tests/kernel-wakeup/run.sh
```

Before the fix, four of the 16 cases failed, two with SMP enabled and two
without SMP. The bound IPC Receive wakeup set its receiver READY and enqueued
it, but left an idle CPU idle and did not immediately dispatch a newly-ready
higher-priority receiver. Ordinary notification Wait wakeups did dispatch the
receiver. The source difference was
`notification_port::try_wake_binded_process` calling `mark_scheduled`, while
the explicit Wait path called `mark_scheduled_with_preemption`.

The remaining 12 controls pass: ordinary Wait wakeups from idle and a busy CPU;
equal/lower-priority bound wakeups not preempting a busy CPU; and Timer-priority
30 standalone Reply followed by Receive dispatching its priority-28 USB caller,
both with and without a pending Timer notification. A periodic notification to
an already-ready caller preserves its original normal reply payload.

The same bound-wakeup change was first validated in a temporary source copy,
then applied to A9N with user approval on 2026-10-13. At that point, A9N's
version was 0.3.7 with stage `ntfn-switch`. The current version is 0.3.8 with
stage restored to `smp`. All 16 cases pass against the actual patched source.
The existing priority check and remote-core reschedule IPI path are retained;
this fix adds no new affinity, priority, IRQ or quantum policy. An alternate
source copy can still be checked with
`NANAMI_TEST_NOTIFICATION_SOURCE=/absolute/path/notification_port.cpp` before
the same command. This is a demonstrated dispatch defect/latency dependency,
not proof that it alone caused the physical 30-second post-Reply stall. The
standalone Reply/Receive controls did not reproduce that stall. Do not treat
the host test result as a verified physical-machine fix or work around it by blindly
changing driver affinity/priorities or spinning on Receive.

Patched-image validation on 2026-10-13: both normal-affinity/untraced and
same-core/init-trace/split-Reply images built successfully. Each snapshot QEMU
run used four CPUs, 8 GiB, USB2 BOT root storage, high PCI MMIO, HPET and no
PS/2 controller. Both booted to Desktop without injected mouse input, accepted
keyboard-only Shell commands, wrote to the disposable USB snapshot, launched
HTTP before/after HID hotplug, moved the cursor, closed Shell with the replugged
mouse and released held Shift on detach. Serial output in both images reported
`0.3.7-ntfn-switch+x86-64-Release-20261013`; the same-core image also reached
`timer-interval.end ok=true` after standalone Reply/Receive. Native Nanami
tests passed 504 cases (7 ignored), and A9N's direct-schedule suite passed 18
cases with ASan/UBSan. Physical hardware was not available for validation.

The normal image was built at `out/ntfn-switch.img` using a fresh
`out/ntfn-switch-rootfs.img`, with diagnostic flags set to zero. For the same
conditions as the last reported physical log, use the separate diagnostic
outputs (rebuilding these staging outputs replaces their contents):

```sh
NANAMI_SAME_CORE_DIAGNOSTIC=1 \
NANAMI_USB_INIT_TRACE=1 NANAMI_TIMER_SPLIT_REPLY=1 \
NANAMI_PIPELINE_TRACE=0 NANAMI_USB_WAIT_TRACE=0 \
NANAMI_BOOT_PC_TRACE=0 NANAMI_BOOT_IO_TRACE=0 NANAMI_USB_IO_TRACE=0 \
NANAMI_FB_PROFILE=0 \
cargo xtask build --rebuild-rootfs \
  --rootfs out/ntfn-switch-same-core-rootfs.img \
  --output-image out/ntfn-switch-same-core.img
```

Snapshot validation commands:

```sh
TMPDIR=/private/tmp python3 tests/usb/qemu-hid.py \
  --image out/ntfn-switch.img \
  --usb-storage --usb2 --memory 8G --smp 4 --high-mmio
TMPDIR=/private/tmp python3 tests/usb/qemu-hid.py \
  --image out/ntfn-switch-same-core.img \
  --usb-storage --usb2 --memory 8G --smp 4 --high-mmio
```

Physical result subsequently supplied by the user: the 0.3.7 `ntfn-switch`
same-core/split-Reply image still stopped after `reply-split.returned ok=true`
and `receive-enter path=split`, before USB's `timer-interval.end`. Keyboard,
storage and mouse enumeration had completed. The bound-Receive dispatch fix
therefore did not resolve the reported physical stall. The USB caller's normal
Timer RPC uses the separate `BLOCKED_REPLY` path, not a bound-Receive wait.

Source review also confirms that standalone Reply with no outstanding caller
returns success, so the Reply return alone does not establish a USB READY
transition. Receive can return a pending notification without blocking; its
entry log alone does not establish a scheduler selection or successful x86
context restoration. The actual reply links, ready queue, selected process and
IRQ/return state on the physical machine remain unobserved.

Additional host review corrected stale quantum-reset expectations in
`tests/alter-linux/ipc-fastpath-tests.cpp`: fast Reply-Receive must preserve a
caller's two remaining ticks, including a higher-priority fallback selection.
The production kernel was not changed in this follow-up. Both SMP modes passed
the full IPC fixture under ASan/UBSan (20,000 FIFO steps, 1,000 round trips and
0..255-word Call/Reply/Reply-Receive payloads per mode); external-IRQ completion
and LAPIC start-order fixtures also passed under ASan/UBSan. These HAL-mocked
results still do not reproduce or explain the physical stall.

```sh
SANITIZE=1 sh tests/alter-linux/test-ipc-fastpath.sh
```
