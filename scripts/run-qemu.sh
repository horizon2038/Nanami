#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SPENCER_DIR="$ROOT_DIR/spencer"
ARCH="${ARCH:-x86-64}"
PROFILE="${PROFILE:-release}"
case "$ARCH" in
  x86-64|x86_64)
    TARGET_ARCH=x86_64
    PLATFORM="${PLATFORM:-pc99}"
    DEFAULT_QEMU=qemu-system-x86_64
    DEFAULT_CPU=max
    ;;
  aarch64)
    TARGET_ARCH=aarch64
    PLATFORM="${PLATFORM:-qemu}"
    DEFAULT_QEMU=qemu-system-aarch64
    DEFAULT_CPU=cortex-a72
    ;;
  *)
    echo "[nanami-run] ARCH must be x86-64, x86_64, or aarch64" >&2
    exit 1
    ;;
esac
case "$TARGET_ARCH/$PLATFORM" in
  x86_64/pc99|aarch64/qemu) ;;
  *)
    echo "[nanami-run] QEMU supports only x86_64/pc99 and aarch64/qemu (got $TARGET_ARCH/$PLATFORM)" >&2
    exit 1
    ;;
esac
OUT_DIR="$SPENCER_DIR/out/${TARGET_ARCH}-${PLATFORM}-${PROFILE}"
IMG="$OUT_DIR/spencer.img"
OVMF_CODE="$SPENCER_DIR/a9nloader-rs/tools/OVMF_CODE.fd"
OVMF_VARS_SRC="$SPENCER_DIR/a9nloader-rs/tools/OVMF_VARS.fd"
OVMF_VARS_RUNTIME="$OUT_DIR/OVMF_VARS.nanami.fd"

QEMU="${QEMU:-$DEFAULT_QEMU}"
QEMU_MEMORY="${QEMU_MEMORY:-4G}"
QEMU_CPU="${QEMU_CPU:-$DEFAULT_CPU}"
QEMU_SMP="${QEMU_SMP:-4}"
QEMU_ACCEL="${QEMU_ACCEL:-auto}"
QEMU_HPET="${QEMU_HPET:-on}"
USB_INPUT="${USB_INPUT:-off}"
NET_MODE="${NET_MODE:-}"
NET_DEVICE="${NET_DEVICE:-virtio}"
BLOCK_IMAGE="${BLOCK_IMAGE:-}"
BLOCK_IMAGE_FORMAT="${BLOCK_IMAGE_FORMAT:-raw}"
STORAGE_DEVICE="${STORAGE_DEVICE:-ahci}"
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
  if [ -n "$EXTRA_LINUX_BINS" ] || [ -n "$EXTRA_FREEBSD_BINS" ] || [ -n "${LINUX_ROOTFS_DIR:-}" ] || [ -n "$ROOTFS_APPS" ] || [ "${ROOTFS_REBUILD:-0}" = "1" ]; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers/apps" \
      -path '*/build/*.elf' \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if find "$ROOT_DIR/nanami/servers/target/${TARGET_ARCH}-unknown-a9n/release" \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  for manifest in \
      "$ROOT_DIR/nanami/servers/system-list" \
      "$ROOT_DIR/nanami/servers/session-list" \
      "$ROOT_DIR/nanami/servers/system-list.$TARGET_ARCH" \
      "$ROOT_DIR/nanami/servers/session-list.$TARGET_ARCH"; do
    if [ -f "$manifest" ] && [ "$manifest" -nt "$BLOCK_IMAGE" ]; then
      return 0
    fi
  done
  if find "$ROOT_DIR/nanami/servers/apps/honoka/assets/themes" \
      -type f -newer "$BLOCK_IMAGE" -print -quit 2>/dev/null | grep -q .; then
    return 0
  fi
  if [ "$ROOT_DIR/scripts/create-ext2-image.sh" -nt "$BLOCK_IMAGE" ]; then
    return 0
  fi
  return 1
}

case "$QEMU_SMP" in
  ''|*[!0-9]*)
    echo "[nanami-run] QEMU_SMP must be an integer from 1 to 64" >&2
    exit 1
    ;;
esac
if [ "$QEMU_SMP" -lt 1 ] || [ "$QEMU_SMP" -gt 64 ]; then
  echo "[nanami-run] QEMU_SMP must be an integer from 1 to 64" >&2
  exit 1
fi

case "$QEMU_HPET" in
  on|off) ;;
  *)
    echo "[nanami-run] QEMU_HPET must be on or off" >&2
    exit 1
    ;;
