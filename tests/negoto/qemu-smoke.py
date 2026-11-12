#!/usr/bin/env python3
"""Exercise Negoto's real management service and unmodified BusyBox inside QEMU."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import select
import shutil
import subprocess
import tempfile
import time
import host_stats
import scope_report
import scope_disk
import host_affinity
import files_paint

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, choices=[1, 4], default=4)
parser.add_argument("--accel", choices=["tcg", "kvm", "hvf"], default="tcg")
parser.add_argument("--cpu", help="override CPU model (max for TCG, host for KVM/HVF)")
parser.add_argument("--host-process-stats", action="store_true",
                    help="diagnostic Linux QEMU CPU-accounting snapshots around app launch")
parser.add_argument("--host-cpus",
                    help="Linux KVM host CPU list for taskset, e.g. 0,2,4,6,8 (record as a distinct baseline)")
parser.add_argument("--host-vcpu-cpus", help="diagnostic per-vCPU CPU list, e.g. 0,2,4,6; requires --host-io-cpu")
parser.add_argument("--host-io-cpu", type=int, help="diagnostic CPU for QEMU main/auxiliary threads")
parser.add_argument("--timeout", type=float, default=180)
parser.add_argument("--host-poll-ms", type=int, default=200,
                    help="host serial observation interval (1..1000 ms); native relay remains 100 ms")
parser.add_argument("--scope-disk-readout", action="store_true",
                    help="retain private qcow2 overlay and extract fixture packets after QEMU exits (diagnostic only)")
parser.add_argument("--app-syscall-scope", choices=["baseline", "off", "count", "time"],
                    help="diagnostic aw-scope owner from before observer READY to first ping, with post-VM packet extraction")
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
parser.add_argument("--app-launch-rtt", action="store_true",
                    help="require post-ping raw/Xlib128 results (image built with --with-launch-rtt)")
profile = parser.add_mutually_exclusive_group()
profile.add_argument("--syscall-scope-fixture", choices=["active", "off"],
                     help="functional native scope/standard-prctl control; requires test bundle")
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
if args.host_vcpu_cpus is not None or args.host_io_cpu is not None:
    if (args.accel != "kvm" or not hasattr(os, "sched_setaffinity") or not args.tinycore_app_launch_perf
            or args.host_cpus is not None or args.host_vcpu_cpus is None or args.host_io_cpu is None):
        parser.error("explicit CPU placement requires Linux KVM app-launch, both CPU options, and excludes --host-cpus")
    try:
        args.host_vcpu_cpus = [int(value) for value in args.host_vcpu_cpus.split(",")]
    except ValueError:
        parser.error("--host-vcpu-cpus requires comma-separated CPU numbers")
    selected = args.host_vcpu_cpus + [args.host_io_cpu]
    if (len(args.host_vcpu_cpus) != args.smp or len(set(selected)) != len(selected)
            or not set(selected).issubset(os.sched_getaffinity(0)) or shutil.which("taskset") is None):
        parser.error("explicit CPU placement requires one distinct available CPU per vCPU plus I/O, and taskset")
if args.scope_disk_readout and not args.syscall_scope_fixture:
    parser.error("--scope-disk-readout requires --syscall-scope-fixture")
if args.app_syscall_scope:
    if not args.tinycore_app_launch_perf or args.app_launch_ready != "ping" or args.app_launch_timeline:
        parser.error("--app-syscall-scope requires app-launch perf/ping and excludes timeline input")
if not 1 <= args.host_poll_ms <= 1000:
    parser.error("--host-poll-ms must be between 1 and 1000")
if args.app_launch_rtt and (not args.tinycore_app_launch_perf or args.app_launch_ready != "ping"):
    parser.error("--app-launch-rtt requires --tinycore-app-launch-perf and --app-launch-ready ping")
if args.app_launch_rtt and args.app_launch_timeline:
    parser.error("--app-launch-rtt cannot overlap timeline screenshots and keyboard input")
if args.host_process_stats and not args.tinycore_app_launch_perf:
    parser.error("--host-process-stats requires --tinycore-app-launch-perf")
if args.host_cpus is not None:
    if args.accel != "kvm" or not hasattr(os, "sched_getaffinity"):
        parser.error("--host-cpus requires Linux KVM")
    try:
        host_cpus = [int(value) for value in args.host_cpus.split(",")]
    except ValueError:
        parser.error("--host-cpus requires comma-separated CPU numbers")
    if len(host_cpus) != len(set(host_cpus)) or not set(host_cpus).issubset(os.sched_getaffinity(0)):
        parser.error("--host-cpus must be distinct available CPUs")
    if shutil.which("taskset") is None:
        parser.error("--host-cpus requires taskset")
    args.host_cpus = ",".join(str(cpu) for cpu in host_cpus)
if args.tcg_threads is not None and args.accel != "tcg":
    parser.error("--tcg-threads requires --accel tcg")
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
cpu = args.cpu or ("max" if args.accel == "tcg" else "host")
disk = f"format=raw,snapshot=on,file={args.image.resolve()}"
if args.scope_disk_readout or args.app_syscall_scope:
    overlay = logs / "scope-overlay.qcow2"
    subprocess.run(["qemu-img", "create", "-f", "qcow2", "-F", "raw", "-b",
                    str(args.image.resolve()), str(overlay)], check=True, capture_output=True)
    disk = f"format=qcow2,file={overlay}"
command = ["qemu-system-x86_64", "-machine", "hpet=on,i8042=off", "-cpu", cpu, "-smp", str(args.smp),
           "-m", "4G", "-display", "none", "-serial", f"file:{serial}", "-monitor", "stdio", "-nic", "none",
           "-fw_cfg", "name=opt/ovmf/X-PciMmio64Mb,string=0", "-device", "qemu-xhci,id=xhci,addr=6,msi=off,msix=off",
           "-device", "usb-kbd,bus=xhci.0,port=1", "-device", "usb-mouse,bus=xhci.0,port=2",
           "-drive", f"if=pflash,format=raw,readonly=on,file={firmware / 'OVMF_CODE.fd'}",
           "-drive", f"if=pflash,format=raw,file={logs / 'OVMF_VARS.fd'}",
           "-device", "ich9-ahci,id=ahci,addr=3",
           "-drive", f"if=none,id=disk,{disk}",
           "-device", "ide-hd,drive=disk,bus=ahci.0,bootindex=1", "-no-reboot", "-no-shutdown"]
if args.normal_devices:
    command[command.index("hpet=on,i8042=off")] = "hpet=on"
if args.net == "user" or (args.net is None and args.normal_devices):
    at = command.index("-nic")
    command[at:at + 2] = ["-netdev", "user,id=net0", "-device",
                         "virtio-net-pci,netdev=net0,addr=5,disable-legacy=off,disable-modern=on"]
if args.tcg_threads is not None:
    command += ["-accel", f"tcg,thread={args.tcg_threads}"]
else:
    command += ["-accel", args.accel]
if args.host_cpus is not None:
    command = ["taskset", "--cpu-list", args.host_cpus, *command]
if args.host_vcpu_cpus is not None:
    command = ["taskset", "--cpu-list", ",".join(map(str, args.host_vcpu_cpus + [args.host_io_cpu])), *command]
(logs / "command.json").write_text(json.dumps(command, indent=2) + "\n")
(logs / "host-manifest.json").write_text(json.dumps(dict(
    runner_sha256=hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
    host_stats_sha256=hashlib.sha256(pathlib.Path(host_stats.__file__).read_bytes()).hexdigest(),
    scope_report_sha256=hashlib.sha256(pathlib.Path(scope_report.__file__).read_bytes()).hexdigest(),
    scope_disk_sha256=hashlib.sha256(pathlib.Path(scope_disk.__file__).read_bytes()).hexdigest(),
    host_affinity_sha256=hashlib.sha256(pathlib.Path(host_affinity.__file__).read_bytes()).hexdigest(),
    kernel_release=os.uname().release if hasattr(os, "uname") else None,
    machine=os.uname().machine if hasattr(os, "uname") else None), indent=2) + "\n")
with (logs / "qemu.log").open("w") as diagnostics:
    # Keep disposable snapshot writes off the persistent log filesystem.
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=diagnostics,
                               env=dict(os.environ, TMPDIR=os.environ.get("QEMU_TMPDIR", "/tmp")))
    try:
        def checked_serial():
            if process.poll() is not None:
                raise RuntimeError("QEMU exited; see qemu.log")
            text = serial.read_text(errors="replace") if serial.exists() else ""
            plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
            if ("NEGOTO-SMOKE-FAIL" in plain or "FATAL FAULT" in plain or "[view.err]" in plain
                    or (args.app_launch_rtt and "APP-X11-RTT-FAIL" in plain)
                    or (args.app_syscall_scope and "APP-SCOPE-FAIL" in plain)
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
                time.sleep(args.host_poll_ms / 1000)
            raise RuntimeError("QEMU timed out; see serial.log and timeout.png")

        def send(value=None):
            status = process.poll()
            if status is not None:
                raise RuntimeError(f"QEMU exited status={status}; see qemu.log")
            if value is not None:
                try:
                    process.stdin.write(value.encode() + b"\n")
                    process.stdin.flush()
                except BrokenPipeError as error:
                    raise RuntimeError("QEMU monitor closed while sending command") from error
            data = b""
            deadline = time.monotonic() + 5
            while b"(qemu)" not in data:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
                    raise RuntimeError("QEMU monitor timed out")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError(f"QEMU monitor closed; status={process.poll()}")
                data += chunk
            return data

        def click_at(x, y, click=True, before_press=None):
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
                if before_press is not None:
                    before_press()
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
            acknowledged = time.monotonic_ns()
            short_captures = []
            # The installed Xfe configuration binds Ctrl-L to clear_location.
            # One real chord should visibly clear /root; decode after VM exit.
            # These captures perturb subsequent work and are diagnostic only.
            for delay_ms in (50, 500):
                time.sleep(max(0, (challenge + delay_ms * 1_000_000 - time.monotonic_ns()) / 1e9))
                path = logs / f'app-launch-files-clear-location-{delay_ms}ms.png'
                begin, end = screenshot_at(path)
                short_captures.append(dict(screenshot=path.name, requested_ms=delay_ms,
                    capture_begin_ns=begin - pressed, capture_end_ns=end - pressed))
                print(f'APP-LAUNCH-TIMELINE phase=clear-location requested-ms={delay_ms} input-begin-ns={challenge - pressed} input-command-ack-ns={acknowledged - pressed} capture-begin-ns={begin - pressed} capture-end-ns={end - pressed} screenshot={path.name}', flush=True)
            (logs / 'app-launch-single-input.json').write_text(json.dumps(dict(
                command='sendkey ctrl-l 40', hold_ms=40,
                input_begin_ns=challenge - pressed, input_command_ack_ns=acknowledged - pressed,
                precondition_screenshot='app-launch-files-after-ping-2000ms.png',
                captures=short_captures, painted_input_readiness_verified=False,
                performance_admitted=False), indent=2) + '\n')
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

        # Consume the unsolicited greeting before sending any command. Otherwise
        # the first send can return this prompt instead of its own response.
        (logs / "monitor-greeting.log").write_bytes(send())

        # SMP serial output can interleave inside Negoto's one-off startup
        # line. The guest verifies its service connection directly; use GUI
        # readiness here only to avoid typing before the shell accepts input.
        # Honoka's final online line can interleave too; completed initial
        # rendering is the preceding, equally useful readiness observation.
        if (args.scope_disk_readout or args.app_syscall_scope or args.tinycore_file_mapping
                or args.app_launch_timeline or args.tinycore_apps):
            # Diagnostic command delivery only: font logs can split mid-tag.
            # This fixed settle is not a boot/readiness timing observation.
            # File-mapping/timeline/apps are functional-only: the font line can
            # interleave despite a visible input prompt. Native execution,
            # assertions and cleanup remain mandatory below.
            wait_for(lambda text: "[              honoka] initial render done" in text)
            time.sleep(10)
        else:
            wait_for(lambda text: re.search(r"\[\s*shell\] font init end", text)
                     and ("[              honoka] online" in text
                          or "[              honoka] initial render done" in text))
        time.sleep(2)
        if args.tinycore_app_launch_perf:
            cpu_info = send("info cpus")
            (logs / "vcpu-threads.txt").write_bytes(cpu_info)
            if args.host_vcpu_cpus is not None:
                vcpu_tids = host_affinity.parse_vcpus(cpu_info.decode(errors="strict"), args.smp)
                placement = {"before_workload": host_affinity.apply(process.pid, vcpu_tids, args.host_vcpu_cpus, args.host_io_cpu)}
                (logs / "host-explicit-affinity.json").write_text(json.dumps(placement, indent=2) + "\n")
            # Darwin QEMU can report getpid() for every CPU's thread_id.
            # Preserve raw data, but do not infer host concurrency from it.
            print(f"APP-ACCEL-REQUEST accel={args.accel} cpu={cpu} tcg-threads={args.tcg_threads or 'default'}", flush=True)
        keys = {"/": "slash", "-": "minus", ".": "dot", "\n": "ret"}
        guest = "/alter/linux/bin/negoto-smoke"
        if args.syscall_scope_fixture:
            guest += f" --syscall-scope-fixture {args.syscall_scope_fixture}"
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
        if args.syscall_scope_fixture:
            enabled = args.syscall_scope_fixture == "active"
            marker = f"SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled={int(enabled)} readout={2 if enabled else 0}"
            wait_for(lambda text: re.search(r"(?m)^" + re.escape(marker) + r"\r?\n", text))
            result = scope_report.validate_checked_fixture(checked_serial(), enabled)
            if args.scope_disk_readout:
                # The native fixture syncs VFS before the footer. All host
                # conversion/extraction happens after this VM has been reaped.
                process.terminate()
                process.wait(timeout=5)
                result.update(schema=3, report_transport="synced-private-overlay",
                              bootstrap="diagnostic-compositor-render-plus-fixed-10s-settle",
                              raw_packets=scope_disk.extract(overlay, logs, enabled))
            (logs / "scope-fixture-result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"PASS scope fixture enabled={int(enabled)} smp={args.smp}; no performance admission", flush=True)
            raise SystemExit(0)
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
            if args.host_vcpu_cpus is not None:
                placement["before_launch"] = host_affinity.apply(process.pid, vcpu_tids, args.host_vcpu_cpus, args.host_io_cpu)
                (logs / "host-explicit-affinity.json").write_text(json.dumps(placement, indent=2) + "\n")
            if args.host_cpus is not None:
                expected = {int(cpu) for cpu in args.host_cpus.split(",")}
                affinities = {int(path.name): sorted(os.sched_getaffinity(int(path.name)))
                              for path in pathlib.Path(f"/proc/{process.pid}/task").iterdir()}
                if not affinities or any(set(cpus) != expected for cpus in affinities.values()):
                    raise RuntimeError("QEMU thread affinity does not match --host-cpus")
                (logs / "host-affinity.json").write_text(json.dumps(dict(
                    expected=sorted(expected), observed_threads=affinities), indent=2) + "\n")
            click_at(300, 300)
            keys.update({">": "shift-dot", "&": "shift-7", " ": "spc"})
            observer_mode = " ping" if args.app_launch_ready == "ping" else ""
            command_text = f"/bin/aw {label}{observer_mode} &\n"
            if args.app_syscall_scope and args.app_syscall_scope != "baseline":
                command_text = f"/bin/aw-scope {label} ping {args.app_syscall_scope} &\n"
            for character in command_text:
                send(f"sendkey {keys.get(character, character)} 40")
                time.sleep(0.25)
            wait_for(lambda text: f"APP-OBSERVER-READY {label}\n" in text.replace("\r\n", "\n"))
            completion = f"APP-WINDOW-READY {label}"
            if args.app_launch_ready == "ping":
                completion = f"APP-EVENTLOOP-REPLY {label}"
            assert completion not in checked_serial(), "stale window observation"
            if args.host_vcpu_cpus is not None:
                placement["at_press"] = host_affinity.verify(process.pid, vcpu_tids, args.host_vcpu_cpus, args.host_io_cpu)
                (logs / "host-explicit-affinity.json").write_text(json.dumps(placement, indent=2) + "\n")
            host_samples = {}

            def sample_before_press():
                host_samples["cpu_cost_admitted"] = False
                host_samples["physical_cpu_attribution_verified"] = False
                host_samples["before"] = host_stats.snapshot(process.pid)
                host_samples["before_threads"] = host_stats.thread_snapshot(process.pid, cpu_info, args.smp)

            pressed, release_begin, released = click_at(*dock_at(label.capitalize()),
                before_press=sample_before_press if args.host_process_stats else None)
            wait_for(lambda text: re.search(r"(?m)^" + re.escape(completion) + r"(?: ping-roundtrip-ns=\d+)?\r?\n", text))
            ready_at = time.monotonic_ns()
            elapsed = ready_at - pressed
            if args.host_process_stats:
                host_samples["after"] = host_stats.snapshot(process.pid)
                host_samples["after_threads"] = host_stats.thread_snapshot(process.pid, cpu_info, args.smp)
                host_samples["interval"] = host_stats.interval(host_samples["before"], host_samples["after"])
                host_samples["thread_interval"] = host_stats.thread_interval(
                    host_samples["before_threads"], host_samples["after_threads"], host_samples["interval"])
                (logs / "host-process-stats.json").write_text(json.dumps(host_samples, indent=2) + "\n")
            (logs / "app-launch-result.json").write_text(json.dumps(dict(
                application=label, endpoint=args.app_launch_ready,
                accel=args.accel, cpu=cpu, smp=args.smp,
                host_ns=elapsed, release_begin_ns=release_begin - pressed,
                release_end_ns=released - pressed,
                host_poll_ms=args.host_poll_ms, native_observer_poll_ms=100,
                diagnostic_timeline=args.app_launch_timeline, performance_admitted=False,
                host_from_release_ack_ns=ready_at - released), indent=2) + "\n")
            screenshot = logs / f'app-launch-{label}.png'
            capture_begin, capture_end = screenshot_at(screenshot)
            (logs / "app-launch-capture.json").write_text(json.dumps(dict(
                screenshot=screenshot.name, capture_begin_ns=capture_begin - pressed,
                capture_end_ns=capture_end - pressed, first_ping_observed_ns=ready_at - pressed,
                painted_input_readiness_verified=False,
            ), indent=2) + "\n")
            print(f"APP-LAUNCH-PERF application={label} host-ns={elapsed} release-command-begin-ns={release_begin - pressed} release-command-end-ns={released - pressed} host-from-release-ack-ns={ready_at - released} native-observer-poll-ms=100 host-poll-ms={args.host_poll_ms} window={args.app_launch_ready}", flush=True)
            if args.app_launch_ready == "ping":
                print(next(line for line in checked_serial().splitlines() if line.startswith(completion)), flush=True)
            if args.app_launch_timeline:
                launch_timeline(ready_at, pressed)
            if args.app_launch_rtt:
                wait_for(lambda text: re.search(r"(?m)^APP-X11-RTT-PASS count=128 raw-ns=\d+,\d+,\d+ xlib-ns=\d+,\d+,\d+\r?\n", text))
                report = next(line for line in checked_serial().splitlines() if line.startswith("APP-X11-RTT-PASS "))
                (logs / "app-launch-rtt.txt").write_text(report + "\n")
                print(report, flush=True)
            if args.host_vcpu_cpus is not None:
                placement["after_measurement"] = host_affinity.verify(process.pid, vcpu_tids, args.host_vcpu_cpus, args.host_io_cpu)
                (logs / "host-explicit-affinity.json").write_text(json.dumps(placement, indent=2) + "\n")
            if args.app_syscall_scope:
                if args.app_syscall_scope != "baseline":
                    wait_for(lambda text: re.search(r"(?m)^APP-SCOPE-SAVED " + re.escape(label) + r"\r?\n", text))
                process.terminate()
                process.wait(timeout=5)
                packets = {}
                if args.app_syscall_scope != "baseline":
                    packets = scope_disk.extract(overlay, logs, args.app_syscall_scope != "off",
                                                 application_mode=args.app_syscall_scope)
                (logs / "app-syscall-scope-result.json").write_text(json.dumps(dict(
                    schema=1, application=label, mode=args.app_syscall_scope,
                    functional=True, performance_admitted=False, raw_packets=packets,
                    boundary="before-observer-ready-through-first-ping-reply",
                    coverage="all-linux-dispatch-in-one-alter-including-background",
                    bootstrap="diagnostic-compositor-render-plus-fixed-10s-settle",
                    counter_identity="local-unverified"), indent=2) + "\n")
            # This cold-launch measurement ends with the VM, not an OCI-reuse
            # assertion. The independent desktop regression covers cleanup.
            if args.app_launch_timeline:
                print(f"APP-LAUNCH-TIMELINE-FUNCTIONAL-PASS application={label}; painted/input readiness unverified; no performance admission", flush=True)
            elif args.app_syscall_scope:
                print(f"APP-LAUNCH-SCOPE-FUNCTIONAL-PASS application={label}; no performance admission", flush=True)
            else:
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
    except (Exception, KeyboardInterrupt) as error:
        (logs / "failure.json").write_text(json.dumps(dict(
            exception=type(error).__name__, message=str(error), qemu_status=process.poll(),
        ), indent=2) + "\n")
        if process.poll() is None:
            for query in ["info status", "info cpus", "info registers", "info block"]:
                try:
                    (logs / (query.replace(" ", "-") + ".log")).write_bytes(send(query))
                except Exception as monitor_error:
                    (logs / "monitor-failure.log").write_text(str(monitor_error) + "\n")
                    break
            try:
                screenshot_at(logs / "timeout.png")
            except Exception as screenshot_error:
                (logs / "screenshot-failure.log").write_text(str(screenshot_error) + "\n")
        raise
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        if args.app_launch_timeline:
            # Decode only after wait(): pixel comparison must not compete with
            # the VM. Captured-state matches are not a readiness/latency PASS.
            report = files_paint.audit_timeline(logs)
            (logs / "app-launch-paint-reference.json").write_text(
                json.dumps(report, indent=2) + "\n")
