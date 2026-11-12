"""Write the locked x86-64 profile's glibc cache without executing guest ELF.

The old cache layout and flags are defined by glibc's dl-cache.h/ldconfig.h.
Only the profile's three flat library directories are indexed; host libraries,
ld.so.conf includes and CPU-specific subdirectories never enter this cache.
"""
from functools import cmp_to_key
from pathlib import Path, PurePosixPath
import struct

DIRECTORIES = ("usr/local/lib", "usr/lib", "lib")


def guest_file(root, name):
    """Resolve both absolute and relative links inside the guest namespace."""
    remaining = list(PurePosixPath(name).parts)
    resolved = []
    links = 0
    while remaining:
        part = remaining.pop(0)
        if part in ("/", "."):
            continue
        if part == "..":
            if not resolved:
                raise ValueError(f"library link escapes guest root: {name}")
            resolved.pop()
            continue
        path = root.joinpath(*resolved, part)
        if path.is_symlink():
            links += 1
            if links > 40:
                raise ValueError(f"library link cycle: {name}")
            target = path.readlink()
            if target.is_absolute():
                resolved = []
            remaining = list(target.parts) + remaining
        else:
            resolved.append(part)
    return root.joinpath(*resolved)


def soname(data):
    """Read DT_SONAME from a bounded ELF64 little-endian x86-64 DSO."""
    if not data.startswith(b"\x7fELF"):
        return None  # linker scripts and non-ELF auxiliary files
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("expected an ELF64 little-endian library")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    if header[1:4] != (3, 62, 1):
        raise ValueError("expected an x86-64 shared library")
    offset, size, count = header[5], header[9], header[10]
    if size != 56 or not count or offset + size * count > len(data):
        raise ValueError("invalid library program headers")
    loads, dynamic = [], None
    for index in range(count):
        kind, _, start, address, _, length, _, _ = struct.unpack_from("<IIQQQQQQ", data, offset + index * size)
        if start + length > len(data):
            raise ValueError("library segment is truncated")
        if kind == 1:
            loads.append((address, start, length))
        elif kind == 2:
            dynamic = (start, length)
    if dynamic is None or dynamic[1] % 16:
        raise ValueError("invalid library dynamic section")
    tags = {}
    for offset in range(dynamic[0], sum(dynamic), 16):
        tag, value = struct.unpack_from("<qQ", data, offset)
        if tag == 0:
            break
        tags[tag] = value
    else:
        raise ValueError("unterminated library dynamic section")
    if 14 not in tags:
        return None
    if 5 not in tags or 10 not in tags or tags[14] >= tags[10]:
        raise ValueError("invalid library SONAME string table")
    for address, start, length in loads:
        delta = tags[5] - address
        if 0 <= delta and delta + tags[10] <= length:
            strings = data[start + delta:start + delta + tags[10]]
            end = strings.find(b"\0", tags[14])
            if end == -1:
                break
            name = strings[tags[14]:end].decode("ascii")
            if not name or "/" in name or name in (".", ".."):
                break
            return name
    raise ValueError("invalid library SONAME")


def libcmp(left, right):
    """glibc searches SONAMEs in descending order, comparing digits numerically."""
    a = b = 0
    while a < len(left) and b < len(right):
        if left[a].isdigit() != right[b].isdigit():
            return 1 if left[a].isdigit() else -1
        if left[a].isdigit() and right[b].isdigit():
            end_a, end_b = a, b
            while end_a < len(left) and left[end_a].isdigit():
                end_a += 1
            while end_b < len(right) and right[end_b].isdigit():
                end_b += 1
            difference = int(left[a:end_a]) - int(right[b:end_b])
            if difference:
                return difference
            a, b = end_a, end_b
        else:
            difference = ord(left[a]) - ord(right[b])
            if difference:
                return difference
            a += 1
            b += 1
    return (a < len(left)) - (b < len(right))


def generate(root):
    root = Path(root)
    entries = []
    for directory in DIRECTORIES:
        found = set()
        for path in sorted((root / directory).iterdir()):
            if ".so" not in path.name:
                continue
            target = guest_file(root, f"{directory}/{path.name}")
            if not target.is_file():
                continue
            data = target.read_bytes()
            name = soname(data)
            if name is None and not data.startswith(b"\x7fELF"):
                continue
            name = name or path.name
            if path.is_symlink() and (path.name == name or
                    (path.name.endswith(".so") and name.startswith(path.name))):
                name = path.name
            if name in found:
                continue
            alias = guest_file(root, f"{directory}/{name}")
            if not alias.is_file() or alias.read_bytes() != data:
                raise ValueError(f"library SONAME link is missing or inconsistent: {directory}/{name}")
            found.add(name)
            entries.append((name, f"/{directory}/{name}"))
    # Python's stable sort preserves the explicit search-directory priority.
    entries.sort(key=cmp_to_key(lambda a, b: libcmp(b[0], a[0])))
    strings, offsets = bytearray(), {}
    def intern(value):
        if value not in offsets:
            offsets[value] = len(strings)
            strings.extend(value.encode("ascii") + b"\0")
        return offsets[value]
    records = [struct.pack("<iII", 0x303, intern(name), intern(path)) for name, path in entries]
    return b"ld.so-1.7.0\0" + struct.pack("<I", len(entries)) + b"".join(records) + strings
