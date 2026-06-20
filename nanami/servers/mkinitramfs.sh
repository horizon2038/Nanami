#!/bin/sh
set -eu

ROOT_DIR=$(cd "$(dirname "$0")" && pwd)
STAGE_DIR="$ROOT_DIR/build/initramfs"
OUT="$ROOT_DIR/initramfs.cpio"
APPS="${APPS:-all}"

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/bin"

is_selected() {
    app_name="$1"
    app_kind="$2" # cpp | rust

    case ",$APPS," in
        *,all,*) return 0 ;;
    esac

    case ",$APPS," in
        *,$app_name,*) return 0 ;;
    esac

    case ",$APPS," in
        *,cpp,*) [ "$app_kind" = "cpp" ] && return 0 ;;
        *,rust,*) [ "$app_kind" = "rust" ] && return 0 ;;
    esac

    return 1
}

copy_count=0

for app_dir in "$ROOT_DIR"/apps/*; do
    [ -d "$app_dir" ] || continue
    app_name=$(basename "$app_dir")

    # C++ app outputs: apps/<name>/build/*.elf
    if [ -f "$app_dir/Makefile" ] && is_selected "$app_name" cpp; then
        for elf in "$app_dir"/build/*.elf; do
            [ -f "$elf" ] || continue
            dst="$STAGE_DIR/bin/$(basename "$elf")"
            cp "$elf" "$dst"
            echo "[initramfs] + $dst (from $elf)"
            copy_count=$((copy_count + 1))
        done
    fi

    # Rust app outputs: apps/<name>/target/.../release/<crate-name>
    if [ -f "$app_dir/Cargo.toml" ] && is_selected "$app_name" rust; then
        crate_name=$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p' "$app_dir/Cargo.toml" | head -n 1)
        if [ -n "$crate_name" ]; then
            bin="$app_dir/target/x86_64-unknown-a9n/release/$crate_name"
            if [ -f "$bin" ]; then
                dst="$STAGE_DIR/bin/$app_name.elf"
                cp "$bin" "$dst"
                echo "[initramfs] + $dst (from $bin)"
                copy_count=$((copy_count + 1))
            fi
        fi
    fi

done

if [ "$copy_count" -eq 0 ]; then
    echo "[initramfs] warning: no binaries selected (APPS=$APPS)"
fi

(
    cd "$STAGE_DIR"
    find . -print | LC_ALL=C sort | cpio -o -H newc --quiet
) > "$OUT"

echo "[initramfs] created: $OUT entries=$copy_count apps=$APPS"
