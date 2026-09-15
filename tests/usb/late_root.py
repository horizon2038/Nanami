"""Create a private firmware boot disk with no Nanami root partition.

Only a newly created clone is edited. The supplied image is never opened for
writing; the complete image remains available for later virtual USB attachment.
"""
import pathlib
import shutil
import struct
import subprocess
import sys
import zlib

ROOT_TYPE = bytes.fromhex('616e616e696d534fa0004e414e414d49')


def boot_without_root(source: pathlib.Path, destination: pathlib.Path):
    assert source.is_file() and not destination.exists()
    if sys.platform == 'darwin':
        subprocess.run(['cp', '-c', str(source), str(destination)], check=True)
    else:
        shutil.copyfile(source, destination)
    with destination.open('r+b') as image:
        image.seek(512)
        primary = image.read(512)
        backup_lba = struct.unpack_from('<Q', primary, 32)[0]
        for lba in (1, backup_lba):
            image.seek(lba * 512)
            header = bytearray(image.read(512))
            assert header[:8] == b'EFI PART'
            size, recorded_crc = struct.unpack_from('<II', header, 12)
            assert 92 <= size <= 512
            struct.pack_into('<I', header, 16, 0)
            assert zlib.crc32(header[:size]) == recorded_crc
            entry_lba, count, stride, recorded_crc = struct.unpack_from('<QIII', header, 72)
            assert 128 <= stride and 0 < count * stride <= 16384
            image.seek(entry_lba * 512)
            entries = bytearray(image.read(count * stride))
            assert zlib.crc32(entries) == recorded_crc
            removed = 0
            for offset in range(0, len(entries), stride):
                if entries[offset:offset + 16] == ROOT_TYPE:
                    entries[offset:offset + stride] = bytes(stride)
                    removed += 1
            assert removed == 1
            struct.pack_into('<I', header, 88, zlib.crc32(entries))
            struct.pack_into('<I', header, 16, zlib.crc32(header[:size]))
            image.seek(entry_lba * 512)
            image.write(entries)
            image.seek(lba * 512)
            image.write(header)
