#!/usr/bin/env python3
"""Verify a stopped --writeback-smoke disk, without mounting or modifying it."""
import argparse
import pathlib
import struct
import subprocess
import tempfile
import zlib
from late_root import ROOT_TYPE

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('image', type=pathlib.Path)
parser.add_argument('--debugfs', default='debugfs')
args = parser.parse_args()
assert args.image.is_file(), 'Only regular QEMU image files are accepted'
with args.image.open('rb') as image, tempfile.TemporaryDirectory(prefix='nanami-writeback-check-') as work:
    image.seek(512)
    header = bytearray(image.read(512))
    assert header[:8] == b'EFI PART'
    size, checksum = struct.unpack_from('<II', header, 12)
    assert 92 <= size <= 512
    struct.pack_into('<I', header, 16, 0)
    assert zlib.crc32(header[:size]) == checksum
    entry_lba, count, stride, checksum = struct.unpack_from('<QIII', header, 72)
    assert stride >= 128 and 0 < count * stride <= 16384
    image.seek(entry_lba * 512)
    entries = image.read(count * stride)
    assert zlib.crc32(entries) == checksum
    roots = [struct.unpack_from('<QQ', entries, offset + 32)
             for offset in range(0, len(entries), stride)
             if entries[offset:offset + 16] == ROOT_TYPE]
    assert len(roots) == 1
    first, last = roots[0]
    assert 0 < first <= last < args.image.stat().st_size // 512
    image.seek(first * 512)
    root = pathlib.Path(work) / 'root.ext2'
    remaining = (last - first + 1) * 512
    with root.open('xb') as out:
        while remaining:
            data = image.read(min(remaining, 1024 * 1024))
            assert data
            out.write(data)
            remaining -= len(data)

    def contents(name):
        return subprocess.run([args.debugfs, '-R', f'cat /alter/linux/{name}', str(root)],
                              check=True, capture_output=True).stdout

    data = contents('writeback-data')
    assert len(data) == 256 * 8192, f'persisted size mismatch: {len(data)}'
    for block in range(256):
        expected = bytes((block + index * 17) & 255 for index in range(8192))
        assert data[block * 8192:(block + 1) * 8192] == expected, f'persisted block {block} mismatch'
    assert contents('writeback-sync') == bytes([0x73]) * 8192
print('PASS: offline disk contains the exact fsync and O_SYNC payloads (2 MiB + 8 KiB)')
