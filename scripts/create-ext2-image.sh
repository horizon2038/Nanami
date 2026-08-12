#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/.." && pwd)

usage() {
  cat >&2 <<'USAGE'
Usage: create-ext2-image.sh <size-mb> [output.img]

Creates a raw ext2 image for Nanami's virtio-blk/ext2-server path.
The default output path is <repo>/out/ext2.img.
apps/ is seeded into /bin under extensionless executable names. If the image cannot contain the selected files,
the writer fails instead of silently skipping entries. Set ROOTFS_APPS to a
space-separated list of app names to intentionally seed only selected apps.
Set EXTRA_LINUX_BINS to a shell-quoted list of external Linux binaries to seed
into /alter/linux/bin. Extra binaries are stored under Linux-visible executable
names: a source named busybox.elf is installed as /alter/linux/bin/busybox.
Set EXTRA_FREEBSD_BINS to a shell-quoted list of external FreeBSD binaries to
seed into /alter/freebsd/bin. Extra binaries are stored under FreeBSD-visible
executable names: a source named sh.elf is installed as /alter/freebsd/bin/sh.

Examples:
  ./scripts/create-ext2-image.sh 8
  ./scripts/create-ext2-image.sh 64 out/disk.img
  EXTRA_LINUX_BINS="qjs lua" ./scripts/create-ext2-image.sh 64 out/ext2.img
  EXTRA_FREEBSD_BINS="sh ls" ./scripts/create-ext2-image.sh 64 out/ext2.img
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
OUT="${2:-$ROOT_DIR/out/ext2.img}"
EXTRA_LINUX_BINS="${EXTRA_LINUX_BINS:-}"
EXTRA_FREEBSD_BINS="${EXTRA_FREEBSD_BINS:-}"

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

python3 - "$SIZE_MB" "$OUT" "$ROOT_DIR" "$EXTRA_LINUX_BINS" "$EXTRA_FREEBSD_BINS" <<'PY'
import math
import os
import shlex
import struct
import sys

size_mb = int(sys.argv[1])
out = sys.argv[2]
root_dir = sys.argv[3]
extra_linux_bins = sys.argv[4]
extra_freebsd_bins = sys.argv[5]
rootfs_apps_filter = os.environ.get('ROOTFS_APPS', '').strip()
servers_dir = os.path.join(root_dir, 'nanami', 'servers')
apps_dir = os.path.join(servers_dir, 'apps')
rust_target_dir = os.environ.get('CARGO_TARGET_DIR', os.path.join(servers_dir, 'target'))
block_size = 1024
blocks_count = size_mb * 1024 * 1024 // block_size
if blocks_count < 256:
    raise SystemExit("[ext2-image] built-in writer requires at least 256 blocks")

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
EXT2_NANAMI_INO = 11
EXT2_BIN_INO = 12
EXT2_SYSTEM_LIST_INO = 13
EXT2_SESSION_LIST_INO = 14
EXT2_ALTER_INO = 15
EXT2_ALTER_LINUX_INO = 16
EXT2_ALTER_LINUX_BIN_INO = 17
EXT2_ALTER_LINUX_ETC_INO = 18
EXT2_ALTER_LINUX_DEV_INO = 19
EXT2_ALTER_LINUX_TMP_INO = 20
EXT2_ALTER_LINUX_USR_INO = 21
EXT2_ALTER_LINUX_USR_BIN_INO = 22
EXT2_ALTER_FREEBSD_INO = 23
EXT2_ALTER_FREEBSD_BIN_INO = 24
EXT2_ALTER_FREEBSD_ETC_INO = 25
EXT2_ALTER_FREEBSD_DEV_INO = 26
EXT2_ALTER_FREEBSD_TMP_INO = 27
EXT2_ALTER_FREEBSD_USR_INO = 28
EXT2_ALTER_FREEBSD_USR_BIN_INO = 29
EXT2_HONOKA_INO = 30
EXT2_FIRST_HONOKA_FILE_INO = 31
EXT2_FIRST_APP_INO = 64
EXT2_S_IFDIR = 0x4000
EXT2_S_IFREG = 0x8000
EXT2_FT_REG_FILE = 1
EXT2_FT_DIR = 2
EXT2_MAX_DIRECT_BLOCKS = 12
EXT2_SINGLE_INDIRECT_INDEX = 12
EXT2_DOUBLE_INDIRECT_INDEX = 13
EXT2_SINGLE_INDIRECT_CAPACITY = block_size // 4
EXT2_DOUBLE_INDIRECT_CAPACITY = EXT2_SINGLE_INDIRECT_CAPACITY * EXT2_SINGLE_INDIRECT_CAPACITY
EXT2_MAX_FILE_BLOCKS = (
    EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY + EXT2_DOUBLE_INDIRECT_CAPACITY
)

image = bytearray(blocks_count * block_size)

def read_optional(path):
    try:
        with open(path, 'rb') as f:
            return f.read()
    except FileNotFoundError:
        return b''

def parse_crate_name(cargo_toml):
    in_package = False
    with open(cargo_toml, 'r', encoding='utf-8') as f:
        for raw in f:
            line = raw.strip()
            if line == '[package]':
                in_package = True
                continue
            if line.startswith('[') and line != '[package]':
                in_package = False
            if in_package and line.startswith('name') and '=' in line:
                value = line.split('=', 1)[1].strip()
                if value.startswith('"') and '"' in value[1:]:
                    return value.split('"', 2)[1]
    return None

def collect_rootfs_binaries():
    binaries = []
    if not os.path.isdir(apps_dir):
        return binaries
    for current, dirs, _files in os.walk(apps_dir):
        dirs[:] = [d for d in dirs if d not in ('target', 'build', '.git')]
        rel = os.path.relpath(current, apps_dir)
        if rel == '.':
            continue
        app_name = rel.replace(os.sep, '-')
        cargo_toml = os.path.join(current, 'Cargo.toml')
        makefile = os.path.join(current, 'Makefile')
        if os.path.isfile(cargo_toml):
            crate_name = parse_crate_name(cargo_toml)
            if crate_name:
                binary_path = os.path.join(
                    rust_target_dir, 'x86_64-unknown-a9n', 'release', crate_name
                )
                if os.path.isfile(binary_path):
                    binaries.append((app_name, binary_path))
        if os.path.isfile(makefile):
            build_dir = os.path.join(current, 'build')
            if os.path.isdir(build_dir):
                for name in sorted(os.listdir(build_dir)):
                    if name.endswith('.elf'):
                        binaries.append((name[:-4], os.path.join(build_dir, name)))
    return binaries

def rootfs_app_allowed(name):
    if not rootfs_apps_filter:
        return True
    wanted = set(rootfs_apps_filter.split())
    app = name[:-4] if name.endswith('.elf') else name
    legacy = app + '.elf'
    return app in wanted or name in wanted or legacy in wanted

def resolve_extra(path, label):
    candidates = []
    if os.path.isabs(path):
        candidates.append(path)
    else:
        candidates.append(os.path.abspath(path))
        candidates.append(os.path.join(root_dir, path))
        candidates.append(os.path.join(root_dir, '..', path))
    for candidate in candidates:
        if os.path.isfile(candidate):
            return candidate
    raise SystemExit(f'[ext2-image] missing extra {label} binary: {path}')

def collect_extra_binaries(raw, label):
    binaries = []
    for token in shlex.split(raw):
        path = resolve_extra(token, label)
        base = os.path.basename(path)
        out_name = base[:-4] if base.endswith('.elf') else base
        binaries.append((out_name, path))
    return binaries

system_list = read_optional(os.path.join(servers_dir, 'system-list'))
session_list = read_optional(os.path.join(servers_dir, 'session-list'))
honoka_theme_dir = os.path.join(apps_dir, 'honoka', 'assets', 'themes')
manifest_files = []
if system_list:
    manifest_files.append(('system-list', EXT2_SYSTEM_LIST_INO, system_list))
if session_list:
    manifest_files.append(('session-list', EXT2_SESSION_LIST_INO, session_list))

honoka_files = []
honoka_config = read_optional(os.path.join(honoka_theme_dir, 'config'))
if not honoka_config:
    raise SystemExit('[ext2-image] missing Honoka theme config')
honoka_files.append(('config', EXT2_FIRST_HONOKA_FILE_INO, honoka_config))
for name in sorted(os.listdir(honoka_theme_dir)):
    if not name.endswith('.theme'):
        continue
    data = read_optional(os.path.join(honoka_theme_dir, name))
    if not data:
        raise SystemExit(f'[ext2-image] empty Honoka theme: {name}')
    inode = EXT2_FIRST_HONOKA_FILE_INO + len(honoka_files)
    if inode >= EXT2_FIRST_APP_INO:
        raise SystemExit('[ext2-image] too many Honoka theme files')
    honoka_files.append((name, inode, data))
if len(honoka_files) == 1:
    raise SystemExit('[ext2-image] no Honoka .theme files found')

def manifest_bin_order():
    order = []
    for data in (system_list, session_list):
        for raw in data.decode('utf-8', errors='ignore').splitlines():
            line = raw.strip()
            if not line or line.startswith('#'):
                continue
            try:
                parts = shlex.split(line)
            except ValueError:
                continue
            if len(parts) >= 3 and parts[2].startswith('/bin/'):
                order.append(os.path.basename(parts[2]))
    return order

rootfs_binaries = []
linux_binaries = []
freebsd_binaries = []
seen_bin_names = set()
candidates = []
for name, path in collect_rootfs_binaries():
    if rootfs_app_allowed(name):
        candidates.append((name, path))
linux_candidates = collect_extra_binaries(extra_linux_bins, 'linux')
freebsd_candidates = collect_extra_binaries(extra_freebsd_bins, 'freebsd')

if not rootfs_apps_filter:
    ordered_bins = manifest_bin_order()
    priority = {name: index for index, name in enumerate(ordered_bins)}
    candidates.sort(
        key=lambda item: (
            priority.get(item[0], len(priority) + 1),
            os.path.getsize(item[1]),
            item[0],
        )
    )

for name, path in candidates:
    if name in seen_bin_names:
        raise SystemExit(f'[ext2-image] duplicate /bin entry: {name}')
    seen_bin_names.add(name)
    data = read_optional(path)
    inode = EXT2_FIRST_APP_INO + len(rootfs_binaries)
    rootfs_binaries.append((name, inode, data))

seen_linux_names = set()
for name, path in linux_candidates:
    if name in seen_linux_names:
        raise SystemExit(f'[ext2-image] duplicate /alter/linux/bin entry: {name}')
    seen_linux_names.add(name)
    data = read_optional(path)
    inode = EXT2_FIRST_APP_INO + len(rootfs_binaries) + len(linux_binaries)
    linux_binaries.append((name, inode, data))

seen_freebsd_names = set()
for name, path in freebsd_candidates:
    if name in seen_freebsd_names:
        raise SystemExit(f'[ext2-image] duplicate /alter/freebsd/bin entry: {name}')
    seen_freebsd_names.add(name)
    data = read_optional(path)
    inode = EXT2_FIRST_APP_INO + len(rootfs_binaries) + len(linux_binaries) + len(freebsd_binaries)
    freebsd_binaries.append((name, inode, data))

for directory, files in (
    ('/bin', rootfs_binaries),
    ('/alter/linux/bin', linux_binaries),
    ('/alter/freebsd/bin', freebsd_binaries),
):
    for name, _inode, data in files:
        needed = (len(data) + block_size - 1) // block_size
        if needed > EXT2_MAX_FILE_BLOCKS:
            limit = EXT2_MAX_FILE_BLOCKS * block_size
            raise SystemExit(
                f"[ext2-image] {directory}/{name} is too large for current ext2-server file mapping "
                f"({len(data)} bytes > {limit} bytes)"
            )

for name, _inode, data in honoka_files:
    if len(data) > EXT2_MAX_DIRECT_BLOCKS * block_size:
        raise SystemExit(f"[ext2-image] /.honoka/{name} is too large")

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
nanami_block = None
bin_block = None
alter_block = None
alter_linux_block = None
alter_linux_bin_block = None
alter_linux_etc_block = None
alter_linux_dev_block = None
alter_linux_tmp_block = None
alter_linux_usr_block = None
alter_linux_usr_bin_block = None
alter_freebsd_block = None
alter_freebsd_bin_block = None
alter_freebsd_etc_block = None
alter_freebsd_dev_block = None
alter_freebsd_tmp_block = None
alter_freebsd_usr_block = None
alter_freebsd_usr_bin_block = None
honoka_block = None
manifest_blocks = {}
honoka_blocks = {}
rootfs_blocks = {}
linux_blocks = {}
freebsd_blocks = {}
rootfs_indirect_blocks = {}
linux_indirect_blocks = {}
freebsd_indirect_blocks = {}
rootfs_double_indirect_blocks = {}
linux_double_indirect_blocks = {}
freebsd_double_indirect_blocks = {}
rootfs_double_indirect_lists = {}
linux_double_indirect_lists = {}
freebsd_double_indirect_lists = {}

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
        raise SystemExit("[ext2-image] image too small for built-in ext2 metadata")

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
        'start': start,
        'count': count,
        'first_data': first_data,
        'used_blocks': used_blocks,
        'used_inodes': used_inodes,
        'free_blocks': count - used_blocks,
        'free_inodes': inodes_per_group - used_inodes,
        'used_dirs': 19 if group == 0 else 0,
    })

