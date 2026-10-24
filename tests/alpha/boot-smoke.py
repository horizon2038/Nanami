#!/usr/bin/env python3
"""Two-phase boot to configured services, using QEMU snapshot disks and no network."""
import argparse
import json
import pathlib
import re
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--arch", choices=["x86_64", "aarch64"], required=True)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--rootfs", type=pathlib.Path)
parser.add_argument("--storage", choices=["ahci", "virtio", "usb"], help="x86_64 storage transport")
parser.add_argument("--expect-domain", action="store_true", help="Require the optional isolated-child guest fixtures")
parser.add_argument("--expect-kernel-capabilities", action="store_true", help="Require the optional real-kernel capability observations")
parser.add_argument("--expect-suspend", action="store_true", help="Run the kernel capability/suspend probes without unrelated Alter service-log readiness checks")
parser.add_argument("--expect-handover", action="store_true", help="Require managed stable-handle Provider exchange after the child fixture")
parser.add_argument("--expect-imports", action="store_true", help="Require scoped, quota-bounded imports and upstream stable-handle Provider exchange")
parser.add_argument("--expect-recursive-imports", action="store_true", help="Also require actual child/grandchild reimport and intermediate memory/session accounting")
parser.add_argument("--expect-buffers", action="store_true", help="Require managed data persistence, abort restoration and a real stale-access page fault")
parser.add_argument("--expect-events", action="store_true", help="Require direct event/data queue handover, pending work, abort and client closure")
parser.add_argument("--smp", type=int, default=4)
parser.add_argument("--timeout", type=float, default=120)
args = parser.parse_args()
if args.expect_suspend:
    args.expect_kernel_capabilities = True
if args.expect_recursive_imports:
    args.expect_imports = True
if not args.image.is_file() or (args.rootfs and not args.rootfs.is_file()):
    parser.error("images must be regular files, not host disks")
if args.arch == "aarch64" and args.rootfs is None:
    parser.error("AArch64 requires --rootfs")
if args.arch == "aarch64" and args.storage is not None:
    parser.error("--storage is only used for x86_64")
repo = pathlib.Path(__file__).resolve().parents[2]
output = pathlib.Path(tempfile.mkdtemp(prefix="nanami-alpha-boot-"))
serial = output / "serial.log"
print(f"Test logs: {output}", flush=True)
command = [str(repo / "target/debug/nanami-xtask"), "run", "--arch", args.arch,
           "--image", str(args.image.resolve()), "--smp", str(args.smp),
           "--snapshot", "--headless", "--net", "none", "--print-command"]
if args.rootfs:
    command.extend(["--rootfs", str(args.rootfs.resolve())])
if args.storage:
    command.extend(["--storage", args.storage])
raw = json.loads(subprocess.check_output(command, cwd=repo))
qemu = []
index = 0
while index < len(raw):
    option = raw[index]
    if option == "-nographic":
        qemu.extend(["-display", "none"])
    elif option == "-serial":
        index += 1
    else:
        qemu.append(option)
    index += 1
