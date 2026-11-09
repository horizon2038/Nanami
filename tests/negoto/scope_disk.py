"""Post-VM extraction of diagnostic packets from an isolated writable overlay."""
import hashlib
import json
from pathlib import Path
import struct
import subprocess


def root_partition(image):
    with Path(image).open("rb") as stream:
        stream.seek(512)
        header = stream.read(512)
        if len(header) != 512 or header[:8] != b"EFI PART":
            raise ValueError("expected GPT image")
        table = struct.unpack_from("<Q", header, 72)[0] * 512
        count, size = struct.unpack_from("<II", header, 80)
        if not (0 < count <= 4096 and 128 <= size <= 4096):
            raise ValueError("invalid GPT table bounds")
        stream.seek(table)
        matches = []
        for _ in range(count):
            entry = stream.read(size)
            if len(entry) != size:
                raise ValueError("truncated GPT table")
            if entry[56:128].decode("utf-16le").rstrip("\0") == "Nanami Root":
                first, last = struct.unpack_from("<QQ", entry, 32)
                matches.append((first * 512, (last - first + 1) * 512))
        if len(matches) != 1:
            raise ValueError("expected exactly one Nanami Root partition")
        offset, length = matches[0]
        if offset < table + count * size or length <= 0 or offset + length > Path(image).stat().st_size:
            raise ValueError("root partition outside image")
        return offset, length


def validate_packet(packet, mode):
    """Independent decoder for the known fixture, including every zero row."""
    if mode not in (0, 1) or len(packet) != 8272:
        raise ValueError("invalid packet size/mode")
    words = struct.unpack("<1034Q", packet)
    if (words[0], words[1], words[2], words[5], words[6], words[7]) != (
            0x00315043534e414e, mode + 1, mode, 1, 513, 1) or words[4] < words[3]:
        raise ValueError("invalid packet header")
    expected = {39: 7, 57: 1, 60: 1, 61: 1, 157: 2, 512: 1} if mode == 0 else {39: 5}
    for number in range(513):
        calls, ticks = words[8 + number * 2:10 + number * 2]
        if calls != expected.get(number, 0):
            raise ValueError(f"unexpected fixture count row {number}")
        if mode == 1 and number == 39:
            if not 0 < ticks <= words[4] - words[3]:
                raise ValueError("timed subtotal outside phase")
        elif ticks:
            raise ValueError("unexpected fixture ticks")
    return dict(epoch=words[1], mode=mode, start=words[3], end=words[4],
                rows={str(n): dict(calls=words[8+n*2], ticks=words[9+n*2]) for n in expected})


def validate_workload_packet(packet, mode):
    if mode not in (0, 1) or len(packet) != 8272:
        raise ValueError("invalid workload packet size/mode")
    words = struct.unpack("<1034Q", packet)
    if (words[0], words[1], words[2], words[5], words[6], words[7]) != (
            0x00315043534e414e, 1, mode, 1, 513, 1) or words[4] < words[3]:
        raise ValueError("invalid workload packet header")
    calls, ticks = words[8::2], words[9::2]
    if not any(calls) or (mode == 0 and any(ticks)) or sum(ticks) > words[4] - words[3]:
        raise ValueError("invalid workload subtotal")
    if any(tick and not count for count, tick in zip(calls, ticks)):
        raise ValueError("ticks without calls")
    return dict(epoch=1, mode=mode, start=words[3], end=words[4],
                rows={str(n): dict(calls=calls[n], ticks=ticks[n]) for n in range(513) if calls[n]},
                total_calls=sum(calls), inclusive_ticks=sum(ticks),
                counter_identity="local-unverified", performance_admitted=False)


def extract(overlay, directory, enabled, application_mode=None):
    """Caller must stop/reap QEMU first. No original image is opened writable."""
    directory = Path(directory)
    commands = []

    def command(argv):
        result = subprocess.run(argv, capture_output=True, text=True)
        commands.append(dict(argv=argv, status=result.returncode,
                             stdout=result.stdout, stderr=result.stderr))
        result.check_returncode()
        return result.stdout + result.stderr

    try:
        raw = directory / "scope-export-disk.img"
        root = directory / "scope-export-rootfs.img"
        if raw.exists() or root.exists():
            raise ValueError("refuse to overwrite earlier extraction")
        command(["qemu-img", "convert", "-f", "qcow2", "-O", "raw", str(overlay), str(raw)])
        offset, length = root_partition(raw)
        with raw.open("rb") as source, root.open("xb") as target:
            source.seek(offset)
            remaining = length
            while remaining:
                chunk = source.read(min(1024 * 1024, remaining))
                if not chunk:
                    raise ValueError("truncated root partition")
                target.write(chunk)
                remaining -= len(chunk)
        packets = {}
        specs = [(mode, f"scope-{name}.bin", f"scope-packet-{mode}.bin")
                 for mode, name in enumerate(("count", "time"))]
        if application_mode is not None:
            if application_mode not in ("off", "count", "time"):
                raise ValueError("invalid application scope mode")
            specs = [(int(application_mode == "time"), "app-syscall-scope.bin", "scope-app-packet.bin")]
        for mode, name, local_name in specs:
            path = f"/alter/linux/tinycore/rootfs/tmp/{name}"
            destination = directory / local_name
            if destination.exists():
                raise ValueError("refuse to overwrite earlier packet")
            stat = command(["debugfs", "-R", f"stat {path}", str(root)])
            if not enabled:
                if f"{path}: File not found by ext2_lookup" not in stat:
                    raise ValueError("disabled fixture packet present or stat failed")
                continue
            command(["debugfs", "-R", f"dump {path} {json.dumps(str(destination))}", str(root)])
            packet = destination.read_bytes()  # debugfs can report failure with exit0.
            decoded = (validate_packet if application_mode is None else validate_workload_packet)(packet, mode)
            packets[destination.name] = dict(bytes=len(packet), sha256=hashlib.sha256(packet).hexdigest(),
                                             decoded=decoded)
        return packets
    finally:
        (directory / "scope-disk-extraction.json").write_text(json.dumps(commands, indent=2) + "\n")