def mark_inode_used(inode):
    if inode < 1 or inode > inodes_count:
        raise SystemExit(f"[ext2-image] invalid inode allocation: {inode}")
    group = (inode - 1) // inodes_per_group
    desc = gdt[group]
    bit = (inode - 1) % inodes_per_group
    bitmap_off = desc['inode_bitmap'] * block_size
    mask = 1 << (bit % 8)
    byte_off = bitmap_off + bit // 8
    if (image[byte_off] & mask) != 0:
        raise SystemExit(f"[ext2-image] duplicate inode allocation: {inode}")
    image[byte_off] |= mask
    desc['used_inodes'] += 1

for inode in [
    EXT2_NANAMI_INO,
    EXT2_BIN_INO,
    EXT2_ALTER_INO,
    EXT2_ALTER_LINUX_INO,
    EXT2_ALTER_LINUX_BIN_INO,
    EXT2_ALTER_LINUX_ETC_INO,
    EXT2_ALTER_LINUX_DEV_INO,
    EXT2_ALTER_LINUX_TMP_INO,
    EXT2_ALTER_LINUX_USR_INO,
    EXT2_ALTER_LINUX_USR_BIN_INO,
    EXT2_ALTER_FREEBSD_INO,
    EXT2_ALTER_FREEBSD_BIN_INO,
    EXT2_ALTER_FREEBSD_ETC_INO,
    EXT2_ALTER_FREEBSD_DEV_INO,
    EXT2_ALTER_FREEBSD_TMP_INO,
    EXT2_ALTER_FREEBSD_USR_INO,
    EXT2_ALTER_FREEBSD_USR_BIN_INO,
    EXT2_HONOKA_INO,
]:
    mark_inode_used(inode)
