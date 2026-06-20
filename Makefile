.PHONY: all servers init image run clean

all: image

servers:
	$(MAKE) -C nanami/servers initramfs

init: servers
	mkdir -p out/targets
	sed 's#../spencer/Nun/arch/x86_64/user.ld#$(CURDIR)/spencer/Nun/arch/x86_64/user.ld#g' \
		targets/x86_64-unknown-a9n.json > out/targets/x86_64-unknown-a9n.json
	ARCH=x86_64 CARGO_TARGET_DIR=$(CURDIR)/target cargo build \
		--manifest-path nanami/Cargo.toml \
		--target out/targets/x86_64-unknown-a9n.json \
		-Z build-std=core,alloc,compiler_builtins \
		-Z build-std-features=compiler-builtins-mem \
		-Z json-target-spec \
		--release

image:
	./scripts/build-image.sh

run:
	./scripts/run-qemu.sh

clean:
	$(MAKE) -C nanami/servers clean
	rm -rf target out
