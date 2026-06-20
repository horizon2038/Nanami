# Nanami

Layout:

- `nanami/`: Nanami OS source, user-space servers, SDK, and docs.
- `spencer/`: submodule target for [Spencer](https://github.com/horizon2038/spencer), used to build [A9N](https://github.com/horizon2038/A9N), [A9NLoader-rs](https://github.com/horizon2038/a9nloader-rs), and the bootable image.
- `targets/`: Nanami-owned target templates used for external OS injection.
- `out/`: generated boot images.

## How to use

### Build

```bash
cd nanami
make image
```

### Run

```bash
cd nanami
make run
```

### Example

#### Runtime Options

```bash
NET_MODE=bridged BRIDGE_IF=en0 make run
NET_MODE=user HOSTFWD_HTTP=tcp:127.0.0.1:1234-:80 make run
QEMU_ACCEL=kvm make run
QEMU_ACCEL=none make run
```

`NET_MODE` defaults to `bridged` on macOS and `user` elsewhere. `QEMU_ACCEL=auto` uses KVM on Linux when `/dev/kvm` exists, HVF on Intel macOS, and no accelerator on Apple Silicon.

The build wrapper first builds `nanami/servers/initramfs.cpio`, then delegates kernel, bootloader, OS payload build, FAT image creation, and QEMU execution to Spencer's `cargo xtask`.

Nanami is injected into Spencer as an external OS payload:

```bash
cargo xtask build \
  --arch x86-64 \
  --platform qemu \
  --release \
  --os-manifest /path/to/nanami/nanami/Cargo.toml \
  --os-target-json /path/to/generated/x86_64-unknown-a9n.json \
  --os-binary nanami
```

Spencer's image builder writes Nanami directly as `/kernel/init.elf`; no post-build FAT image patching is used.
