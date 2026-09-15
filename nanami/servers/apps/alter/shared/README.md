# Alter shared implementation

Linux syscall dispatch remains in `src/personality/linux.rs`. It retains the
existing public interface and routes to private modules in `src/personality/linux/`:

| Responsibility | Modules |
| --- | --- |
| File descriptors and file I/O | `descriptors`, `file_io`, `vectored`, `filesystem`, `metadata`, `paths` |
| Virtual files and devices | `virtual_files`, `terminal`, `pipe`, `input_device`, `framebuffer`, `graphics_session` |
| Network | `sockets`, `socket_io`, `network_wait`, `netlink`, `network_packet` |
| Process images | `fork`, `exec`, `exec_image`, `exec_stack`, `exec_strings` |
| Memory | `vm`, `guest_memory`, `memory_copy` |
| Waiting and system calls | `wait_signals`, `readiness`, `clocks`, `system` |
| Linux ABI values and diagnostics | `constants`, `errors`, `tracing` |

`linux/arch/` still supplies architecture-selected syscall numbers. Native
architecture/register operations remain in their existing architecture layer;
splitting the personality does not change register-writeback behavior.

The parent module imports the private implementation; helpers shared by siblings
use `pub(super)`, not the public API. Public wake/cleanup entry points are
re-exported at the original `personality::linux` paths. Keep new handlers in the
responsible module, leaving only dispatch in the parent.

`src/common/state.rs` defines Runtime's data and initialization. Its implementation
is divided into `state/{processes,descriptors,mappings,waiters,events,io}.rs`.
These are separate inherent `impl Runtime` blocks, with no new runtime
indirection, allocation, or trait dispatch.

Native POSIX I/O selection belongs in `state/io.rs`. Linux `write`, `writev` and
`pwrite` stage data in the negotiated delegated buffer when available; the native
POSIX service retains descriptor and shared-offset ownership. See
[POSIX transfer validation](../../../../../tests/native-performance/posix-io.md)
for the protocol, compatibility boundary and regression tests.
