#!/usr/bin/env python3
"""Check cargo xtask's real QEMU argv; optionally snapshot-boot an existing disk."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def launch(repo, extra=(), settings=None):
    env = os.environ.copy()
    for key in ("NET_MODE", "BRIDGE_IF"):
        env.pop(key, None)
    env.update(settings or {})
    completed = subprocess.run(
        ["cargo", "xtask", "run", "--arch", "x86_64", "--print-command",
         "--accel", "tcg", *extra],
        cwd=repo, env=env, check=True, capture_output=True, text=True,
    )
    return json.loads(completed.stdout)


def check_network(repo):
    for settings, extra, expected in [
        ({}, (), "user,"),
        ({"BRIDGE_IF": "bridge0"}, (), "bridged"),
        ({"BRIDGE_IF": "bridge0", "NET_MODE": "user"}, (), "user,"),
        ({"BRIDGE_IF": "bridge0"}, ("--net", "none"), "none"),
        ({"BRIDGE_IF": "bridge0", "NET_MODE": "none"}, ("--net", "user"), "user,"),
    ]:
        arguments = launch(repo, extra, settings)[1:]
        if expected == "none":
            assert "-netdev" not in arguments
            assert arguments[arguments.index("-net") + 1] == "none"
        else:
            backend = arguments[arguments.index("-netdev") + 1]
            assert ("ifname=bridge0" in backend or "br=bridge0" in backend) if expected == "bridged" else backend.startswith(expected)
        print(f"PASS: network {settings}, {extra} -> {expected}", flush=True)


def check_devices(arguments, storage, usb_input):
    devices = [arguments[i + 1] for i, arg in enumerate(arguments) if arg == "-device"]
    controllers = [i for i, device in enumerate(devices) if device.startswith("qemu-xhci,")]
    needs_usb = storage == "usb" or usb_input
    assert len(controllers) == int(needs_usb), devices
    for i, device in enumerate(devices):
        if "bus=xhci.0" in device:
            assert controllers and controllers[0] < i, ("xHCI must precede its child", devices)
        if "bus=ahci.0" in device:
            assert any(d.startswith("ich9-ahci,id=ahci,") for d in devices[:i]), devices
    for prefix, expected in [
        ("usb-storage,", storage == "usb"), ("usb-kbd,", usb_input),
        ("usb-mouse,", usb_input), ("ide-hd,", storage == "ahci"),
        ("virtio-blk-pci,", storage == "virtio"),
    ]:
        assert sum(d.startswith(prefix) for d in devices) == int(expected), devices


def boot(arguments, qemu, repo, logs, hold_after_online=0):
    serial = logs / "serial.log"
    arguments = arguments.copy()
    arguments[arguments.index("-serial") + 1] = f"file:{serial}"
    # print-command never creates writable firmware. Keep this test's vars private.
    variables = logs / "OVMF_VARS.fd"
    shutil.copyfile(repo / "spencer/a9nloader-rs/tools/OVMF_VARS.fd", variables)
    for i, arg in enumerate(arguments):
        if arg.startswith("if=pflash,format=raw,file="):
            arguments[i] = f"if=pflash,format=raw,file={str(variables).replace(',', ',,')}"
    command = [qemu, *arguments, "-display", "none", "-monitor", "none",
               "-snapshot", "-no-reboot"]
    (logs / "boot-command.json").write_text(json.dumps(command, indent=2))
    with (logs / "qemu.log").open("w") as diagnostics:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                   stdout=diagnostics, stderr=diagnostics)
        try:
            deadline = time.monotonic() + 120
            online_at = None
            while time.monotonic() < deadline:
                text = serial.read_text(errors="replace") if serial.exists() else ""
                if "[              honoka] online" in text:
                    if online_at is None:
                        online_at = time.monotonic()
                    if time.monotonic() - online_at >= hold_after_online:
                        print("PASS: Rust launcher boots USB rootfs to Honoka without mouse input", flush=True)
                        return
                if process.poll() is not None:
                    raise RuntimeError(f"QEMU exited during boot; inspect {logs / 'qemu.log'}")
                time.sleep(0.2)
            raise RuntimeError(f"Boot timed out; inspect {serial}")
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--boot-image", type=Path, help="Also boot this image with snapshot=on")
    parser.add_argument("--qemu", default="qemu-system-x86_64")
    parser.add_argument("--hold-after-online", type=float, default=0,
                        help="Keep the no-input snapshot VM running 0-30 seconds for periodic diagnostics")
    options = parser.parse_args()
    if not 0 <= options.hold_after_online <= 30:
        parser.error("--hold-after-online must be between 0 and 30 seconds")
    repo = Path(__file__).resolve().parents[2]
    for storage in ("ahci", "virtio", "usb"):
        for usb_input in (False, True):
            extra = ["--net", "none", "--storage", storage]
            if not usb_input:
                extra.append("--no-usb-input")
            command = launch(repo, extra)
            check_devices(command[1:], storage, usb_input)
            print(f"PASS: storage={storage}, usb_input={usb_input}", flush=True)
    check_network(repo)

    if options.boot_image:
        logs = Path(tempfile.mkdtemp(prefix="nanami-launcher-"))
        print(f"Logs: {logs}", flush=True)
        command = launch(repo, ["--net", "none", "--storage", "usb",
                                "--image", str(options.boot_image.resolve())])
        check_devices(command[1:], "usb", True)
        boot(command[1:], options.qemu, repo, logs, options.hold_after_online)


if __name__ == "__main__":
    main()