for _name, inode, _data in manifest_files:
    mark_inode_used(inode)
for _name, inode, _data in honoka_files:
    mark_inode_used(inode)
for _name, inode, _data in rootfs_binaries:
    mark_inode_used(inode)
for _name, inode, _data in linux_binaries:
    mark_inode_used(inode)
for _name, inode, _data in freebsd_binaries:
    mark_inode_used(inode)

next_alloc_group = 0
next_alloc_block = gdt[0]['first_data']

def global_use_block(abs_block):
    if abs_block < 0 or abs_block >= blocks_count:
        raise SystemExit(f"[ext2-image] invalid block allocation: {abs_block}")
    group = abs_block // blocks_per_group
    desc = gdt[group]
    index = abs_block - desc['start']
    if (image[desc['block_bitmap'] * block_size + index // 8] & (1 << (index % 8))) != 0:
        raise SystemExit(f"[ext2-image] duplicate block allocation: {abs_block}")
    mark_bitmap(desc['block_bitmap'] * block_size, index)
    desc['used_blocks'] += 1

def alloc_data_block():
    global next_alloc_group, next_alloc_block
    scans = 0
    while scans < groups:
        group = next_alloc_group
        desc = gdt[group]
        block = max(next_alloc_block, desc['first_data'])
        end = desc['start'] + desc['count']
        while block < end:
            index = block - desc['start']
            if (image[desc['block_bitmap'] * block_size + index // 8] & (1 << (index % 8))) == 0:
                global_use_block(block)
                next_alloc_group = group
                next_alloc_block = block + 1
                return block
            block += 1
        next_alloc_group = (group + 1) % groups
        next_alloc_block = gdt[next_alloc_group]['first_data']
        scans += 1
    raise SystemExit("[ext2-image] image too small for rootfs contents")

def alloc_file_blocks(data):
    needed = max(1, (len(data) + block_size - 1) // block_size)
    return [alloc_data_block() for _ in range(needed)]

root_block = alloc_data_block()
nanami_block = alloc_data_block()
bin_block = alloc_data_block()
alter_block = alloc_data_block()
alter_linux_block = alloc_data_block()
alter_linux_bin_block = alloc_data_block()
alter_linux_etc_block = alloc_data_block()
alter_linux_dev_block = alloc_data_block()
alter_linux_tmp_block = alloc_data_block()
alter_linux_usr_block = alloc_data_block()
alter_linux_usr_bin_block = alloc_data_block()
alter_freebsd_block = alloc_data_block()
alter_freebsd_bin_block = alloc_data_block()
alter_freebsd_etc_block = alloc_data_block()
alter_freebsd_dev_block = alloc_data_block()
alter_freebsd_tmp_block = alloc_data_block()
alter_freebsd_usr_block = alloc_data_block()
alter_freebsd_usr_bin_block = alloc_data_block()
honoka_block = alloc_data_block()

for name, _inode, data in manifest_files:
    manifest_blocks[name] = alloc_file_blocks(data)

for name, _inode, data in honoka_files:
    honoka_blocks[name] = alloc_file_blocks(data)

for name, _inode, data in rootfs_binaries:
    blocks = alloc_file_blocks(data)
    rootfs_blocks[name] = blocks
    needed = len(blocks)
    if needed > EXT2_MAX_DIRECT_BLOCKS:
        rootfs_indirect_blocks[name] = alloc_data_block()
    if needed > EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        rootfs_double_indirect_blocks[name] = alloc_data_block()
        remaining = needed - EXT2_MAX_DIRECT_BLOCKS - EXT2_SINGLE_INDIRECT_CAPACITY
        second_level_count = (remaining + EXT2_SINGLE_INDIRECT_CAPACITY - 1) // EXT2_SINGLE_INDIRECT_CAPACITY
        rootfs_double_indirect_lists[name] = [
            alloc_data_block() for _ in range(second_level_count)
        ]

for name, _inode, data in linux_binaries:
    blocks = alloc_file_blocks(data)
    linux_blocks[name] = blocks
    needed = len(blocks)
    if needed > EXT2_MAX_DIRECT_BLOCKS:
        linux_indirect_blocks[name] = alloc_data_block()
    if needed > EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        linux_double_indirect_blocks[name] = alloc_data_block()
        remaining = needed - EXT2_MAX_DIRECT_BLOCKS - EXT2_SINGLE_INDIRECT_CAPACITY
        second_level_count = (remaining + EXT2_SINGLE_INDIRECT_CAPACITY - 1) // EXT2_SINGLE_INDIRECT_CAPACITY
        linux_double_indirect_lists[name] = [
            alloc_data_block() for _ in range(second_level_count)
        ]

for name, _inode, data in freebsd_binaries:
    blocks = alloc_file_blocks(data)
    freebsd_blocks[name] = blocks
    needed = len(blocks)
    if needed > EXT2_MAX_DIRECT_BLOCKS:
        freebsd_indirect_blocks[name] = alloc_data_block()
    if needed > EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        freebsd_double_indirect_blocks[name] = alloc_data_block()
        remaining = needed - EXT2_MAX_DIRECT_BLOCKS - EXT2_SINGLE_INDIRECT_CAPACITY
        second_level_count = (remaining + EXT2_SINGLE_INDIRECT_CAPACITY - 1) // EXT2_SINGLE_INDIRECT_CAPACITY
        freebsd_double_indirect_lists[name] = [
            alloc_data_block() for _ in range(second_level_count)
        ]

used_blocks_total = sum(desc['used_blocks'] for desc in gdt)
used_inodes_total = sum(desc['used_inodes'] for desc in gdt)
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
    w16(off + 12, desc['count'] - desc['used_blocks'])
    w16(off + 14, inodes_per_group - desc['used_inodes'])
    w16(off + 16, desc['used_dirs'])

# Root inode (inode 2) in group 0.
def inode_off(inode):
    if inode < 1 or inode > inodes_count:
        raise SystemExit(f"[ext2-image] invalid inode write: {inode}")
    inode_index = inode - 1
    group = inode_index // inodes_per_group
    group_inode_index = inode_index % inodes_per_group
    return gdt[group]['inode_table'] * block_size + group_inode_index * inode_size

def write_inode(
    inode,
    mode,
    size,
    links,
    blocks,
    indirect_block=0,
    double_indirect_block=0,
    extra_metadata_blocks=0,
):
    off = inode_off(inode)
    w16(off + 0, mode)
    w16(off + 2, 0)
    w32(off + 4, size)
    w32(off + 8, 0)
    w32(off + 12, 0)
    w32(off + 16, 0)
    w32(off + 20, 0)
    w16(off + 24, 0)
    w16(off + 26, links)
    # ext2 i_blocks is counted in 512-byte sectors, not filesystem blocks.
    sector_blocks = (
        len(blocks)
        + (1 if indirect_block else 0)
        + (1 if double_indirect_block else 0)
        + extra_metadata_blocks
    )
    w32(off + 28, sector_blocks * (block_size // 512))
    for i, block in enumerate(blocks[:EXT2_MAX_DIRECT_BLOCKS]):
        w32(off + 40 + i * 4, block)
    if indirect_block:
        w32(off + 40 + EXT2_SINGLE_INDIRECT_INDEX * 4, indirect_block)
    if double_indirect_block:
        w32(off + 40 + EXT2_DOUBLE_INDIRECT_INDEX * 4, double_indirect_block)

def write_file_data(blocks, data):
    for i, block in enumerate(blocks):
        start = i * block_size
        end = min(start + block_size, len(data))
        image[block * block_size:block * block_size + (end - start)] = data[start:end]

write_inode(EXT2_ROOT_INO, EXT2_S_IFDIR | 0o755, block_size, 6, [root_block])
write_inode(EXT2_NANAMI_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [nanami_block])
write_inode(EXT2_BIN_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [bin_block])
write_inode(EXT2_HONOKA_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [honoka_block])
write_inode(EXT2_ALTER_INO, EXT2_S_IFDIR | 0o755, block_size, 3, [alter_block])
write_inode(EXT2_ALTER_LINUX_INO, EXT2_S_IFDIR | 0o755, block_size, 7, [alter_linux_block])
write_inode(EXT2_ALTER_LINUX_BIN_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_linux_bin_block])
write_inode(EXT2_ALTER_LINUX_ETC_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_linux_etc_block])
write_inode(EXT2_ALTER_LINUX_DEV_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_linux_dev_block])
write_inode(EXT2_ALTER_LINUX_TMP_INO, EXT2_S_IFDIR | 0o777, block_size, 2, [alter_linux_tmp_block])
write_inode(EXT2_ALTER_LINUX_USR_INO, EXT2_S_IFDIR | 0o755, block_size, 3, [alter_linux_usr_block])
write_inode(EXT2_ALTER_LINUX_USR_BIN_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_linux_usr_bin_block])
write_inode(EXT2_ALTER_FREEBSD_INO, EXT2_S_IFDIR | 0o755, block_size, 7, [alter_freebsd_block])
write_inode(EXT2_ALTER_FREEBSD_BIN_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_freebsd_bin_block])
write_inode(EXT2_ALTER_FREEBSD_ETC_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_freebsd_etc_block])
write_inode(EXT2_ALTER_FREEBSD_DEV_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_freebsd_dev_block])
write_inode(EXT2_ALTER_FREEBSD_TMP_INO, EXT2_S_IFDIR | 0o777, block_size, 2, [alter_freebsd_tmp_block])
write_inode(EXT2_ALTER_FREEBSD_USR_INO, EXT2_S_IFDIR | 0o755, block_size, 3, [alter_freebsd_usr_block])
write_inode(EXT2_ALTER_FREEBSD_USR_BIN_INO, EXT2_S_IFDIR | 0o755, block_size, 2, [alter_freebsd_usr_bin_block])

