#!/usr/bin/env python3
"""Run Shell's shutdown in an isolated disk snapshot; require guest ACPI S5."""
import argparse
import json
import pathlib
import re
import shutil
import select
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--smp", type=int, default=4)
parser.add_argument("--machine", choices=["pc", "q35"], default="pc")
parser.add_argument("--hpet", choices=["on", "off"], default="on")
parser.add_argument("--storage", choices=["ahci", "usb"], default="ahci")
parser.add_argument("--usb-input", action="store_true")
parser.add_argument("--require-usb-acpi-irq", action="store_true")
parser.add_argument("--require-usb-acpi-fallback", action="store_true",
                    help="q35's active-high GSI link must not be programmed as active-low")
parser.add_argument("--i8042", choices=["on", "off"], default="on")
parser.add_argument("--read-iops", type=int, default=0,
                    help="Limit guest disk reads to exercise USB completion waits (0: unlimited)")
args = parser.parse_args()
if args.read_iops < 0:
    parser.error("--read-iops must be nonnegative")
if args.require_usb_acpi_irq and args.require_usb_acpi_fallback:
    parser.error("IRQ and fallback assertions are mutually exclusive")
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix="nanami-shutdown-"))
firmware = repo / "spencer/a9nloader-rs/tools"
shutil.copyfile(firmware / "OVMF_VARS.fd", logs / "OVMF_VARS.fd")
serial = logs / "serial.log"
print(f"Logs: {logs}", flush=True)
devices = (["-device", "ich9-ahci,id=storage,addr=3", "-device",
            "ide-hd,drive=disk,bus=storage.0,bootindex=1"] if args.storage == "ahci" else
           ["-device", "ich9-ahci,id=unused-sata,addr=4",
            "-device", "qemu-xhci,id=storage,addr=3", "-device",
            "usb-storage,drive=disk,bus=storage.0,bootindex=1"])
if args.usb_input:
    if args.storage != "usb":
        devices += ["-device", "qemu-xhci,id=input"]
    bus = "storage.0" if args.storage == "usb" else "input.0"
    devices += ["-device", f"usb-kbd,bus={bus}", "-device", f"usb-mouse,bus={bus}"]
command = [
    "qemu-system-x86_64", "-machine", f"{args.machine},hpet={args.hpet},i8042={args.i8042}", "-cpu", "max",
    "-smp", str(args.smp), "-m", "4G", "-display", "none", "-nic", "none",
    "-serial", f"file:{serial}", "-qmp", "stdio",
    "-drive", f"if=pflash,format=raw,readonly=on,file={firmware}/OVMF_CODE.fd",
    "-drive", f"if=pflash,format=raw,file={logs}/OVMF_VARS.fd",
    "-drive", f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}"
    + (f",throttling.iops-read={args.read_iops}" if args.read_iops else ""),
    *devices, "-no-reboot",
]
with (logs / "qemu.log").open("w") as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics, bufsize=0)
    boot_started = time.monotonic()
    try:
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            text = serial.read_text(errors="replace") if serial.exists() else ""
            if "[              honoka] online" in text and re.search(r"\[\s*shell\] font init end", text) and "S5 on demand" in text:
                break
            if process.poll() is not None:
                raise RuntimeError("Guest exited during boot")
            time.sleep(0.2)
        else:
            raise RuntimeError("Boot timed out")
        print(f"Boot ready without input: {time.monotonic() - boot_started:.2f} seconds", flush=True)
        if args.require_usb_acpi_irq:
            assert re.search(r"USB PCI [0-9a-f]+ pin=[1-4] -> GSI=(1[6-9]|2[0-3]) level/low", text), "No ACPI INTx route"
            assert re.search(r"xHCI ports=.*irq=true", text), "USB IRQ not bound"
            assert "ACPI IRQ unavailable" not in text
        if args.require_usb_acpi_fallback:
            assert "route unavailable: Unsupported" in text
            assert re.search(r"xHCI ports=.*irq=false", text)
        time.sleep(3)
        with process.stdin as output, process.stdout as input_stream:
            events = []

            def receive():
                if not select.select([input_stream], [], [], 30)[0]:
                    raise RuntimeError("QMP timed out; inspect serial.log")
                message = json.loads(input_stream.readline())
                if "event" in message:
                    events.append(message)
                return message

            def call(name, arguments=None):
                output.write((json.dumps({"execute": name, "arguments": arguments or {}}) + "\n").encode())
                while True:
                    message = receive()
                    if "error" in message:
                        raise RuntimeError(message)
                    if "return" in message:
                        return message["return"]

            receive()  # QMP greeting
            call("qmp_capabilities")
            for key in [*"shutdown", "ret"]:
                call("human-monitor-command", {"command-line": f"sendkey {key} 50"})
                time.sleep(0.35)
            while not any(event["event"] == "SHUTDOWN" for event in events):
                receive()
            shutdown = next(event for event in events if event["event"] == "SHUTDOWN")
            if not shutdown.get("data", {}).get("guest") or shutdown["data"].get("reason") != "guest-shutdown":
                raise RuntimeError(f"Not an ACPI guest shutdown: {shutdown}")
            (logs / "events.json").write_text(json.dumps(events, indent=2))
        if process.wait(timeout=10) != 0:
            raise RuntimeError("QEMU failed")
        text = serial.read_text(errors="replace")
        assert "storage synchronized and frozen" in text
        assert "[acpi-server] entering S5" in text or "entering S5" in text
        assert text.index("storage synchronized and frozen") < text.index("entering S5")
        print(f"PASS ACPI S5: machine={args.machine}, smp={args.smp}, hpet={args.hpet}, storage={args.storage}, usb-input={args.usb_input}, i8042={args.i8042}, read-iops={args.read_iops}")
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
