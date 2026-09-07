.PHONY: all apps cpp rust initramfs initramfs-rust-only initramfs-cpp-only clean

CORE_SERVICE_DIRS := $(sort $(wildcard core-services/*) $(wildcard core-services/*/*))
APP_DIRS := $(sort $(wildcard apps/*) $(wildcard apps/*/*))
BUILD_DIRS := $(CORE_SERVICE_DIRS) $(APP_DIRS)
ARCH ?= x86_64
NANAMI_RUST_TARGET_NAME := $(if $(filter aarch64,$(ARCH)),aarch64-unknown-a9n,x86_64-unknown-a9n)
NANAMI_RUST_TARGET_JSON := $(abspath sdk/arch/$(ARCH)/$(NANAMI_RUST_TARGET_NAME).json)
RUST_APP_DIRS := $(foreach d,$(BUILD_DIRS),$(if $(wildcard $(d)/Cargo.toml),$(d),))
ifeq ($(ARCH),aarch64)
# Drivers for devices that cannot exist on the current platform are not part
# of that target. The manager selects from the binaries built for its platform.
RUST_APP_DIRS := $(filter-out \
	core-services/ahci-server \
	core-services/hpet-server \
	core-services/timer-server,\
	$(RUST_APP_DIRS))
endif
CPP_APP_DIRS := $(foreach d,$(BUILD_DIRS),$(if $(wildcard $(d)/Makefile),$(d),))
CARGO_TARGET_DIR ?= $(CURDIR)/target
NANAMI_USER_LINKER_SCRIPT := $(abspath sdk/arch/$(ARCH)/linker/nanami_user.ld)
RUSTFLAGS ?= -C link-arg=-T$(NANAMI_USER_LINKER_SCRIPT)
export CARGO_TARGET_DIR
export RUSTFLAGS
export NANAMI_RUST_TARGET_NAME
export NANAMI_TARGET_ARCH := $(ARCH)

all: initramfs

apps: cpp rust

cpp:
	@set -e; \
	for d in $(CPP_APP_DIRS); do \
		echo "[build][cpp] $$d"; \
		$(MAKE) -C $$d all; \
	done

rust:
	@set -e; \
	for d in $(RUST_APP_DIRS); do \
		echo "[build][rust] $$d"; \
		cd $$d && cargo build --release --target "$(NANAMI_RUST_TARGET_JSON)" -Z json-target-spec; \
		cd - >/dev/null; \
	done

initramfs: apps
	./mkinitramfs.sh

initramfs-rust-only: rust
	APPS=rust ./mkinitramfs.sh

initramfs-cpp-only: cpp
	APPS=cpp ./mkinitramfs.sh

clean:
	@set -e; \
	for d in $(CPP_APP_DIRS); do \
		$(MAKE) -C $$d clean || true; \
	done; \
	@if [ -n "$(strip $(RUST_APP_DIRS))" ]; then \
		cargo clean --manifest-path "$(firstword $(RUST_APP_DIRS))/Cargo.toml" \
			--target-dir "$(CARGO_TARGET_DIR)"; \
	fi
	rm -rf build
	rm -f initramfs.cpio
