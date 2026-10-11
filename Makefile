# Optional convenience only. The portable interface is cargo xtask.
.PHONY: all image servers fs-image run
ARCH ?= x86_64
all: image
image:
	cargo xtask build --arch $(ARCH) $(ARGS)
servers:
	cargo xtask servers --arch $(ARCH) $(ARGS)
fs-image:
	cargo xtask rootfs --arch $(ARCH) $(ARGS)
run:
	cargo xtask run --arch $(ARCH) $(ARGS)