qemu.extend(["-serial", f"file:{serial}", "-monitor", "stdio", "-no-reboot", "-no-shutdown"])
with (output / "qemu.log").open("w") as errors, (output / "monitor.log").open("w") as monitor:
    process = subprocess.Popen(qemu, cwd=repo, stdin=subprocess.PIPE, stdout=monitor, stderr=errors)
    text = ""
    try:
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError("QEMU exited; see qemu.log")
            text = serial.read_text(errors="replace") if serial.exists() else ""
            if re.search(r"panic|construction failed|bootstrap failed|resumed unexpectedly|mount failed|boot-list (?:spawn failed|invalid)|\[domain\] launch failed|DOMAIN-\w+-FAIL|HANDOVER-\w+-FAIL|IMPORT-\w+-FAIL|BUFFER-\w+-FAIL|EVENT-\w+-FAIL|CAPABILITY-PROBE-FAIL|spawn summary ok=\d+ failed=[1-9]", text, re.I):
                raise RuntimeError("Guest boot failed; see serial.log")
            markers = ["[boot] Alpha Booter preparing independent Alpha", "[boot] handoff ready", "[boot] launcher suspended; Alpha owns independent address space"]
            positions = [text.find(marker) for marker in markers]
            ready = "session summary ok=0 failed=0" if args.arch == "aarch64" else "[              honoka] online"
            if all(position >= 0 for position in positions) and ready in text:
                assert positions == sorted(positions), "Invalid handoff order"
                if "service registered: vfs-service" not in text:
                    time.sleep(0.15)
                    continue
                if args.arch == "aarch64" and not args.expect_suspend:
                    # Alter registers only after connecting to POSIX and
                    # attaching its shared buffers through real POSIX IPC.
                    # That also proves POSIX readiness if its log interleaves.
                    services = ["terminal-service", "alter-linux"]
                    terminal_dependency = (
                        args.expect_buffers and re.search(r"\[domain\] parent=0 child=\d+ label=buffers budget=536870912 started", text)
                    ) or (
                        args.expect_recursive_imports and re.search(r"\[domain\] parent=0 child=\d+ label=relay budget=1073741824 started", text)
                    ) or (
                        args.expect_events and re.search(r"\[domain\] parent=0 child=\d+ label=events budget=536870912 started", text)
                    )
                    if terminal_dependency:
                        # These shipped fixtures use after=terminal-service.
                        # Its actual launch proves Alpha observed registration,
                        # even if that component's serial line interleaves.
                        services = ["alter-linux"]
                    if any(f"service registered: {service}" not in text for service in services):
                        time.sleep(0.15)
                        continue
                if args.expect_handover and "HANDOVER-PROBE-PASS stable-handle=true old-stopped=true calls=4 no-replay=true wo-client=true closed=true" not in text:
                    time.sleep(0.15)
                    continue
                if args.expect_kernel_capabilities and (
                    "CAPABILITY-PROBE-DONE" not in text or "CAPABILITY-PROBE-PASS" not in text
                    or "SUSPEND-PROBE-PASS" not in text
                ):
                    time.sleep(0.15)
                    continue
                import_markers = ["IMPORT-MANAGER-PASS upstream-switched=true old-stopped=true", "IMPORT-CLIENT-PASS alias=true no-parent-fallback=true scoped-session=true stable-handle=true wo-client=true quota=2 closed=true"]
                if args.expect_recursive_imports:
                    # Identical guest assertions, shorter terminal tokens. The
                    # character-at-a-time debug ABI can interleave startup lines
                    # even on one CPU. Never infer success from partial text.
                    import_markers = ["I-M-OK", "I-C-OK"]
                if args.expect_imports and not all(marker in text for marker in import_markers):
                    time.sleep(0.15)
                    continue
                if args.expect_recursive_imports:
                    if "I-R-OK" not in text or not all(
                        re.search(pattern, text) for pattern in [
                            r"\[domain\] parent=0 child=\d+ label=relay budget=1073741824 started",
                            r"\[domain\] parent=[1-9]\d* child=\d+ label=nested budget=536870912 started",
                        ]
                    ):
                        time.sleep(0.15)
                        continue
                if args.expect_buffers:
                    # Guest assertions receive the actual kernel Fault event,
                    # check sender/address, withhold the reply, then stop/reap-
                    # reject the old Provider. Do not depend on default fault
                    # logger formatting or accept a metadata-only unmap proof.
                    if "BUFFER-PROBE-PASS persisted=13 fault=true" not in text:
                        time.sleep(0.15)
                        continue
                if args.expect_events and "EVENT-PROBE-PASS" not in text:
                    time.sleep(0.15)
                    continue
                if args.expect_domain:
                    if not re.search(r"DOMAIN-PROBE-PASS pid=1 leaf=2 bytes=536870912 no-parent-fallback=true leaf-echo=true", text):
                        time.sleep(0.15)
                        continue
                    # The external after= dependency establishes the parent's
                    # same-name provider before the probe runs. Its absence in
                    # the child, followed by a child-local echo, is the proof;
                    # SMP can interleave the parent's diagnostic serial line.
                    print("PASS bounded Child Alpha, local process/service creation, no unimported parent namespace, and same-name parent/child providers", flush=True)
                if args.expect_kernel_capabilities:
                    print("PASS kernel peer aliases, weak deletion, Node drainage, Generic reuse, synchronous suspend/register access/resume, self suspend and process revoke", flush=True)
                if args.expect_handover:
                    print("PASS one managed connect, S1 replies, incompatible prepare rejected, quiesce/commit, S1 stopped, same handle reaches S2, and WO handle closure; no old-process or closed-alias reclamation claim", flush=True)
                if args.expect_imports:
                    print("PASS explicit import alias, hidden parent names, scoped upstream Session, direct WO handle across Provider exchange, and retained two-connection quota", flush=True)
                if args.expect_recursive_imports:
                    print("PASS real child/grandchild Alpha reimport, intermediate upstream Session quota, and delegated-memory exclusion; normal CALLs use the root stable endpoint directly", flush=True)
                if args.expect_buffers:
                    print("PASS opaque session buffer persistence, stable client VA, exact abort restoration, quiesced close, and real stale Provider access fault; backing remains charged", flush=True)
                if args.expect_events:
                    print("PASS direct work Notification, retained queue cursors, pending work through abort/commit, old event withdrawal, same client handles and quiesced close; backing remains charged", flush=True)
                print(f"PASS independent Alpha, stopped Booter, rootfs and configured services ({args.arch}, smp={args.smp})", flush=True)
                break
            time.sleep(0.15)
        else:
            raise RuntimeError("Boot timed out; see serial.log")
    except BaseException:
        # Read-only diagnostics of the stopped snapshot guest. Keep the queue,
        # header and the 64-KiB block-ABI payload/status boundary for inspection.
        dma = re.search(r"dma_paddr=(0x[0-9a-fA-F]+)", text)
        if dma and process.poll() is None:
            address = int(dma.group(1), 16)
            commands = ["stop", f"xp /8gx {address:#x}",
                        f"xp /8hx {address + 0x1000:#x}",
                        f"xp /8gx {address + 0x2000:#x}",
                        f"xp /4gx {address + 0x4000:#x}",
                        f"xp /4gx {address + 0x14000:#x}"]
            try:
                process.stdin.write(("\n".join(commands) + "\n").encode())
                process.stdin.flush()
                time.sleep(0.2)
            except (BrokenPipeError, OSError):
                pass
        raise
    finally:
        try:
            process.stdin.write(b"quit\n")
            process.stdin.flush()
        except (BrokenPipeError, OSError):
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
