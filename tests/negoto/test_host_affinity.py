import unittest
from unittest.mock import patch
import host_affinity as affinity


class HostAffinityTests(unittest.TestCase):
    def test_monitor_rows_require_complete_unique_nonconflicting_threads(self):
        text = "* CPU #0: thread_id=101 model=host\r\n CPU #1: thread_id=102 model=host\r\n"
        self.assertEqual(affinity.parse_vcpus(text + text, 2), [101, 102])
        for invalid in [text, text.replace("102", "101"), text + text.replace("101", "103")]:
            with self.assertRaises(ValueError):
                affinity.parse_vcpus(invalid, 4 if invalid == text else 2)

    def test_plan_rejects_foreign_main_or_duplicate_assignments(self):
        for vcpus, cpus, io in [([100, 101], [0, 2], 4), ([101, 999], [0, 2], 4),
                                ([101, 101], [0, 2], 4), ([101, 102], [0, 0], 4),
                                ([101, 102], [0, 2], 2)]:
            with self.assertRaises(ValueError):
                affinity.expected_threads(100, vcpus, cpus, io, {100, 101, 102, 103})

    def test_apply_pins_auxiliaries_first_and_verify_rejects_drift_or_new_workers(self):
        tids = {100, 101, 102, 103}
        masks, order = {}, []
        def set_mask(tid, mask):
            order.append(tid)
            masks[tid] = set(mask)
        with patch.object(affinity, "thread_ids", side_effect=lambda pid: tids.copy()), \
             patch.object(affinity.os, "sched_setaffinity", side_effect=set_mask), \
             patch.object(affinity.os, "sched_getaffinity", side_effect=lambda tid: masks[tid]):
            result = affinity.apply(100, [101, 102], [0, 2], 4)
            self.assertEqual(result["observed"], {100: [4], 101: [0], 102: [2], 103: [4]})
            self.assertEqual(set(order[:2]), {100, 103})
            masks[101] = {0, 2}
            with self.assertRaises(RuntimeError):
                affinity.verify(100, [101, 102], [0, 2], 4)
            masks[101] = {0}
            tids.add(104)
            masks[104] = {0}
            with self.assertRaises(RuntimeError):
                affinity.verify(100, [101, 102], [0, 2], 4)
            masks[104] = {4}
            affinity.verify(100, [101, 102], [0, 2], 4)
