#!/usr/bin/env python3
"""Assemble a pinned Tiny Core userspace and an explicit Alter boot profile.

No root privileges, Linux kernel, host chroot, mount, or device nodes are needed.
Network inputs are the pinned rootfs and optional official TCZ desktop graph.
"""
import argparse
from dataclasses import dataclass
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import tempfile
import urllib.request

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
VERSION = "17.1"
URL = "https://distro.ibiblio.org/tinycorelinux/17.x/x86_64/release/distribution_files/rootfs64.gz"
SHA256 = "4eff7d1c521a77f00da07c56dd80d2c9ca4d667bf0919bcb7f05f86bf663a6ae"
MAX_ARCHIVE = 16 * 1024 * 1024
MAX_UNPACKED = 64 * 1024 * 1024


@dataclass
class Entry:
    name: str
    mode: int
    data: bytes
    inode: tuple
    links: int


def archive_entries(compressed):
    """Validate the entire newc archive before touching the output filesystem."""
    with gzip.GzipFile(fileobj=compressed) as stream:
        data = stream.read(MAX_UNPACKED + 1)
    if len(data) > MAX_UNPACKED:
        raise ValueError("rootfs exceeds the unpacked size limit")
    offset = 0
    entries = {}
    while offset + 110 <= len(data):
        header = data[offset:offset + 110]
        if header[:6] != b"070701":
            raise ValueError("expected a newc cpio header")
        fields = [int(header[i:i + 8], 16) for i in range(6, 110, 8)]
        ino, mode, _, _, links, _, size, major, minor, _, _, namesize, _ = fields
        begin = offset + 110
        end = begin + namesize
        if namesize < 1 or end > len(data) or data[end - 1] != 0:
            raise ValueError("invalid cpio pathname")
        name = data[begin:end - 1].decode("utf-8")
        begin = (end + 3) & ~3
        end = begin + size
        if end > len(data):
            raise ValueError("truncated cpio contents")
        if name == "TRAILER!!!":
            if any(data[(end + 3) & ~3:]):
                raise ValueError("unexpected data after cpio trailer")
            break
        while name.startswith("./"):
            name = name[2:]
        if name not in ("", "."):
            if name.startswith("/") or "\0" in name or any(
                part in ("", ".", "..") for part in name.split("/")
            ):
                raise ValueError(f"unsafe archive pathname: {name!r}")
            if name in entries:
                raise ValueError(f"duplicate archive pathname: {name}")
            entries[name] = Entry(name, mode, data[begin:end], (major, minor, ino), links)
        offset = (end + 3) & ~3
    else:
        raise ValueError("missing cpio trailer")
    for name in entries:
        for parent in PurePosixPath(name).parents:
            entry = entries.get(str(parent))
            if entry is not None and not stat.S_ISDIR(entry.mode):
                raise ValueError(f"archive traverses a non-directory: {name}")
    return entries


def unpack(entries, destination):
    """Links are created last, so no archive write follows a guest symlink."""
    destination.mkdir()
    skipped = []
    hardlinks = {}
    for entry in entries.values():
        if stat.S_ISREG(entry.mode) and entry.links > 1:
            group = hardlinks.setdefault(entry.inode, [])
            group.append(entry)
    payloads = {}
    for inode, group in hardlinks.items():
        bodies = [entry.data for entry in group if entry.data]
        if len(bodies) > 1 and any(body != bodies[0] for body in bodies):
            raise ValueError("conflicting hardlink contents")
        payloads[inode] = bodies[0] if bodies else b""
    for entry in sorted(entries.values(), key=lambda e: (e.name.count("/"), e.name)):
        path = destination / entry.name
        if stat.S_ISDIR(entry.mode):
            path.mkdir(parents=True, exist_ok=True)
            path.chmod(stat.S_IMODE(entry.mode) & 0o1777)
        elif stat.S_ISREG(entry.mode):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payloads.get(entry.inode, entry.data))
            # Do not install setuid executables on the build host. Some upstream
            # setuid binaries are execute-only: the packager must read them.
            path.chmod((stat.S_IMODE(entry.mode) & 0o777) | 0o400)
        elif not stat.S_ISLNK(entry.mode):
            # Alter owns /dev. Never mknod or pretend host files are devices.
            if not entry.name.startswith("dev/"):
                raise ValueError(f"unsupported special archive member: {entry.name}")
            skipped.append(entry.name)
    for entry in entries.values():
        if stat.S_ISLNK(entry.mode):
            target = entry.data.decode("utf-8")
            if not target or "\0" in target:
                raise ValueError(f"invalid link: {entry.name}")
            path = destination / entry.name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.symlink_to(target)
    return sorted(skipped)


