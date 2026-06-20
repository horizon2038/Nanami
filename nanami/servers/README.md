# Nanami User SDK

`nanami/servers` provides the SDK used to build Nanami OS components.

## Structure

- `sdk/arch/<arch>/...`
  - Architecture dependent parts (linker script, startup, syscall/ABI stubs).
- `sdk/cpp/...`
  - Architecture independent C++ headers and Nanami library logic.
- `sdk/rust/libnanami`
  - Rust Nanami helper library using crates.io `a9n_abi` / `a9n-types`.
- `sdk/build/cpp_app.mk`
  - Reusable build system for C++ components.
- `apps/*`
  - Example components.

## Build

```sh
make -C nanami/nanami/servers initramfs
```

This builds apps and packages them into `initramfs.cpio` (newc format).
Nanami embeds this archive via `include_bytes!` and loads each ELF component.