for name, inode, data in manifest_files:
    write_inode(inode, EXT2_S_IFREG | 0o644, len(data), 1, manifest_blocks[name])
    write_file_data(manifest_blocks[name], data)

for name, inode, data in honoka_files:
    write_inode(inode, EXT2_S_IFREG | 0o644, len(data), 1, honoka_blocks[name])
    write_file_data(honoka_blocks[name], data)

for name, inode, data in rootfs_binaries:
    blocks = rootfs_blocks[name]
    indirect = rootfs_indirect_blocks.get(name, 0)
    double_indirect = rootfs_double_indirect_blocks.get(name, 0)
    second_level_blocks = rootfs_double_indirect_lists.get(name, [])
    write_inode(
        inode,
        EXT2_S_IFREG | 0o755,
        len(data),
        1,
        blocks,
        indirect,
        double_indirect,
        len(second_level_blocks),
    )
    write_file_data(blocks, data)
    if indirect:
        indirect_off = indirect * block_size
        single_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS:EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY
        ]
        for index, block in enumerate(single_blocks):
            w32(indirect_off + index * 4, block)
    if double_indirect:
        remaining_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        ]
        double_off = double_indirect * block_size
        for first_index, second_level_block in enumerate(second_level_blocks):
            w32(double_off + first_index * 4, second_level_block)
            second_off = second_level_block * block_size
            start_index = first_index * EXT2_SINGLE_INDIRECT_CAPACITY
            end_index = min(start_index + EXT2_SINGLE_INDIRECT_CAPACITY, len(remaining_blocks))
            for second_index, data_block in enumerate(remaining_blocks[start_index:end_index]):
                w32(second_off + second_index * 4, data_block)

