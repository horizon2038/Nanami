#!/usr/bin/env python3
"""Native monitor, cursor and paint workflow on a private QEMU disk, without networking."""
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

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, default=4)
parser.add_argument("--expect-two-phase", action="store_true", help="Require Booter handoff and launcher suspension before services run")
parser.add_argument("--debugfs", default=shutil.which("debugfs") or "/opt/homebrew/opt/e2fsprogs/sbin/debugfs")
args = parser.parse_args()
if not args.image.is_file():
    parser.error("--image must be a regular file")
if not pathlib.Path(args.debugfs).is_file():
    parser.error("--debugfs must identify e2fsprogs debugfs")
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-desktop-test-"))
print(f"Test logs: {logs}", flush=True)
disk = logs / "boot.img"
if sys.platform == "darwin":
    subprocess.run(["cp", "-c", str(args.image.resolve()), str(disk)], check=True)
else:
    shutil.copyfile(args.image, disk)
firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
command = ["qemu-system-x86_64", "-machine", "hpet=on,i8042=off", "-cpu", "max", "-smp", str(args.smp),
           "-m", "4G", "-display", "none", "-serial", f"file:{serial}", "-monitor", "stdio", "-nic", "none",
           "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=0", "-device", "qemu-xhci,id=xhci,addr=6,msi=off,msix=off",
           "-device", "usb-kbd,bus=xhci.0,port=1", "-device", "usb-mouse,bus=xhci.0,port=2",
           "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
           "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
           "-device", "ich9-ahci,id=ahci,addr=3", "-drive", f"if=none,id=disk,format=raw,cache=writeback,file={disk}",
           "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1", "--no-reboot", "--no-shutdown"]

with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def text():
            value = serial.read_text(errors="replace") if serial.exists() else ""
            return re.sub(r"\[\s*(Saran|native-ui|performance-monitor)\]", r"[\1]", value)

        def wait_for(predicate, timeout=120):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("QEMU exited; see qemu.log")
                value = text()
                if re.search(r"\[(?:Saran|performance-monitor)\] (?:panic|error):", value):
                    raise RuntimeError("App failure; see serial.log")
                if predicate(value):
                    return
                time.sleep(0.15)
            raise RuntimeError("Guest timed out; see screen.ppm and serial.log")

        def send(command):
            process.stdin.write(command.encode() + b"\n")
            process.stdin.flush()
            data = b""
            while b"(qemu)" not in data:
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise RuntimeError("HMP timed out")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError("HMP closed")
                data += chunk

        def key(value):
            send(f"sendkey {value} 40")
            time.sleep(0.25)

        def type_text(value):
            keys = {" ": "spc", "/": "slash", "-": "minus", ".": "dot", "\n": "ret", "#": "shift-3"}
            for c in value:
                key("shift-" + c.lower() if c.isupper() else keys.get(c, c))

        def move(x, y):
            send("mouse_move -4000 -4000")
            time.sleep(2)
            send(f"mouse_move {x} {y}")
            time.sleep(1)

        def click(x, y, button=1):
            move(x, y)
            send(f"mouse_button {button}")
            time.sleep(0.3)
            send("mouse_button 0")
            time.sleep(0.4)

        def drag(x0, y0, x1, y1):
            move(x0, y0)
            send("mouse_button 1")
            time.sleep(0.3)
            send(f"mouse_move {x1-x0} {y1-y0}")
            time.sleep(1)
            send("mouse_button 0")
            time.sleep(0.5)

        def shot(name):
            time.sleep(0.7)
            path = logs / (name + ".ppm")
            send(f"screendump {path}")
            send(f"screendump {logs / (name + '.png')} -f png")
            _, dimensions, _, pixels = path.read_bytes().split(b"\n", 3)
            width, height = map(int, dimensions.split())
            return width, height, pixels

        def crop(picture, x, y, width, height):
            stride, _, pixels = picture
            return b"".join(pixels[((y+row)*stride+x)*3:((y+row)*stride+x+width)*3] for row in range(height))

        wait_for(lambda value: re.search(r"\[\s*shell\] font init end", value) and "[              honoka] online" in value)
        if args.expect_two_phase:
            serial_text = text()
            markers = ["[boot] Alpha Booter preparing independent Alpha", "[boot] handoff ready", "[boot] launcher suspended; Alpha owns independent address space"]
            positions = [serial_text.find(marker) for marker in markers]
            assert all(position >= 0 for position in positions) and positions == sorted(positions), "Two-phase boot order missing"
            assert "retired Booter resumed unexpectedly" not in serial_text
            print("PASS Booter -> independent Alpha -> stopped launcher -> normal services", flush=True)
        time.sleep(2)
        type_text("performance-monitor\n")
        wait_for(lambda value: "[performance-monitor] online" in value)
        time.sleep(3)
        shot("monitor-memory")
        key("2")
        shot("monitor-processes")
        key("3")
        shot("monitor-system")
        key("1")
        key("spc")
        paused = shot("monitor-paused")
        time.sleep(2)
        unchanged = shot("monitor-still-paused")
        assert crop(paused, 184, 150, 760, 600) == crop(unchanged, 184, 150, 760, 600), "Paused monitor repainted or sampled"
        key("f5")
        refreshed = shot("monitor-refresh")
        assert crop(paused, 184, 150, 760, 600) != crop(refreshed, 184, 150, 760, 600), "Manual refresh did not update sample count"
        key("minus")
        key("spc")
        time.sleep(2.4)
        shot("monitor-resumed")
        key("ctrl-w")
        click(300, 90)
        type_text("saran\n")
        wait_for(lambda value: "[Saran] online canvas=640x400" in value)
        time.sleep(1)
        # Saran client origin (94,108), centered 640x400 document origin (284,254).
        click(94 + 90 + 26*5 + 4, 108 + 80)  # red foreground
        key("b")
        drag(284+60, 254+70, 284+360, 254+200)
        key("r")
        drag(284+100, 254+100, 284+250, 254+220)
        key("o")
        drag(284+300, 254+70, 284+500, 254+200)
        key("f")
        click(284+130, 254+170)
        click(94+330, 108+52)  # custom foreground color
        type_text("#204080")
        key("ret")
        key("l")
        drag(284+350, 254+250, 284+580, 254+330)
        move(1100, 800)  # Compare document pixels without the desktop cursor overlay.
        original = shot("saran-drawing")
        key("ctrl-r")
        shot("saran-rotated")
        key("ctrl-z")
        key("ctrl-h")
        shot("saran-flipped")
        key("ctrl-z")
        key("m")
        drag(284+100, 254+100, 284+250, 254+220)
        key("ctrl-shift-x")
        shot("saran-cropped")
        key("ctrl-z")
        move(1100, 800)
        transformed_back = shot("saran-transforms-undone")
        assert crop(original,284,254,640,400) == crop(transformed_back,284,254,640,400), "Transform undo lost pixels"
        key("ctrl-z")
        shot("saran-undo")
        key("ctrl-y")
        key("ctrl-shift-s")
        type_text("/alter/linux/tmp/saran-test.bmp")
        shot("saran-save-as")
        key("ret")
        wait_for(lambda value: "[native-ui] saved path=/alter/linux/tmp/saran-test.bmp" in value)
        shot("saran-saved")
        # Shrinking changes only the viewport, never document dimensions/pixels.
        drag(90+912+4, 78+620+30, 90+72+4, 78+32+30)
        wait_for(lambda value: "[Saran] resized width=72 height=32 canvas=640x400" in value)
        shot("saran-minimum")
        drag(90+72+4, 78+32+30, 90+912+4, 78+620+30)
        wait_for(lambda value: "[Saran] resized width=912 height=620 canvas=640x400" in value)
        restored = shot("saran-restored")
        # Paint one temporary stroke, cancel the close, then undo it.
        key("b")
        drag(284+20, 254+20, 284+40, 254+40)
        key("ctrl-w")
        shot("saran-unsaved")
        key("esc")
        key("ctrl-z")
        key("ctrl-w")
        click(300, 90)
        type_text("saran /alter/linux/tmp/saran-test.bmp\n")
        wait_for(lambda value: "[native-ui] opened path=/alter/linux/tmp/saran-test.bmp" in value)
        reopened = shot("saran-reopened")
        # Image-only comparison avoids toolbar/tool/dirty-state differences.
        assert crop(restored, 284, 254, 640, 400) == crop(reopened, 284, 254, 640, 400), "BMP reopen or resize lost image pixels"
        key("ctrl-w")
        print(f"PASS monitor pause/refresh/tabs, Saran drawing/undo/save/resize/close/reopen (smp={args.smp})", flush=True)
    finally:
        try:
            send(f"screendump {logs / 'screen.ppm'}")
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

