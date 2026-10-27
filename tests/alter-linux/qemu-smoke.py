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
parser.add_argument("--storage", choices=("ahci", "virtio", "usb"), default="ahci",
                    help="root-disk transport for driver/PCI regression tests")
parser.add_argument("--program", action="append", help="guest basename and optional arguments (repeatable)")
parser.add_argument("--ctrl-c-after", type=float,
                    help="send real Ctrl-C this many seconds after each checked guest starts")
parser.add_argument("--capture-running-after", type=float,
                    help="capture each guest's running screen this many seconds after launch (0-30)")
parser.add_argument("--visual-program",
                    help="after checked fixtures, launch this guest untraced and capture two animation frames")
parser.add_argument("--nvim-editor", action="store_true",
                    help="edit/save/quit in untraced Neovim, then verify the file with BusyBox")
parser.add_argument("--nvim-colors", action="store_true",
                    help="with --nvim-editor, verify automatic RGB and indexed-color pixels")
parser.add_argument("--nvim-pty", action="store_true",
                    help="run the PTY fixture, then Neovim :terminal with BusyBox, Ctrl-C, resize and exit")
parser.add_argument("--diagnostics", action="store_true", help="send explicit Alter traces to serial (-t -d)")
parser.add_argument("--native-info", action="store_true",
                    help="also capture pci-info, its numeric/verbose modes and net-info for visual inspection")
parser.add_argument("--expect-framebuffer", action="store_true",
                    help="require exactly one fb-server startup, published after initialization, with no active native display")
parser.add_argument("--gpu", choices=("std", "virtio"), default="std",
                    help="select the emulated display; defaults to the framebuffer regression")
parser.add_argument("--expect-virtio-gpu", action="store_true",
                    help="require the selected virtio-gpu service and Honoka GPU protocol canvas")
parser.add_argument("--virtio-gpu-iommu", action="store_true",
                    help="offer unsupported ACCESS_PLATFORM to test pre-activation framebuffer fallback")
parser.add_argument("--network", action="store_true", help="attach an isolated user-mode virtio NIC (no external network access)")
parser.add_argument("--internet", action="store_true", help="allow outbound NAT for DNS/ICMP interoperability tests (no port forwarding)")
parser.add_argument("--ui-stress", action="store_true", help="exercise scrollback wrap, page scrolling, clear, and command history; retain screenshots for inspection")
args = parser.parse_args()
if args.nvim_colors and not args.nvim_editor:
    parser.error("--nvim-colors requires --nvim-editor")
if args.ctrl_c_after is not None and not 0 < args.ctrl_c_after <= 30:
    parser.error("--ctrl-c-after must be greater than 0 and at most 30 seconds")
if args.capture_running_after is not None and not 0 <= args.capture_running_after <= 30:
    parser.error("--capture-running-after must be between 0 and 30 seconds")
if args.visual_program and args.ui_stress:
    parser.error("--visual-program leaves its guest running; it cannot precede --ui-stress")
if args.expect_framebuffer and args.expect_virtio_gpu:
    parser.error("framebuffer and native virtio-gpu expectations are mutually exclusive")
if args.expect_virtio_gpu and args.gpu != "virtio":
    parser.error("--expect-virtio-gpu requires --gpu virtio")
if args.virtio_gpu_iommu and args.gpu != "virtio":
    parser.error("--virtio-gpu-iommu requires --gpu virtio")
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-alter-test-"))


def verify_ppm_color(path, color):
    # QEMU's binary PPM avoids a host image-library dependency. Count complete
    # RGB pixels, not byte substrings that could straddle adjacent pixels.
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            fields = path.read_bytes().split(b"\n", 3)
        except FileNotFoundError:
            fields = []
        if len(fields) == 4:
            magic, dimensions, maximum, pixels = fields
            width, height = map(int, dimensions.split())
            if len(pixels) == width * height * 3:
                break
        # HMP may return its prompt before the screenshot is written.
        time.sleep(0.05)
    else:
        raise RuntimeError(f"QEMU screenshot was not completed: {path}")
    if magic != b"P6" or maximum != b"255":
        raise RuntimeError(f"Unexpected QEMU PPM format: {path}")
    # Shell currently uses opacity 224. Most of its test window lies over the
    # white desktop; allow the exact compositor blend as well as opaque pixels.
    blended = bytes((channel * 224 + 255 * (255 - 224) + 127) // 255 for channel in color)
    expected = {color, blended}
    count = sum(pixels[index:index + 3] in expected for index in range(0, len(pixels), 3))
    if count < 10000:
        raise RuntimeError(f"Expected Neovim background {color.hex()}, found only {count} pixels: {path}")


firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
print(f"Test logs: {logs}", flush=True)
command = [
    "qemu-system-x86_64", "-machine", "hpet=on", "-cpu", "max", "-smp", str(args.smp),
    "-m", "4G", "-display", "none", "-serial", f"file:{serial}",
    "-monitor", "stdio",
    *(["-netdev", "user,id=net0" + ("" if args.internet else ",restrict=on"), "-device",
       "virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on"]
      if args.network or args.internet else ["-nic", "none"]),
    "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
    "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
    "-drive", f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}",
]
if args.gpu == "virtio":
    gpu = "virtio-vga,addr=2,xres=1920,yres=1080"
    if args.virtio_gpu_iommu:
        gpu += ",iommu_platform=on"
    command += ["-vga", "none", "-device", gpu]
