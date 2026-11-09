import unittest
import copy
from pathlib import Path
import tempfile
from unittest.mock import patch

from host_stats import interval, parse_stat, parse_schedstat, thread_interval, thread_snapshot


class ProcessAccountingTests(unittest.TestCase):
    def test_stat_fields_survive_spaces_and_parentheses_in_comm(self):
        fields = ["0"] * 50
        fields[0] = "S"
        fields[11], fields[12], fields[19], fields[40] = "200", "30", "777", "150"
        parsed = parse_stat("42 (qemu ) worker) " + " ".join(fields))
        self.assertEqual(parsed, dict(pid=42, user_ticks=200, system_ticks=30,
                                     start_ticks=777, guest_ticks=150))
        fields[40] = "250"
        with self.assertRaisesRegex(ValueError, "guest time exceeds"):
            parse_stat("42 (qemu) " + " ".join(fields))
        with self.assertRaises(ValueError):
            parse_stat("42 (truncated) S 0 0")

    def snapshots(self):
        before = dict(available=True, pid=42, start_ticks=777, ticks_per_second=100,
                      read_begin_ns=10, read_end_ns=20,
                      user_ticks=200, system_ticks=30, guest_ticks=150)
        after = dict(before, read_begin_ns=1010, read_end_ns=1020,
                     user_ticks=400, system_ticks=60, guest_ticks=300)
        return before, after

    def test_guest_time_is_not_double_counted_as_extra_cpu_time(self):
        before, after = self.snapshots()
        result = interval(before, after)
        self.assertTrue(result["available"])
        self.assertEqual(result["cpu_seconds"], 2.3)
        self.assertEqual(result["guest_seconds"], 1.5)
        self.assertEqual(result["user_excluding_guest_seconds"], 0.5)
        self.assertEqual(result["system_seconds"], 0.3)
        self.assertEqual((result["wall_lower_ns"], result["wall_upper_ns"]), (990, 1010))

    def test_reuse_resets_and_missing_accounting_do_not_become_samples(self):
        before, after = self.snapshots()
        for change in [dict(pid=43), dict(start_ticks=888), dict(ticks_per_second=250),
                       dict(user_ticks=199), dict(guest_ticks=500), dict(available=False),
                       dict(read_begin_ns=0)]:
            with self.subTest(change=change):
                self.assertFalse(interval(before, dict(after, **change))["available"])

    def test_inconsistent_start_is_rejected_even_when_delta_looks_valid(self):
        # Observed on this nested-KVM host: a positive plausible delta does
        # not make contradictory cumulative counters valid.
        before, after = self.snapshots()
        before.update(user_ticks=2378, guest_ticks=2570)
        after.update(user_ticks=3010, guest_ticks=3180)
        self.assertFalse(interval(before, after)["available"])


class ThreadAccountingTests(unittest.TestCase):
    def snapshots(self):
        first = dict(available=True, pid=42, vcpu_tids=[43], runnable_delay_available=False,
                     threads={})
        for tid, role in [(42, "main"), (43, "vcpu0")]:
            first["threads"][str(tid)] = dict(available=True, start_ticks=tid + 100, role=role,
                read_begin_ns=10, read_end_ns=20,
                scheduler=dict(runtime_ns=100, runnable_ns=0, slices=2))
        last = copy.deepcopy(first)
        for entry in last["threads"].values():
            entry.update(read_begin_ns=1010, read_end_ns=1020,
                         scheduler=dict(runtime_ns=300, runnable_ns=25, slices=5))
        return first, last

    def test_scheduler_fields_are_strict_unsigned_counters(self):
        self.assertEqual(parse_schedstat("12 0 3\n"), dict(runtime_ns=12, runnable_ns=0, slices=3))
        for bad in ["1 2", "1 2 3 4", "-1 0 2", "1 -1 2", "1 0 x", f"{1 << 64} 0 2"]:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                parse_schedstat(bad)

    def test_reported_runtime_is_separate_from_unvalidated_delay_and_cpu_cost(self):
        first, last = self.snapshots()
        result = thread_interval(first, last, dict(available=True))
        self.assertTrue(result["available"])
        self.assertEqual(result["reported_runtime_sum_ns"], 400)
        self.assertFalse(result["runnable_delay_available"])
        self.assertFalse(result["cpu_cost_admitted"])
        self.assertNotIn("reported_runnable_ns", result["threads"]["43"])
        self.assertEqual(result["threads"]["43"]["role"], "vcpu0")
        first["runnable_delay_available"] = last["runnable_delay_available"] = True
        self.assertEqual(thread_interval(first, last, dict(available=True))["threads"]["43"]["reported_runnable_ns"], 25)

    def test_reuse_worker_churn_and_counter_regression_do_not_become_samples(self):
        first, original = self.snapshots()
        for change in ["pid", "vcpu", "worker", "start", "role", "counter", "bounds", "partial"]:
            last = copy.deepcopy(original)
            if change == "pid": last["pid"] = 99
            if change == "vcpu": last["vcpu_tids"] = [44]
            if change == "worker": last["threads"]["44"] = copy.deepcopy(last["threads"]["43"])
            if change == "start": last["threads"]["43"]["start_ticks"] += 1
            if change == "role": last["threads"]["43"]["role"] = "other"
            if change == "counter": last["threads"]["43"]["scheduler"]["runtime_ns"] = 99
            if change == "bounds": last["threads"]["43"]["read_end_ns"] = 1009
            if change == "partial": last["available"] = False
            with self.subTest(change=change):
                self.assertFalse(thread_interval(first, last, dict(available=True))["available"])
        self.assertFalse(thread_interval(first, original, dict(available=False))["available"])

    def test_snapshot_retains_raw_inconsistent_thread_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def stat(pid, guest):
                fields = ["0"] * 50
                fields[0], fields[11], fields[12], fields[19], fields[40] = "S", "200", "30", "777", str(guest)
                return f"{pid} (qemu ) worker) " + " ".join(fields)
            for pid in [42, 43]:
                base = root / f"proc/42/task/{pid}"
                base.mkdir(parents=True)
                (base / "stat").write_text(stat(pid, 150))
                (base / "schedstat").write_text("100 0 3\n")
            with patch("host_stats.Path", side_effect=lambda value: root / str(value).lstrip("/")):
                cpu = b"* CPU #0: thread_id=43 model=host\r\n"
                self.assertTrue(thread_snapshot(42, cpu, 1)["available"])
                (root / "proc/42/task/43/stat").write_text(stat(43, 250))
                result = thread_snapshot(42, cpu, 1)
                self.assertFalse(result["available"])
                self.assertIn("guest time exceeds", result["threads"]["43"]["reason"])
                self.assertEqual(result["threads"]["43"]["stat_raw"], stat(43, 250))
                self.assertFalse(result["runnable_delay_available"])


if __name__ == "__main__":
    unittest.main()
