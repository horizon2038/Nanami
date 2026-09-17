# ext2 dirty buffers and durable synchronization

## Contract

Regular writes now update ext2's dirty buffers, not the storage medium before
each reply. Close is not a durability barrier. Applications that need confirmed
persistence must use `fsync`/`fdatasync`, `syncfs`, or open with `O_SYNC`/`O_DSYNC`.
The latter two flags currently both request full synchronization. Alter forwards
these through POSIX and VFS; fd ownership and open-file-description sharing
remain enforced. Linux `sync` waits for writeback but returns zero, matching
[Linux's syscall contract](https://github.com/torvalds/linux/blob/master/fs/sync.c).
Use the fd-based operations to observe an I/O error.

The initial implementation synchronizes the **whole mount**, including directory,
inode, bitmap and free-count updates. It is stronger (and potentially more work)
than per-file synchronization. It does not introduce journaling or atomic crash
recovery. Power loss before synchronization can lose recent updates and may
require fsck; even later modifications are not protected by an earlier fsync.
Durability requires a device that correctly implements its flush command.

## Implementation

- `ext2-server/src/block_cache.rs`: a 1-MiB payload budget on the heap, indexed by
  physical block number with a B-tree. Entry data uses the mounted block size,
  not a worst-case inline buffer. CLOCK evicts only clean entries. The payload,
  index and entry count are bounded; no unbounded dirty growth or linear lookup.
- `block_io.rs`: reads, including mixed multi-block reads, see dirty buffers.
  Cache misses are batched with explicit shared-buffer offsets. Pressure flushes
  preserve the caller's staged payload/metadata, even on failure.
- `writeback.rs`: ext2 itself owns the flusher. The first dirty request arms a
  one-shot notification for approximately one second later; subsequent writes
  do not postpone it. Clean idle filesystems do not poll or rearm the timer.
  Existing platform timer drivers are unchanged. Without a timer, or if arming
  fails, writeback completes synchronously at the request boundary.
- Dirty blocks are sorted and coalesced up to the block IPC transfer limit
  (currently 16 KiB): data writes, FLUSH, metadata writes, FLUSH. There is no
  per-block FLUSH. Empty phases and already-clean synchronizations issue no I/O.
- Short writes, WRITE failures and FLUSH failures retain dirty buffers and latch
  a mount-wide error. They are not replayed. Further modifications and fd-based
  synchronization return errors; the server must be restarted/remounted after
  fixing the device. This is deliberately conservative, not per-inode errseq.

Block WRITE now means device acceptance, not media persistence. Rebuild the SDK,
ext2 and drivers together. Explicit FLUSH uses USB SYNCHRONIZE CACHE (IMMED=0),
AHCI FLUSH CACHE [EXT], or negotiated virtio-blk FLUSH. A virtio device without
FLUSH or CONFIG_WCE uses the specification's writethrough mode. The demo RAM
block device has no extra cache to flush, but remains inherently nonpersistent.

## Tests

```sh
cargo test --manifest-path tests/native-performance/Cargo.toml -- --test-threads=1
cargo test --release --manifest-path tests/native-performance/Cargo.toml -- --test-threads=1
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test storage --test posix-io -- --test-threads=1
```

The production cache, transfer and writeback modules run against a fake device
with separate accepted and durable images. Coverage includes overwrite
coalescing, dirty read hits, mixed miss runs, pressure/eviction, scratch-buffer
preservation, ordering barriers, short/failed writes, failed FLUSH, sticky errors,
ownership, O_SYNC forwarding and one-shot/fallback behavior.

The freestanding x86-64 guest fixture `tests/alter-linux/writeback.c` can be built
without libc (command in its header). Add the output named `writeback-test` to
`EXTRA_LINUX_BINS` and rebuild the image. Then:

```sh
python3 tests/usb/qemu-hid.py \
  --image spencer/out/x86_64-pc99-release/spencer.img \
  --smp 4 --usb-storage --no-network --bash-smoke --writeback-smoke
python3 tests/usb/check_writeback.py <printed-log-directory>/boot.img --debugfs <debugfs-path>
```

For AHCI omit `--usb-storage`; for virtio use `--virtio-storage` instead.
The harness writes only a private regular-file clone, uses `cache=writeback`,
and stops guest CPUs after the fixture succeeds. It retains that clone for
offline inspection; no host disks, mounts or physical USB devices are used.
The guest tests read-your-writes, dup/closed/invalid fds, all four sync syscalls,
O_SYNC/O_DSYNC, F_SETFL preservation, parent-directory synchronization (including
Alter's virtual root overlay), and 2-MiB pressure across indirect blocks. Offline debugfs then checks all
2 MiB plus the separate O_SYNC payload, not the guest's cached view.

This does not simulate a physical USB controller's volatile cache or power loss.
Actual hardware latency and power-loss behavior still require hardware tests.

Validation on 2026-09-16:

- 242 host correctness tests passed in debug/release (5 opt-in benchmarks ignored);
  storage/POSIX/Alter sync tests passed 47 cases under AddressSanitizer.
- The separate block startup test passed 7 cases. All native Rust services/apps
  built for x86_64 and AArch64; the x86_64 boot image was rebuilt.
- USB3/4 CPU and virtio/4 CPU passed the original sync/pressure fixture and offline
  content checks. USB2/1 CPU and AHCI/1 CPU passed the extended fixture above and
  the same offline checks. AHCI's first attempt typed before USB input attached;
  the harness now waits for input readiness. The directory test exposed the
  virtual-root fsync rejection, which was corrected and retested.
- USB3/4 CPU/PIT Doom save/load reduced storage operations to 8 WRITE / 2 FLUSH /
  5 READ, compared with 174 / 174 / 250 before writeback. Subsequent untraced bash
  input/writes, cursor movement and the desktop clock continued working.
- No kernel or loader source changes were made for writeback. AArch64 runtime,
  physical USB save latency and physical power-loss recovery remain untested.
