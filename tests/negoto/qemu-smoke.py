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
parser.add_argument("--tcg-threads", choices=["single", "multi"],
                    help="explicit TCG threading for emulator-mode comparisons; otherwise use QEMU's default")
parser.add_argument("--app-launch-ready", choices=["viewable", "ping"], default="viewable",
                    help="launch completion gate: first window, or an advertised EWMH event-loop response")
parser.add_argument("--app-launch-timeline", action="store_true",
                    help="diagnostic Files screenshots and real keyboard challenge after ping; not a painted/readiness assertion")
profile = parser.add_mutually_exclusive_group()
profile.add_argument("--tinycore", action="store_true", help="test the Tiny Core Alter boot profile")
profile.add_argument("--tinycore-desktop", action="store_true", help="test the real Xfbdev/FLWM desktop")
profile.add_argument("--tinycore-file-mapping", action="store_true",
                     help="run the desktop prelude through the real file/copyin mapping regression, without GUI assertions")
profile.add_argument("--tinycore-apps", action="store_true", help="test installed GUI applications with real X windows")
profile.add_argument("--tinycore-app-launch-perf", choices=["terminal", "files", "editor"],
                     help="measure a real dock click through an independently observed viewable X11 window")
profile.add_argument("--tinycore-perf", action="store_true", help="measure fixed AF_UNIX round-trip workloads")
profile.add_argument("--tinycore-process-perf", action="store_true", help="measure Alpha creation, Negoto prepare and Linux fork/exec separately")
profile.add_argument("--tinycore-startup-perf", action="store_true", help="measure real OCI create/start through an X11 terminal connection")
profile.add_argument("--ram-sharing-reuse", action="store_true", help="check private-page reuse while a suspended peer retains a shared mapping")
profile.add_argument("--ram-alias-retirement", action="store_true", help="check RAM reclamation when its final owner holds two aliases")
profile.add_argument("--private-readonly-fork", action="store_true", help="verify real RO-fork RAM savings and privileged-write isolation")
args = parser.parse_args()
if args.app_launch_ready != "viewable" and not args.tinycore_app_launch_perf:
    parser.error("--app-launch-ready requires --tinycore-app-launch-perf")
if args.app_launch_timeline and (args.tinycore_app_launch_perf != "files" or args.app_launch_ready != "ping"):
    parser.error("--app-launch-timeline requires --tinycore-app-launch-perf files --app-launch-ready ping")
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
if args.tcg_threads is not None:
    command += ["-accel", f"tcg,thread={args.tcg_threads}"]
