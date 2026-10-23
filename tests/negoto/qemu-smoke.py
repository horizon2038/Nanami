#!/usr/bin/env python3
"""Exercise Negoto's real management service and unmodified BusyBox inside QEMU."""
import argparse
import os
import pathlib
import re
import select
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, choices=[1, 4], default=4)
parser.add_argument("--timeout", type=float, default=180)
args = parser.parse_args()
if not args.image.is_file():
    parser.error("--image must be a regular test image")
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-negoto-qemu-"))
firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
print(f"Test logs: {logs}", flush=True)
command = ["qemu-system-x86_64", "-machine", "hpet=on,i8042=off", "-cpu", "max", "-smp", str(args.smp),
           "-m", "4G", "-display", "none", "-serial", f"file:{serial}", "-monitor", "stdio", "-nic", "none",
           "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=0", "-device", "qemu-xhci,id=xhci,addr=6,msi=off,msix=off",
           "-device", "usb-kbd,bus=xhci.0,port=1", "-device", "usb-mouse,bus=xhci.0,port=2",
           "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
           "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
           "-device", "ich9-ahci,id=ahci,addr=3",
           "-drive", f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}",
           "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1", "-no-reboot", "-no-shutdown"]
with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def wait_for(predicate):
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("QEMU exited; see qemu.log")
                text = serial.read_text(errors="replace") if serial.exists() else ""
                if "NEGOTO-SMOKE-FAIL" in text or re.search(r"\[\s*negoto\] panic", text):
                    raise RuntimeError("Negoto regression failed; see serial.log")
                if predicate(text):
                    return
                time.sleep(0.2)
            raise RuntimeError("QEMU timed out; see serial.log")

        def send(value):
            process.stdin.write(value.encode() + b"\n")
            process.stdin.flush()
            data = b""
            while b"(qemu)" not in data:
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise RuntimeError("QEMU monitor timed out")
                data += os.read(process.stdout.fileno(), 4096)

        # SMP serial output can interleave inside Negoto's one-off startup
        # line. The guest verifies its service connection directly; use GUI
        # readiness here only to avoid typing before the shell accepts input.
        wait_for(lambda text: re.search(r"\[\s*shell\] font init end", text)
                 and "[              honoka] online" in text)
        time.sleep(2)
        keys = {"/": "slash", "-": "minus", "\n": "ret"}
        for character in "/alter/linux/bin/negoto-smoke\n":
            send(f"sendkey {keys.get(character, character)} 40")
            time.sleep(0.25)
        wait_for(lambda text: "NEGOTO-SMOKE-PASS reuse=20 cleanup=true" in text)
        text = serial.read_text(errors="replace")
        for marker in ["NEGOTO-LIFECYCLE-PASS", "NEGOTO-READONLY-PASS", "NEGOTO-KILL-PASS", "NEGOTO-CLI-PASS", "NEGOTO-INTERACTIVE-PASS"]:
            if marker not in text:
                raise RuntimeError(f"Missing {marker}")
        print(f"PASS OCI lifecycle, rootfs/cwd/env, fork/exec, readonly, kill, 20 reuse cycles, native CLI and interactive shell (smp={args.smp})", flush=True)
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
