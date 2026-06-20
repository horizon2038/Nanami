#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
NANAMI_DIR="$ROOT_DIR/nanami"
SPENCER_DIR="$ROOT_DIR/spencer"
ARCH="${ARCH:-x86-64}"
PLATFORM="${PLATFORM:-qemu}"
PROFILE="${PROFILE:-release}"
TARGET_JSON="$ROOT_DIR/out/targets/x86_64-unknown-a9n.json"

PROFILE_ARGS=()
if [ "$PROFILE" = "release" ]; then
  PROFILE_ARGS+=(--release)
elif [ "$PROFILE" != "debug" ]; then
  echo "[nanami-build] PROFILE must be release or debug" >&2
  exit 1
fi

mkdir -p "$(dirname "$TARGET_JSON")"
sed "s#../spencer/Nun/arch/x86_64/user.ld#$SPENCER_DIR/Nun/arch/x86_64/user.ld#g" \
  "$ROOT_DIR/targets/x86_64-unknown-a9n.json" > "$TARGET_JSON"

echo "[nanami-build] build user-space initramfs"
make -C "$NANAMI_DIR/servers" initramfs

echo "[nanami-build] delegate image build to Spencer xtask"
(
  cd "$SPENCER_DIR"
  CARGO_TARGET_DIR="$ROOT_DIR/out/spencer-xtask-target" cargo xtask build \
    --arch "$ARCH" \
    --platform "$PLATFORM" \
    "${PROFILE_ARGS[@]}" \
    --os-manifest "$NANAMI_DIR/Cargo.toml" \
    --os-target-json "$TARGET_JSON" \
    --os-binary nanami
)

echo "[nanami-build] image ready: $SPENCER_DIR/out/x86_64-${PLATFORM}-${PROFILE}/spencer.img"
