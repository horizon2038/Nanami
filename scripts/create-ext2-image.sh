#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: create-ext2-image.sh <size-mb> [output.img]

Creates an empty raw ext2 image for Nanami's virtio-blk/ext2-server path.
The default output path is out/ext2.img.

Examples:
  ./scripts/create-ext2-image.sh 8
  ./scripts/create-ext2-image.sh 64 out/disk.img
USAGE
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
  usage
  exit 0
fi

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  usage
  exit 1
fi

SIZE_MB="$1"
OUT="${2:-out/ext2.img}"

case "$SIZE_MB" in
  ''|*[!0-9]*)
    echo "[ext2-image] size-mb must be a positive integer" >&2
    exit 1
    ;;
esac

if [ "$SIZE_MB" -lt 1 ]; then
  echo "[ext2-image] size-mb must be >= 1" >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"

MKFS=""
if command -v mke2fs >/dev/null 2>&1; then
  MKFS="mke2fs"
elif command -v mkfs.ext2 >/dev/null 2>&1; then
  MKFS="mkfs.ext2"
fi

if [ -n "$MKFS" ]; then
  truncate -s "${SIZE_MB}M" "$OUT"
  "$MKFS" -q -F -t ext2 -b 1024 -O filetype "$OUT"
  echo "[ext2-image] created: $OUT size=${SIZE_MB}MiB via $MKFS"
  exit 0
fi

python3 - "$SIZE_MB" "$OUT" <<'PY'
import math
import os
import struct
import sys

size_mb = int(sys.argv[1])
out = sys.argv[2]
block_size = 1024
blocks_count = size_mb * 1024 * 1024 // block_size
if blocks_count < 256:
    raise SystemExit("[ext2-image] fallback requires at least 256 blocks")

blocks_per_group = 8192
inodes_per_group = 1024
inode_size = 128
inode_table_blocks = (inodes_per_group * inode_size + block_size - 1) // block_size
groups = (blocks_count + blocks_per_group - 1) // blocks_per_group
gdt_blocks = (groups * 32 + block_size - 1) // block_size
inodes_count = groups * inodes_per_group

EXT2_SUPER_MAGIC = 0xEF53
EXT2_VALID_FS = 1
EXT2_ERRORS_CONTINUE = 1
EXT2_GOOD_OLD_REV = 0
EXT2_DYNAMIC_REV = 1
EXT2_FEATURE_INCOMPAT_FILETYPE = 0x0002
EXT2_ROOT_INO = 2
EXT2_GOOD_OLD_FIRST_INO = 11
EXT2_S_IFDIR = 0x4000
EXT2_FT_DIR = 2

image = bytearray(blocks_count * block_size)

def w16(off, value):
    struct.pack_into('<H', image, off, value & 0xffff)

def w32(off, value):
    struct.pack_into('<I', image, off, value & 0xffffffff)