def acquire(archive):
    if archive is not None:
        data = archive.read_bytes()
    else:
        cache = HERE / "downloads" / f"rootfs64-{VERSION}.gz"
        if cache.exists():
            data = cache.read_bytes()
        else:
            print(f"Downloading {URL}", flush=True)
            with urllib.request.urlopen(URL, timeout=60) as stream:
                data = stream.read(MAX_ARCHIVE + 1)
            if len(data) > MAX_ARCHIVE or hashlib.sha256(data).hexdigest() != SHA256:
                raise ValueError("download checksum/size mismatch; no files extracted")
            cache.parent.mkdir(parents=True, exist_ok=True)
            cache.write_bytes(data)
    if len(data) > MAX_ARCHIVE or hashlib.sha256(data).hexdigest() != SHA256:
        raise ValueError("rootfs checksum/size mismatch; no files extracted")
    # A local --archive also seeds the pinned cache for subsequent rebuilds.
    if archive is not None:
        cache = HERE / "downloads" / f"rootfs64-{VERSION}.gz"
        cache.parent.mkdir(parents=True, exist_ok=True)
        if not cache.exists():
            cache.write_bytes(data)
    return data


def configuration(smoke=False, graphical=False):
    args = ["/bin/busybox", "sh", "/init"]
    if smoke:
        args.append("--smoke")
    if graphical:
        args.append("--desktop")
    return {
        "ociVersion": "1.3.0",
        "root": {"path": "../tinycore/rootfs", "readonly": False},
        "process": {
            "terminal": False,
            "user": {"uid": 0, "gid": 0},
            "args": args,
            "env": ["PATH=/bin:/sbin:/usr/bin:/usr/sbin", "HOME=/root", "USER=root",
                    "TERM=nanami-256color", "LANG=C", "LC_ALL=C"],
            "cwd": "/",
        },
        "annotations": {"io.nanami.tinycore.profile": "alter-desktop-v1" if graphical else "alter-console-v1",
                        "io.nanami.graphics": str(graphical).lower()},
    }