# Check actual on-disk BMP independently of the application's decoder.
with disk.open("rb") as image:
    image.seek(512)
    header = image.read(512)
    assert header[:8] == b"EFI PART"
    lba, count, stride = struct.unpack_from("<QII", header, 72)
    assert 0 < count*stride <= 16384 and stride >= 128
    image.seek(lba*512)
    entries = image.read(count*stride)
    root_type = bytes.fromhex("616e616e696d534fa0004e414e414d49")
    roots = [struct.unpack_from("<QQ", entries, i+32) for i in range(0,len(entries),stride) if entries[i:i+16] == root_type]
    assert len(roots) == 1
    first, last = roots[0]
    assert 0 < first <= last < disk.stat().st_size//512
    image.seek(first*512)
    root = logs / "root.ext2"
    with root.open("xb") as output:
        remaining = (last-first+1)*512
        while remaining:
            data = image.read(min(remaining,1024*1024))
            assert data
            output.write(data)
            remaining -= len(data)
    result = subprocess.run([args.debugfs,"-R","cat /alter/linux/tmp/saran-test.bmp",str(root)],check=True,capture_output=True)
    bmp = result.stdout
    assert bmp[:2] == b"BM"
    assert struct.unpack_from("<iiHHI",bmp,18) == (640,400,1,24,0)
    assert struct.unpack_from("<I",bmp,2)[0] == len(bmp) == 54+640*3*400
    def pixel(x,y):
        at = 54+(399-y)*640*3+x*3
        b,g,r = bmp[at:at+3]
        return r<<16 | g<<8 | b
    assert pixel(0,0) == 0xffffff
    assert pixel(60,70) == 0xe04435
    assert pixel(130,170) == 0xe04435
    assert pixel(350,250) == 0x204080
    assert pixel(580,330) == 0x204080
print("PASS independently decoded persisted BMP size, orientation, white background, brush/fill/line colors",flush=True)
