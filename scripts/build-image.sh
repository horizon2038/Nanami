#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
NANAMI_DIR="$ROOT_DIR/nanami"
SPENCER_DIR="$ROOT_DIR/spencer"
ARCH="${ARCH:-x86-64}"
PROFILE="${PROFILE:-release}"
ROOTFS_IMAGE="${ROOTFS_IMAGE:-$ROOT_DIR/out/ext2.img}"
ROOTFS_REBUILD="${ROOTFS_REBUILD:-1}"
EXTRA_LINUX_BINS="${EXTRA_LINUX_BINS:-}"
EXTRA_FREEBSD_BINS="${EXTRA_FREEBSD_BINS:-}"
ROOTFS_APPS="${ROOTFS_APPS:-}"

case "$ROOTFS_IMAGE" in
  /*) ;;
  *) ROOTFS_IMAGE="$ROOT_DIR/$ROOTFS_IMAGE" ;;
esac

case "$ARCH" in
  x86-64|x86_64)
    TARGET_ARCH="x86_64"
    SPENCER_ARCH="x86-64"
    PLATFORM="${PLATFORM:-pc99}"
    ;;
  aarch64)
    TARGET_ARCH="aarch64"
    SPENCER_ARCH="aarch64"
    PLATFORM="${PLATFORM:-qemu}"
    ;;
  *)
    echo "[nanami-build] ARCH must be x86-64, x86_64, or aarch64" >&2
    exit 1
    ;;
esac

case "$TARGET_ARCH/$PLATFORM" in
  x86_64/pc99|aarch64/qemu|aarch64/rpi4b) ;;
  *)
    echo "[nanami-build] unsupported target: $TARGET_ARCH/$PLATFORM (use x86_64/pc99, aarch64/qemu, or aarch64/rpi4b)" >&2
    exit 1
    ;;
esac

TARGET_NAME="${TARGET_ARCH}-unknown-a9n"
TARGET_JSON="$ROOT_DIR/out/targets/${TARGET_NAME}.json"

PROFILE_ARGS=()
if [ "$PROFILE" = "release" ]; then
  PROFILE_ARGS+=(--release)
elif [ "$PROFILE" != "debug" ]; then
  echo "[nanami-build] PROFILE must be release or debug" >&2
  exit 1
fi

mkdir -p "$(dirname "$TARGET_JSON")"
sed "s#../spencer/Nun/arch/${TARGET_ARCH}/user.ld#$SPENCER_DIR/Nun/arch/${TARGET_ARCH}/user.ld#g" \
  "$ROOT_DIR/targets/${TARGET_NAME}.json" > "$TARGET_JSON"

echo "[nanami-build] build user-space initramfs"
if [ "$TARGET_ARCH" = "aarch64" ]; then
  make -C "$NANAMI_DIR/servers" ARCH="$TARGET_ARCH" initramfs-rust-only
else
  make -C "$NANAMI_DIR/servers" ARCH="$TARGET_ARCH" initramfs
fi

SPENCER_ROOTFS_ARGS=()
if [ "$TARGET_ARCH" = "x86_64" ]; then
  if [ "$ROOTFS_REBUILD" = "1" ] || [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "[nanami-build] create Nanami root filesystem: $ROOTFS_IMAGE"
    EXTRA_LINUX_BINS="$EXTRA_LINUX_BINS" EXTRA_FREEBSD_BINS="$EXTRA_FREEBSD_BINS" ROOTFS_APPS="$ROOTFS_APPS" \
      NANAMI_TARGET_ARCH="$TARGET_ARCH" \
      "$ROOT_DIR/scripts/create-ext2-image.sh" "${SIZE_MB:-64}" "$ROOTFS_IMAGE"
  fi
  if [ ! -f "$ROOTFS_IMAGE" ]; then
    echo "[nanami-build] root filesystem not found: $ROOTFS_IMAGE" >&2
    exit 1
  fi
  SPENCER_ROOTFS_ARGS+=(--rootfs-image "$ROOTFS_IMAGE")
fi

echo "[nanami-build] delegate image build to Spencer xtask"
(
  cd "$SPENCER_DIR"
  CARGO_TARGET_DIR="$ROOT_DIR/out/spencer-xtask-target" cargo xtask build \
    --arch "$SPENCER_ARCH" \
    --platform "$PLATFORM" \
    --enable-smp \
    "${PROFILE_ARGS[@]}" \
    --os-manifest "$NANAMI_DIR/Cargo.toml" \
    --os-target-json "$TARGET_JSON" \
    --os-binary nanami \
    "${SPENCER_ROOTFS_ARGS[@]}"
)

echo "[nanami-build] image ready: $SPENCER_DIR/out/${TARGET_ARCH}-${PLATFORM}-${PROFILE}/spencer.img"
