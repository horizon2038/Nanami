import hashlib
import pathlib
import struct
import tempfile
import unittest
import zlib

import files_paint


def chunk(kind, payload):
    return (len(payload).to_bytes(4, "big") + kind + payload
            + zlib.crc32(kind + payload).to_bytes(4, "big"))


def image(rows, modes, color=2, extra_stream=b"", width=3):
    bpp = 3 if color == 2 else 4
    previous = bytes(width * bpp)
    raw = bytearray()
    for row, mode in zip(rows, modes):
        raw.append(mode)
        for index, value in enumerate(row):
            left = row[index - bpp] if index >= bpp else 0
            up = previous[index]
            corner = previous[index - bpp] if index >= bpp else 0
            estimates = (0, left, up, (left + up) // 2)
            if mode == 4:
                prediction = left + up - corner
                candidates = (left, up, corner)
                predictor = min(candidates, key=lambda candidate: abs(prediction - candidate))
            elif mode < 4:
                predictor = estimates[mode]
            else:
                predictor = 0
            raw.append((value - predictor) % 256)
        previous = row
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, len(rows), 8, color, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw) + extra_stream)
            + chunk(b"IEND", b""))


class FilesPaintTests(unittest.TestCase):
    def test_every_filter_and_cross_row_prediction(self):
        rows = [bytes((index * 73 + row * 101) % 256 for index in range(9))
                for row in range(5)]
        width, height, pixels = files_paint.decode_png(image(rows, range(5)))
        self.assertEqual((width, height), (3, 5))
        self.assertEqual(pixels, b"".join(rows))
        cropped = b"".join(row[3:9] for row in rows[1:4])
        self.assertEqual(files_paint.region_hash(width, height, pixels, (1, 1, 3, 4)),
                         hashlib.sha256(cropped).hexdigest())

    def test_opaque_rgba_has_same_rgb(self):
        rows = [bytes([1, 3, 5, 255, 7, 11, 13, 255, 17, 19, 23, 255])]
        _, _, pixels = files_paint.decode_png(image(rows, [4], color=6))
        self.assertEqual(pixels, bytes([1, 3, 5, 7, 11, 13, 17, 19, 23]))

    def test_transparency_is_not_silently_discarded(self):
        rows = [bytes([1, 3, 5, 0] * 3)]
        with self.assertRaisesRegex(ValueError, "nonopaque"):
            files_paint.decode_png(image(rows, [0], color=6))

    def test_corrupt_crc_and_truncated_chunk_fail(self):
        data = bytearray(image([bytes(range(9))], [0]))
        data[-1] ^= 1
        with self.assertRaisesRegex(ValueError, "CRC"):
            files_paint.decode_png(data)
        with self.assertRaises(ValueError):
            files_paint.decode_png(data[:20])

    def test_scanline_stream_end_and_length_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "scanline filter"):
            files_paint.decode_png(image([bytes(range(9))], [5]))
        with self.assertRaisesRegex(ValueError, "stream"):
            files_paint.decode_png(image([bytes(range(9))], [0], extra_stream=b"tail"))
        data = image([bytes(range(9))], [0])
        with self.assertRaisesRegex(ValueError, "end"):
            files_paint.decode_png(data + b"tail")
        with self.assertRaisesRegex(ValueError, "length"):
            files_paint.decode_png(image([bytes(range(8))], [0]))

    def test_header_limits_and_unsupported_critical_chunks(self):
        header = chunk(b"IHDR", struct.pack(">IIBBBBB", 4096, 2161, 8, 2, 0, 0, 0))
        with self.assertRaisesRegex(ValueError, "dimensions"):
            files_paint.decode_png(b"\x89PNG\r\n\x1a\n" + header)
        data = image([bytes(range(9))], [0])
        with self.assertRaisesRegex(ValueError, "critical"):
            files_paint.decode_png(data[:33] + chunk(b"ABCD", b"") + data[33:])
        with self.assertRaisesRegex(ValueError, "transparency"):
            files_paint.decode_png(data[:33] + chunk(b"tRNS", bytes(6)) + data[33:])

    def test_rectangle_rejects_missing_or_malformed_pixels(self):
        with self.assertRaises(ValueError):
            files_paint.region_hash(3, 1, bytes(9))
        with self.assertRaises(ValueError):
            files_paint.region_hash(3, 1, bytes(8), (0, 0, 3, 1))

    def test_missing_and_invalid_files_stay_unverified(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            (directory / "app-launch-files.png").write_bytes(b"not png")
            report = files_paint.audit_timeline(directory)
        self.assertFalse(report["painted_input_readiness_verified"])
        self.assertFalse(report["performance_admitted"])
        self.assertEqual(len(report["screenshots"]), 6)
        self.assertTrue(all("error" in item and not item["reference_matches"]
                            for item in report["screenshots"]))

    def test_single_input_metadata_does_not_assert_visual_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            (directory / "app-launch-single-input.json").write_text(
                '{"painted_input_readiness_verified": true}')
            report = files_paint.audit_timeline(directory)
        self.assertEqual(len(report["screenshots"]), 8)
        self.assertEqual([item["expected_state"] for item in report["screenshots"][-2:]],
                         ["root-location-empty", "root-location-empty"])
        self.assertFalse(any(item["reference_matches"] for item in report["screenshots"]))
        self.assertFalse(report["painted_input_readiness_verified"])


if __name__ == "__main__":
    unittest.main()
