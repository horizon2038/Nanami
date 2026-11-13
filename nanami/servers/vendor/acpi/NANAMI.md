# Local acpi 6.1.1 patch

Source: the crates.io `acpi` 6.1.1 release from
<https://github.com/rust-osdev/acpi>. `Cargo.toml.orig`, upstream source and
MIT/Apache-2.0 licences are retained. This directory is used by both acpi-server
and tests/acpi through Cargo's `[patch.crates-io]` mechanism.

Local interpreter changes:

- `src/aml/object.rs`: `as_integer` follows transparent `LocalOrArg`
  references before reading an integer. Explicit `RefOf` and unresolved
  references are not converted.
- `src/aml/mod.rs`: Increment/Decrement unwrap a transparent reference before
  updating its integer, using the interpreter's existing helper and lock.
- `Store` contributes its result to enclosing expressions, rather than leaving
  the parent's operand list incomplete. `Acquire` returns zero on success and
  a nonzero integer on mutex timeout, as an AML expression requires.
- Failed method evaluation clears saved caller frames so another request cannot
  accidentally resume them. An invalid comparison records its scope and byte
  offset only on failure, without successful-op tracing.
- `Mid` resolves transparent source references, preserves String/Buffer type,
  and clamps slices to the source length with overflow-safe arithmetic (ACPI
  6.6, 19.6.86). Explicit references are not silently dereferenced.
- Like-typed string/buffer comparisons use complete unsigned lexicographic
  comparison and length (19.6.70/71), not a truncated four-byte integer. Long
  buffers cannot overflow a fixed-size temporary. LAnd/LOr require integers.

Without these fixes, valid AML such as `If (Arg0)`, `Sleep (Arg0)` and
`Increment (Local0)` can fail with `expected: Integer, got: Integer`.
The reproduction and failure-path tests are in `tests/acpi/references.rs`.
They include real AML `If (LEqual (Store (...), ...))` and `Acquire` predicates,
timeout results, and rejection of genuinely uninitialized locals. These tests
do not prove which expression a particular physical machine's `_PTS` executes.
Remove this patch when an upstream release provides equivalent fixes and
passes those tests. Firmware errors are still fatal to a shutdown request;
the power service does not skip `_PTS`/`_GTS` on failure.