def mark_bitmap(bitmap, index):
    image[bitmap + index // 8] |= 1 << (index % 8)

def block_group_start(group):
    return group * blocks_per_group

def group_block_count(group):
    start = block_group_start(group)
    return max(0, min(blocks_per_group, blocks_count - start))

gdt = []
used_blocks_total = 0
used_inodes_total = 0
used_dirs_total = 1
root_block = None

for group in range(groups):
    start = block_group_start(group)
    count = group_block_count(group)
    if count == 0:
        continue
    meta_start = start
    if group == 0:
        # Block 0 is the boot block. Block 1 contains the primary superblock.
        # The group descriptor table starts at block 2 for 1KiB ext2.
        meta_start = 2 + gdt_blocks
    block_bitmap = meta_start
    inode_bitmap = block_bitmap + 1
    inode_table = inode_bitmap + 1
    first_data = inode_table + inode_table_blocks
    if first_data > start + count:
        raise SystemExit("[ext2-image] image too small for fallback ext2 metadata")

    used_blocks = 0
    bitmap_off = block_bitmap * block_size
    # Mark non-existent blocks in the last group as used.
    idx = count
    while idx < blocks_per_group:
        mark_bitmap(bitmap_off, idx)
        idx += 1

    def use_block(abs_block):
        nonlocal_used[0] += 1
        mark_bitmap(bitmap_off, abs_block - start)

    nonlocal_used = [0]
    if group == 0:
        for block in range(0, 2 + gdt_blocks):
            use_block(block)
    use_block(block_bitmap)
    use_block(inode_bitmap)
    for block in range(inode_table, inode_table + inode_table_blocks):
        use_block(block)
    if group == 0:
        root_block = first_data
        use_block(root_block)

    used_blocks = nonlocal_used[0]
    used_blocks_total += used_blocks

    inode_bitmap_off = inode_bitmap * block_size
    used_inodes = 0
    if group == 0:
        # ext2 reserves inode 1..10; inode 2 is the root directory.
        for ino_index in range(EXT2_GOOD_OLD_FIRST_INO - 1):
            mark_bitmap(inode_bitmap_off, ino_index)
            used_inodes += 1
    used_inodes_total += used_inodes

    gdt.append({
        'block_bitmap': block_bitmap,
        'inode_bitmap': inode_bitmap,
        'inode_table': inode_table,
        'free_blocks': count - used_blocks,
        'free_inodes': inodes_per_group - used_inodes,
        'used_dirs': 1 if group == 0 else 0,
    })

free_blocks_count = blocks_count - used_blocks_total
free_inodes_count = inodes_count - used_inodes_total

# Superblock at byte 1024.
s = 1024
w32(s + 0, inodes_count)
w32(s + 4, blocks_count)
w32(s + 8, 0)  # reserved blocks
w32(s + 12, free_blocks_count)
w32(s + 16, free_inodes_count)
w32(s + 20, 1)  # first data block for 1KiB ext2
w32(s + 24, 0)  # log block size
w32(s + 28, 0)  # log fragment size
w32(s + 32, blocks_per_group)
w32(s + 36, blocks_per_group)
w32(s + 40, inodes_per_group)
w32(s + 44, 0)  # mtime
w32(s + 48, 0)  # wtime
w16(s + 52, 0)  # mount count
w16(s + 54, 0xffff)  # max mount count
w16(s + 56, EXT2_SUPER_MAGIC)
w16(s + 58, EXT2_VALID_FS)
w16(s + 60, EXT2_ERRORS_CONTINUE)
w16(s + 62, 0)  # minor revision
w32(s + 64, 0)  # last check
w32(s + 68, 0)  # check interval
w32(s + 72, 0)  # creator OS Linux
w32(s + 76, EXT2_DYNAMIC_REV)
w16(s + 80, 0)
w16(s + 82, 0)
w32(s + 84, EXT2_GOOD_OLD_FIRST_INO)
w16(s + 88, inode_size)
w16(s + 90, 0)  # block group number
w32(s + 92, 0)  # compatible features
w32(s + 96, EXT2_FEATURE_INCOMPAT_FILETYPE)
w32(s + 100, 0)  # readonly compatible features
image[s + 104:s + 120] = b'NanamiExt2Image!'
volume = b'Nanami ext2'
image[s + 120:s + 120 + len(volume)] = volume

# Group descriptor table.
gdt_off = 2 * block_size
for i, desc in enumerate(gdt):
    off = gdt_off + i * 32
    w32(off + 0, desc['block_bitmap'])
    w32(off + 4, desc['inode_bitmap'])
    w32(off + 8, desc['inode_table'])
    w16(off + 12, desc['free_blocks'])
    w16(off + 14, desc['free_inodes'])
    w16(off + 16, desc['used_dirs'])

# Root inode (inode 2) in group 0.
root_inode_off = gdt[0]['inode_table'] * block_size + (EXT2_ROOT_INO - 1) * inode_size
w16(root_inode_off + 0, EXT2_S_IFDIR | 0o755)
w16(root_inode_off + 2, 0)
w32(root_inode_off + 4, block_size)
w32(root_inode_off + 8, 0)
w32(root_inode_off + 12, 0)
w32(root_inode_off + 16, 0)
w32(root_inode_off + 20, 0)
w16(root_inode_off + 24, 0)
w16(root_inode_off + 26, 2)
w32(root_inode_off + 28, 2)  # i_blocks in 512-byte sectors
w32(root_inode_off + 40, root_block)

# Root directory block.
def write_dirent(off, inode, rec_len, name, file_type):
    name_b = name.encode('ascii')
    w32(off + 0, inode)
    w16(off + 4, rec_len)
    image[off + 6] = len(name_b)
    image[off + 7] = file_type
    image[off + 8:off + 8 + len(name_b)] = name_b

root_off = root_block * block_size
write_dirent(root_off, EXT2_ROOT_INO, 12, '.', EXT2_FT_DIR)
write_dirent(root_off + 12, EXT2_ROOT_INO, block_size - 12, '..', EXT2_FT_DIR)

with open(out, 'wb') as f:
    f.write(image)

print(f"[ext2-image] created: {out} size={size_mb}MiB via built-in fallback")
print(f"[ext2-image] blocks={blocks_count} groups={groups} inodes={inodes_count} root_block={root_block}")
PY