else:
    command += ["-vga", "std"]
if args.storage == "ahci":
    command += ["-device", "ich9-ahci,id=ahci,addr=3",
                "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1"]
elif args.storage == "virtio":
    command += ["-device", "virtio-blk-pci,drive=disk,addr=3,disable-legacy=off,disable-modern=on,bootindex=1"]
else:
    command += ["-device", "qemu-xhci,id=xhci,addr=3",
                "-device", "usb-storage,drive=disk,bus=xhci.0,bootindex=1"]
command += ["--no-reboot", "--no-shutdown"]
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
                 and ("[              honoka] online" in text
                      or "[              honoka] initial render done" in text))
        # Font initialization precedes creating/focusing the shell window.
        time.sleep(2)
        monitor = process.stdin
        if args.expect_framebuffer:
            text = serial.read_text(errors="replace")
            startups = re.findall(r"spawned \./bin/fb-server pid=\d+", text)
            # SMP serial messages can interleave inside the prefix of the
            # multi-part mapping log. The cache marker is emitted only after
            # mapping, validation and the initial framebuffer clear complete.
            initialized = re.search(r"unchanged-pixel cache bytes=", text)
            published = re.search(r"\[\s*fb-server\] service registered: display_service", text)
            if len(startups) != 1 or initialized is None or published is None:
                raise RuntimeError("Framebuffer fallback was not started/published exactly once")
            if initialized.start() >= published.start():
                raise RuntimeError("Framebuffer service was published before initialization")
            if re.search(r"\[\s*driver-manager\] (?:Intel )?native display ready", text):
                raise RuntimeError("Native and framebuffer display backends are both active")
            print(f"PASS framebuffer fallback (smp={args.smp})", flush=True)

        if args.expect_virtio_gpu:
            text = serial.read_text(errors="replace")
            # Another CPU may print between "spawned" and the image path.
            if len(re.findall(r"\./bin/virtio-gpu-server pid=\d+", text)) != 1:
                raise RuntimeError("Virtio GPU was not selected exactly once")
            if re.search(r"\[\s*virtio-gpu\] native 2D scanout=", text) is None or "GPU surface ready" not in text:
                raise RuntimeError("Virtio GPU or Honoka common GPU canvas did not initialize")
            if re.search(r"spawned \./bin/fb-server pid=\d+", text):
                raise RuntimeError("A framebuffer writer competes with native virtio scanout")
            if re.search(r"\[\s*virtio-gpu\] controlq failed", text):
                raise RuntimeError("Virtio control queue failed during desktop initialization")
            print(f"PASS virtio-gpu common protocol (smp={args.smp})", flush=True)

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

        def type_keys(text):
            keys = {" ": "spc", "/": "slash", "-": "minus", ".": "dot", "\n": "ret",
                    ":": "shift-semicolon", "=": "equal", "#": "shift-3", "!": "shift-1"}
            for character in text:
                key = keys.get(character, character)
                if character.isascii() and character.isupper():
                    key = "shift-" + character.lower()
                send(f"sendkey {key} 50")
                time.sleep(0.35)

        def type_command(command):
            type_keys(command + "\n")

        programs = args.program
        if programs is None:
            if args.nvim_pty:
                programs = []
            elif args.nvim_editor:
                programs = ["nvim --headless -u /dev/null -i NONE -c q"]
            else:
                programs = ["linux-syscall-smoke", "glibc-true", "glibc-regression"]
        if args.nvim_pty:
            if "nanami-pty" not in programs:
                programs = ["nanami-pty", *programs]
            programs = [*programs,
                        "nvim --headless -u /dev/null -i NONE -l /tmp/nvim-pty.lua",
                        "busybox grep -x nvim-pty-ok /tmp/nvim-pty-ok"]
        for index, program in enumerate(programs):
            # Match against the same decoded/newline-normalized text used below.
            # Byte offsets drift with CRLF or UTF-8 serial output.
            start = len(serial.read_text(errors="replace"))
            flags = "-t -d" if args.diagnostics else "-t"
            type_command(f"alter {flags} /alter/linux/bin/{program}")
            if args.ctrl_c_after is not None:
                wait_for(lambda text: re.search(
                    r"managed rootfs process image=" + re.escape(program.split()[0]) + r" pid=(\d+)",
                    text[start:]))
                time.sleep(args.ctrl_c_after)
                before_interrupt = serial.read_text(errors="replace")[start:]
                if re.search(r"exit pid=\d+ syscall=\d+ status=", before_interrupt):
                    raise RuntimeError("Guest exited before Ctrl-C; interruption was not exercised")
                send("sendkey ctrl-c 80")
            if args.capture_running_after is not None:
                wait_for(lambda text: re.search(
                    r"managed rootfs process image=" + re.escape(program.split()[0]) + r" pid=(\d+)",
                    text[start:]))
                time.sleep(args.capture_running_after)
                send(f"screendump {logs / f'running-{index}.png'} -f png")
            def guest_exit(text):
                text = text[start:]
                spawned = re.search(r"managed rootfs process image=" + re.escape(program.split()[0]) + r" pid=(\d+)", text)
                if not spawned:
                    return None
                return re.search(r"exit pid=" + spawned[1] + r" syscall=\d+ status=(\d+)", text)
            exited = wait_for(guest_exit)
            if exited[1] != "0":
                raise RuntimeError(f"{program} exited with status {exited[1]}")
            print(f"PASS {program} (smp={args.smp})", flush=True)
            time.sleep(1)
        if args.native_info:
            for label, command in [("pci", "pci-info"), ("pci-numeric", "pci-info -n"),
                                   ("pci-verbose", "pci-info -nn -v"), ("network", "net-info")]:
                type_command("clear")
                type_command(command)
                time.sleep(2)
                send(f"screendump {logs / f'native-{label}.png'} -f png")
            print("Captured native info commands; inspect native-*.png", flush=True)
        if args.nvim_editor:
            start = len(serial.read_text(errors="replace"))
            flags = "-t -d " if args.diagnostics else ""
            type_command(f"alter {flags}/alter/linux/bin/nvim -u /dev/null -i NONE --noplugin /tmp/nvim-editor.txt")
            wait_for(lambda text: re.search(r"managed rootfs process image=nvim pid=(\d+)", text[start:]))
            # Keep strace out of the alternate screen; it is itself terminal output.
            time.sleep(7)
            send(f"screendump {logs / 'nvim-started.png'} -f png")
            type_keys("invim-compat-ok")
            send("sendkey esc 50")
            time.sleep(0.5)
            send(f"screendump {logs / 'nvim-edited.png'} -f png")
            if args.nvim_pty:
                type_keys(":w\n")
                type_keys(":terminal /bin/busybox sh -i\n")
                time.sleep(3)
                send("sendkey i 50")
                type_keys("echo ui-pty-visible\n")
                time.sleep(1)
                send(f"screendump {logs / 'nvim-pty-visible.png'} -f png")
                type_keys("exit\n")
                time.sleep(2)
                send("sendkey ctrl-backslash 50")
                send("sendkey ctrl-n 50")
                type_keys(":bdelete!\n")
                time.sleep(1)
            if args.nvim_colors:
                for label, commands, color in [
                    ("rgb", ["hi Normal guifg=#12ab34 guibg=#253647"], bytes((37, 54, 71))),
                    ("indexed", ["set notermguicolors", "hi Normal ctermfg=196 ctermbg=22"], bytes((0, 95, 0))),
                ]:
                    for command in commands:
                        type_keys(":" + command + "\n")
                    time.sleep(1)
                    screenshot = logs / f"nvim-{label}.ppm"
                    send(f"screendump {screenshot}")
                    verify_ppm_color(screenshot, color)
                print(f"PASS Neovim automatic RGB and indexed-color rendering (smp={args.smp})", flush=True)
            type_keys(":wq\n")
            time.sleep(2)
            send(f"screendump {logs / 'nvim-after-quit.png'} -f png")
            start = len(serial.read_text(errors="replace"))
            type_command("alter -t /alter/linux/bin/busybox grep -x nvim-compat-ok /tmp/nvim-editor.txt")

            def verified_file(text):
                text = text[start:]
                spawned = re.search(r"managed rootfs process image=busybox pid=(\d+)", text)
                if not spawned:
                    return None
                return re.search(r"exit pid=" + spawned[1] + r" syscall=\d+ status=(\d+)", text)

            verified = wait_for(verified_file)
            if verified[1] != "0":
                raise RuntimeError("Neovim edit/save/quit verification failed")
            mode = "serial-traced" if args.diagnostics else "untraced"
            print(f"PASS {mode} Neovim edit/save/quit and independent readback (smp={args.smp})", flush=True)
        if args.visual_program:
            start = len(serial.read_text(errors="replace"))
            type_command(f"alter /alter/linux/bin/{args.visual_program}")
            wait_for(lambda text: re.search(
                r"managed rootfs process image=" + re.escape(args.visual_program.split()[0]) + r" pid=(\d+)",
                text[start:]))
            # Syscall traces share the application's terminal and disrupt curses
            # rendering. Capture the real untraced screen separately; exit-code
            # verification remains in the checked --program runs above.
            for frame in range(2):
                time.sleep(1)
                send(f"screendump {logs / f'visual-{frame}.png'} -f png")
            print(f"Captured untraced {args.visual_program}; inspect visual-*.png", flush=True)
        if args.ui_stress:
            type_command("clear")
            # Repeated help output exceeds the shell's 128-row capacity.
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
