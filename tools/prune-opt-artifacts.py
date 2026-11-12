#!/usr/bin/env python3
"""Prune complete optimization artifact trees; dry run unless --apply is given."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--current", required=True)
    parser.add_argument("--previous", required=True)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--prune-build-cache", action="store_true",
                        help="also remove reproducible host build caches and obsolete baseline images")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    base = root / "thirdparty/tinycore/build-opt-kvm-rtt"
    logs = root / "session-handoff/logs/linux-native-scope-20261107"
    names = {args.current, args.previous}
    assert len(names) == 2
    assert all(name and Path(name).name == name and name not in {".", ".."} for name in names)
    assert base.is_dir() and not base.is_symlink()
    for process in Path("/proc").iterdir():
        if process.name.isdigit():
            try:
                command = (process / "comm").read_text().strip()
            except (FileNotFoundError, ProcessLookupError):
                continue
            assert not command.startswith(("qemu-system", "cargo", "rustc")), command

    protected = {}
    runs = set()
    for name in names:
        directory = base / name
        assert directory.is_dir() and not directory.is_symlink()
        checks = json.loads((logs / name / "checks.json").read_text())
        for role, expected in checks["images"].items():
            image = directory / (role + ".img")
            assert sha(image) == expected, image
        for path in directory.rglob("*"):
            assert not path.is_symlink(), path
            if path.is_file():
                protected[path] = sha(path)
        runs.add("nanami-negoto-qemu-" + checks["runs"]["files"]["run"])

    current_checks = json.loads((logs / args.current / "checks.json").read_text())
    for field in ("normal", "additional_normal"):
        for name, expected in current_checks.get(field, {}).items():
            path = root / name
            assert sha(path) == expected, path
            protected[path] = expected

    scope = base / "scope-disk-runs"
    assert scope.is_dir() and not scope.is_symlink()
    for name in runs:
        directory = scope / name
        assert directory.is_dir() and not directory.is_symlink()
        for path in directory.rglob("*"):
            assert not path.is_symlink(), path
            if path.is_file():
                protected[path] = sha(path)
    obsolete = [path for path in base.iterdir()
                if path.name not in names | {"scope-disk-runs"}]
    obsolete += [path for path in scope.iterdir() if path.name not in runs]
    obsolete += [root / "thirdparty/tinycore" / name
                 for name in ("build-opt-baseline", "build-opt-97b0ac1")
                 if (root / "thirdparty/tinycore" / name).exists()]
    if args.prune_build_cache:
        obsolete += [root / name for name in (
            "target",
            "thirdparty/tinycore/build-desktop",
            "out/spencer-before-kvm-build-20261029.img",
            "out/alpha-target",
            "nanami/servers/target/release",
            "nanami/servers/target/x86_64-unknown-a9n/release/.fingerprint",
            "nanami/servers/target/x86_64-unknown-a9n/release/build",
            "nanami/servers/target/x86_64-unknown-a9n/release/incremental",
            "nanami/servers/target/x86_64-unknown-a9n/release/examples",
            "spencer/out/x86_64-pc99-release/a9nloader_target_dir",
            "spencer/out/x86_64-pc99-release/a9nloader/deps",
            "spencer/out/x86_64-pc99-release/a9nloader/.fingerprint",
            "spencer/out/x86_64-pc99-release/a9nloader/build",
            "spencer/out/x86_64-pc99-release/nun_os_target_dir/release",
            "spencer/out/x86_64-pc99-release/nun_os_target_dir/x86_64-unknown-a9n/release/.fingerprint",
            "spencer/out/x86_64-pc99-release/nun_os_target_dir/x86_64-unknown-a9n/release/build",
            "spencer/out/x86_64-pc99-release/nun_os_target_dir/x86_64-unknown-a9n/release/incremental",
            "spencer/out/x86_64-pc99-release/nun_os_target_dir/x86_64-unknown-a9n/release/examples",
        ) if (root / name).exists()]
    for directory in obsolete:
        assert not any(path == directory or directory in path.parents for path in protected), directory

    rows = []
    inodes = {}
    for path in sorted(obsolete):
        files = [path] if path.is_symlink() or path.is_file() else list(path.rglob("*"))
        count = 0
        for file in files:
            if file.is_symlink() or file.is_file():
                info = file.lstat()
                inodes[info.st_dev, info.st_ino] = info.st_blocks * 512
                count += 1
        rows.append(dict(path=str(path.relative_to(root)), files=count))
    if args.apply:
        for path in obsolete:
            if path.is_symlink() or path.is_file():
                path.unlink()
            else:
                shutil.rmtree(path)
        assert all(not path.exists() and not path.is_symlink() for path in obsolete)
        for path, expected in protected.items():
            assert sha(path) == expected, path
    print(json.dumps(dict(applied=args.apply, current=args.current, previous=args.previous,
                          obsolete=rows, files=sum(row["files"] for row in rows),
                          unique_file_allocated_bytes=sum(inodes.values()),
                          protected_files=len(protected), protected_sha256_verified=True), indent=2))


if __name__ == "__main__":
    main()
