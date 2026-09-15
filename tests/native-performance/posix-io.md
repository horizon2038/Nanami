# Alter / POSIX delegated writes and module split

## Scope

The Linux personality entry point shrank from 9,324 to 671 lines; its handlers
and helpers are grouped by responsibility in private modules. The existing
Runtime methods moved from the 1,617-line `state.rs` into six modules, leaving
556 lines of definitions/initialization. POSIX's 836-line `main.rs` is now 186
lines: startup, dispatch and logging. Its filesystem, I/O and buffer-attachment
handlers live in separate modules alongside the existing process/fd/path/env
modules. The SDK's POSIX I/O calls and ext2's file-I/O handlers were also split
out of their service entry points.

Function-body comparison, ignoring visibility and formatting, found only three
changed pre-existing Linux functions (`sys_write`, `sys_writev`,
`sys_positioned_io`) among 321. All 78 pre-existing Runtime methods/helpers
were preserved. The changes to those three handlers select the write buffer
and request route below. Register-writeback logic is unchanged.

## Transfer path

Previously a regular file write went through:

`guest → Alter/POSIX shared buffer → POSIX/ext2 shared buffer → ext2 storage I/O`

With the previously negotiated direct-I/O buffer it now goes through:

`guest → Alter/ext2 delegated shared buffer → ext2 storage I/O`

Control still passes through POSIX, which validates the fd and maintains the
shared open-file-description offset. The optimization removes **one full-payload
copy** at POSIX, not every copy and not an IPC hop. No additional shared buffer
is allocated for writing: the direct-read buffer is reused. A full 64-KiB buffer
can be transferred instead of the conventional scratch capacity of 64 KiB − 512.
`writev` retains its existing batching and partial-progress behavior.

- `POSIX_REQUEST_WRITE_DIRECT` / `POSIX_REQUEST_PWRITE_DIRECT` use the same
  argument layout as write/pwrite, but offsets refer to the delegated buffer.
- `VFS_REQUEST_WRITE_DELEGATED` packs delegate ID and input offset as delegated
  read already does. ext2 checks both delegation ownership and handle ownership,
  then validates the buffer range with checked arithmetic.
- Conventional clients still use the existing requests and copy path. Alter
  chooses that path when direct-buffer attachment is unavailable. Rebuild Alter,
  POSIX, ext2 and the SDK together: successful attachment to an older service
  does not negotiate support for the newly added write request codes.
- A failed write is never retried through the other route: storage may already
  have committed part of it. Errors do not advance POSIX's shared offset.
- Positioned I/O leaves the shared offset untouched. Existing append behavior,
  including Linux-style append with pwrite, is preserved. The append fstat IPC
  remains; this change does not add cross-client atomic-append guarantees.
- Read/write success updates only the offset field. Read-only path/environment/
  cwd operations borrow the session instead of explicitly copying it. These
  source-level copy removals are not a separate measured speedup claim.

An x86_64 release disassembly confirms that `handle_write_with_mode`'s direct
branch calls `vfs_write_delegated` without passing through `memcpy`. The
conventional branch retains its payload `memcpy` before `vfs_write`.

## Regression tests

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test posix-io
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test posix-io
```

The 16 tests include the production Alter Runtime I/O methods, SDK POSIX I/O
helpers, POSIX I/O handlers/state types and ext2 file-I/O handlers. IPC, session
lookup and block storage are mocked. Tests cover direct and conventional routes,
nonzero buffer offsets, full-capacity transfer, short writes, dup-shared offsets,
pread/pwrite, append, invalid descriptors, overflow, missing/inactive/unowned
delegates, separate handle ownership and no retries after errors. Direct-handler
tests deliberately leave POSIX's intermediate-buffer pointers null.

Validation on 2026-09-15:

- All 38 host correctness tests passed in release and AddressSanitizer runs
  (the two pre-existing timing benchmarks remain ignored).
- All selected native Rust apps/services built in release for x86_64 and AArch64.
- QEMU with one and four CPUs passed `linux-syscall-smoke`, `glibc-true`, and
  `glibc-regression`, including vectored I/O and dynamic-loader coverage.
  Tests used snapshot disks, temporary firmware variables and no networking.
- A9N source was unchanged; its existing kernel ELF remained byte-identical:
  SHA-256 `bb98956fa845b877fc3067cbd7c23c9995f2d30f848b78e99455bc488a50bb5a`.

Physical-hardware throughput and AArch64 runtime behavior were not measured.
Copy elimination is verified; no end-to-end speed multiplier is claimed.
