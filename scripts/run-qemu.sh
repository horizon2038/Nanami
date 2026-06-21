#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SPENCER_DIR="$ROOT_DIR/spencer"
ARCH="${ARCH:-x86-64}"
PLATFORM="${PLATFORM:-qemu}"
PROFILE="${PROFILE:-release}"
TARGET_ARCH="x86_64"
OUT_DIR="$SPENCER_DIR/out/${TARGET_ARCH}-${PLATFORM}-${PROFILE}"
IMG="$OUT_DIR/spencer.img"
OVMF_CODE="$SPENCER_DIR/a9nloader-rs/tools/OVMF_CODE.fd"
OVMF_VARS_SRC="$SPENCER_DIR/a9nloader-rs/tools/OVMF_VARS.fd"
OVMF_VARS_RUNTIME="$OUT_DIR/OVMF_VARS.nanami.fd"

QEMU="${QEMU:-qemu-system-x86_64}"
QEMU_MEMORY="${QEMU_MEMORY:-4G}"
QEMU_CPU="${QEMU_CPU:-max}"
QEMU_SMP="${QEMU_SMP:-1}"
QEMU_ACCEL="${QEMU_ACCEL:-auto}"
NET_MODE="${NET_MODE:-}"
NET_DEVICE="${NET_DEVICE:-virtio}"
BLOCK_IMAGE="${BLOCK_IMAGE:-}"
BLOCK_IMAGE_FORMAT="${BLOCK_IMAGE_FORMAT:-raw}"
BRIDGE_IF="${BRIDGE_IF:-en0}"
HOSTFWD_HTTP="${HOSTFWD_HTTP:-tcp:127.0.0.1:1234-:80}"
PCAP="${PCAP:-$ROOT_DIR/out/net0.pcap}"
QEMU_USE_SUDO="${QEMU_USE_SUDO:-auto}"

if [ "$ARCH" != "x86-64" ] && [ "$ARCH" != "x86_64" ]; then
  echo "[nanami-run] only x86-64 QEMU is currently supported" >&2
  exit 1
fi

if [ -z "$NET_MODE" ]; then
  if [ "$(uname -s)" = "Darwin" ]; then
    NET_MODE="bridged"
  else
    NET_MODE="user"
  fi
fi

if [ -z "$BLOCK_IMAGE" ]; then
  if [ -f "out/ext2.img" ]; then
    BLOCK_IMAGE="$(pwd)/out/ext2.img"
  fi
fi

"$ROOT_DIR/scripts/build-image.sh"

if [ ! -f "$IMG" ]; then
  echo "[nanami-run] image not found: $IMG" >&2
  exit 1
fi

cp "$OVMF_VARS_SRC" "$OVMF_VARS_RUNTIME"

args=(
  -m "$QEMU_MEMORY"
  -cpu "$QEMU_CPU"
  -smp "$QEMU_SMP"
  -serial mon:stdio
  -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
  -drive "if=pflash,format=raw,file=$OVMF_VARS_RUNTIME"
  -drive "format=raw,file=$IMG"
  --no-reboot
  --no-shutdown
)

if [ -z "$BLOCK_IMAGE" ]; then
  echo "[nanami-run] BLOCK_IMAGE is required because ramdisk block-device-server is disabled." >&2
  echo "[nanami-run] Set BLOCK_IMAGE=/path/to/ext2.img, or place ext2.img under out/, nanami/servers/, or repository root." >&2
  exit 1
fi

if [ ! -f "$BLOCK_IMAGE" ]; then
  echo "[nanami-run] BLOCK_IMAGE not found: $BLOCK_IMAGE" >&2
  exit 1
fi

args+=(
  -drive "if=none,id=blk0,format=$BLOCK_IMAGE_FORMAT,file=$BLOCK_IMAGE"
  -device "virtio-blk-pci,drive=blk0,disable-legacy=off,disable-modern=on"
)

if [ "$QEMU_ACCEL" = "auto" ]; then
  case "$(uname -s)" in
    Linux)
      if [ -e /dev/kvm ]; then
        args+=(-accel kvm)
      fi
      ;;
    Darwin)
      # x86_64 guests on Apple Silicon cannot use HVF; allow explicit QEMU_ACCEL=hvf on Intel Macs.
      if [ "$(uname -m)" = "x86_64" ]; then
        args+=(-accel hvf)
      fi
      ;;
  esac
elif [ "$QEMU_ACCEL" != "none" ]; then
  args+=(-accel "$QEMU_ACCEL")
fi

case "$NET_DEVICE" in
  virtio)
    netdev_device=( -device virtio-net,netdev=net0,disable-legacy=off,disable-modern=on )
    ;;
  e1000)
    netdev_device=( -device e1000,netdev=net0 )
    ;;
  *)
    echo "[nanami-run] NET_DEVICE must be virtio or e1000" >&2
    exit 1
    ;;
esac

case "$NET_MODE" in
  user)
    args+=(-netdev "user,id=net0,hostfwd=$HOSTFWD_HTTP")
    args+=("${netdev_device[@]}")
    ;;
  bridged)
    args+=(-netdev "vmnet-bridged,id=net0,ifname=$BRIDGE_IF")
    args+=("${netdev_device[@]}")
    if [ "$PCAP" != "none" ]; then
      mkdir -p "$(dirname "$PCAP")"
      args+=(-object "filter-dump,id=net0-dump,netdev=net0,file=$PCAP")
    fi
    ;;
  none)
    args+=(-net none)
    ;;
  *)
    echo "[nanami-run] NET_MODE must be user, bridged, or none" >&2
    exit 1
    ;;
esac

if [ "$QEMU_USE_SUDO" = "auto" ] && [ "$NET_MODE" = "bridged" ] && [ "$(uname -s)" = "Darwin" ]; then
  exec sudo "$QEMU" "${args[@]}" "$@"
elif [ "$QEMU_USE_SUDO" = "1" ] || [ "$QEMU_USE_SUDO" = "true" ]; then
  exec sudo "$QEMU" "${args[@]}" "$@"
else
  exec "$QEMU" "${args[@]}" "$@"
fi