def assemble(archive, rebuild=False, graphical=False, offline=False, output=None):
    import io
    entries = archive_entries(io.BytesIO(archive))
    release = entries.get("etc/os-release")
    if release is None or f"VERSION_ID={VERSION}".encode() not in release.data:
        raise ValueError("unexpected Tiny Core release")
    output = Path(output) if output is not None else HERE / ("build-desktop" if graphical else "build")
    if output.resolve() in (Path("/"), Path.home().resolve(), REPO.resolve(), HERE.resolve()):
        raise ValueError("output must be a dedicated generated build directory")
    if output.is_symlink():
        raise ValueError("build/ must not be a symlink")
    if output.exists() and not rebuild:
        raise ValueError("build/ already exists; use --rebuild to replace generated files")
    if output.exists() and not (output / "manifest.json").is_file():
        raise ValueError("refusing to replace an unrecognized build directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".tinycore-", dir=output.parent))
    try:
        upstream = staging / "upstream"
        skipped = unpack(entries, upstream)
        overlay = staging / "overlay"
        bundle = overlay / "tinycore"
        bundle.mkdir(parents=True)
        guest = bundle / "rootfs"
        shutil.copytree(upstream, guest, symlinks=True)
        desktop_lock = None
        if graphical:
            import desktop
            desktop_lock = desktop.install(guest, offline)
        profile = HERE / "profile"
        changes = []
        for source in sorted(profile.rglob("*")):
            if source.is_file():
                relative = source.relative_to(profile)
                target = guest / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                if target.is_symlink():
                    raise ValueError(f"profile would overwrite a symlink: {relative}")
                shutil.copyfile(source, target)
                mode = 0o644
                if source.read_bytes().startswith(b"#!"):
                    mode = 0o755
                target.chmod(mode)
                changes.append(str(relative))
        for name, smoke in [("tinycore", False), ("tinycore-smoke", True)]:
            directory = overlay / name
            directory.mkdir(exist_ok=True)
            (directory / "config.json").write_text(json.dumps(configuration(smoke), indent=2) + "\n")
        if graphical:
            directory = overlay / "tinycore-desktop"
            directory.mkdir()
            (directory / "config.json").write_text(json.dumps(configuration(graphical=True), indent=2) + "\n")
        manifest = {"version": VERSION, "url": URL, "sha256": SHA256,
                    "profile": "alter-console-v1", "overlaid_files": changes,
                    "omitted_device_nodes": skipped,
                    "entry": ["/bin/busybox", "sh", "/init"]}
        if desktop_lock is not None:
            manifest["desktop"] = desktop_lock
        (staging / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        if output.exists():
            shutil.rmtree(output)
        staging.rename(output)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    print(f"Tiny Core {VERSION} assembled at {output}", flush=True)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, help="use a local checksum-pinned rootfs64.gz")
    parser.add_argument("--rebuild", action="store_true", help="replace only this generated build/")
    parser.add_argument("--image", action="store_true", help="also build a separate Nanami boot disk")
    parser.add_argument("--desktop", action="store_true", help="include the pinned Xfbdev/FLWM desktop in build-desktop/")
    parser.add_argument("--with-tests", action="store_true", help="with --image, include the native integration probe")
    parser.add_argument("--offline", action="store_true", help="use cached/local archive and Cargo dependencies only")
    parser.add_argument("--output-dir", type=Path, help="assemble a separate build without replacing an existing disk")
    parser.add_argument("--size-mib", type=int, help="rootfs capacity (desktop: 256 MiB; console: 128 MiB)")
    args = parser.parse_args()
    if args.with_tests and not args.image:
        parser.error("--with-tests requires --image")
    size_mib = args.size_mib
    if size_mib is None:
        size_mib = 128
        if args.desktop:
            size_mib = 256
    if size_mib < 32:
        parser.error("--size-mib must be at least 32")
    if args.offline and args.archive is None and not (HERE / "downloads" / f"rootfs64-{VERSION}.gz").is_file():
        parser.error("--offline requires --archive or a cached download")
    output = assemble(acquire(args.archive), args.rebuild, args.desktop, args.offline, args.output_dir)
    if args.image:
        cargo = os.environ.get("CARGO", "cargo")
        extra = ["--offline"] if args.offline else []
        payload = []
        if args.with_tests:
            if args.desktop:
                script = output / "overlay/tinycore/rootfs/usr/local/bin/nanami-desktop-test"
                shutil.copyfile(REPO / "tests/alter-linux/desktop-apps.sh", script)
                script.chmod(0o755)
            for name, define, source in [("nanami-chmod", [], "chmod.c"),
                                         ("nanami-chmod-readonly", ["-DCHMOD_READONLY"], "chmod.c"),
                                         ("nanami-procfs", [], "procfs.c"),
                                         ("nanami-desktop-syscalls", [], "desktop-syscalls.c"),
                                         ("nanami-desktop-performance", [], "desktop-performance.c"),
                                         ("nanami-desktop-pointer", [], "desktop-pointer.c"),
                                         ("nanami-namespace", [], "namespace.c"),
                                         ("nanami-process-performance", [], "process-performance.c"),
                                         ("nanami-exec-cache", [], "exec-cache.c"),
                                         ("nanami-file-mapping", [], "file-mapping.c")]:
                binary = output / "overlay/tinycore/rootfs/bin" / name
                subprocess.run([os.environ.get("CC", "clang"), "--target=x86_64-unknown-linux-gnu",
                                "-fuse-ld=lld", "-nostdlib", "-static", "-ffreestanding", "-fno-builtin",
                                "-fno-stack-protector", "-mno-sse", "-mno-sse2", "-mno-red-zone",
                                "-O2", "-Wall", "-Wextra", "-Werror", "-Wl,-e,_start,--image-base=0x400000",
                                *define, str(REPO / "tests/alter-linux" / source),
                                str(REPO / "tests/alter-linux/signals-x86_64.S"),
                                *([str(REPO / "tests/alter-linux/desktop-syscalls-x86_64.S")]
                                  if source == "desktop-syscalls.c" else []),
                                "-o", str(binary)], check=True)
                config = configuration()
                config["process"]["args"] = [f"/bin/{name}"]
                config["root"]["readonly"] = bool(define)
                bundle = output / "overlay" / name
                bundle.mkdir()
                (bundle / "config.json").write_text(json.dumps(config, indent=2) + "\n")
            target = REPO / "nanami/servers/targets/x86_64/x86_64-unknown-a9n.json"
            environment = os.environ.copy()
            environment["CARGO_TARGET_DIR"] = str(output / "probe-target")
            environment.pop("RUSTFLAGS", None)
            environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
                "-C", f"link-arg=-T{target.parent / 'linker/nanami_user.ld'}"])
            subprocess.run([cargo, "build", "--manifest-path", str(REPO / "tests/negoto/guest/Cargo.toml"),
                            "--target", str(target), "-Zjson-target-spec",
                            "-Zbuild-std=core,alloc,compiler_builtins",
                            "-Zbuild-std-features=compiler-builtins-mem", "--release", *extra],
                           cwd=REPO, env=environment, check=True)
            payload = ["--linux-bin", str(output / "probe-target/x86_64-unknown-a9n/release/negoto-smoke")]
        subprocess.run([cargo, "xtask", "build", "--skip-cpp", "--no-linux-bins", *payload,
                        "--linux-rootfs-dir", str(output / "overlay"), "--size-mib", str(size_mib),
                        "--rootfs", str(output / "rootfs.img"),
                        "--output-image", str(output / "nanami-tinycore.img"), *extra],
                       cwd=REPO, check=True)


if __name__ == "__main__":
    main()
