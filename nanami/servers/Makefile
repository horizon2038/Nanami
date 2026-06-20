.PHONY: all apps cpp rust initramfs initramfs-rust-only initramfs-cpp-only clean

APP_DIRS := $(sort $(wildcard apps/*))
RUST_APP_DIRS := $(foreach d,$(APP_DIRS),$(if $(wildcard $(d)/Cargo.toml),$(d),))
CPP_APP_DIRS := $(foreach d,$(APP_DIRS),$(if $(wildcard $(d)/Makefile),$(d),))

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
		cd $$d && cargo build --release -Z json-target-spec; \
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
	for d in $(RUST_APP_DIRS); do \
		cd $$d && cargo clean; \
		cd - >/dev/null; \
	done
	rm -rf build
	rm -f initramfs.cpio
