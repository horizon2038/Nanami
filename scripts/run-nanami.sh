#!/usr/bin/env bash
set -euo pipefail

NANAMI_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$NANAMI_ROOT"

usage() {
    printf 'Usage: %s [image|run]\n' "$0"
    printf '  image  Build the x86_64 hardware image (default).\n'
    printf '  run    Build and launch QEMU with USB storage/input and HPET.\n'
    printf 'LINUX_BIN_DIR defaults to the repository bin/ directory.\n'
    printf 'All regular files are collected recursively, including hidden files.\n'
    printf 'Files are installed by basename into /alter/linux/bin (duplicates are errors).\n'
    printf 'Symlinks and special files are rejected; files are not filtered by ABI or name.\n'
    printf 'EXTRA_LINUX_BINS overrides directory collection (empty disables these extras).\n'
    printf 'NET_MODE=user|bridged|none; BRIDGE_IF selects the bridged host interface.\n'
    printf 'Setting BRIDGE_IF defaults NET_MODE to bridged; otherwise it defaults to user.\n'
    printf 'QEMU_USE_SUDO defaults to auto (sudo for macOS bridged networking).\n'
    printf 'Both modes regenerate out/ext2.img and the boot image. Back up saved data first.\n'
}

case "${1:-image}" in
    image|run) ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac

# macOS / Homebrew toolchains.
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/make/libexec/gnubin:/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:/opt/homebrew/bin:$PATH"

export ARCH=x86_64
export PLATFORM=pc99
export PROFILE=release

# Use USB IRQs and disable the high-volume diagnostic logs.
export NANAMI_USB_POLL_ONLY=0
export NANAMI_BOOT_PC_TRACE=0
export NANAMI_BOOT_IO_TRACE=0
export NANAMI_USB_IO_TRACE=0

# Include the latest servers and applications in the root filesystem.
export ROOTFS_IMAGE="$NANAMI_ROOT/out/ext2.img"
export ROOTFS_REBUILD=1
export SIZE_MB="${SIZE_MB:-64}"

# Quote each path for the image builder's POSIX shlex parser. Bash's %q can
# emit $'...' strings that shlex does not understand, so use single quotes.
collect_linux_bins() (
    shopt -s nullglob dotglob
    local entry quoted
    if [ ! -r "$1" ] || [ ! -x "$1" ]; then
        printf '[nanami] Cannot read Linux binary directory: %s\n' "$1" >&2
        exit 1
    fi
    for entry in "$1"/*; do
        if [ -L "$entry" ]; then
            printf '[nanami] Linux binary directory contains a symlink: %s\n' "$entry" >&2
            exit 1
        elif [ -d "$entry" ]; then
            collect_linux_bins "$entry" || exit 1
        elif [ -f "$entry" ]; then
            quoted="${entry//\'/\'\\\'\'}"
            printf "'%s' " "$quoted"
        else
            printf '[nanami] Linux binary directory contains a special file: %s\n' "$entry" >&2
            exit 1
        fi
    done
)

# Discover payloads from the chosen directory instead of maintaining a list.
# Explicit overrides, including an empty list, remain authoritative.
if [ "${EXTRA_LINUX_BINS+x}" != x ]; then
    linux_bin_dir="${LINUX_BIN_DIR:-$NANAMI_ROOT/bin}"
    if [ ! -d "$linux_bin_dir" ]; then
        printf '[nanami] Linux binary directory not found: %s\n' "$linux_bin_dir" >&2
        exit 1
    fi
    linux_bin_dir="$(cd "$linux_bin_dir" && pwd)"
    EXTRA_LINUX_BINS="$(collect_linux_bins "$linux_bin_dir")"
    printf '[nanami] Including files from: %s\n' "$linux_bin_dir"
fi
export EXTRA_LINUX_BINS

# LINUX_ROOTFS_DIR is inherited for additional data and shared libraries.
# Packaging executables alone does not supply their dynamic dependencies.
# export LINUX_ROOTFS_DIR="/path/to/linux-rootfs"

printf '[nanami] Regenerating rootfs and boot image; existing image contents will be replaced.\n'

case "${1:-image}" in
    image)
        # Rebuild the SMP kernel, loader, Nanami, servers and complete disk image.
        bash "$NANAMI_ROOT/scripts/build-image.sh"
        printf '\nImage: %s\n' \
            "$NANAMI_ROOT/spencer/out/x86_64-pc99-release/spencer.img"
        ;;
    run)
        # run-qemu.sh builds the image before launching QEMU.
        export BLOCK_IMAGE="$ROOTFS_IMAGE"
        export QEMU_SMP=4
        export QEMU_MEMORY=4G
        export QEMU_HPET=on
        export QEMU_ACCEL=auto
        export STORAGE_DEVICE=usb
        export USB_INPUT=on
        if [ -z "${NET_MODE:-}" ]; then
            if [ -n "${BRIDGE_IF:-}" ]; then
                NET_MODE=bridged
            else
                NET_MODE=user
            fi
        fi
        export NET_MODE
        export QEMU_USE_SUDO="${QEMU_USE_SUDO:-auto}"

        bash "$NANAMI_ROOT/scripts/run-qemu.sh"
        ;;
esac
