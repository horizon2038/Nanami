#!/usr/bin/env python3
"""Run the existing integer-only Linux workload under an unmodified Linux kernel.

Offline: callers supply the official Tiny Core kernel/modules and pinned rootfs.
Only a test PID 1 is appended to the initramfs. The real benchmark, BusyBox and
dynamic libraries are read from the supplied Nanami disk via AHCI/ext2, read-only.
This is a process-path control, not a full desktop-readiness measurement.
"""
import argparse
import gzip
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
ROOTFS_SHA256 = "4eff7d1c521a77f00da07c56dd80d2c9ca4d667bf0919bcb7f05f86bf663a6ae"
METRICS = {
    "getpid-2048", "getuid-2048", "copyout-normal", "copyout-128-vmas",
    "copyout-256-vmas", "fork-8", "fork-exec-8", "fork-128-vmas-8",
    "fork-256-vmas-8",
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def init_overlay(script):
    """A deterministic one-file newc archive; no host extraction/device nodes."""
    result = bytearray()
    for inode, name, mode, data in [
        (1, "nanami-control-init", 0o100755, script),
        (2, "TRAILER!!!", 0, b""),
    ]:
        encoded = name.encode() + b"\0"
        fields = [inode, mode, 0, 0, 1, 0, len(data), 0, 0, 0, 0, len(encoded), 0]
        result += b"070701" + b"".join(f"{value:08x}".encode() for value in fields)
        result += encoded
        result += b"\0" * (-len(result) % 4)
        result += data
        result += b"\0" * (-len(result) % 4)
    result += b"\0" * (-len(result) % 512)
    return bytes(result)


def parse_metrics(text):
    values = {}
    for key, value in re.findall(r"PROCESS-PERF ([a-z0-9-]+)-ns=(\d+)", text):
        values.setdefault(key, []).append(int(value))
    if set(values) != METRICS or any(
        len(samples) != 3 or any(value <= 0 for value in samples) for samples in values.values()
    ):
        raise ValueError("expected exactly three samples of all nine Linux metrics")
    if "PROCESS-PERFORMANCE-PASS" not in text or "LINUX-CONTROL-PASS" not in text:
        raise ValueError("missing complete Linux control markers")
    if "LINUX-CONTROL-FAIL" in text or "Kernel panic" in text:
        raise ValueError("failed Linux control")
    return values


def command(args, serial, firmware_vars, initrd):
    firmware = REPO / "spencer/a9nloader-rs/tools"
    result = [
        "qemu-system-x86_64", "-machine", "hpet=on", "-cpu", "max", "-smp", str(args.smp),
        "-m", "4G", "-display", "none", "-serial", f"file:{serial}", "-monitor", "stdio",
        "-netdev", "user,id=net0", "-device",
        "virtio-net-pci,netdev=net0,addr=5,disable-legacy=off,disable-modern=on",
        "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=0",
        "-device", "qemu-xhci,id=xhci,addr=6,msi=off,msix=off",
        "-device", "usb-kbd,bus=xhci.0,port=1", "-device", "usb-mouse,bus=xhci.0,port=2",
        "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
        "-drive", f"if=pflash,format=raw,file={firmware_vars}",
        "-device", "ich9-ahci,id=ahci,addr=3",
        # ide-hd needs writable media. Its writes stay in a discarded snapshot;
        # PID1 independently mounts the ext2 partition read-only, as recorded.
        "-drive", f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}",
        "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1",
        "-kernel", str(args.kernel.resolve()), "-initrd", str(initrd),
        "-append", "console=ttyS0 rdinit=/nanami-control-init loglevel=3 panic=-1",
        "-no-reboot", "-no-shutdown",
    ]
    if args.tcg_threads is not None:
        result += ["-accel", f"tcg,thread={args.tcg_threads}"]
    return result


def run(args):
    for path in (args.kernel, args.modules, args.rootfs, args.image):
        if not path.is_file():
            raise ValueError(f"not a regular input file: {path}")
    if sha256(args.rootfs) != ROOTFS_SHA256:
        raise ValueError("Tiny Core rootfs does not match the project's pinned17.1 input")
    if args.kernel.read_bytes()[0x202:0x206] != b"HdrS":
        raise ValueError("not an x86 Linux boot image")
    args.output.mkdir(parents=True, exist_ok=False)
    hashes = {name: sha256(path) for name, path in [
        ("kernel", args.kernel), ("modules", args.modules),
        ("rootfs", args.rootfs), ("image", args.image),
    ]}
    script = pathlib.Path(__file__).with_name("linux-control-init.sh").read_bytes()
    initrd = args.output / "control-initrd.cpio.gz"
    # Linux supports independently terminated concatenated compressed archives:
    # https://docs.kernel.org/driver-api/early-userspace/buffer-format.html
    with initrd.open("xb") as stream:
        for path in (args.rootfs, args.modules):
            with path.open("rb") as source:
                shutil.copyfileobj(source, stream)
        stream.write(gzip.compress(init_overlay(script), mtime=0))
    hashes["initrd"] = sha256(initrd)
    firmware_vars = args.output / "OVMF_VARS.fd"
    shutil.copyfile(REPO / "spencer/a9nloader-rs/tools/OVMF_VARS.fd", firmware_vars)
    serial = args.output / "serial.log"
    argv = command(args, serial, firmware_vars, initrd)
    manifest = {"inputs_sha256": hashes, "command": argv, "smp": args.smp}
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Linux control logs: {args.output}", flush=True)
    with (args.output / "qemu.log").open("w") as diagnostics:
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=diagnostics, stderr=diagnostics)
        try:
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                text = serial.read_text(errors="replace") if serial.exists() else ""
                if "LINUX-CONTROL-FAIL" in text or "Kernel panic" in text:
                    raise RuntimeError("Linux control failed; see serial.log")
                if "LINUX-CONTROL-PASS" in text:
                    values = parse_metrics(text)
                    (args.output / "metrics.json").write_text(json.dumps(values, indent=2) + "\n")
                    print(json.dumps(values), flush=True)
                    return
                if process.poll() is not None:
                    raise RuntimeError("QEMU exited before control completion")
                time.sleep(0.1)
            raise TimeoutError("Linux control timed out; see serial.log")
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kernel", type=pathlib.Path, required=True)
    parser.add_argument("--modules", type=pathlib.Path, required=True)
    parser.add_argument("--rootfs", type=pathlib.Path, required=True)
    parser.add_argument("--image", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--smp", type=int, choices=[1, 4], default=4)
    parser.add_argument("--tcg-threads", choices=["single", "multi"])
    parser.add_argument("--timeout", type=float, default=120)
    run(parser.parse_args())


if __name__ == "__main__":
    main()
