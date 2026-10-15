# Performance monitor

Run `performance-monitor` in Shell. The native Honoka window has Memory,
Processes and System tabs; all values come from existing Nanami/timer APIs.
No A9N ABI extension is required.

- Memory: used/free/total physical allocator bytes, percentage, change since the
  previous sample, and min/max/average over the retained history. Small changes
  are displayed in bytes/KiB, rather than rounded away in MiB.
- Processes: current alive and exited-but-not-reaped counts, their histories and
  changes. Alive includes sleeping/blocked tasks; this is not a runnable-task or
  lifetime-process counter. The two line colors have explicit labels.
- System: running kernel/Nanami versions, architecture/platform, uptime, online
  cores and the process-affinity mask. An assigned core is not necessarily busy.
- Retains the last 120 samples in an O(1) ring. X positions use actual monotonic
  sample timestamps, so changing the interval does not relabel old data.
- Choose 0.5/1/2-second sampling, pause, refresh manually or reset history.
  Unavailable statistics remain unavailable; gaps are not replaced by fake zeros.

| Keyboard | Action |
| --- | --- |
| 1 / 2 / 3 | Memory / Processes / System |
| Space or P | Pause / Resume |
| F5 or R | Refresh once, including while paused |
| C | Reset history |
| + / - | Faster / slower sampling |
| Ctrl+W or Ctrl+Q | Close |

The current kernel API has no per-process CPU accounting, so this app does not
invent CPU utilization or process CPU/RSS tables. Physical memory is the Alpha
allocator's tracked memory, not host/QEMU RSS.

Input redraws do not add history samples. A replaceable one-shot timer implements
the chosen cadence; pausing cancels it and closing releases it. There is no
blinking live indicator, duplicate present RPC, or input-triggered resampling.
The restrained toolbar, graph and aligned detail rows follow `ai-deodrization.md`
and reuse Honoka's Hack font and the text editor's neutral colors.

Host tests: `cargo test --manifest-path tests/native-performance/Cargo.toml --test desktop-tools`.
QEMU workflow: see [desktop validation](../../../../tests/desktop-tools/README.md).
