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
EXTRA_LINUX_BINS="${EXTRA_LINUX_BINS:-}"
EXTRA_FREEBSD_BINS="${EXTRA_FREEBSD_BINS:-}"
ROOTFS_APPS="${ROOTFS_APPS:-}"
BRIDGE_IF_EXPLICIT=0
if [ -n "${BRIDGE_IF:-}" ]; then
  BRIDGE_IF_EXPLICIT=1
fi
BRIDGE_IF="${BRIDGE_IF:-}"
HOSTFWD_HTTP="${HOSTFWD_HTTP:-tcp:127.0.0.1:1234-:80}"
PCAP="${PCAP:-$ROOT_DIR/out/net0.pcap}"
QEMU_USE_SUDO="${QEMU_USE_SUDO:-auto}"
BLOCK_IMAGE_IS_DEFAULT=0

default_block_image_stale() {
  if [ ! -f "$BLOCK_IMAGE" ]; then
    return 0
  fi
  if [ -n "$EXTRA_LINUX_BINS" ] || [ -n "$EXTRA_FREEBSD_BINS" ] || [ -n "$ROOTFS_APPS" ] || [ "${ROOTFS_REBUILD:-0}" = "1" ]; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers/apps" \
      -path '*/build/*.elf' \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers/target/x86_64-unknown-a9n/release" \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers" \
      \( -name system-list -o -name session-list \) \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers/apps/honoka/assets/themes" \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if [ "$ROOT_DIR/scripts/create-ext2-image.sh" -nt "$BLOCK_IMAGE" ]; then
    return 0
  fi
  return 1
}

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

default_ipv4_interface() {
  case "$(uname -s)" in
    Darwin)
      route -n get default 2>/dev/null | awk '/^[[:space:]]*interface:/{print $2; exit}'
      ;;
    Linux)
      ip -4 route show default 2>/dev/null | awk '{for (i = 1; i <= NF; i++) if ($i == "dev" && i < NF) {print $(i + 1); exit}}'
      ;;
  esac
}

if [ "$NET_MODE" = "bridged" ]; then
  DEFAULT_ROUTE_IF="$(default_ipv4_interface)"
  if [ -z "$BRIDGE_IF" ]; then
    BRIDGE_IF="$DEFAULT_ROUTE_IF"
  fi
  if [ -z "$BRIDGE_IF" ]; then
    echo "[nanami-run] could not detect the default IPv4 interface; set BRIDGE_IF" >&2
    exit 1
  fi
  if [ "$BRIDGE_IF_EXPLICIT" -eq 1 ] && [ -n "$DEFAULT_ROUTE_IF" ] && [ "$BRIDGE_IF" != "$DEFAULT_ROUTE_IF" ]; then
    echo "[nanami-run] warning: BRIDGE_IF=$BRIDGE_IF differs from the default IPv4 interface $DEFAULT_ROUTE_IF" >&2
    echo "[nanami-run] local host access to the guest may route through $DEFAULT_ROUTE_IF instead" >&2
  fi
fi

if [ -z "$BLOCK_IMAGE" ]; then
  BLOCK_IMAGE="$ROOT_DIR/out/ext2.img"
  BLOCK_IMAGE_IS_DEFAULT=1
fi

"$ROOT_DIR/scripts/build-image.sh"

if [ ! -f "$IMG" ]; then
  echo "[nanami-run] image not found: $IMG" >&2
  exit 1
fi

if [ "$BLOCK_IMAGE_IS_DEFAULT" -eq 1 ]; then
  REBUILD_BLOCK_IMAGE=0
  if default_block_image_stale; then
    REBUILD_BLOCK_IMAGE=1
  fi
else
  REBUILD_BLOCK_IMAGE=0
  if [ ! -f "$BLOCK_IMAGE" ] || [ -n "$EXTRA_LINUX_BINS" ] || [ -n "$EXTRA_FREEBSD_BINS" ] || [ -n "$ROOTFS_APPS" ] || [ "${ROOTFS_REBUILD:-0}" = "1" ]; then
    REBUILD_BLOCK_IMAGE=1
  fi
fi

if [ "$REBUILD_BLOCK_IMAGE" -eq 1 ]; then
  if [ ! -f "$BLOCK_IMAGE" ]; then
    echo "[nanami-run] creating BLOCK_IMAGE: $BLOCK_IMAGE"
  else
    echo "[nanami-run] rebuilding BLOCK_IMAGE: $BLOCK_IMAGE"
  fi
  EXTRA_LINUX_BINS="$EXTRA_LINUX_BINS" EXTRA_FREEBSD_BINS="$EXTRA_FREEBSD_BINS" ROOTFS_APPS="$ROOTFS_APPS" \
    "$ROOT_DIR/scripts/create-ext2-image.sh" "${SIZE_MB:-64}" "$BLOCK_IMAGE"
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

if [ ! -f "$BLOCK_IMAGE" ]; then
  echo "[nanami-run] BLOCK_IMAGE not found: $BLOCK_IMAGE" >&2
  exit 1
fi

args+=(
  -drive "if=none,id=blk0,format=$BLOCK_IMAGE_FORMAT,file=$BLOCK_IMAGE"
  -device "virtio-blk-pci,drive=blk0,addr=3,disable-legacy=off,disable-modern=on"
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
    netdev_device=( -device virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on )
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