esac

case "$USB_INPUT/$TARGET_ARCH" in
  off/*|on/x86_64) ;;
  *)
    echo "[nanami-run] USB_INPUT must be off or on (x86_64 xHCI only)" >&2
    exit 1
    ;;
esac

if [ -z "$NET_MODE" ]; then
  if [ "$TARGET_ARCH" = "aarch64" ]; then
    NET_MODE="none"
  elif [ "$(uname -s)" = "Darwin" ]; then
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
  if [ "$TARGET_ARCH" = "aarch64" ]; then
    BLOCK_IMAGE="$ROOT_DIR/out/ext2-aarch64.img"
  else
    BLOCK_IMAGE="$ROOT_DIR/out/ext2.img"
  fi
  BLOCK_IMAGE_IS_DEFAULT=1
fi
case "$BLOCK_IMAGE" in
  /*) ;;
  *) BLOCK_IMAGE="$ROOT_DIR/$BLOCK_IMAGE" ;;
esac
if [ "$TARGET_ARCH" = "x86_64" ] && [ "$BLOCK_IMAGE_FORMAT" != "raw" ]; then
  echo "[nanami-run] x86_64 BLOCK_IMAGE must be a raw ext2 staging image" >&2
  exit 1
fi

if [ "$TARGET_ARCH" = "x86_64" ]; then
  if [ "$BLOCK_IMAGE_IS_DEFAULT" -eq 1 ]; then
    REBUILD_BLOCK_IMAGE=1
  else
    REBUILD_BLOCK_IMAGE=0
    if [ ! -f "$BLOCK_IMAGE" ] || [ -n "$EXTRA_LINUX_BINS" ] || [ -n "$EXTRA_FREEBSD_BINS" ] || [ -n "${LINUX_ROOTFS_DIR:-}" ] || [ -n "$ROOTFS_APPS" ] || [ "${ROOTFS_REBUILD:-0}" = "1" ]; then
      REBUILD_BLOCK_IMAGE=1
    fi
  fi
  ROOTFS_IMAGE="$BLOCK_IMAGE" ROOTFS_REBUILD="$REBUILD_BLOCK_IMAGE" \
    EXTRA_LINUX_BINS="$EXTRA_LINUX_BINS" EXTRA_FREEBSD_BINS="$EXTRA_FREEBSD_BINS" ROOTFS_APPS="$ROOTFS_APPS" \
    ARCH="$ARCH" PLATFORM="$PLATFORM" PROFILE="$PROFILE" "$ROOT_DIR/scripts/build-image.sh"
else
  ARCH="$ARCH" PLATFORM="$PLATFORM" PROFILE="$PROFILE" "$ROOT_DIR/scripts/build-image.sh"
fi

if [ ! -f "$IMG" ]; then
  echo "[nanami-run] image not found: $IMG" >&2
  exit 1
fi

if [ "$TARGET_ARCH" = "x86_64" ]; then
  REBUILD_BLOCK_IMAGE=0
elif [ "$BLOCK_IMAGE_IS_DEFAULT" -eq 1 ]; then
  REBUILD_BLOCK_IMAGE=0
  if default_block_image_stale; then
    REBUILD_BLOCK_IMAGE=1
  fi
else
  REBUILD_BLOCK_IMAGE=0
  if [ ! -f "$BLOCK_IMAGE" ] || [ -n "$EXTRA_LINUX_BINS" ] || [ -n "$EXTRA_FREEBSD_BINS" ] || [ -n "${LINUX_ROOTFS_DIR:-}" ] || [ -n "$ROOTFS_APPS" ] || [ "${ROOTFS_REBUILD:-0}" = "1" ]; then
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
    NANAMI_TARGET_ARCH="$TARGET_ARCH" \
      "$ROOT_DIR/scripts/create-ext2-image.sh" "${SIZE_MB:-64}" "$BLOCK_IMAGE"
fi

if [ ! -f "$BLOCK_IMAGE" ]; then
  echo "[nanami-run] BLOCK_IMAGE not found: $BLOCK_IMAGE" >&2
  exit 1
fi

ACCEL_ARGS=()
if [ "$QEMU_ACCEL" = "auto" ]; then
  case "$(uname -s)" in
    Linux)
      if [ -e /dev/kvm ] && { [ "$(uname -m)" = "$TARGET_ARCH" ] || { [ "$(uname -m)" = "arm64" ] && [ "$TARGET_ARCH" = "aarch64" ]; }; }; then
        ACCEL_ARGS=(-accel kvm)
        if [ "$TARGET_ARCH" = "aarch64" ] && [ "$QEMU_CPU" = "$DEFAULT_CPU" ]; then
          QEMU_CPU=host
        fi
      fi
      ;;
    Darwin)
      # A9N's AArch64 QEMU platform currently uses GICv2, which HVF cannot
      # emulate. Keep AArch64 on MTTCG until the kernel gains GICv3 support.
      if [ "$TARGET_ARCH" != "aarch64" ] && { [ "$(uname -m)" = "$TARGET_ARCH" ] || { [ "$(uname -m)" = "arm64" ] && [ "$TARGET_ARCH" = "aarch64" ]; }; }; then
        ACCEL_ARGS=(-accel hvf)
      fi
      ;;
  esac
elif [ "$QEMU_ACCEL" != "none" ]; then
  ACCEL_ARGS=(-accel "$QEMU_ACCEL")
  if [ "$TARGET_ARCH" = "aarch64" ] && [ "$QEMU_CPU" = "$DEFAULT_CPU" ]; then
    case "$QEMU_ACCEL" in
      hvf|kvm) QEMU_CPU=host ;;
    esac
  fi
fi

if [ "$TARGET_ARCH" = "aarch64" ] && [ "${#ACCEL_ARGS[@]}" -eq 0 ] && [ "$QEMU_ACCEL" = "auto" ]; then
  ACCEL_ARGS=(-accel tcg,thread=multi)
fi

if [ "$TARGET_ARCH" = "x86_64" ]; then
  case "$STORAGE_DEVICE" in
    ahci)
      storage_args=(
        -device "ich9-ahci,id=ahci,addr=3"
        -drive "if=none,id=nanami-disk,format=raw,file=$IMG"
        -device "ide-hd,drive=nanami-disk,bus=ahci.0,bootindex=1"
      )
      ;;
    virtio)
      storage_args=(
        -drive "if=none,id=nanami-disk,format=raw,file=$IMG"
        -device "virtio-blk-pci,drive=nanami-disk,addr=3,bootindex=1,disable-legacy=off,disable-modern=on"
      )
      ;;
    *)
      echo "[nanami-run] STORAGE_DEVICE must be ahci or virtio" >&2
      exit 1
      ;;
  esac
  cp "$OVMF_VARS_SRC" "$OVMF_VARS_RUNTIME"
  args=(
    -machine "hpet=$QEMU_HPET"
    -m "$QEMU_MEMORY"
    -cpu "$QEMU_CPU"
    -smp "$QEMU_SMP"
    -serial mon:stdio
    -drive "if=pflash,format=raw,readonly=on,file=$OVMF_CODE"
    -drive "if=pflash,format=raw,file=$OVMF_VARS_RUNTIME"
    "${storage_args[@]}"
    --no-reboot
    --no-shutdown
  )
  if [ "$USB_INPUT" = "on" ]; then
    args+=(
      -fw_cfg "name=opt/ovmf/X-PciMmio64Mb,string=0"
      -device "qemu-xhci,id=xhci,addr=6,msi=off,msix=off"
      -device "usb-kbd,id=usb-kbd,bus=xhci.0,port=1"
      -device "usb-mouse,id=usb-mouse,bus=xhci.0,port=2"
    )
  fi
else
  UBOOT="$OUT_DIR/u-boot/u-boot.bin"
  if [ ! -f "$UBOOT" ]; then
    echo "[nanami-run] AArch64 U-Boot artifact not found: $UBOOT" >&2
    exit 1
  fi
  args=(
    -machine virt,gic-version=2
    -m "$QEMU_MEMORY"
    -cpu "$QEMU_CPU"
    -smp "$QEMU_SMP"
    -nographic
    -bios "$UBOOT"
    -drive "if=none,id=boot,format=raw,file=$IMG"
    -device "virtio-blk-pci,drive=boot"
    -drive "if=none,id=blk0,format=$BLOCK_IMAGE_FORMAT,file=$BLOCK_IMAGE"
    -device "virtio-blk-device,drive=blk0"
    -global virtio-mmio.force-legacy=false
    --no-reboot
    --no-shutdown
  )
fi
args+=("${ACCEL_ARGS[@]}")

if [ "$TARGET_ARCH" = "aarch64" ] && [ "$NET_MODE" != "none" ]; then
  echo "[nanami-run] AArch64 currently supports NET_MODE=none; virtio-net is still x86_64-only" >&2
  exit 1
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
