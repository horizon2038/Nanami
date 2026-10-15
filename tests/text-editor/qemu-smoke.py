#!/usr/bin/env python3
"""Exercise the native editor on a private disk, then inspect persisted text.

Uses HMP over pipes (no Unix socket, host disk passthrough or networking).
The supplied boot image is only read; all guest writes go to a temporary copy.
"""
import argparse
import os
import pathlib
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, default=4)
parser.add_argument("--debugfs", default=shutil.which("debugfs") or "/opt/homebrew/opt/e2fsprogs/sbin/debugfs")
args = parser.parse_args()
if not args.image.is_file():
    parser.error("--image must be a regular image file")
if not pathlib.Path(args.debugfs).is_file():
    parser.error("set --debugfs to the e2fsprogs debugfs executable")
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-editor-test-"))
print(f"Test logs: {logs}", flush=True)
disk = logs / "boot.img"
if sys.platform == "darwin":
    subprocess.run(["cp", "-c", str(args.image.resolve()), str(disk)], check=True)
else:
    shutil.copyfile(args.image, disk)
firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
command = [
    "qemu-system-x86_64", "-machine", "hpet=on,i8042=off", "-cpu", "max", "-smp", str(args.smp),
    "-m", "4G", "-display", "none", "-serial", f"file:{serial}", "-monitor", "stdio", "-nic", "none",
    "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=0",
    "-device", "qemu-xhci,id=xhci,addr=6,msi=off,msix=off",
    "-device", "usb-kbd,bus=xhci.0,port=1", "-device", "usb-mouse,bus=xhci.0,port=2",
    "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
    "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
    "-device", "ich9-ahci,id=ahci,addr=3",
    "-drive", f"if=none,id=disk,format=raw,cache=writeback,file={disk}",
    "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1", "--no-reboot", "--no-shutdown",
]

with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def contents():
            text = serial.read_text(errors="replace") if serial.exists() else ""
            return re.sub(r"\[\s*text-editor\]", "[text-editor]", text)

        def wait_for(predicate, timeout=100):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("QEMU exited; inspect qemu.log")
                text = contents()
                if re.search(r"\[text-editor\] (panic|out of memory|IPC failed)", text):
                    raise RuntimeError("Editor failed; inspect serial.log")
                if predicate(text):
                    return
                time.sleep(0.15)
            raise RuntimeError("Guest timed out; inspect serial.log and screen.png")

        def send(command):
            process.stdin.write(command.encode() + b"\n")
            process.stdin.flush()
            received = b""
            while b"(qemu)" not in received:
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise RuntimeError("QEMU monitor timed out")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError("QEMU monitor closed")
                received += chunk

        def key(value):
            send(f"sendkey {value} 40")
            time.sleep(0.18)

        def type_text(value):
            keys = {" ": "spc", "/": "slash", "-": "minus", ".": "dot", ",": "comma", "\n": "ret", ":": "shift-semicolon"}
            for c in value:
                if c.isupper():
                    key("shift-" + c.lower())
                else:
                    key(keys.get(c, c))

        def screenshot(name):
            time.sleep(0.6)
            send(f"screendump {logs / (name + '.png')} -f png")

        def move(x, y):
            send("mouse_move -4000 -4000")
            # USB mouse reports carry bounded deltas; allow the reset motion to
            # drain before adding the target delta, also under multi-core TCG.
            time.sleep(2)
            send(f"mouse_move {x} {y}")
            time.sleep(1)

        def click(x, y):
            move(x, y)
            send("mouse_button 1")
            time.sleep(0.3)
            send("mouse_button 0")
            time.sleep(0.4)

        wait_for(lambda text: re.search(r"\[\s*shell\] font init end", text)
                 and "[              honoka] online" in text)
        time.sleep(2)
        type_text("text-editor\n")
        wait_for(lambda text: "[text-editor] online" in text)
        time.sleep(1)
        type_text("Notes\n\nA small native text editor.\nOpen, edit, save.\n")
        key("ctrl-z")
        key("ctrl-y")
        screenshot("editing")
        key("ctrl-shift-s")
        type_text("/alter/linux/tmp/editor-note.txt")
        screenshot("save-as")
        key("ret")
        wait_for(lambda text: "[text-editor] saved path=/alter/linux/tmp/editor-note.txt" in text)

        # Find wraps from EOF, selects the match; typing replaces only that range.
        key("ctrl-f")
        type_text("small")
        key("ret")
        screenshot("find")
        key("esc")
        type_text("simple")
        key("ctrl-s")
        wait_for(lambda text: text.count("[text-editor] saved path=/alter/linux/tmp/editor-note.txt") == 2)
        key("ctrl-shift-s")
        type_text("/alter/linux/tmp/editor-full.txt")
        key("ret")
        wait_for(lambda text: "[text-editor] saved path=/alter/linux/tmp/editor-full.txt" in text)

        # Dirty-close must cancel without losing the edit, then Undo restores clean.
        type_text("x")
        key("ctrl-w")
        screenshot("unsaved")
        key("esc")
        key("ctrl-z")
        screenshot("close-cancelled")

        # Exercise visible toolbar hit testing and reloading through the real VFS.
        click(220 + 4 + 95, 120 + 26 + 16)  # Open...
        key("ctrl-a")
        type_text("/alter/linux/tmp/editor-note.txt")
        key("ret")
        wait_for(lambda text: "[text-editor] opened path=/alter/linux/tmp/editor-note.txt" in text)
        screenshot("reopened")

        # Resize via the compositor, including its 72x32 client minimum.
        move(220 + 840 + 4, 120 + 536 + 26)
        send("mouse_button 1")
        send("mouse_move -768 -504")
        time.sleep(0.3)
        send("mouse_button 0")
        wait_for(lambda text: "[text-editor] resized width=72 height=32" in text)
        screenshot("minimum")
        move(220 + 72 + 4, 120 + 32 + 26)
        send("mouse_button 1")
        send("mouse_move 768 504")
        time.sleep(0.3)
        send("mouse_button 0")
        wait_for(lambda text: "[text-editor] resized width=840 height=536" in text)
        screenshot("resized")

        # Save a shorter file, then an empty one: ftruncate must remove the old tail.
        key("ctrl-a")
        type_text("short\n")
        key("ctrl-s")
        wait_for(lambda text: "saved path=/alter/linux/tmp/editor-note.txt bytes=6" in text)
        key("ctrl-shift-s")
        type_text("/alter/linux/tmp/editor-empty.txt")
        key("ret")
        wait_for(lambda text: "saved path=/alter/linux/tmp/editor-empty.txt bytes=6" in text)
        key("ctrl-a")
        key("backspace")
        key("ctrl-s")
        wait_for(lambda text: "saved path=/alter/linux/tmp/editor-empty.txt bytes=0" in text)
        screenshot("empty")
        key("ctrl-w")
        time.sleep(1)
        # Honoka clears focus when the focused window is destroyed.
        click(300, 90)
        type_text("text-editor /nanami/session-list\n")
        wait_for(lambda text: "[text-editor] opened path=/nanami/session-list" in text)
        screenshot("argument")
        key("ctrl-w")
        screenshot("closed")
        assert "panic" not in contents().lower(), "Unexpected guest panic"
        print(f"PASS editor input, save, find, undo, close guard, open, resize, CLI path (smp={args.smp})", flush=True)
    finally:
        try:
            send(f"screendump {logs / 'screen.png'} -f png")
            send("quit")
        except (OSError, RuntimeError):
            pass
        process.stdin.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()

