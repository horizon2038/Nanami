#!/bin/sh
set -eu

ROOT_DIR=$(cd "$(dirname "$0")" && pwd)
STAGE_DIR="$ROOT_DIR/build/initramfs"
OUT="$ROOT_DIR/initramfs.cpio"
APPS="${APPS:-all}"
EXTRA_LINUX_BINS="${EXTRA_LINUX_BINS:-}"
EXTRA_FREEBSD_BINS="${EXTRA_FREEBSD_BINS:-}"

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/bin"
mkdir -p "$STAGE_DIR/nanami"

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

for manifest in boot-list; do
    if [ -f "$ROOT_DIR/$manifest" ]; then
        cp "$ROOT_DIR/$manifest" "$STAGE_DIR/nanami/$manifest"
        echo "[initramfs] + $STAGE_DIR/nanami/$manifest (from $ROOT_DIR/$manifest)"
        copy_count=$((copy_count + 1))
    fi
done

resolve_extra_linux_binary() {
    path="$1"
    case "$path" in
        /*)
            [ -f "$path" ] && printf '%s\n' "$path" && return 0
            ;;
        *)
            if [ -f "$path" ]; then
                printf '%s\n' "$path"
                return 0
            fi
            if [ -f "$ROOT_DIR/$path" ]; then
                printf '%s\n' "$ROOT_DIR/$path"
                return 0
            fi
            if [ -f "$ROOT_DIR/../../$path" ]; then
                printf '%s\n' "$ROOT_DIR/../../$path"
                return 0
            fi
            ;;
    esac
    return 1
}

for app_dir in "$ROOT_DIR"/core-services/* "$ROOT_DIR"/core-services/*/*; do
    [ -d "$app_dir" ] || continue
    app_rel=${app_dir#"$ROOT_DIR"/core-services/}
    app_name=$(printf '%s' "$app_rel" | tr '/' '-')

    # C++ app outputs: core-services/<name>/build/*.elf
    if [ -f "$app_dir/Makefile" ] && is_selected "$app_name" cpp; then
        for elf in "$app_dir"/build/*.elf; do
            [ -f "$elf" ] || continue
            name=$(basename "$elf")
            dst="$STAGE_DIR/bin/${name%.elf}"
            cp "$elf" "$dst"
            echo "[initramfs] + $dst (from $elf)"
            copy_count=$((copy_count + 1))
        done
    fi

    # Rust app outputs: core-services/<name>/target/.../release/<crate-name>
    if [ -f "$app_dir/Cargo.toml" ] && is_selected "$app_name" rust; then
        crate_name=$(sed -n 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)"/\1/p' "$app_dir/Cargo.toml" | head -n 1)
        if [ -n "$crate_name" ]; then
            bin="$app_dir/target/x86_64-unknown-a9n/release/$crate_name"
            if [ -f "$bin" ]; then
                dst="$STAGE_DIR/bin/$app_name"
                cp "$bin" "$dst"
                echo "[initramfs] + $dst (from $bin)"
                copy_count=$((copy_count + 1))
            fi
        fi
    fi

done

if [ -n "$EXTRA_LINUX_BINS" ]; then
    echo "[initramfs] warning: EXTRA_LINUX_BINS is ignored for initramfs; rootfs image owns Linux binaries" >&2
fi
if [ -n "$EXTRA_FREEBSD_BINS" ]; then
    echo "[initramfs] warning: EXTRA_FREEBSD_BINS is ignored for initramfs; rootfs image owns FreeBSD binaries" >&2
fi

if [ "$copy_count" -eq 0 ]; then
    echo "[initramfs] warning: no binaries selected (APPS=$APPS)"
fi

(
    cd "$STAGE_DIR"
    find . -print | LC_ALL=C sort | cpio -o -H newc --quiet
) > "$OUT"

echo "[initramfs] created: $OUT entries=$copy_count apps=$APPS"
