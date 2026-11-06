import importlib.util
import pathlib
import unittest
from types import SimpleNamespace

SOURCE = pathlib.Path(__file__).with_name("linux-qemu-control.py")
SPEC = importlib.util.spec_from_file_location("linux_control", SOURCE)
control = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(control)


class LinuxControlTests(unittest.TestCase):
    def test_overlay_is_one_regular_root_owned_executable_and_a_complete_trailer(self):
        payload = b"#!/bin/busybox sh\necho control\n"
        archive = control.init_overlay(payload)
        self.assertEqual(len(archive) % 512, 0)
        self.assertEqual(archive[:6], b"070701")
        fields = [int(archive[i:i + 8], 16) for i in range(6, 110, 8)]
        self.assertEqual(fields[:6], [1, 0o100755, 0, 0, 1, 0])
        name_end = 110 + fields[11]
        self.assertEqual(archive[110:name_end], b"nanami-control-init\0")
        data_at = (name_end + 3) & ~3
        self.assertEqual(archive[data_at:data_at + fields[6]], payload)
        trailer = (data_at + len(payload) + 3) & ~3
        self.assertEqual(archive[trailer:trailer + 6], b"070701")
        self.assertIn(b"TRAILER!!!\0", archive[trailer:])
        self.assertEqual(archive, control.init_overlay(payload))

    def test_command_matches_normal_devices_and_protects_the_real_disk_with_snapshot(self):
        args = SimpleNamespace(image=pathlib.Path("/tmp/control.img"), kernel=pathlib.Path("/tmp/kernel"), smp=4, tcg_threads=None)
        argv = control.command(args, "/tmp/serial.log", "/tmp/vars.fd", "/tmp/initrd")
        self.assertEqual(argv[argv.index("-machine") + 1], "hpet=on")
        self.assertEqual(argv[argv.index("-cpu") + 1], "max")
        self.assertEqual(argv[argv.index("-smp") + 1], "4")
        self.assertIn("ich9-ahci,id=ahci,addr=3", argv)
        self.assertIn(f"if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}", argv)
        self.assertNotIn("-accel", argv)
        args.tcg_threads = "single"
        self.assertIn("tcg,thread=single", control.command(args, "serial", "vars", "initrd"))

    def test_metrics_require_all_samples_success_and_no_failure_marker(self):
        text = "".join(f"PROCESS-PERF {key}-ns={sample + 1}\n" for key in sorted(control.METRICS) for sample in range(3))
        text += "PROCESS-PERFORMANCE-PASS\nLINUX-CONTROL-PASS\n"
        self.assertEqual(set(control.parse_metrics(text)), control.METRICS)
        for bad in [text.replace("getpid-2048", "unknown"), text.replace("PROCESS-PERFORMANCE-PASS", "missing"), text.replace("-ns=1", "-ns=0"), text + "LINUX-CONTROL-FAIL", text + "Kernel panic", text + "PROCESS-PERF fork-8-ns=1\n"]:
            with self.assertRaises(ValueError):
                control.parse_metrics(bad)


if __name__ == "__main__":
    unittest.main()
