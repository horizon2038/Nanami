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
parser.add_argument("--normal-devices", action="store_true",
                    help="match xtask's default PS/2 and virtio-net user network")
parser.add_argument("--net", choices=["none", "user"],
                    help="override networking when isolating normal-device regressions")
profile = parser.add_mutually_exclusive_group()
profile.add_argument("--tinycore", action="store_true", help="test the Tiny Core Alter boot profile")
profile.add_argument("--tinycore-desktop", action="store_true", help="test the real Xfbdev/FLWM desktop")
profile.add_argument("--tinycore-apps", action="store_true", help="test installed GUI applications with real X windows")
profile.add_argument("--tinycore-perf", action="store_true", help="measure fixed AF_UNIX round-trip workloads")
profile.add_argument("--tinycore-process-perf", action="store_true", help="measure Alpha creation, Negoto prepare and Linux fork/exec separately")
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
if args.normal_devices:
    command[command.index("hpet=on,i8042=off")] = "hpet=on"
if args.net == "user" or (args.net is None and args.normal_devices):
    at = command.index("-nic")
    command[at:at + 2] = ["-netdev", "user,id=net0", "-device",
                         "virtio-net-pci,netdev=net0,addr=5,disable-legacy=off,disable-modern=on"]
with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def checked_serial():
            if process.poll() is not None:
                raise RuntimeError("QEMU exited; see qemu.log")
            text = serial.read_text(errors="replace") if serial.exists() else ""
            plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
            if ("NEGOTO-SMOKE-FAIL" in plain or "FATAL FAULT" in plain
                    or (args.tinycore_desktop and ("[proc.err] spawn failed" in plain
                                                 or "[shm.err] share rollback" in plain))
                    or re.search(r"\bPANIC\b|\bpanic(?:ked)?\b", plain, re.IGNORECASE)):
                raise RuntimeError("Guest regression failed; see serial.log")
            return text

        def wait_for(predicate):
            deadline = time.monotonic() + args.timeout
            while time.monotonic() < deadline:
                text = checked_serial()
                if predicate(text):
                    return
                time.sleep(0.2)
            send(f"screendump {logs / 'timeout.png'} -f png")
            raise RuntimeError("QEMU timed out; see serial.log and timeout.png")

        def send(value):
            process.stdin.write(value.encode() + b"\n")
            process.stdin.flush()
            data = b""
            while b"(qemu)" not in data:
                if not select.select([process.stdout], [], [], 5)[0]:
                    raise RuntimeError("QEMU monitor timed out")
                data += os.read(process.stdout.fileno(), 4096)

        def click_at(x, y, click=True):
            for _ in range(30):
                send("mouse_move -100 -100")
                time.sleep(0.08)
            while x or y:
                dx, dy = min(x, 40), min(y, 40)
                send(f"mouse_move {dx} {dy}")
                x -= dx
                y -= dy
                time.sleep(0.1)
            if click:
                send("mouse_button 1")
                time.sleep(0.1)
                send("mouse_button 0")

        def dock_at(label):
            # 800x600 guest at host (82,110), centered 32px icons / 8px gaps.
            config = repo / "thirdparty/tinycore/profile/etc/nanami-wbar"
            labels = [line[3:].strip() for line in config.read_text().splitlines()
                      if line.startswith("t: ") and not line[3:].startswith("/")]
            return (482 + (2 * labels.index(label) - len(labels) + 1) * 20, 688)

        def terminal_marker(command_text, marker, screenshot):
            # PTY initialization can flush keys sent before the new shell's
            # prompt. Retry input, not the launcher, until independent VFS
            # readback proves that this terminal executed the command.
            for _ in range(4):
                send("sendkey ctrl-u 40")
                for character in command_text + "\n":
                    send(f"sendkey {keys.get(character, character)} 40")
                    time.sleep(0.25)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and marker not in checked_serial():
                    time.sleep(0.2)
                if marker in checked_serial():
                    return
            send(f"screendump {logs / screenshot} -f png")
            raise RuntimeError("X terminal did not execute its command")

        # SMP serial output can interleave inside Negoto's one-off startup
        # line. The guest verifies its service connection directly; use GUI
        # readiness here only to avoid typing before the shell accepts input.
        # Honoka's final online line can interleave too; completed initial
        # rendering is the preceding, equally useful readiness observation.
        wait_for(lambda text: re.search(r"\[\s*shell\] font init end", text)
                 and ("[              honoka] online" in text
                      or "[              honoka] initial render done" in text))
        time.sleep(2)
        keys = {"/": "slash", "-": "minus", "\n": "ret"}
        guest = "/alter/linux/bin/negoto-smoke"
        if args.tinycore:
            guest += " --tinycore"
        if args.tinycore_desktop:
            guest += " --tinycore-desktop"
        if args.tinycore_apps:
            guest += " --tinycore-apps"
        if args.tinycore_perf:
            guest += " --tinycore-perf"
        if args.tinycore_process_perf:
            guest += " --tinycore-process-perf"
        keys[" "] = "spc"
        for character in guest + "\n":
            send(f"sendkey {keys.get(character, character)} 40")
            time.sleep(0.25)
        if args.tinycore_process_perf:
            # The summary prefix precedes the captured workload output. Do not
            # stop QEMU halfway through UART emission of the timing samples.
            wait_for(lambda text: "TINYCORE-PROCESS-PERFORMANCE-PASS" in text
                     and re.search(r"(?m)^PROCESS-PERFORMANCE-PASS\r?$", text))
            for line in checked_serial().splitlines():
                if "PROCESS-PERF " in line:
                    print(line, flush=True)
            raise SystemExit(0)
        if args.tinycore_perf:
            wait_for(lambda text: "TINYCORE-PERFORMANCE-PASS" in text)
            text = checked_serial()
            for line in text.splitlines():
                if line.startswith("FRAME-PERF "):
                    print(line, flush=True)
            print(text.split("TINYCORE-PERFORMANCE-PASS", 1)[1], flush=True)
            raise SystemExit(0)

        if args.tinycore_apps:
            wait_for(lambda text: "TINYCORE-APPLICATIONS-READY" in text)
            time.sleep(3)
            click_at(300, 300)
            keys.update({">": "shift-dot", "<": "shift-comma", "&": "shift-7"})
            for character in "/bin/nanami-desktop-pointer &\n":
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            wait_for(lambda text: "X11-POINTER " in text)
            # Compare X QueryPointer with known positions in the 800x600
            # backing, through USB -> Honoka -> Alter -> PS/2 -> unmodified X.
            for target in [(300, 300), (700, 550), (200, 200)]:
                click_at(*target, click=False)
                expected = f"X11-POINTER {target[0] - 84} {target[1] - 110}"
                wait_for(lambda text: expected in text)
            for _ in range(80):
                for dx, dy in [(7, 3), (-7, -3)]:
                    send(f"mouse_move {dx} {dy}")
                    time.sleep(0.002)
            # A new sentinel avoids accepting a pre-burst observation.
            send("mouse_move 11 13")
            wait_for(lambda text: "X11-POINTER 127 103" in text)
            print("PASS X11 pointer coordinates after motion burst/reversal", flush=True)
            click_at(300, 300)
            # Keep the test alive after its terminal closes, so that the
            # terminal cannot cover newly mapped GUI windows in the screenshot.
            command_text = "/bin/busybox nohup /bin/busybox sh /usr/local/bin/nanami-desktop-test > /tmp/desktop-apps-run.log 2>&1 < /dev/null & exit\n"
            for character in command_text:
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            for application in ["files", "images", "paint", "calculator", "clock"]:
                wait_for(lambda text: f"TINYCORE-APP-WINDOW {application}" in text)
                send(f"screendump {logs / f'tinycore-app-{application}.png'} -f png")
            wait_for(lambda text: "TINYCORE-APPLICATIONS-VERIFIED" in text)
            click_at(*dock_at("Exit desktop"))
            wait_for(lambda text: "TINYCORE-APPLICATIONS-PASS windows=true cleanup=true" in text)
            print(f"PASS Tiny Core GUI applications: Files, Images, Paint, Calculator and Clock (smp={args.smp})", flush=True)
            raise SystemExit(0)

        def exercise_desktop(cycle):
            wait_for(lambda text: f"TINYCORE-DESKTOP-READY cycle={cycle}" in text)
            time.sleep(3)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-ready.png'} -f png")
            # Actual USB mouse -> Honoka queue -> PS/2 -> X11/FLWM title drag.
            # USB boot mice have signed 8-bit relative reports. Reset with
            # bounded deltas rather than overflowing a single huge report.
            for _ in range(30):
                send("mouse_move -100 -100")
                time.sleep(0.15)
            for dx, dy in [(30, 0)] * 10 + [(0, 29)] * 5:
                send(f"mouse_move {dx} {dy}")
                time.sleep(0.15)
            send("mouse_button 1")
            time.sleep(0.2)
            for _ in range(9):
                send("mouse_move 10 5")
                time.sleep(0.15)
            send("mouse_button 0")
            time.sleep(1)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-drag.png'} -f png")
            send("mouse_move -40 90")
            send("mouse_button 1")
            time.sleep(0.1)
            send("mouse_button 0")
            keys[">"] = "shift-dot"
            for character in "echo desktop-input-ok > /tmp/desktop-input-ok\n":
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            wait_for(lambda text: f"TINYCORE-DESKTOP-INPUT-PASS cycle={cycle}" in text)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-input.png'} -f png")
            for character in "exit\n":
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            time.sleep(3)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-terminal-closed.png'} -f png")
            # The initial terminal is gone; only a real dock action can create
            # the new X terminal that writes the independently read marker.
            click_at(*dock_at("Terminal"))
            time.sleep(4)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-launched.png'} -f png")
            terminal_marker("echo desktop-launch-ok > /tmp/desktop-launch-ok",
                            f"TINYCORE-DESKTOP-LAUNCH-PASS cycle={cycle}",
                            f"tinycore-desktop-{cycle}-launch-timeout.png")
            if cycle == 2:
                # Leave no other interactive terminal that could accidentally
                # execute the Information window's verification command.
                for character in "exit\n":
                    send(f"sendkey {keys.get(character, character)} 40")
                    time.sleep(0.25)
                time.sleep(3)
            else:
                click_at(*dock_at("Editor"))
                time.sleep(10)
                send(f"screendump {logs / f'tinycore-desktop-{cycle}-editor.png'} -f png")
                # Stock wbar is not always-on-top. Move the editor off the dock.
                click_at(400, 296, click=False)
                send("mouse_button 1")
                for _ in range(10):
                    send("mouse_move -10 -10")
                    time.sleep(0.15)
                send("mouse_button 0")
                time.sleep(1)
                send(f"screendump {logs / f'tinycore-desktop-{cycle}-apps.png'} -f png")
            click_at(*dock_at("Tiny Core information"))
            time.sleep(6)
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-info.png'} -f png")
            click_at(300, 300)
            terminal_marker("echo desktop-info-ok > /tmp/desktop-info-ok",
                            f"TINYCORE-DESKTOP-INFO-PASS cycle={cycle}",
                            f"tinycore-desktop-{cycle}-info-timeout.png")
            send(f"screendump {logs / f'tinycore-desktop-{cycle}-info-command.png'} -f png")
            click_at(*dock_at("Exit desktop"))
            wait_for(lambda text: f"TINYCORE-DESKTOP-SESSION-PASS cycle={cycle}" in text)
            send(f"screendump {logs / f'tinycore-desktop-exit-{cycle}.png'} -f png")

        if args.tinycore_desktop:
            for cycle in range(1, 3):
                exercise_desktop(cycle)
            wait_for(lambda text: "TINYCORE-DESKTOP-PASS x11=true input=true cleanup=true" in text)
            print(f"PASS Tiny Core desktop twice in one boot: X11/FLWM/wbar, Terminal/Editor/Information launch, real input and Exit/restart cleanup (smp={args.smp})", flush=True)
            raise SystemExit(0)
        marker = "NEGOTO-SMOKE-PASS reuse=20 cleanup=true"
        if args.tinycore:
            marker = "TINYCORE-SMOKE-PASS init=true dynamic=true interactive=true"
        wait_for(lambda text: marker in text)
        text = checked_serial()
        required = ["NEGOTO-LIFECYCLE-PASS", "NEGOTO-READONLY-PASS", "NEGOTO-KILL-PASS", "NEGOTO-SIGNALS-PASS", "NEGOTO-SIGNAL-ABI-PASS", "NEGOTO-ASYNC-SIGNALS-PASS", "NEGOTO-CTRL-C-PASS", "NEGOTO-CLI-PASS", "NEGOTO-INTERACTIVE-PASS"]
        if args.tinycore:
            required = ["TINYCORE-CHMOD-PASS", "TINYCORE-PS-PASS", "TINYCORE-INIT-PASS", "TINYCORE-CONSOLE-PASS"]
        for marker in required:
            if marker not in text:
                raise RuntimeError(f"Missing {marker}")
        if args.tinycore:
            # Also retain the normal GUI launch, not only the synthetic console
            # used by the assertions above. The disk remains a QEMU snapshot.
            for command_text in ["negoto run --bundle /alter/linux/tinycore tinycore", "ls", "cat /etc/os-release"]:
                time.sleep(5)
                checked_serial()
                for character in command_text + "\n":
                    send(f"sendkey {keys.get(character, character)} 40")
                    time.sleep(0.25)
            time.sleep(5)
            checked_serial()
            send(f"screendump {logs / 'tinycore-console.png'} -f png")
            for character in "exit\n":
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            time.sleep(1)
            checked_serial()
            print(f"PASS Tiny Core init, stock dynamic BusyBox, private root, symlinks, fork/exec, pipes, interactive shell, Ctrl-C and cleanup (smp={args.smp})", flush=True)
        else:
            print(f"PASS OCI lifecycle, rootfs/cwd/env, fork/exec, readonly, asynchronous signals/GPRs/ABI, 20 reuse cycles, native CLI and interactive shell (smp={args.smp})", flush=True)
    except (Exception, KeyboardInterrupt):
        if process.poll() is None:
            try:
                send(f"screendump {logs / 'failure.png'} -f png")
            except Exception:
                pass
        raise
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
