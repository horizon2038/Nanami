.PHONY: all servers init image fs-image run clean

ARCH ?= x86_64
TARGET_ARCH := $(if $(filter x86-64 x86_64,$(ARCH)),x86_64,$(if $(filter aarch64,$(ARCH)),aarch64,))
ifeq ($(TARGET_ARCH),)
$(error ARCH must be x86_64, x86-64, or aarch64)
endif
TARGET_NAME := $(TARGET_ARCH)-unknown-a9n
DEFAULT_FS_IMAGE := $(CURDIR)/out/$(if $(filter aarch64,$(TARGET_ARCH)),ext2-aarch64.img,ext2.img)

all: image

servers:
	$(MAKE) -C nanami/servers ARCH=$(TARGET_ARCH) $(if $(filter aarch64,$(TARGET_ARCH)),initramfs-rust-only,initramfs)

init: servers
	mkdir -p out/targets
	sed 's#../spencer/Nun/arch/$(TARGET_ARCH)/user.ld#$(CURDIR)/spencer/Nun/arch/$(TARGET_ARCH)/user.ld#g' \
		targets/$(TARGET_NAME).json > out/targets/$(TARGET_NAME).json
	ARCH=$(TARGET_ARCH) CARGO_TARGET_DIR=$(CURDIR)/target cargo build \
		--manifest-path nanami/Cargo.toml \
		--target out/targets/$(TARGET_NAME).json \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		-Z json-target-spec \
		--release

image:
	ARCH=$(TARGET_ARCH) ./scripts/build-image.sh

fs-image:
	NANAMI_TARGET_ARCH=$(TARGET_ARCH) \
		./scripts/create-ext2-image.sh $${SIZE_MB:-64} $${OUT:-$(DEFAULT_FS_IMAGE)}

run:
	ARCH=$(TARGET_ARCH) ./scripts/run-qemu.sh

clean:
	$(MAKE) -C nanami/servers clean
	rm -rf target out
