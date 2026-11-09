# Alpha boot smoke tests

Build a separate boot image, then exercise the normal launcher and configured
services through to Honoka. All guest disk and firmware writes use QEMU snapshots.
Commands run from the repository root:

```sh
cargo xtask build --output-image out/boot-test.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/boot-test.img --accel tcg --smp 4
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/boot-test.img --accel kvm --smp 1
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/boot-test.img --accel kvm --smp 4
```

KVM requires a host with accessible `/dev/kvm`; its default CPU model is `host`.
Use `--cpu` to override it. The runner defaults to the launcher's `auto`
accelerator selection when `--accel` is omitted. Logs include the exact QEMU
command, serial output and, on failure, the stopped CPUs' registers.

Run the KVM cases on an AMD host to cover SYSRET's stack-selector behavior.
A9N must set RPL=3 in STAR[49:48]: AMD SYSRET copies those bits into SS, so
leaving them zero causes a later interrupt return to fail with `#GP(0x20)`.
TCG alone does not expose this failure. See the STAR register description in the
[AMD64 Architecture Programmer's Manual, Volume 2](https://docs.amd.com/api/khub/documents/sD1_QL~h4Afq2_tvzxqqSQ/content).

The `--expect-*` modes require images built with the corresponding optional
domain fixtures; see `nanami/docs/kernel-change-request.md` for those workflows.