for name, inode, data in linux_binaries:
    blocks = linux_blocks[name]
    indirect = linux_indirect_blocks.get(name, 0)
    double_indirect = linux_double_indirect_blocks.get(name, 0)
    second_level_blocks = linux_double_indirect_lists.get(name, [])
    write_inode(
        inode,
        EXT2_S_IFREG | 0o755,
        len(data),
        2,
        blocks,
        indirect,
        double_indirect,
        len(second_level_blocks),
    )
    write_file_data(blocks, data)
    if indirect:
        indirect_off = indirect * block_size
        single_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS:EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY
        ]
        for index, block in enumerate(single_blocks):
            w32(indirect_off + index * 4, block)
    if double_indirect:
        remaining_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        ]
        double_off = double_indirect * block_size
        for first_index, second_level_block in enumerate(second_level_blocks):
            w32(double_off + first_index * 4, second_level_block)
            second_off = second_level_block * block_size
            start_index = first_index * EXT2_SINGLE_INDIRECT_CAPACITY
            end_index = min(start_index + EXT2_SINGLE_INDIRECT_CAPACITY, len(remaining_blocks))
            for second_index, data_block in enumerate(remaining_blocks[start_index:end_index]):
                w32(second_off + second_index * 4, data_block)

for name, inode, data in freebsd_binaries:
    blocks = freebsd_blocks[name]
    indirect = freebsd_indirect_blocks.get(name, 0)
    double_indirect = freebsd_double_indirect_blocks.get(name, 0)
    second_level_blocks = freebsd_double_indirect_lists.get(name, [])
    write_inode(
        inode,
        EXT2_S_IFREG | 0o755,
        len(data),
        2,
        blocks,
        indirect,
        double_indirect,
        len(second_level_blocks),
    )
    write_file_data(blocks, data)
    if indirect:
        indirect_off = indirect * block_size
        single_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS:EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY
        ]
        for index, block in enumerate(single_blocks):
            w32(indirect_off + index * 4, block)
    if double_indirect:
        remaining_blocks = blocks[
            EXT2_MAX_DIRECT_BLOCKS + EXT2_SINGLE_INDIRECT_CAPACITY:
        ]
        double_off = double_indirect * block_size
        for first_index, second_level_block in enumerate(second_level_blocks):
            w32(double_off + first_index * 4, second_level_block)
            second_off = second_level_block * block_size
            start_index = first_index * EXT2_SINGLE_INDIRECT_CAPACITY
            end_index = min(start_index + EXT2_SINGLE_INDIRECT_CAPACITY, len(remaining_blocks))
            for second_index, data_block in enumerate(remaining_blocks[start_index:end_index]):
                w32(second_off + second_index * 4, data_block)