# Validate the actual persisted file bytes independently, without mounting a disk.
with disk.open("rb") as image:
    image.seek(512)
    header = bytearray(image.read(512))
    assert header[:8] == b"EFI PART"
    size, checksum = struct.unpack_from("<II", header, 12)
    assert 92 <= size <= 512
    struct.pack_into("<I", header, 16, 0)
    assert zlib.crc32(header[:size]) == checksum
    entry_lba, count, stride, checksum = struct.unpack_from("<QIII", header, 72)
    assert stride >= 128 and 0 < count * stride <= 16384
    image.seek(entry_lba * 512)
    entries = image.read(count * stride)
    assert zlib.crc32(entries) == checksum
    root_type = bytes.fromhex("616e616e696d534fa0004e414e414d49")
    roots = [struct.unpack_from("<QQ", entries, offset + 32)
             for offset in range(0, len(entries), stride) if entries[offset:offset + 16] == root_type]
    assert len(roots) == 1
    first, last = roots[0]
    assert 0 < first <= last < disk.stat().st_size // 512
    image.seek(first * 512)
    root = logs / "root.ext2"
    remaining = (last - first + 1) * 512
    with root.open("xb") as output:
        while remaining:
            data = image.read(min(remaining, 1024 * 1024))
            assert data
            output.write(data)
            remaining -= len(data)
    for path, expected in [("editor-full.txt", b"Notes\n\nA simple native text editor.\nOpen, edit, save.\n"),
                           ("editor-note.txt", b"short\n"), ("editor-empty.txt", b"")]:
        result = subprocess.run([args.debugfs, "-R", f"cat /alter/linux/tmp/{path}", str(root)], check=True, capture_output=True)
        assert b"not found" not in result.stderr.lower(), result.stderr
        assert result.stdout == expected, (path, result.stdout, expected)
print("PASS independently verified saved, truncated and empty file bytes on disk", flush=True)
