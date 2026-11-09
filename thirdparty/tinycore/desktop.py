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
    "binaries": ["xwininfo", "xsetroot"],
    "data_directories": [],
    "dependencies": ["libX11.tcz", "libxcb.tcz", "libXmu.tcz", "libXcursor.tcz"],
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
# Default desktop configuration, not work to repeat in every guest session.
PROFILE_LINKS = {
    "root/.wmx/Terminal": "/usr/local/bin/aterm",
    "root/.wmx/Files": "/usr/local/bin/xfe",
    "root/.wmx/Editor": "/usr/local/bin/editor",
    "root/.wmx/Images": "/usr/local/bin/flpicsee",
    "root/.wmx/Paint": "/usr/local/bin/mtpaint",
    "root/.wmx/Calculator": "/usr/local/bin/galculator",
    "root/.wmx/Clock": "/usr/local/bin/flit",
    "root/Pictures/TinyCore.png": "/usr/local/share/pixmaps/logo.png",
}


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


def configure_links(root):
    """Install image defaults without following or replacing destination aliases."""
    if root.is_symlink() or not root.is_dir():
        raise ValueError("desktop profile root must be a real directory")
    with tempfile.TemporaryDirectory(prefix="tinycore-profile-links-") as temporary:
        source = Path(temporary)
        for relative, target in PROFILE_LINKS.items():
            link = source / relative
            link.parent.mkdir(parents=True, exist_ok=True)
            link.symlink_to(target)
        merge(source, root)
    return dict(PROFILE_LINKS)


def build_readiness(root):
    """Build profile-owned glue against the pinned, unchanged runtime libraries."""
    return build_xlib_tool(root, HERE / "tools/wait-for-x.c", "usr/local/bin/nanami-wait-for-x")


def build_font_cache(root):
    """Precompute stock caches for the zero timestamps written by ImageBuilder.

    Use the locked guest loader, libraries and fc-cache, never the host's
    fontconfig version. Only the private staging copy receives packed mtimes;
    runtime directory validation remains entirely upstream's responsibility.
    """
    root = Path(root).resolve(strict=True)
    inputs = ("usr/local/etc/fonts", "usr/local/share/fontconfig",
              "usr/local/lib/X11/fonts")
    for relative in inputs:
        directory = root / relative
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"font cache input is not a real directory: {relative}")
        for path in directory.rglob("*"):
            if not path.resolve(strict=True).is_relative_to(root):
                raise ValueError(f"font cache input escapes profile: {path}")
    destination = root / "var/cache/fontconfig"
    for directory in (root / "var", root / "var/cache"):
        if directory.is_symlink():
            raise ValueError("font cache destination traverses a symlink")
    if destination.exists() or destination.is_symlink():
        raise ValueError("font cache destination is not empty")
    with tempfile.TemporaryDirectory(prefix="tinycore-font-cache-") as temporary:
        staging = Path(temporary)
        for relative in inputs:
            shutil.copytree(root / relative, staging / relative, symlinks=True)
        for path in [staging, *staging.rglob("*")]:
            if not path.is_symlink():
                os.utime(path, ns=(0, 0))
        (staging / "var/cache/fontconfig").mkdir(parents=True)
        environment = os.environ.copy()
        for key in list(environment):
            if key.startswith("FONTCONFIG_") or key == "SOURCE_DATE_EPOCH":
                del environment[key]
        environment.update(FONTCONFIG_FILE="/usr/local/etc/fonts/fonts.conf",
                           FONTCONFIG_PATH="/usr/local/etc/fonts", LC_ALL="C")
        subprocess.run([str(root / "lib/ld-linux-x86-64.so.2"), "--library-path",
                        f"{root}/usr/local/lib:{root}/lib",
                        str(root / "usr/local/bin/fc-cache"), "-s", "-y", str(staging)],
                       env=environment, check=True)
        caches = staging / "var/cache/fontconfig"
        if not list(caches.glob("*-le64.cache-9")):
            raise ValueError("pinned fontconfig did not produce its cache format")
        for path in caches.iterdir():
            if path.is_symlink() or not path.is_file():
                raise ValueError("unexpected font cache output")
        shutil.copytree(caches, destination)
        destination.chmod(0o755)
        for path in destination.iterdir():
            path.chmod(0o644)
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(destination.iterdir())}


def build_window_observer(root, steady_rtt=False, syscall_scope=False):
    """Install the test-only launch observer; never modify the desktop clients."""
    source = HERE.parents[1] / "tests/alter-linux/desktop-window-observer.c"
    flags = ["-DAPP_OBSERVER_STEADY_RTT"] if steady_rtt else []
    if syscall_scope:
        flags.append("-DAPP_OBSERVER_SYSCALL_SCOPE")
    return build_xlib_tool(root, source, "bin/aw-scope" if syscall_scope else "bin/aw", flags)


def build_xlib_tool(root, source, destination, extra_flags=(),
                    runtime_libraries=("usr/local/lib/libX11.so.6", "lib/libc.so.6")):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        raise ValueError("desktop profile root must be a real directory")
    resolved = root.resolve()
    relative_path = Path(destination)
    if relative_path.is_absolute() or ".." in relative_path.parts:
        raise ValueError("desktop helper destination must stay inside the profile")
    for relative in reversed(relative_path.parents):
        directory = root / relative
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"desktop helper directory is not real: {relative}")
    libraries = []
    for relative in runtime_libraries:
        library = (root / relative).resolve(strict=True)
        if not library.is_relative_to(resolved) or not library.is_file():
            raise ValueError(f"profile library escapes root: {relative}")
        libraries.append(str(library))
    binary = root / relative_path
    if binary.parent.is_symlink() or binary.is_symlink() or binary.exists():
        raise ValueError("desktop readiness destination is not empty")
    subprocess.run([
        os.environ.get("CC", "clang"), "--target=x86_64-unknown-linux-gnu", "-fuse-ld=lld",
        "-nostdlib", "-ffreestanding", "-fno-builtin", "-fno-stack-protector",
        "-fno-pie", "-no-pie", "-mno-sse", "-mno-sse2", "-mno-red-zone",
        "-O2", "-Wall", "-Wextra", "-Werror", "-Wl,-e,_start,--image-base=0x400000",
        "-Wl,--dynamic-linker=/lib/ld-linux-x86-64.so.2,--allow-shlib-undefined",
        *extra_flags, str(source), str(HERE / "tools/start-x86_64.S"),
        *libraries, "-o", str(binary),
    ], check=True)
    binary.chmod(0o755)
    return {str(binary.relative_to(root)): hashlib.sha256(binary.read_bytes()).hexdigest()}


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
