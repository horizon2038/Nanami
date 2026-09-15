#!/usr/bin/env python3
"""Boot a snapshot, exercise Alter through the shell, and check guest exit codes."""
import argparse
import pathlib
import os
import re
import select
import shutil
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, default=4)
parser.add_argument("--program", action="append", help="run only this guest basename (repeatable)")
parser.add_argument("--ui-stress", action="store_true", help="exercise scrollback wrap, page scrolling, clear, and command history; retain screenshots for inspection")
args = parser.parse_args()
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-alter-test-"))
firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
print(f"Test logs: {logs}", flush=True)
command = [
    "qemu-system-x86_64", "-machine", "hpet=on", "-cpu", "max", "-smp", str(args.smp),
    "-m", "4G", "-display", "none", "-serial", f"file:{serial}",
    "-monitor", "stdio", "-nic", "none",
    "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
    "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
    "-device", "ich9-ahci,id=ahci,addr=3",
    "-drive", f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}",
    "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1", "--no-reboot", "--no-shutdown",
]
monitor = None
with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def wait_for(predicate, timeout=120):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("QEMU exited; see qemu.log")
                text = serial.read_text(errors="replace") if serial.exists() else ""
                result = predicate(text)
                if result:
                    return result
                time.sleep(0.2)
            raise RuntimeError("Guest timed out; see serial.log and screen.png")

        wait_for(lambda text: re.search(r"\[\s*shell\] font init end", text)
                 and "[              honoka] online" in text)
        # Font initialization precedes creating/focusing the shell window.
        time.sleep(2)
        monitor = process.stdin

        def send(command):
            monitor.write(command.encode() + b"\n")
            monitor.flush()
            # Drain the prompt so HMP cannot block on a full pipe.
            received = b""
            while b"(qemu)" not in received:
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise RuntimeError("QEMU monitor timed out")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError("QEMU monitor closed")
                received += chunk

        def type_command(command):
            keys = {" ": "spc", "/": "slash", "-": "minus", ".": "dot", "\n": "ret"}
            for character in command + "\n":
                send(f"sendkey {keys.get(character, character)} 50")
                time.sleep(0.35)

        for program in args.program or ["linux-syscall-smoke", "glibc-true", "glibc-regression"]:
            start = serial.stat().st_size
            type_command(f"alter -t /alter/linux/bin/{program}")
            def guest_exit(text):
                text = text[start:]
                spawned = re.search(r"managed rootfs process image=" + re.escape(program) + r" pid=(\d+)", text)
                if not spawned:
                    return None
                return re.search(r"exit pid=" + spawned[1] + r" syscall=\d+ status=(\d+)", text)
            exited = wait_for(guest_exit)
            if exited[1] != "0":
                raise RuntimeError(f"{program} exited with status {exited[1]}")
            print(f"PASS {program} (smp={args.smp})", flush=True)
            time.sleep(1)
        if args.ui_stress:
            type_command("clear")
            # Each help adds eight rows: exceed the shell's 128-row capacity.
            for _ in range(18):
                type_command("help")
            type_command("echo scroll-bottom-marker")
            # The current shell recognizes unextended navigation scancodes.
            # Keypad keys exercise those paths; PS/2 extended-key handling is
            # a pre-existing, separate limitation.
            send("sendkey kp_9 50")
            time.sleep(1)
            send(f"screendump {logs / 'scroll-up.png'} -f png")
            send("sendkey kp_3 50")
            time.sleep(1)
            send(f"screendump {logs / 'scroll-bottom.png'} -f png")
            type_command("clear")
            type_command("echo ring-cache-ok")
            send("sendkey kp_8 50")
            time.sleep(0.5)
            send("sendkey ret 50")
            send("mouse_move -120 -80")
            time.sleep(1)
            print(f"UI stress finished; inspect scroll-up.png, scroll-bottom.png, screen.png (smp={args.smp})", flush=True)
        send(f"screendump {logs / 'screen.png'} -f png")
    finally:
        if monitor is not None:
            try:
                monitor.write(f"screendump {logs / 'screen.png'} -f png\nquit\n".encode())
                monitor.flush()
            except OSError:
                pass
            monitor.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
