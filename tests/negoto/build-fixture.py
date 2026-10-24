#!/usr/bin/env python3
"""Build an OCI bundle using an unchanged local BusyBox ELF and a private boot image."""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--busybox", type=pathlib.Path, default=pathlib.Path("bin/busybox"))
parser.add_argument("--rootfs", type=pathlib.Path, required=True)
parser.add_argument("--image", type=pathlib.Path, required=True)
args = parser.parse_args()
repo = pathlib.Path(__file__).resolve().parents[2]
if args.rootfs.exists() or args.image.exists():
    parser.error("use new rootfs/image paths; existing guest data is not replaced")
busybox = args.busybox.resolve()
data = busybox.read_bytes()
if data[:4] != b"\x7fELF" or int.from_bytes(data[18:20], "little") != 62:
    parser.error("--busybox must be an x86_64 Linux ELF")
print(f"BusyBox SHA-256 (copied unchanged): {hashlib.sha256(data).hexdigest()}", flush=True)
payload = pathlib.Path(tempfile.mkdtemp(prefix="nanami-negoto-payload-"))
bundle = payload / "bundles/busybox"
for directory in ["bin", "work", "etc"]:
    (bundle / "rootfs" / directory).mkdir(parents=True, exist_ok=True)
shutil.copyfile(busybox, bundle / "rootfs/bin/busybox")
(bundle / "rootfs/bin/busybox").chmod(0o755)
subprocess.run([os.environ.get("CLANG", "/opt/homebrew/opt/llvm/bin/clang"),
                "--target=x86_64-unknown-linux-gnu", "-fuse-ld=lld", "-nostdlib", "-static",
                "-ffreestanding", "-fno-builtin", "-fno-stack-protector", "-mno-sse", "-mno-sse2",
                "-mno-red-zone", "-O2", "-Wl,-e,_start,--image-base=0x400000",
                str(repo / "tests/alter-linux/signals.c"), str(repo / "tests/alter-linux/signals-x86_64.S"),
                "-o", str(bundle / "rootfs/bin/signal-test")], check=True)
script = ('test "$NEGOTO_VALUE" = "oci untouched" || exit 41; '
          'test "$(pwd)" = /work || exit 42; '
          '/bin/busybox test ! -e /nanami/system-list || exit 43; '
          '/bin/busybox test ! -e /alter/linux/bundles/busybox/rootfs/etc/token || exit 44; '
          '/bin/busybox true || exit 45; printf "NEGOTO-BUSYBOX-OK\\n" > result')
signal_script = ('trap "printf signal-ok > /work/signal-result" USR1; '
                 'kill -USR1 $$ || exit 81; '
                 'test "$(/bin/busybox cat /work/signal-result)" = signal-ok || exit 82; '
                 'trap "exit 83" USR2; trap "" USR2; kill -USR2 $$; exit 0')
(bundle / "rootfs/etc/token").write_text("container-only\n")
base = {"ociVersion": "1.3.0", "root": {"path": "rootfs", "readonly": False},
        "process": {"terminal": False, "user": {"uid": 0, "gid": 0}, "cwd": "/work",
                    "args": ["/bin/busybox", "sh", "-c", script],
                    "env": ["PATH=/bin:/usr/bin", "NEGOTO_VALUE=oci untouched"]},
        "annotations": {"io.negoto.test": "unmodified-busybox"}, "linux": {"namespaces": []}}
(bundle / "config.json").write_text(json.dumps(base))
for name, argv, readonly in [
    ("readonly", ["/bin/busybox", "sh", "-c", "if printf forbidden > /work/forbidden; then exit 50; fi; exit 0"], True),
    ("hold", ["/bin/busybox", "sleep", "300"], False),
    ("true", ["/bin/busybox", "true"], False),
    ("signals", ["/bin/busybox", "sh", "-c", signal_script], False),
    ("signal-abi", ["/bin/signal-test"], False),
    ("interactive", ["/bin/busybox", "sh", "-i"], True),
    ("unsupported", ["/bin/busybox", "true"], False),
]:
    config = json.loads(json.dumps(base))
    config["root"] = {"path": "/alter/linux/bundles/busybox/rootfs", "readonly": readonly}
    config["process"]["args"] = argv
    if name == "unsupported":
        config["linux"]["namespaces"] = [{"type": "network"}]
    directory = payload / "bundles" / name
    directory.mkdir(parents=True)
    (directory / "config.json").write_text(json.dumps(config))

cargo = os.environ.get("CARGO", "cargo")
target = repo / "nanami/servers/targets/x86_64/x86_64-unknown-a9n.json"
environment = os.environ.copy()
environment["CARGO_TARGET_DIR"] = str(repo / "out/negoto-fixtures-target")
environment.pop("RUSTFLAGS", None)
environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", f"link-arg=-T{target.parent / 'linker/nanami_user.ld'}"])
subprocess.run([cargo, "build", "--manifest-path", str(repo / "tests/negoto/guest/Cargo.toml"), "--target", str(target),
                "-Z", "json-target-spec", "-Z", "build-std=core,alloc,compiler_builtins", "-Z",
                "build-std-features=compiler-builtins-mem", "--release", "--offline"], cwd=repo, env=environment, check=True)
fixture = repo / "out/negoto-fixtures-target/x86_64-unknown-a9n/release/negoto-smoke"
subprocess.run([cargo, "xtask", "build", "--no-linux-bins", "--linux-bin", str(fixture),
                "--linux-bin", str(busybox),
                "--linux-rootfs-dir", str(payload), "--rootfs", str(args.rootfs.resolve()),
                "--output-image", str(args.image.resolve()), "--offline"], cwd=repo, check=True)
print(f"Fixture payload retained at {payload}", flush=True)
