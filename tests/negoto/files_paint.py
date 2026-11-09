"""Conservative, post-VM comparison with audited Files fixture states.

Matching pixels establish a captured state, not an input latency or launch PASS.
References: session-handoff/logs/linux-native-scope-20261106/paint-oracle-audit/.
Single-input reference: the adjacent single-input/ audit.
"""
import hashlib
import json
import pathlib
import struct
import zlib

CLIENT = (103, 111, 846, 593)
REFERENCES = {
    "root": "2d9a405b2eda7042d223dc0c3b75d0b8cfa846d300f04eaa5db9c496fedd9f86",
    "usr-local": "0c85ad46018d954c45ef18fea91eda57c9f0efc16cb94e247aad9467ce7deb70",
    # Visually audited single Ctrl-L result in KVM run ale_x5c8 (+500ms).
    "root-location-empty": "029873f07acfdd805eaa1b87029e02e0290f3c13b07de5a8406ed47be420c428",
}
MAX_PIXELS = 4096 * 2160
MAX_FILE_BYTES = 64 * 1024 * 1024


def decode_png(data):
    """Decode only bounded, opaque RGB/RGBA8 noninterlaced PNGs; fail closed."""
    if len(data) > MAX_FILE_BYTES or data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("invalid PNG signature or file size")
    offset = 8
    header = None
    compressed = bytearray()
    ended = False
    data_ended = False
    seen_data = False
    while offset < len(data):
        if offset + 12 > len(data):
            raise ValueError("truncated PNG chunk")
        length = int.from_bytes(data[offset:offset + 4], "big")
        end = offset + 12 + length
        if end > len(data):
            raise ValueError("truncated PNG payload")
        kind = data[offset + 4:offset + 8]
        payload = data[offset + 8:end - 4]
        if zlib.crc32(kind + payload) != int.from_bytes(data[end - 4:end], "big"):
            raise ValueError("PNG CRC mismatch")
        if header is None and kind != b"IHDR":
            raise ValueError("PNG header must be first")
        if seen_data and kind != b"IDAT":
            data_ended = True
        if kind == b"IHDR":
            if header is not None or length != 13:
                raise ValueError("invalid PNG header")
            header = struct.unpack(">IIBBBBB", payload)
            width, height, depth, color, compression, filtering, interlace = header
            if (width == 0 or height == 0 or width * height > MAX_PIXELS
                    or depth != 8 or color not in (2, 6)
                    or compression != 0 or filtering != 0 or interlace != 0):
                raise ValueError("unsupported PNG format or dimensions")
        elif kind == b"IDAT":
            if data_ended:
                raise ValueError("nonconsecutive PNG image chunks")
            seen_data = True
            compressed.extend(payload)
        elif kind == b"IEND":
            if length != 0 or not seen_data or end != len(data):
                raise ValueError("invalid PNG end")
            ended = True
            break
        elif kind == b"tRNS" or not kind[0] & 32:
            raise ValueError("unsupported PNG transparency or critical chunk")
        offset = end
    if not ended:
        raise ValueError("missing PNG end")
    bpp = 3 if color == 2 else 4
    stride = width * bpp
    expected = (stride + 1) * height
    inflater = zlib.decompressobj()
    raw = inflater.decompress(compressed, expected + 1)
    if (len(raw) != expected or not inflater.eof
            or inflater.unconsumed_tail or inflater.unused_data):
        raise ValueError("invalid PNG decompressed length or stream")
    previous = bytearray(stride)
    pixels = bytearray()
    for y in range(height):
        start = y * (stride + 1)
        mode = raw[start]
        if mode > 4:
            raise ValueError("invalid PNG scanline filter")
        row = bytearray(raw[start + 1:start + 1 + stride])
        for x in range(stride):
            left = row[x - bpp] if x >= bpp else 0
            up = previous[x]
            corner = previous[x - bpp] if x >= bpp else 0
            if mode == 1:
                predictor = left
            elif mode == 2:
                predictor = up
            elif mode == 3:
                predictor = (left + up) // 2
            elif mode == 4:
                value = left + up - corner
                dl, du, dc = abs(value - left), abs(value - up), abs(value - corner)
                if dl <= du and dl <= dc:
                    predictor = left
                elif du <= dc:
                    predictor = up
                else:
                    predictor = corner
            else:
                predictor = 0
            row[x] = (row[x] + predictor) & 255
        if bpp == 4:
            if any(value != 255 for value in row[3::4]):
                raise ValueError("nonopaque PNG pixels")
            pixels.extend(value for x, value in enumerate(row) if x % 4 != 3)
        else:
            pixels.extend(row)
        previous = row
    return width, height, pixels


def region_hash(width, height, pixels, rectangle=CLIENT):
    x0, y0, x1, y1 = rectangle
    if (len(pixels) != width * height * 3 or not 0 <= x0 < x1 <= width
            or not 0 <= y0 < y1 <= height):
        raise ValueError("invalid client rectangle or RGB size")
    digest = hashlib.sha256()
    for y in range(y0, y1):
        digest.update(pixels[(y * width + x0) * 3:(y * width + x1) * 3])
    return digest.hexdigest()


def inspect(path, expected_state):
    result = {"screenshot": path.name, "expected_state": expected_state,
              "reference_matches": False}
    try:
        if path.stat().st_size > MAX_FILE_BYTES:
            raise ValueError("PNG file too large")
        data = path.read_bytes()
        result["png_sha256"] = hashlib.sha256(data).hexdigest()
        width, height, pixels = decode_png(data)
        result.update(width=width, height=height)
        digest = region_hash(width, height, pixels)
        result["client_sha256"] = digest
        result["reference_matches"] = digest == REFERENCES[expected_state]
    except (OSError, ValueError, zlib.error) as error:
        result["error"] = str(error)
    return result


def audit_timeline(directory):
    """Call after QEMU has exited. This function does not infer completion time."""
    names = [("app-launch-files.png", "root")]
    names += [(f"app-launch-files-after-ping-{delay}ms.png", "root")
              for delay in (200, 500, 1000, 2000)]
    names.append(("app-launch-files-keyboard-challenge.png", "usr-local"))
    if (directory / "app-launch-single-input.json").is_file():
        names += [(f"app-launch-files-clear-location-{delay}ms.png", "root-location-empty")
                  for delay in (50, 500)]
    return {"client_rectangle": CLIENT, "references": REFERENCES,
            "painted_input_readiness_verified": False,
            "performance_admitted": False,
            "interpretation": "Exact captured fixture states only; no latency or input readiness inference",
            "screenshots": [inspect(directory / name, state) for name, state in names]}


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=pathlib.Path)
    options = parser.parse_args()
    print(json.dumps(audit_timeline(options.directory), indent=2))
