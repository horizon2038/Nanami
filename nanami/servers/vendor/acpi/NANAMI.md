# Local acpi 6.1.1 patch

Source: the crates.io `acpi` 6.1.1 release from
<https://github.com/rust-osdev/acpi>. `Cargo.toml.orig`, upstream source and
MIT/Apache-2.0 licences are retained. This directory is used by both acpi-server
and tests/acpi through Cargo's `[patch.crates-io]` mechanism.

Only two interpreter sites differ from that release:

- `src/aml/object.rs`: `as_integer` follows transparent `LocalOrArg`
  references before reading an integer. Explicit `RefOf` and unresolved
  references are not converted.
- `src/aml/mod.rs`: Increment/Decrement unwrap a transparent reference before
  updating its integer, using the interpreter's existing helper and lock.

Without these fixes, valid AML such as `If (Arg0)`, `Sleep (Arg0)` and
`Increment (Local0)` can fail with `expected: Integer, got: Integer`.
The reproduction and failure-path tests are in `tests/acpi/references.rs`.
Remove this patch when an upstream release provides equivalent fixes and
passes those tests. Firmware errors are still fatal to a shutdown request;
the power service does not skip `_PTS`/`_GTS` on failure.
