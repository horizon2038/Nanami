"""Checksum-pinned official TCZ desktop packages, expanded at build time.

No guest mounts, host root privileges or package install scripts are required.
Refresh the lock explicitly; ordinary builds consume only the pinned graph.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

HERE = Path(__file__).resolve().parent
BASE = "https://distro.ibiblio.org/tinycorelinux/17.x/x86_64/tcz/"
ROOTS = ["Xfbdev.tcz", "flwm_topside.tcz", "aterm.tcz", "wbar.tcz",
         "xfe.tcz", "flpicsee.tcz", "mtpaint.tcz", "galculator.tcz", "flit.tcz",
         "Xorg-7.7-bin.tcz"]
# This TCZ also contains GL/hardware diagnostics. Installing only these unchanged
# X11 clients does not require its Mesa/LLVM stack. Keep the selection in the lock.
SELECTIONS = {"Xorg-7.7-bin.tcz": {
    "binaries": ["xwininfo"],
    "data_directories": [],
    "dependencies": ["libX11.tcz", "libxcb.tcz"],
}, "util-linux.tcz": {
    # GTK's session libraries need these libraries, not Linux hardware tools.
    # Xlibs already supplies the mcookie executable used by X11.
    "binaries": [],
    "data_directories": ["usr/local/lib"],
    "dependencies": ["readline.tcz", "udev-lib.tcz"],
}, "fox.tcz": {
    # Xfe needs the unmodified FOX runtime, not its unrelated demo programs.
    # libFOX's DT_NEEDED does not include GL/GLU; the full TCZ's demo dependency
    # would otherwise install the unused Mesa/LLVM stack.
    "binaries": [],
    "data_directories": ["usr/local/lib"],
    "dependencies": ["libXft.tcz", "libjpeg-turbo.tcz", "glibc_add_lib.tcz", "libXrandr.tcz",
                     "libpng.tcz", "bzip2-lib.tcz"],
}}
# Both runtime ABIs coexist. Keep 1.4's unversioned development-link aliases;
# 1.3 applications link to the distinct, unchanged .so.1.3 runtime libraries.
OMIT_SYMLINKS = {"fltk-1.3.tcz": ["usr/local/lib/libfltk.so",
                                 "usr/local/lib/libfltk_forms.so",
                                 "usr/local/lib/libfltk_images.so"]}
TOOLS_URL = "https://codeload.github.com/plougher/squashfs-tools/tar.gz/refs/tags/4.6.1"
TOOLS_SHA256 = "9c4974e07c61547dae14af4ed1f358b7d04618ae194e54d6be72ee126f0d2f53"
CACHE = HERE / "downloads" / "desktop"
LOCK = HERE / "desktop.lock.json"
LIMIT = 32 * 1024 * 1024


def download(url, optional=False):
    for attempt in range(3):
        try:
            with urllib.request.urlopen(url, timeout=30) as stream:
                data = stream.read(LIMIT + 1)
            break
        except urllib.error.HTTPError as error:
            if optional and error.code == 404:
                return b""
            # The public mirror intermittently returns 403 as well as 5xx.
            # Bound retries; never substitute unchecked package contents.
            if error.code not in (403, 429, 500, 502, 503, 504) or attempt == 2:
                raise
            time.sleep(attempt + 1)
    if len(data) > LIMIT:
        raise ValueError(f"download exceeds limit: {url}")
    return data


def package_name(name):
    if not re.fullmatch(r"[A-Za-z0-9_+.-]+\.tcz", name):
        raise ValueError(f"invalid desktop dependency: {name!r}")
    return name


def refresh(extend=False):
    CACHE.mkdir(parents=True, exist_ok=True)
    packages = {}
    if extend:
        previous = json.loads(LOCK.read_text())
        if previous["repository"] != BASE:
            raise ValueError("existing lock cannot be extended by this profile")
        packages.update(previous["packages"])
        for name, metadata in list(packages.items()):
            if metadata.get("selection") != SELECTIONS.get(name):
                del packages[name]
    visiting = set()
    resolved = set()

    def visit(name):
        package_name(name)
        if name in resolved:
            return
        if name in visiting:
            raise ValueError(f"cyclic desktop dependency: {name}")
        visiting.add(name)
        if name in packages:
            metadata = packages[name]
            dependencies = metadata.get("selection", {}).get("dependencies", metadata["dependencies"])
            for dependency in dependencies:
                visit(dependency)
            visiting.remove(name)
            resolved.add(name)
            return
        dependencies = [line.strip() for line in download(BASE + name + ".dep", True).decode().splitlines()
                        if line.strip() and not line.lstrip().startswith("#")]
        selection = SELECTIONS.get(name)
        installed_dependencies = dependencies
        if selection is not None:
            installed_dependencies = selection["dependencies"]
        for dependency in installed_dependencies:
            visit(dependency)
        checksum = download(BASE + name + ".md5.txt").decode().split()[0]
        if not re.fullmatch(r"[a-fA-F0-9]{32}", checksum):
            raise ValueError(f"invalid upstream checksum: {name}")
        path = CACHE / name
        data = path.read_bytes() if path.exists() else download(BASE + name)
        if hashlib.md5(data).hexdigest() != checksum.lower():
            raise ValueError(f"upstream checksum changed: {name}; inspect the cached input")
        path.write_bytes(data)
        packages[name] = {"sha256": hashlib.sha256(data).hexdigest(),
                          "md5": checksum.lower(), "bytes": len(data), "dependencies": dependencies}
        if selection is not None:
            packages[name]["selection"] = selection
        visiting.remove(name)
        resolved.add(name)
        print(f"Pinned {name}: {len(data)} bytes", flush=True)

    for name in ROOTS:
        visit(name)
    reachable = set()

    def mark(name):
        if name in reachable:
            return
        reachable.add(name)
        metadata = packages[name]
        dependencies = metadata.get("selection", {}).get("dependencies", metadata["dependencies"])
        for dependency in dependencies:
            mark(dependency)

    for name in ROOTS:
        mark(name)
    packages = {name: metadata for name, metadata in packages.items() if name in reachable}
    for name, metadata in packages.items():
        if name in OMIT_SYMLINKS:
            metadata["omitted_symlinks"] = OMIT_SYMLINKS[name]
    LOCK.write_text(json.dumps({"repository": BASE, "roots": ROOTS, "packages": packages}, indent=2) + "\n")


def cached(name, checksum, url, offline):
    path = CACHE / name
    if not path.exists():
        if offline:
            raise ValueError(f"missing offline desktop input: {name}")
        data = download(url)
        if hashlib.sha256(data).hexdigest() != checksum:
            raise ValueError(f"desktop checksum mismatch: {name}")
        CACHE.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    if hashlib.sha256(path.read_bytes()).hexdigest() != checksum:
        raise ValueError(f"cached desktop checksum mismatch: {name}")
    return path


def extractor(offline):
    supplied = os.environ.get("UNSQUASHFS") or shutil.which("unsquashfs")
    if supplied:
        return supplied
    executable = CACHE / "host-tools/squashfs-tools-4.6.1/squashfs-tools/unsquashfs"
    if executable.is_file():
        return str(executable)
    archive = cached("squashfs-tools-4.6.1.tar.gz", TOOLS_SHA256, TOOLS_URL, offline)
    directory = CACHE / "host-tools"
    directory.mkdir(exist_ok=True)
    with tarfile.open(archive) as source:
        source.extractall(directory, filter="data")
    cflags = []
    ldflags = []
    if shutil.which("pkg-config"):
        cflags = subprocess.check_output(["pkg-config", "--cflags", "liblzma"], text=True).split()
        ldflags = subprocess.check_output(["pkg-config", "--libs-only-L", "liblzma"], text=True).split()
    subprocess.run(["make", "-C", str(executable.parent), "-j4", "unsquashfs",
                    "CONFIG=1", "GZIP_SUPPORT=1", "COMP_DEFAULT=gzip", "XZ_SUPPORT=1", "XATTR_SUPPORT=0",
                    f"CC={os.environ.get('HOST_CC', 'cc')}", f"EXTRA_CFLAGS={' '.join(cflags)}",
                    f"EXTRA_LDFLAGS={' '.join(ldflags)}"], check=True)
    return str(executable)


def merge(source, target):
    """Never follow a destination symlink while installing a TCZ tree."""
    for entry in sorted(source.iterdir()):
        destination = target / entry.name
        mode = entry.lstat().st_mode
        if stat.S_ISDIR(mode):
            if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
                raise ValueError(f"desktop directory collision: {destination}")
            destination.mkdir(exist_ok=True)
            merge(entry, destination)
        elif stat.S_ISLNK(mode):
            link = entry.readlink()
            if destination.is_symlink() and destination.readlink() == link:
                continue
            if destination.exists() or destination.is_symlink():
                raise ValueError(f"desktop symlink collision: {destination}")
            destination.symlink_to(link)
        elif stat.S_ISREG(mode):
            if destination.is_symlink():
                raise ValueError(f"desktop write through symlink: {destination}")
            if destination.exists() and destination.read_bytes() != entry.read_bytes():
                raise ValueError(f"desktop file collision: {destination}")
            shutil.copyfile(entry, destination)
            destination.chmod((stat.S_IMODE(mode) & 0o777) | 0o400)
        else:
            raise ValueError(f"desktop contains a host special file: {entry}")


def install(root, offline=False):
    lock = json.loads(LOCK.read_text())
    if lock["repository"] != BASE or lock["roots"] != ROOTS:
        raise ValueError("desktop lock does not describe this profile")
    tool = extractor(offline)
    for name, metadata in lock["packages"].items():
        package_name(name)
        if metadata.get("selection") != SELECTIONS.get(name):
            raise ValueError(f"desktop selection changed: {name}; refresh the lock")
        if metadata.get("omitted_symlinks", []) != OMIT_SYMLINKS.get(name, []):
            raise ValueError(f"desktop symlink policy changed: {name}; refresh the lock")
        archive = cached(name, metadata["sha256"], BASE + name, offline)
        with tempfile.TemporaryDirectory(prefix="tinycore-tcz-") as temporary:
            extracted = Path(temporary) / "root"
            subprocess.run([tool, "-no-progress", "-no-xattrs", "-processors", "2",
                            "-d", str(extracted), str(archive)], check=True)
            for relative in metadata.get("omitted_symlinks", []):
                alias = extracted / relative
                if not alias.is_symlink():
                    raise ValueError(f"expected a development symlink: {relative}")
                alias.unlink()
            selection = metadata.get("selection")
            if selection is None:
                merge(extracted, root)
            else:
                selected = Path(temporary) / "selected"
                selected.mkdir()
                for binary in selection["binaries"]:
                    relative = Path("usr/local/bin") / binary
                    source = extracted / relative
                    if not source.is_file() or source.is_symlink():
                        raise ValueError(f"selected desktop binary missing: {binary}")
                    destination = selected / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(source, destination)
                for relative in selection["data_directories"]:
                    source = extracted / relative
                    if source.is_symlink():
                        raise ValueError(f"selected data directory is a symlink: {relative}")
                    if source.is_dir():
                        shutil.copytree(source, selected / relative, symlinks=True)
                merge(selected, root)
    return lock


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    operation = parser.add_mutually_exclusive_group(required=True)
    operation.add_argument("--refresh-lock", action="store_true")
    operation.add_argument("--extend-lock", action="store_true", help="pin additions without updating existing packages")
    args = parser.parse_args()
    refresh(args.extend_lock)
