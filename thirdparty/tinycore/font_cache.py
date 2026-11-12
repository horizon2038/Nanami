"""Compile pinned font facts into Fontconfig's le64.cache-9 format.

The desktop package graph fixes the fonts and configuration. Its reviewed facts
are data, not host-dependent cache binaries. Normal builds verify every input
and serialize these facts using only Python's standard library. No font scanner,
guest ELF execution or native build tool is needed on either supported host.
"""
import hashlib
import json
from pathlib import Path
import struct

INPUTS = ("usr/local/etc/fonts", "usr/local/share/fontconfig", "usr/local/lib/X11/fonts")
LOCK = Path(__file__).with_name("font-cache.lock.json")
PACKAGE_LOCK = Path(__file__).with_name("desktop.lock.json")


def fingerprint(root):
    digest = hashlib.sha256()
    for relative in INPUTS:
        directory = root / relative
        for path in [directory, *sorted(directory.rglob("*"))]:
            name = path.relative_to(root).as_posix()
            if path.is_symlink():
                value = [name, "link", str(path.readlink())]
            elif path.is_dir():
                value = [name, "directory"]
            elif path.is_file():
                value = [name, "file", hashlib.sha256(path.read_bytes()).hexdigest()]
            else:
                raise ValueError(f"unsupported font cache input: {name}")
            digest.update((json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode())
    return digest.hexdigest()


class Writer:
    def __init__(self):
        self.data = bytearray(64)
        self.shared = {}

    def alloc(self, size):
        self.data.extend(b"\0" * (-len(self.data) % 8))
        offset = len(self.data)
        self.data.extend(b"\0" * size)
        return offset

    def put(self, offset, fmt, *values):
        struct.pack_into("<" + fmt, self.data, offset, *values)

    def string(self, value):
        key = ("string", value)
        if key not in self.shared:
            encoded = value.encode() + b"\0"
            offset = self.alloc(len(encoded))
            self.data[offset:offset + len(encoded)] = encoded
            self.shared[key] = offset
        return self.shared[key]

    def charset(self, pages):
        key = ("charset", json.dumps(pages))
        if key in self.shared:
            return self.shared[key]
        offset = self.alloc(24)
        leaves, numbers = self.alloc(8 * len(pages)), self.alloc(2 * len(pages))
        self.put(offset, "iiqq", -1, len(pages), leaves - offset, numbers - offset)
        for index, (page, bits) in enumerate(pages):
            leaf_key = ("leaf", *bits)
            if leaf_key not in self.shared:
                leaf = self.alloc(32)
                self.put(leaf, "8I", *bits)
                self.shared[leaf_key] = leaf
            self.put(leaves + index * 8, "q", self.shared[leaf_key] - leaves)
            self.put(numbers + index * 2, "H", page)
        self.shared[key] = offset
        return offset

    def langset(self, bits):
        key = ("languages", *bits)
        if key not in self.shared:
            offset = self.alloc(12 + 4 * len(bits))
            self.put(offset, "qI", 0, len(bits))
            self.put(offset + 12, str(len(bits)) + "I", *bits)
            self.shared[key] = offset
        return self.shared[key]

    def values(self, values, pool):
        head = previous = 0
        for kind, binding, index in values:
            value = pool[index]
            offset = self.alloc(32)
            if previous:
                self.put(previous, "q", (offset - previous) | 1)
            else:
                head = offset
            self.put(offset + 8, "i", kind)
            self.put(offset + 24, "i", binding)
            if kind in (1, 4):
                self.put(offset + 16, "i", value)
            elif kind == 2:
                self.put(offset + 16, "d", value)
            elif kind in (3, 6, 8):
                target = {3: self.string, 6: self.charset, 8: self.langset}[kind](value)
                self.put(offset + 16, "q", (target - (offset + 8)) | 1)
            else:
                raise ValueError(f"unsupported pinned font value type: {kind}")
            previous = offset
        return head

    def pattern(self, elements, pool):
        offset = self.alloc(24)
        array = self.alloc(len(elements) * 16)
        self.put(offset, "iiqi", len(elements), len(elements), array - offset, -1)
        for index, (object_id, values) in enumerate(elements):
            entry = array + index * 16
            head = self.values(values, pool)
            self.put(entry, "i4xq", object_id, (head - entry) | 1)
        return offset


def compile_cache(directory, pool):
    writer = Writer()
    path = writer.string(directory["path"])
    children = directory["subdirs"]
    child_array = writer.alloc(8 * len(children))
    for index, child in enumerate(children):
        writer.put(child_array + index * 8, "q", writer.string(child) - child_array)
    fonts = directory["fonts"]
    font_set = writer.alloc(16)
    pointers = writer.alloc(8 * len(fonts))
    writer.put(font_set, "iiq", len(fonts), len(fonts), (pointers - font_set) | 1)
    for index, font in enumerate(fonts):
        writer.put(pointers + index * 8, "q", (writer.pattern(font, pool) - font_set) | 1)
    writer.data.extend(b"\0" * (-len(writer.data) % 8))
    writer.put(0, "IIqqqi4xqi4xq", 0xFC02FC04, 9, len(writer.data), path, child_array,
               len(children), font_set, 0, 0)
    return bytes(writer.data)


def generate(root):
    facts = json.loads(LOCK.read_text())
    package = json.loads(PACKAGE_LOCK.read_text())["packages"]["fontconfig.tcz"]["sha256"]
    if facts["fontconfig_package_sha256"] != package:
        raise ValueError("Fontconfig package changed; update the pinned font facts")
    if facts["format"] != "fontconfig-le64-cache-9" or fingerprint(root) != facts["input_sha256"]:
        raise ValueError("font cache inputs changed; update the pinned font facts with the desktop packages")
    return {hashlib.md5(directory["path"].encode(), usedforsecurity=False).hexdigest() + "-le64.cache-9":
            compile_cache(directory, facts["values"]) for directory in facts["directories"]}
