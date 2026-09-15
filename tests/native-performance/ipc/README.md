# Deferred IPC notifications

`libnanami::call_port` preserves a bound notification when it interrupts Call,
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
ignored the saved notification. All ten current tests pass after the fix.
This establishes the SDK bug, not that every reported hardware hang has the
same cause. Hardware confirmation with the rebuilt rootfs is still required.
