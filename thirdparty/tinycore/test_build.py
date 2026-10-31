"""Archive assembly tests; no network, device nodes or guest execution."""
import gzip
import io
import json
import hashlib
from pathlib import Path
import stat
import tempfile
import unittest
import urllib.error
from unittest import mock

import build
import desktop


def archive(records):
    output = bytearray()
    for number, (name, mode, body) in enumerate([*records, ("TRAILER!!!", 0, b"")]):
        name = name.encode() + b"\0"
        fields = [number, mode, 0, 0, 1, 0, len(body), 0, 0, 0, 0, len(name), 0]
        output.extend(b"070701" + "".join(f"{field:08x}" for field in fields).encode())
        output.extend(name)
        output.extend(b"\0" * (-len(output) % 4))
        output.extend(body)
        output.extend(b"\0" * (-len(output) % 4))
    return gzip.compress(bytes(output))


class ArchiveTests(unittest.TestCase):
    def read(self, records):
        return build.archive_entries(io.BytesIO(archive(records)))

    def test_links_and_devices_are_not_host_paths(self):
        entries = self.read([
            ("bin", stat.S_IFDIR | 0o755, b""),
            ("bin/busybox", stat.S_IFREG | 0o755, b"ELF"),
            ("bin/sh", stat.S_IFLNK | 0o777, b"/bin/busybox"),
            ("dev/null", stat.S_IFCHR | 0o666, b""),
            ("execute-only", stat.S_IFREG | 0o4111, b"payload"),
        ])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "root"
            self.assertEqual(build.unpack(entries, root), ["dev/null"])
            self.assertEqual((root / "bin/sh").readlink(), Path("/bin/busybox"))
            self.assertFalse((root / "dev/null").exists())
            self.assertEqual((root / "execute-only").read_bytes(), b"payload")
            self.assertFalse((root / "execute-only").stat().st_mode & stat.S_ISUID)

    def test_rejects_traversal_absolute_duplicate_and_symlink_parents(self):
        for name in ["../escape", "/absolute", "a/../escape", "a//b", "a/./b"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.read([(name, stat.S_IFREG, b"")])
        with self.assertRaises(ValueError):
            self.read([("same", stat.S_IFREG, b""), ("same", stat.S_IFREG, b"")])
        with self.assertRaises(ValueError):
            self.read([("parent", stat.S_IFLNK, b"/tmp"),
                       ("parent/file", stat.S_IFREG, b"escape")])

    def test_truncated_and_missing_trailer_rejected(self):
        complete = archive([("file", stat.S_IFREG, b"123")])
        raw = gzip.decompress(complete)
        for damaged in [raw[:100], raw[:120], raw[:-40]]:
            with self.subTest(size=len(damaged)), self.assertRaises(ValueError):
                build.archive_entries(io.BytesIO(gzip.compress(damaged)))

    def test_bundle_runs_real_tinycore_busybox_and_profile_init(self):
        config = build.configuration()
        self.assertEqual(config["process"]["args"], ["/bin/busybox", "sh", "/init"])
        self.assertEqual(config["root"]["path"], "../tinycore/rootfs")
        self.assertFalse(config["process"]["terminal"])
        self.assertEqual(build.configuration(True)["process"]["args"][-1], "--smoke")

    def test_graphics_is_explicit_and_console_stays_unprivileged(self):
        self.assertEqual(build.configuration()["annotations"]["io.nanami.graphics"], "false")
        config = build.configuration(graphical=True)
        self.assertEqual(config["process"]["args"][-1], "--desktop")
        self.assertEqual(config["annotations"]["io.nanami.graphics"], "true")
        self.assertEqual(config["annotations"]["io.nanami.tinycore.profile"], "alter-desktop-v1")


class DesktopTests(unittest.TestCase):
    def test_public_mirror_retry_is_bounded_and_does_not_hide_errors(self):
        forbidden = urllib.error.HTTPError("https://example.invalid", 403, "temporary", {}, None)
        response = mock.MagicMock()
        response.__enter__.return_value.read.return_value = b"pinned-input"
        with mock.patch.object(desktop.urllib.request, "urlopen", side_effect=[forbidden, response]) as fetch, mock.patch.object(desktop.time, "sleep"):
            self.assertEqual(desktop.download("https://example.invalid"), b"pinned-input")
            self.assertEqual(fetch.call_count, 2)
        with mock.patch.object(desktop.urllib.request, "urlopen", side_effect=forbidden) as fetch, mock.patch.object(desktop.time, "sleep"):
            with self.assertRaises(urllib.error.HTTPError):
                desktop.download("https://example.invalid")
            self.assertEqual(fetch.call_count, 3)

    def test_optional_missing_dependency_is_not_retried(self):
        missing = urllib.error.HTTPError("https://example.invalid", 404, "missing", {}, None)
        with mock.patch.object(desktop.urllib.request, "urlopen", side_effect=missing) as fetch:
            self.assertEqual(desktop.download("https://example.invalid", True), b"")
            self.assertEqual(fetch.call_count, 1)

    def test_extend_lock_preserves_existing_pins_and_prunes_unreachable_packages(self):
        payload = b"new-tcz"
        old = {"sha256": "original", "md5": "original", "bytes": 1, "dependencies": []}
        previous = {"repository": desktop.BASE, "roots": ["old.tcz", "unused.tcz"],
                    "packages": {"old.tcz": old, "unused.tcz": old}}
        responses = {"new.tcz.dep": b"old.tcz\n",
                     "new.tcz.md5.txt": (hashlib.md5(payload).hexdigest() + "  new.tcz").encode(),
                     "new.tcz": payload}
        with tempfile.TemporaryDirectory() as temporary:
            lock = Path(temporary) / "lock.json"
            lock.write_text(json.dumps(previous))
            with mock.patch.object(desktop, "LOCK", lock), mock.patch.object(desktop, "CACHE", Path(temporary)), mock.patch.object(desktop, "ROOTS", ["old.tcz", "new.tcz"]), mock.patch.object(desktop, "download", side_effect=lambda url, *args: responses[url.removeprefix(desktop.BASE)]):
                desktop.refresh(extend=True)
            result = json.loads(lock.read_text())
            self.assertEqual(result["packages"]["old.tcz"], old)
            self.assertEqual(set(result["packages"]), {"old.tcz", "new.tcz"})
            self.assertEqual(result["packages"]["new.tcz"]["sha256"], hashlib.sha256(payload).hexdigest())

    def test_file_manager_uses_fox_runtime_without_gl_demo_dependencies(self):
        self.assertIn("xfe.tcz", desktop.ROOTS)
        self.assertNotIn("fluff.tcz", desktop.ROOTS)
        selection = desktop.SELECTIONS["fox.tcz"]
        self.assertEqual(selection["binaries"], [])
        self.assertEqual(selection["data_directories"], ["usr/local/lib"])
        self.assertNotIn("glu.tcz", selection["dependencies"])

    def test_x11_diagnostic_selection_does_not_install_gpu_clients(self):
        name = "Xorg-7.7-bin.tcz"
        selection = desktop.SELECTIONS[name]
        self.assertEqual(selection["binaries"], ["xwininfo"])
        self.assertNotIn("libGL.tcz", selection["dependencies"])
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            root = temporary / "guest"
            root.mkdir()
            lock = temporary / "lock.json"
            lock.write_text(json.dumps({"repository": desktop.BASE, "roots": [name],
                                        "packages": {name: {"sha256": "test", "selection": selection}}}))

            def extract(command, **kwargs):
                destination = Path(command[command.index("-d") + 1]) / "usr/local/bin"
                destination.mkdir(parents=True)
                (destination / "xwininfo").write_bytes(b"unchanged-ELF")
                (destination / "xdriinfo").write_bytes(b"not-selected")

            with mock.patch.object(desktop, "ROOTS", [name]), mock.patch.object(desktop, "LOCK", lock), mock.patch.object(desktop, "cached", return_value=temporary / "input.tcz"), mock.patch.object(desktop, "extractor", return_value="unsquashfs"), mock.patch.object(desktop.subprocess, "run", side_effect=extract):
                desktop.install(root, offline=True)
            self.assertEqual((root / "usr/local/bin/xwininfo").read_bytes(), b"unchanged-ELF")
            self.assertFalse((root / "usr/local/bin/xdriinfo").exists())

    def test_profile_documents_are_not_executable_but_launchers_are(self):
        data = archive([("etc/os-release", stat.S_IFREG | 0o644, b"VERSION_ID=17.1\n")])
        with tempfile.TemporaryDirectory() as temporary:
            result = build.assemble(data, output=Path(temporary) / "build")
            root = result / "overlay/tinycore/rootfs"
            self.assertEqual((root / "usr/local/bin/nanami-app").stat().st_mode & 0o777, 0o755)
            self.assertEqual((root / "root/Documents/Welcome.txt").stat().st_mode & 0o777, 0o644)
            self.assertEqual((root / "root/.config/xfe/xferc").stat().st_mode & 0o777, 0o644)

    def test_versioned_fltk_runtime_libraries_coexist_without_replacing_default_aliases(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            root = temporary / "guest"
            root.mkdir()
            lock = temporary / "lock.json"
            lock.write_text(json.dumps({"repository": desktop.BASE,
                "roots": ["fltk-1.4.tcz", "fltk-1.3.tcz"], "packages": {
                    "fltk-1.4.tcz": {"sha256": "test"},
                    "fltk-1.3.tcz": {"sha256": "test", "omitted_symlinks": desktop.OMIT_SYMLINKS["fltk-1.3.tcz"]}}}))

            def extract(command, **kwargs):
                libraries = Path(command[command.index("-d") + 1]) / "usr/local/lib"
                libraries.mkdir(parents=True)
                version = "1.3"
                if command[-1].endswith("1.4.tcz"):
                    version = "1.4"
                for name in ["libfltk", "libfltk_forms", "libfltk_images"]:
                    (libraries / f"{name}.so.{version}").write_bytes(version.encode())
                    (libraries / f"{name}.so").symlink_to(f"{name}.so.{version}")

            with mock.patch.object(desktop, "ROOTS", ["fltk-1.4.tcz", "fltk-1.3.tcz"]), mock.patch.object(desktop, "LOCK", lock), mock.patch.object(desktop, "cached", side_effect=lambda name, *args: Path(name)), mock.patch.object(desktop, "extractor", return_value="unsquashfs"), mock.patch.object(desktop.subprocess, "run", side_effect=extract):
                desktop.install(root, offline=True)
            self.assertEqual((root / "usr/local/lib/libfltk.so").readlink(), Path("libfltk.so.1.4"))
            self.assertEqual((root / "usr/local/lib/libfltk.so.1.3").read_bytes(), b"1.3")

    def test_clock_profile_does_not_advertise_unavailable_audio_and_battery(self):
        config = (build.HERE / "profile/root/.flit.conf").read_text()
        self.assertIn("show_clock = 1", config)
        self.assertIn("show_sound = 0", config)
        self.assertIn("show_battery = 0", config)

    def test_file_manager_defaults_fit_the_nested_desktop(self):
        config = (build.HERE / "profile/root/.config/xfe/xferc").read_text()
        self.assertIn("width=740", config)
        self.assertIn("height=460", config)
        self.assertIn("root_warn=0", config)
        self.assertIn("xterm=aterm", config)

    def test_dock_entries_use_the_shared_launcher_and_have_unique_labels(self):
        config = (build.HERE / "profile/etc/nanami-wbar").read_text()
        labels = [line[3:] for line in config.splitlines()
                  if line.startswith("t: ") and not line[3:].startswith("/")]
        self.assertEqual(len(labels), len(set(labels)))
        self.assertEqual(labels, ["Terminal", "Files", "Editor", "Images", "Paint", "Calculator", "Tiny Core information", "Exit desktop"])
        commands = [line for line in config.splitlines() if line.startswith("c: /bin/busybox sh ")]
        self.assertEqual(len(commands), len(labels))
        self.assertTrue(all(line.startswith("c: /bin/busybox sh /usr/local/bin/nanami-app ") for line in commands))

    def test_clients_require_unix_transport_even_before_x_is_ready(self):
        startup = (build.HERE / "profile/usr/local/bin/nanami-desktop").read_text()
        self.assertIn("export DISPLAY=unix/:0\n", startup)
        self.assertIn("-nolisten tcp", startup)
        probe = 'while ! xset -display "$DISPLAY" q'
        self.assertIn(probe, startup)
        self.assertNotIn("while ! waitforX", startup)
        self.assertLess(startup.index("export DISPLAY=unix/:0"), startup.index(probe))

    def test_merge_preserves_guest_links_and_refuses_destination_aliases(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source"
            target = Path(temporary) / "target"
            source.mkdir()
            target.mkdir()
            (source / "payload").write_bytes(b"ELF")
            (source / "alias").symlink_to("/usr/local/bin/payload")
            desktop.merge(source, target)
            desktop.merge(source, target)  # Identical dependencies may overlap.
            self.assertEqual((target / "payload").read_bytes(), b"ELF")
            self.assertEqual((target / "alias").readlink(), Path("/usr/local/bin/payload"))
            (target / "payload").unlink()
            (target / "payload").symlink_to("../escape")
            with self.assertRaises(ValueError):
                desktop.merge(source, target)
            self.assertFalse((Path(temporary) / "escape").exists())

    def test_merge_rejects_changed_files_and_directory_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source"
            target = Path(temporary) / "target"
            source.mkdir()
            target.mkdir()
            (source / "directory").mkdir()
            (target / "directory").symlink_to("../outside")
            with self.assertRaises(ValueError):
                desktop.merge(source, target)
            (target / "directory").unlink()
            (source / "file").write_bytes(b"first")
            (target / "file").write_bytes(b"second")
            with self.assertRaises(ValueError):
                desktop.merge(source, target)

    def test_offline_cache_requires_sha256_not_just_a_filename(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch.object(desktop, "CACHE", Path(temporary)):
            with self.assertRaises(ValueError):
                desktop.cached("missing.tcz", "0" * 64, "https://example.invalid/", True)
            (Path(temporary) / "bad.tcz").write_bytes(b"corrupt")
            with self.assertRaises(ValueError):
                desktop.cached("bad.tcz", "0" * 64, "https://example.invalid/", True)

    def test_dependency_names_cannot_escape_the_package_cache(self):
        for name in ["../escape.tcz", "/absolute.tcz", "a/b.tcz", "a.tcz?query", "a\ntcz"]:
            with self.subTest(name=name), self.assertRaises(ValueError):
                desktop.package_name(name)


if __name__ == "__main__":
    unittest.main()
