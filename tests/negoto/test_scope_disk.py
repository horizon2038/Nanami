import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest

from scope_disk import extract, root_partition, validate_packet, validate_workload_packet


def packet(mode):
    words = [0] * 1034
    words[:8] = [0x00315043534e414e, mode + 1, mode, 10, 100, 1, 513, 1]
    rows = {39: 7, 57: 1, 60: 1, 61: 1, 157: 2, 512: 1} if mode == 0 else {39: 5}
    for number, calls in rows.items():
        words[8 + number * 2] = calls
    if mode:
        words[9 + 39 * 2] = 40
    return struct.pack("<1034Q", *words)


class ScopeDiskTests(unittest.TestCase):
    def test_workload_rows_preserve_unknown_calls_and_reject_bad_subtotals(self):
        words = list(struct.unpack("<1034Q", packet(1)))
        words[1] = 1  # App observer owns the first scope; fixture time is epoch2.
        words[8 + 512 * 2] = 2
        words[9 + 512 * 2] = 10
        result = validate_workload_packet(struct.pack("<1034Q", *words), 1)
        self.assertEqual(result["total_calls"], 7)
        self.assertEqual(result["inclusive_ticks"], 50)
        self.assertEqual(result["rows"]["512"], dict(calls=2, ticks=10))
        self.assertFalse(result["performance_admitted"])
        for index, value in [(0, 0), (1, 2), (2, 0), (4, 9), (5, 0), (6, 512), (7, 0),
                             (8 + 512 * 2, 0), (9 + 512 * 2, 51)]:
            changed = words.copy()
            changed[index] = value
            with self.subTest(index=index), self.assertRaises(ValueError):
                validate_workload_packet(struct.pack("<1034Q", *changed), 1)
        with self.assertRaises(ValueError):
            validate_workload_packet(packet(1), 0)

    def test_all_packet_rows_and_headers_are_checked(self):
        for mode in (0, 1):
            original = packet(mode)
            self.assertEqual(validate_packet(original, mode)["mode"], mode)
            for index in (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 8 + 39 * 2, 9 + 39 * 2, 1033):
                bad = bytearray(original)
                value = 0 if index == 4 else 2**64 - 1
                bad[index * 8:index * 8 + 8] = value.to_bytes(8, "little")
                with self.subTest(mode=mode, index=index), self.assertRaises(ValueError):
                    validate_packet(bad, mode)
            with self.assertRaises(ValueError):
                validate_packet(original[:-1], mode)

    @unittest.skipUnless(all(shutil.which(x) for x in ("qemu-img", "debugfs", "mke2fs")), "filesystem tools unavailable")
    def test_real_overlay_extraction_is_exact_and_preserves_backing(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            root = directory / "root.img"
            with root.open("wb") as stream:
                stream.truncate(8 * 1024 * 1024)
            def run(argv):
                subprocess.run(argv, check=True, capture_output=True)
            run(["mke2fs", "-q", "-t", "ext2", "-F", str(root)])
            for path in ("/alter", "/alter/linux", "/alter/linux/tinycore", "/alter/linux/tinycore/rootfs", "/alter/linux/tinycore/rootfs/tmp"):
                run(["debugfs", "-w", "-R", "mkdir " + path, str(root)])
            for mode, name in enumerate(("count", "time")):
                source = directory / f"packet-{mode}"
                source.write_bytes(packet(mode))
                run(["debugfs", "-w", "-R", f"write {source} /alter/linux/tinycore/rootfs/tmp/scope-{name}.bin", str(root)])
            disk = directory / "disk.img"
            header = bytearray(2048 * 512)
            header[512:520] = b"EFI PART"
            struct.pack_into("<QII", header, 512 + 72, 2, 1, 128)
            struct.pack_into("<QQ", header, 1024 + 32, 2048, 2048 + root.stat().st_size // 512 - 1)
            label = "Nanami Root".encode("utf-16le")
            header[1024+56:1024+56+len(label)] = label
            disk.write_bytes(header + root.read_bytes())
            before = hashlib.sha256(disk.read_bytes()).hexdigest()
            self.assertEqual(root_partition(disk), (len(header), root.stat().st_size))
            overlay = directory / "overlay.qcow2"
            run(["qemu-img", "create", "-f", "qcow2", "-F", "raw", "-b", str(disk), str(overlay)])
            output = directory / "output"
            output.mkdir()
            result = extract(overlay, output, True)
            for mode in (0, 1):
                data = (output / f"scope-packet-{mode}.bin").read_bytes()
                self.assertEqual(data, packet(mode))
                self.assertEqual(result[f"scope-packet-{mode}.bin"]["sha256"], hashlib.sha256(data).hexdigest())
            self.assertEqual(hashlib.sha256(disk.read_bytes()).hexdigest(), before)
            self.assertEqual(list(output.glob("*.img")), [])
            receipt = output / "scope-disk-extraction.json"
            before_receipt = receipt.read_bytes()
            exported = Path(json.loads(before_receipt)[0]["argv"][-1])
            self.assertFalse(exported.parent.exists())
            with self.assertRaises(ValueError):
                extract(overlay, output, True)
            self.assertEqual(receipt.read_bytes(), before_receipt)
            off_output = directory / "incorrect-off"
            off_output.mkdir()
            with self.assertRaises(ValueError):
                extract(overlay, off_output, False)
            self.assertEqual(list(off_output.glob("*.img")), [])
            self.assertTrue((off_output / "scope-disk-extraction.json").is_file())
            failed_commands = json.loads((off_output / "scope-disk-extraction.json").read_text())
            self.assertFalse(Path(failed_commands[0]["argv"][-1]).parent.exists())
            self.assertTrue(overlay.is_file())