# Root directory block.
def dirent_len(name):
    return (8 + len(name.encode('ascii')) + 3) & ~3

def write_dirent(off, inode, rec_len, name, file_type):
    name_b = name.encode('ascii')
    w32(off + 0, inode)
    w16(off + 4, rec_len)
    image[off + 6] = len(name_b)
    image[off + 7] = file_type
    image[off + 8:off + 8 + len(name_b)] = name_b

def write_directory(block, self_inode, parent_inode, entries):
    base = block * block_size
    records = [
        ('.', self_inode, EXT2_FT_DIR),
        ('..', parent_inode, EXT2_FT_DIR),
    ] + entries
    min_size = sum(dirent_len(name) for name, _inode, _file_type in records)
    if min_size > block_size:
        raise SystemExit("[ext2-image] directory block is too small for selected rootfs entries")
    cursor = base
    for index, (name, inode, file_type) in enumerate(records):
        record_len = block_size - (cursor - base)
        if index + 1 < len(records):
            record_len = dirent_len(name)
        write_dirent(cursor, inode, record_len, name, file_type)
        cursor += record_len

write_directory(root_block, EXT2_ROOT_INO, EXT2_ROOT_INO, [
    ('nanami', EXT2_NANAMI_INO, EXT2_FT_DIR),
    ('bin', EXT2_BIN_INO, EXT2_FT_DIR),
    ('alter', EXT2_ALTER_INO, EXT2_FT_DIR),
    ('.honoka', EXT2_HONOKA_INO, EXT2_FT_DIR),
])
write_directory(nanami_block, EXT2_NANAMI_INO, EXT2_ROOT_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in manifest_files
])
write_directory(bin_block, EXT2_BIN_INO, EXT2_ROOT_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in rootfs_binaries
])
write_directory(honoka_block, EXT2_HONOKA_INO, EXT2_ROOT_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in honoka_files
])
write_directory(alter_block, EXT2_ALTER_INO, EXT2_ROOT_INO, [
    ('linux', EXT2_ALTER_LINUX_INO, EXT2_FT_DIR),
    ('freebsd', EXT2_ALTER_FREEBSD_INO, EXT2_FT_DIR),
])
write_directory(alter_linux_block, EXT2_ALTER_LINUX_INO, EXT2_ALTER_INO, [
    ('bin', EXT2_ALTER_LINUX_BIN_INO, EXT2_FT_DIR),
    ('etc', EXT2_ALTER_LINUX_ETC_INO, EXT2_FT_DIR),
    ('dev', EXT2_ALTER_LINUX_DEV_INO, EXT2_FT_DIR),
    ('tmp', EXT2_ALTER_LINUX_TMP_INO, EXT2_FT_DIR),
    ('usr', EXT2_ALTER_LINUX_USR_INO, EXT2_FT_DIR),
])
write_directory(alter_linux_bin_block, EXT2_ALTER_LINUX_BIN_INO, EXT2_ALTER_LINUX_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in linux_binaries
])
write_directory(alter_linux_etc_block, EXT2_ALTER_LINUX_ETC_INO, EXT2_ALTER_LINUX_INO, [])
write_directory(alter_linux_dev_block, EXT2_ALTER_LINUX_DEV_INO, EXT2_ALTER_LINUX_INO, [])
write_directory(alter_linux_tmp_block, EXT2_ALTER_LINUX_TMP_INO, EXT2_ALTER_LINUX_INO, [])
write_directory(alter_linux_usr_block, EXT2_ALTER_LINUX_USR_INO, EXT2_ALTER_LINUX_INO, [
    ('bin', EXT2_ALTER_LINUX_USR_BIN_INO, EXT2_FT_DIR),
])
write_directory(alter_linux_usr_bin_block, EXT2_ALTER_LINUX_USR_BIN_INO, EXT2_ALTER_LINUX_USR_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in linux_binaries
])
write_directory(alter_freebsd_block, EXT2_ALTER_FREEBSD_INO, EXT2_ALTER_INO, [
    ('bin', EXT2_ALTER_FREEBSD_BIN_INO, EXT2_FT_DIR),
    ('etc', EXT2_ALTER_FREEBSD_ETC_INO, EXT2_FT_DIR),
    ('dev', EXT2_ALTER_FREEBSD_DEV_INO, EXT2_FT_DIR),
    ('tmp', EXT2_ALTER_FREEBSD_TMP_INO, EXT2_FT_DIR),
    ('usr', EXT2_ALTER_FREEBSD_USR_INO, EXT2_FT_DIR),
])
write_directory(alter_freebsd_bin_block, EXT2_ALTER_FREEBSD_BIN_INO, EXT2_ALTER_FREEBSD_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in freebsd_binaries
])
write_directory(alter_freebsd_etc_block, EXT2_ALTER_FREEBSD_ETC_INO, EXT2_ALTER_FREEBSD_INO, [])
write_directory(alter_freebsd_dev_block, EXT2_ALTER_FREEBSD_DEV_INO, EXT2_ALTER_FREEBSD_INO, [])
write_directory(alter_freebsd_tmp_block, EXT2_ALTER_FREEBSD_TMP_INO, EXT2_ALTER_FREEBSD_INO, [])
write_directory(alter_freebsd_usr_block, EXT2_ALTER_FREEBSD_USR_INO, EXT2_ALTER_FREEBSD_INO, [
    ('bin', EXT2_ALTER_FREEBSD_USR_BIN_INO, EXT2_FT_DIR),
])
write_directory(alter_freebsd_usr_bin_block, EXT2_ALTER_FREEBSD_USR_BIN_INO, EXT2_ALTER_FREEBSD_USR_INO, [
    (name, inode, EXT2_FT_REG_FILE) for name, inode, _data in freebsd_binaries
])

with open(out, 'wb') as f:
    f.write(image)

print(f"[ext2-image] created: {out} size={size_mb}MiB via built-in writer")
print(f"[ext2-image] blocks={blocks_count} groups={groups} inodes={inodes_count} root_block={root_block}")
print(f"[ext2-image] seeded /nanami manifests files={len(manifest_files)}")
print(f"[ext2-image] seeded /.honoka files={len(honoka_files)}")
print(f"[ext2-image] seeded /bin apps files={len(rootfs_binaries)}")
print(f"[ext2-image] seeded /alter/linux/bin linux files={len(linux_binaries)}")
print(f"[ext2-image] seeded /alter/freebsd/bin freebsd files={len(freebsd_binaries)}")
PY
