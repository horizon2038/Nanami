#!/usr/bin/env python3
"""Make separate off/active functional images without rebuilding the Kernel."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def root_partition(image):
    with image.open("rb") as stream:
        stream.seek(512)
        header = stream.read(512)
        if header[:8] != b"EFI PART":
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
        if offset < table + count * size or length <= 0 or offset + length > image.stat().st_size:
            raise ValueError("root partition outside image")
        return offset, length


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["image", "rootfs", "alter", "probe", "fixture", "config", "output"]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--observer", type=Path, help="optional separate diagnostic aw-scope ELF")
    args = parser.parse_args()
    inputs = {name: getattr(args, name).resolve() for name in
              ["image", "rootfs", "alter", "probe", "fixture", "config"]}
    if args.observer is not None:
        inputs["observer"] = args.observer.resolve()
    if not all(path.is_file() for path in inputs.values()):
        parser.error("inputs must be regular files")
    for name in ["alter", "probe", "fixture"] + (["observer"] if args.observer is not None else []):
        header = inputs[name].read_bytes()[:20]
        if header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 62:
            parser.error(f"{name} must be a little-endian x86_64 ELF")
    offset, length = root_partition(inputs["image"])
    with inputs["image"].open("rb") as stream:
        stream.seek(offset)
        original_root = stream.read(length)
    if inputs["rootfs"].stat().st_size != length or hashlib.sha256(original_root).hexdigest() != digest(inputs["rootfs"]):
        parser.error("rootfs does not match frozen image partition")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)  # Never overwrite another trial.
    log = dict(inputs={name: dict(path=str(path), sha256=digest(path)) for name, path in inputs.items()}, commands=[])

    def command(argv):
        result = subprocess.run(argv, capture_output=True, text=True)
        log["commands"].append(dict(argv=argv, status=result.returncode,
                                    stdout=result.stdout, stderr=result.stderr))
        result.check_returncode()
        return result.stdout

    def debugfs(root, request):
        return command(["debugfs", "-w", "-R", request, str(root)])

    def put(root, source, destination, executable):
        # debugfs may return zero on command errors: verify actual readback.
        debugfs(root, f"rm {destination}")
        debugfs(root, f"write {json.dumps(str(source))} {destination}")
        if executable:
            debugfs(root, f"set_inode_field {destination} mode 0100755")
        dumped = output / "readback"
        if dumped.exists():
            dumped.unlink()
        debugfs(root, f"dump {destination} {json.dumps(str(dumped))}")
        if not dumped.is_file() or digest(dumped) != digest(source):
            raise ValueError(f"failed readback: {destination}")

    try:
        control = output / "off-rootfs.img"
        active = output / "active-rootfs.img"
        shutil.copyfile(inputs["rootfs"], control)
        put(control, inputs["probe"], "/alter/linux/bin/negoto-smoke", True)
        put(control, inputs["fixture"], "/alter/linux/tinycore/rootfs/bin/nanami-syscall-scope", True)
        if "observer" in inputs:
            put(control, inputs["observer"], "/alter/linux/tinycore/rootfs/bin/aw-scope", True)
        config = json.loads(inputs["config"].read_text())
        config["process"]["args"] = ["/bin/nanami-syscall-scope"]
        config_file = output / "config.json"
        config_file.write_text(json.dumps(config, indent=2) + "\n")
        debugfs(control, "mkdir /alter/linux/nanami-syscall-scope")
        put(control, config_file, "/alter/linux/nanami-syscall-scope/config.json", False)
        shutil.copyfile(control, active)
        put(active, inputs["alter"], "/bin/alter-linux", True)
        log["outputs"] = {}
        for label, root in [("off", control), ("active", active)]:
            command(["e2fsck", "-fn", str(root)])
            image = output / (label + ".img")
            shutil.copyfile(inputs["image"], image)
            with root.open("rb") as source, image.open("r+b") as target:
                target.seek(offset)
                shutil.copyfileobj(source, target)
            with inputs["image"].open("rb") as original, image.open("rb") as patched:
                if original.read(offset) != patched.read(offset):
                    raise ValueError("boot/ESP bytes changed")
                if hashlib.sha256(patched.read(length)).hexdigest() != digest(root):
                    raise ValueError("packaged rootfs differs")
                original.seek(offset + length)
                if original.read() != patched.read():
                    raise ValueError("image trailer changed")
            log["outputs"][label] = dict(image_sha256=digest(image), rootfs_sha256=digest(root))
        if any(digest(path) != log["inputs"][name]["sha256"] for name, path in inputs.items()):
            raise ValueError("input changed during packaging")
    finally:
        (output / "package-log.json").write_text(json.dumps(log, indent=2) + "\n")
    print(f"Scope functional images: {output}")


if __name__ == "__main__":
    main()
