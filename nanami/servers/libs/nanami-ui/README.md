# Native UI helpers

Small `no_std` helpers shared by Performance Monitor and Saran: clipped RAM
surface drawing, the existing Hack font's cached ASCII glyphs, raw-key decoding,
short text fields, a Honoka window and bounded binary VFS reads/writes.

There is no widget framework, scheduler, GPU backend or new OS service. The
drawing/input modules run in host tests; IPC modules and their runtime/client
dependencies are only enabled for Nanami's `target_os = "none"` targets.
Font data and license are reused from Honoka, not downloaded or duplicated.

The window uses the existing SPSC damage ring plus one notification. It falls
back to an invalidation RPC only when the ring is full/unavailable; it does not
issue both paths for every frame. Input is drained in bounded batches, and an
idle app waits on its bound notification without polling.
