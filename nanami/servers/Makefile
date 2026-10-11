# Optional convenience only; no host shell build loops or cpio dependency.
.PHONY: all initramfs initramfs-rust-only rust cpp apps
ARCH ?= x86_64
all initramfs rust cpp apps:
	$(MAKE) -C ../.. servers ARCH=$(ARCH)
initramfs-rust-only:
	$(MAKE) -C ../.. servers ARCH=$(ARCH) ARGS=--skip-cpp
