#!/usr/bin/env python3
"""Build optional child-domain guest fixtures and a separately configured image."""
import argparse
import os
import pathlib
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--arch", choices=["x86_64", "aarch64"], required=True)
parser.add_argument("--image", type=pathlib.Path, required=True)
parser.add_argument("--rootfs", type=pathlib.Path, required=True)
parser.add_argument("--kernel-capabilities", action="store_true", help="Include a private common-ABI kernel capability probe")
parser.add_argument("--service-imports", action="store_true", help="Test explicitly delegated managed Sessions across Alpha domains")
parser.add_argument("--recursive-imports", action="store_true", help="Test reimport and accounting through a child and grandchild Alpha")
parser.add_argument("--session-buffers", action="store_true", help="Test opaque managed buffer handover and real stale-access faults")
parser.add_argument("--service-events", action="store_true", help="Test direct event/data queue handover, pending work, abort and client closure")
args = parser.parse_args()
if sum([args.kernel_capabilities, args.service_imports, args.recursive_imports, args.session_buffers, args.service_events]) > 1:
    parser.error("select one optional fixture configuration")
repo = pathlib.Path(__file__).resolve().parents[2]
fixtures = repo / "tests/alpha/guest"
target = repo / f"nanami/servers/targets/{args.arch}/{args.arch}-unknown-a9n.json"
linker = target.parent / "linker/nanami_user.ld"
environment = os.environ.copy()
environment["CARGO_TARGET_DIR"] = str(repo / "out/domain-fixtures-target")
environment.pop("RUSTFLAGS", None)
environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", f"link-arg=-T{linker}"])
cargo = os.environ.get("CARGO", "cargo")
subprocess.run([cargo, "build", "--locked", "--manifest-path", str(fixtures / "Cargo.toml"),
                "--target", str(target), "-Z", "json-target-spec",
                "-Z", "build-std=core,alloc,compiler_builtins",
                "-Z", "build-std-features=compiler-builtins-mem", "--release"],
               cwd=repo, env=environment, check=True)
artifacts = repo / f"out/domain-fixtures-target/{args.arch}-unknown-a9n/release"
modules = [fixtures / "child.conf", artifacts / "domain-probe", artifacts / "domain-leaf", artifacts / "handover-node", artifacts / "handover-bad"]
boot_list = fixtures / f"boot-{args.arch}.conf"
if args.session_buffers:
    buffers = repo / "tests/alpha/buffers"
    boot_list = buffers / f"boot-{args.arch}.conf"
    modules = [buffers / "child.conf", artifacts / "buffer-manager", artifacts / "buffer-provider"]
if args.service_events:
    events = repo / "tests/alpha/events"
    boot_list = events / f"boot-{args.arch}.conf"
    modules = [events / "child.conf", artifacts / "event-manager", artifacts / "event-provider", artifacts / "event-caller"]
if args.service_imports:
    imports = repo / "tests/alpha/imports"
    boot_list = imports / f"boot-{args.arch}.conf"
    modules = [imports / "child.conf", artifacts / "import-manager", artifacts / "import-client", artifacts / "handover-node"]
if args.recursive_imports:
    imports = repo / "tests/alpha/recursive-imports"
    boot_list = imports / f"boot-{args.arch}.conf"
    modules = [imports / "relay.conf", imports / "child.conf",
               artifacts / "import-manager", artifacts / "import-client",
               artifacts / "import-relay", artifacts / "handover-node"]
if args.kernel_capabilities:
    probe = repo / "tests/alpha/capabilities"
    probe_environment = environment.copy()
    probe_environment["CARGO_TARGET_DIR"] = str(repo / "out/capability-probe-target")
    probe_environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(["-C", f"link-arg=-T{repo / 'targets' / args.arch / 'alpha.ld'}"])
    subprocess.run([cargo, "build", "--locked", "--manifest-path", str(probe / "Cargo.toml"),
                    "--target", str(target), "-Z", "json-target-spec",
                    "-Z", "build-std=core,alloc,compiler_builtins",
                    "-Z", "build-std-features=compiler-builtins-mem", "--release"],
                   cwd=repo, env=probe_environment, check=True)
    modules.extend([probe / "empty.conf", repo / f"out/capability-probe-target/{args.arch}-unknown-a9n/release/nanami-capability-probe"])
    boot_list = probe / f"boot-{args.arch}.conf"
command = [cargo, "xtask", "build", "--arch", args.arch, "--no-linux-bins",
           "--rootfs", str(args.rootfs.resolve()), "--output-image", str(args.image.resolve()),
           "--boot-list", str(boot_list)]
for module in modules:
    command.extend(["--boot-module", str(module)])
subprocess.run(command, cwd=repo, check=True)
