#!/usr/bin/env python3
"""Check production launcher device ordering; optionally boot an existing image."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def capture_arguments():
    # This same executable substitutes for QEMU during the argument tests.
    Path(os.environ["NANAMI_LAUNCHER_CAPTURE"]).write_text(json.dumps(sys.argv[1:]))


def check_wrapper_network(repo, scripts, logs):
    shutil.copyfile(repo / "scripts/run-nanami.sh", scripts / "run-nanami.sh")
    # Stop at the wrapper boundary: no rebuild, networking, sudo, or disk writes.
    (scripts / "run-qemu.sh").write_text(
        'exec "$NANAMI_WRAPPER_PYTHON" "$NANAMI_WRAPPER_RUNNER"\n'
    )
    for index, (settings, expected) in enumerate([
        ({}, ("user", "", "auto")),
        ({"BRIDGE_IF": "en0"}, ("bridged", "en0", "auto")),
        ({"BRIDGE_IF": "en0", "NET_MODE": "user"}, ("user", "en0", "auto")),
        ({"BRIDGE_IF": "en0", "NET_MODE": "none", "QEMU_USE_SUDO": "0"}, ("none", "en0", "0")),
        ({"BRIDGE_IF": "en5", "NET_MODE": "bridged"}, ("bridged", "en5", "auto")),
    ]):
        record = logs / f"wrapper-net-{index}.json"
        env = os.environ.copy()
        for key in ("NET_MODE", "BRIDGE_IF", "QEMU_USE_SUDO", "NANAMI_LAUNCHER_CAPTURE"):
            env.pop(key, None)
        env.update(settings, EXTRA_LINUX_BINS="", NANAMI_WRAPPER_CAPTURE=str(record),
                   NANAMI_WRAPPER_PYTHON=sys.executable, NANAMI_WRAPPER_RUNNER=str(Path(__file__).resolve()))
        subprocess.run(["bash", str(scripts / "run-nanami.sh"), "run"], env=env, check=True)
        assert tuple(json.loads(record.read_text())) == expected
        print(f"PASS: wrapper network {settings} -> {expected}", flush=True)


def check_devices(arguments, storage, usb_input):
    devices = [arguments[i + 1] for i, arg in enumerate(arguments) if arg == "-device"]
    controllers = [i for i, device in enumerate(devices) if device.startswith("qemu-xhci,")]
    needs_usb = storage == "usb" or usb_input == "on"
    assert len(controllers) == int(needs_usb), devices
    for i, device in enumerate(devices):
        if "bus=xhci.0" in device:
            assert controllers and controllers[0] < i, (
                "xHCI must be created before its child", devices
            )
        if "bus=ahci.0" in device:
            assert any(d.startswith("ich9-ahci,id=ahci,") for d in devices[:i]), devices
    assert sum(d.startswith("usb-storage,") for d in devices) == int(storage == "usb")
    assert sum(d.startswith("usb-kbd,") for d in devices) == int(usb_input == "on")
    assert sum(d.startswith("usb-mouse,") for d in devices) == int(usb_input == "on")
    assert sum(d.startswith("ide-hd,") for d in devices) == int(storage == "ahci")
    assert sum(d.startswith("virtio-blk-pci,") for d in devices) == int(storage == "virtio")


def boot(arguments, qemu, logs):
    serial = logs / "serial.log"
    # Keep the production device arguments, but use isolated, headless execution.
    serial_index = arguments.index("-serial")
    arguments[serial_index + 1] = f"file:{serial}"
    command = [qemu, *arguments, "-display", "none", "-monitor", "none",
               "-snapshot", "-no-reboot"]
    (logs / "boot-command.json").write_text(json.dumps(command, indent=2))
    with (logs / "qemu.log").open("w") as diagnostics:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                   stdout=diagnostics, stderr=diagnostics)
        try:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                text = serial.read_text(errors="replace") if serial.exists() else ""
                if "[              honoka] online" in text:
                    print("PASS: production USB launcher boots to Honoka without mouse input", flush=True)
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
    options = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    logs = Path(tempfile.mkdtemp(prefix="nanami-launcher-"))
    print(f"Logs: {logs}", flush=True)
    scripts = logs / "scripts"
    scripts.mkdir()
    shutil.copyfile(repo / "scripts/run-qemu.sh", scripts / "run-qemu.sh")
    # Skip builds in this isolated fixture; never rebuild the user's rootfs/image.
    (scripts / "build-image.sh").symlink_to(shutil.which("true") or "/usr/bin/true")
    capture = logs / "capture-qemu"
    shutil.copyfile(Path(__file__), capture)
    capture.chmod(0o755)
    output = logs / "spencer/out/x86_64-pc99-release"
    output.mkdir(parents=True)
    image = output / "spencer.img"
    image.touch()
    rootfs = logs / "rootfs.img"
    rootfs.touch()
    firmware = logs / "spencer/a9nloader-rs/tools"
    firmware.mkdir(parents=True)
    for name in ("OVMF_CODE.fd", "OVMF_VARS.fd"):
        (firmware / name).touch()

    def launch(storage, usb_input):
        record = logs / f"{storage}-{usb_input}.json"
        env = os.environ.copy()
        env.update(ARCH="x86_64", PLATFORM="pc99", PROFILE="release",
                   QEMU=str(capture), QEMU_USE_SUDO="0", NET_MODE="none",
                   QEMU_ACCEL="none", QEMU_CPU="max", QEMU_MEMORY="4G", QEMU_SMP="4",
                   QEMU_HPET="on", NET_DEVICE="virtio", BLOCK_IMAGE=str(rootfs),
                   BLOCK_IMAGE_FORMAT="raw", ROOTFS_REBUILD="0", EXTRA_LINUX_BINS="",
                   EXTRA_FREEBSD_BINS="", ROOTFS_APPS="", LINUX_ROOTFS_DIR="",
                   STORAGE_DEVICE=storage, USB_INPUT=usb_input,
                   NANAMI_LAUNCHER_CAPTURE=str(record))
        subprocess.run(["bash", str(scripts / "run-qemu.sh")], env=env, check=True)
        return json.loads(record.read_text())

    for storage in ("ahci", "virtio", "usb"):
        for usb_input in ("off", "on"):
            check_devices(launch(storage, usb_input), storage, usb_input)
            print(f"PASS: storage={storage}, USB_INPUT={usb_input}", flush=True)

    if options.boot_image:
        shutil.copyfile(options.boot_image.resolve(), image)
        for name in ("OVMF_CODE.fd", "OVMF_VARS.fd"):
            shutil.copyfile(repo / "spencer/a9nloader-rs/tools" / name, firmware / name)
        arguments = launch("usb", "on")
        check_devices(arguments, "usb", "on")
        boot(arguments, options.qemu, logs)

    check_wrapper_network(repo, scripts, logs)


if __name__ == "__main__":
    if "NANAMI_WRAPPER_CAPTURE" in os.environ:
        Path(os.environ["NANAMI_WRAPPER_CAPTURE"]).write_text(json.dumps([
            os.environ.get(key, "") for key in ("NET_MODE", "BRIDGE_IF", "QEMU_USE_SUDO")
        ]))
    elif "NANAMI_LAUNCHER_CAPTURE" in os.environ:
        capture_arguments()
    else:
        main()
