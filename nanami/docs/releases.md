# Nanami Releases

## Release CI

`.github/workflows/release.yml` follows A9N's annotated-tag release model.
Pushing a `vMAJOR.MINOR.PATCH` tag validates the annotation and checks that the
tag matches `nanami/Cargo.toml`. The reusable `build.yml` runs host regression
tests, builds both supported targets in Release mode, and uploads their
artifacts. Only after both builds succeed does the release job publish a GitHub
Release, using the annotated tag's message as the release notes.

The build workflow also runs on pull requests, pushes to `main`/`develop`, and
manual dispatch. These runs upload build artifacts without publishing a release.
Build jobs have read-only repository permissions; only the publication job gets
`contents: write`. No additional release secret is required.

For example, after committing the version and release changes:

```sh
git tag -a v0.6.0 -F /path/to/release-notes.txt
git push origin v0.6.0
```

Use an English bullet list for `release-notes.txt`. Do not create a lightweight
tag. The workflow does not create tags or replace an existing GitHub Release.

## Assets

Each release contains two archives and a top-level `SHA256SUMS`:

- `nanami-vMAJOR.MINOR.PATCH-x86_64-pc99.tar.gz`: `nanami.img` with an EFI boot
  partition and an embedded ext2 root partition.
- `nanami-vMAJOR.MINOR.PATCH-aarch64-qemu.tar.gz`: `nanami.img` for boot,
  `rootfs.img` as a separate virtio-mmio disk, `u-boot.bin`, and the U-Boot
  source archive and Docker build recipe.

Both archives contain file checksums, project licenses, these instructions,
and `BUILD-INFO.txt` recording the commit, recursive submodule revisions, and
toolchain versions. Verify the archive checksum before extraction and the inner
checksums afterwards:

```sh
sha256sum --check SHA256SUMS
tar -xzf nanami-v0.6.0-x86_64-pc99.tar.gz -C /path/to/empty-directory
cd /path/to/empty-directory
sha256sum --check SHA256SUMS
```

CI uses the pinned Rust nightly, LLVM 19, and CMake 3.31.6. The AArch64 build
uses SPENCER's Docker-based U-Boot build (upstream `v2025.10`, source at
<https://source.denx.de/u-boot/u-boot/>). Dependency licenses remain applicable;
the included Nanami license does not relicense A9N, the loader, or U-Boot.

Images are built from a fresh rootfs with `--no-linux-bins`. Native services,
applications, Alter, and Negoto are included; locally supplied Linux/FreeBSD
binaries and downloaded BusyBox bundles are not embedded. To add guest programs,
build a custom image using `--linux-bin-dir` or `--linux-rootfs-dir`.

## Running an Extracted Image

Run from a Nanami checkout at the commit recorded in `BUILD-INFO.txt`, with the
usual QEMU prerequisites and recursive submodules initialized. Do not pass
`--build`, which would replace the downloaded image.

x86_64:

```sh
cargo xtask run --output-image /path/to/extracted/nanami.img
```

AArch64 (headless):

```sh
mkdir -p spencer/out/aarch64-qemu-release/u-boot
cp /path/to/extracted/u-boot.bin spencer/out/aarch64-qemu-release/u-boot/u-boot.bin
cargo xtask run --arch aarch64 --net none \
  --output-image /path/to/extracted/nanami.img \
  --rootfs /path/to/extracted/rootfs.img
```

The root disk is writable and retains guest changes between runs. Keep a copy
of the downloaded image if you need to return to the original state.