with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
    try:
        def checked_serial():
            if process.poll() is not None:
                raise RuntimeError("QEMU exited; see qemu.log")
            text = serial.read_text(errors="replace") if serial.exists() else ""
            plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
            if ("NEGOTO-SMOKE-FAIL" in plain or "FATAL FAULT" in plain or "[view.err]" in plain
                    or ((args.tinycore_desktop or args.tinycore_file_mapping or args.tinycore_startup_perf or args.tinycore_app_launch_perf) and ("[proc.err] spawn failed" in plain
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
            return data

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
                pressed = time.monotonic_ns()
                send("mouse_button 1")
                time.sleep(0.1)
                release_begin = time.monotonic_ns()
                send("mouse_button 0")
                return pressed, release_begin, time.monotonic_ns()

        def screenshot_at(path):
            # HMP can acknowledge before PNG encoding/writing finishes.
            begin = time.monotonic_ns()
            send(f"screendump {path} -f png")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if path.exists() and path.read_bytes().endswith(b'IEND\xaeB`\x82'):
                    return begin, time.monotonic_ns()
                time.sleep(0.05)
            raise RuntimeError("application screenshot did not finish")

        def launch_timeline(ready_at, pressed):
            # Diagnostic only: these samples are not a completion predicate.
            # Record actual capture bounds, not the requested delay as truth.
            for delay_ms in (200, 500, 1000, 2000):
                time.sleep(max(0, (ready_at + delay_ms * 1_000_000 - time.monotonic_ns()) / 1e9))
                path = logs / f'app-launch-files-after-ping-{delay_ms}ms.png'
                begin, end = screenshot_at(path)
                print(f'APP-LAUNCH-TIMELINE phase=after-ping requested-ms={delay_ms} capture-begin-ns={begin - pressed} capture-end-ns={end - pressed} screenshot={path.name}', flush=True)
            # Real emulated keyboard input goes through Honoka, evdev and X11.
            # A later screenshot is evidence to inspect, never an input PASS.
            challenge = time.monotonic_ns()
            send('sendkey ctrl-l 40')
            time.sleep(0.2)
            send('sendkey ctrl-a 40')
            time.sleep(0.1)
            for character in '/usr/local\n':
                send(f'sendkey {keys.get(character, character)} 40')
                time.sleep(0.1)
            sent = time.monotonic_ns()
            time.sleep(1)
            path = logs / 'app-launch-files-keyboard-challenge.png'
            begin, end = screenshot_at(path)
            print(f'APP-LAUNCH-TIMELINE phase=keyboard-challenge input-begin-ns={challenge - pressed} input-end-ns={sent - pressed} capture-begin-ns={begin - pressed} capture-end-ns={end - pressed} screenshot={path.name}', flush=True)

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
        if args.tinycore_app_launch_perf:
            # The initial monitor prompt can precede the first response. Query
            # twice and retain both raw responses, before any timed click.
            cpu_info = send("info cpus") + send("info cpus")
            (logs / "vcpu-threads.txt").write_bytes(cpu_info)
            # Darwin QEMU can report getpid() for every CPU's thread_id.
            # Preserve raw data, but do not infer host concurrency from it.
            print(f"APP-TCG-REQUEST mode={args.tcg_threads or 'default'}", flush=True)
        keys = {"/": "slash", "-": "minus", ".": "dot", "\n": "ret"}
        guest = "/alter/linux/bin/negoto-smoke"
        if args.ram_sharing_reuse:
            guest = "/alter/linux/bin/ram-share-smoke --ram-sharing-reuse"
        if args.ram_alias_retirement:
            guest = "/alter/linux/bin/ram-share-smoke --ram-alias-retirement"
        if args.private_readonly_fork:
            guest = "/alter/linux/bin/ram-share-smoke --private-readonly-fork"
        if args.tinycore:
            guest += " --tinycore"
        if args.tinycore_desktop or args.tinycore_file_mapping:
            guest += " --tinycore-desktop"
        if args.tinycore_apps or args.tinycore_app_launch_perf:
            guest += " --tinycore-apps"
        if args.tinycore_perf:
            guest += " --tinycore-perf"
        if args.tinycore_process_perf:
            guest += " --tinycore-process-perf"
        if args.tinycore_startup_perf:
            guest += " --tinycore-startup-perf"
        keys[" "] = "spc"
        for character in guest + "\n":
            send(f"sendkey {keys.get(character, character)} 40")
            time.sleep(0.25)
        if args.tinycore_file_mapping:
            marker = "TINYCORE-FILE-MAPPING-PASS"
            wait_for(lambda text: marker in text)
            print(f"PASS {marker} (smp={args.smp}; no GUI assertion)", flush=True)
            raise SystemExit(0)
        if args.private_readonly_fork:
            marker = "PRIVATE-READONLY-FORK-PASS ram=true guarded-write=true isolation=true"
            wait_for(lambda text: re.search(re.escape(marker) + r" source-reap=true\r?\n", text))
            for line in checked_serial().splitlines():
                if "PRIVATE-RO-RAM " in line or line.startswith("PRIVATE-READONLY-FORK-PASS "):
                    print(line, flush=True)
            print(f"PASS {marker} (smp={args.smp})", flush=True)
            raise SystemExit(0)
        if args.ram_sharing_reuse:
            marker = "RAM-SHARING-REUSE-PASS rounds=8 peer-data=true cleanup=true"
            wait_for(lambda text: marker in text)
            print(f"PASS {marker} (smp={args.smp})", flush=True)
            raise SystemExit(0)
        if args.ram_alias_retirement:
            marker = "RAM-ALIAS-RETIREMENT-PASS rounds=4 aliases=2 cleanup=true"
            wait_for(lambda text: marker in text)
            print(f"PASS {marker} (smp={args.smp})", flush=True)
            raise SystemExit(0)
        if args.tinycore_startup_perf:
            wait_for(lambda text: "TINYCORE-STARTUP-PERFORMANCE-PASS samples=3 cleanup=true" in text)
            for line in checked_serial().splitlines():
                if "STARTUP-PERF " in line:
                    print(line, flush=True)
            raise SystemExit(0)
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
            # The prefix is printed before the captured timing report. Wait
            # for its footer too, before quitting QEMU or reading the samples.
            wait_for(lambda text: "TINYCORE-PERFORMANCE-PASS" in text
                     and re.search(r"(?m)^DESKTOP-PERFORMANCE-PASS\r?$",
                                   text.split("TINYCORE-PERFORMANCE-PASS", 1)[1]))
            text = checked_serial()
            for line in text.splitlines():
                if line.startswith("FRAME-PERF "):
                    print(line, flush=True)
            print(text.split("TINYCORE-PERFORMANCE-PASS", 1)[1], flush=True)
            raise SystemExit(0)

        if args.tinycore_app_launch_perf:
            label = args.tinycore_app_launch_perf
            wait_for(lambda text: "TINYCORE-APPLICATIONS-READY" in text)
            click_at(300, 300)
            keys.update({">": "shift-dot", "&": "shift-7", " ": "spc"})
            observer_mode = " ping" if args.app_launch_ready == "ping" else ""
            command_text = f"/bin/aw {label}{observer_mode} &\n"
            for character in command_text:
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            wait_for(lambda text: f"APP-OBSERVER-READY {label}\n" in text.replace("\r\n", "\n"))
            completion = f"APP-WINDOW-READY {label}"
            if args.app_launch_ready == "ping":
                completion = f"APP-EVENTLOOP-REPLY {label}"
            assert completion not in checked_serial(), "stale window observation"
            pressed, release_begin, released = click_at(*dock_at(label.capitalize()))
            wait_for(lambda text: re.search(r"(?m)^" + re.escape(completion) + r"(?: ping-roundtrip-ns=\d+)?\r?\n", text))
            ready_at = time.monotonic_ns()
            elapsed = ready_at - pressed
            screenshot = logs / f'app-launch-{label}.png'
            screenshot_at(screenshot)
            print(f"APP-LAUNCH-PERF application={label} host-ns={elapsed} release-command-begin-ns={release_begin - pressed} release-command-end-ns={released - pressed} host-from-release-ack-ns={ready_at - released} native-observer-poll-ms=100 host-poll-ms=200 window={args.app_launch_ready}", flush=True)
            if args.app_launch_ready == "ping":
                print(next(line for line in checked_serial().splitlines() if line.startswith(completion)), flush=True)
            if args.app_launch_timeline:
                launch_timeline(ready_at, pressed)
            # This cold-launch measurement ends with the VM, not an OCI-reuse
            # assertion. The independent desktop regression covers cleanup.
            print(f"APP-LAUNCH-PERFORMANCE-PASS application={label}", flush=True)
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
            keys["&"] = "shift-7"
            command_text = (f"sh /usr/local/bin/nanami-desktop-links-test {cycle}"
                            " && echo desktop-input-ok > /tmp/desktop-input-ok\n")
            for character in command_text:
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
