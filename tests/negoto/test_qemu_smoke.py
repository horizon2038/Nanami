"""Check report completion without importing the top-level QEMU runner."""
import ast
from pathlib import Path
import re
import unittest


class PerformanceReportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        # Compile the runner's actual predicate without parsing CLI arguments
        # or starting QEMU. A copied predicate would miss runner regressions.
        candidates = [node for node in ast.walk(ast.parse(source))
                      if isinstance(node, ast.Lambda)
                      and any(isinstance(value, ast.Constant)
                              and value.value == "TINYCORE-PERFORMANCE-PASS"
                              for value in ast.walk(node))]
        if len(candidates) != 1:
            raise AssertionError("expected one Tiny Core performance predicate")
        cls.complete = staticmethod(eval(compile(ast.Expression(candidates[0]),
                                                "performance-predicate", "eval"), {"re": re}))

    def test_prefix_and_partial_report_do_not_finish_the_run(self):
        for report in ["", "TINYCORE-PERFORMANCE-PASS",
                       "TINYCORE-PERFORMANCE-PASS DESKTOP-PERF stat-512-ns=141284",
                       "TINYCORE-PERFORMANCE-PASS\nDESKTOP-PERFORMANCE-PAS"]:
            with self.subTest(report=report):
                self.assertFalse(self.complete(report))

    def test_footer_must_follow_the_summary_prefix(self):
        self.assertFalse(self.complete("DESKTOP-PERFORMANCE-PASS\nTINYCORE-PERFORMANCE-PASS"))
        self.assertFalse(self.complete("TINYCORE-PERFORMANCE-PASS\nnot-DESKTOP-PERFORMANCE-PASS"))

    def test_complete_lf_and_crlf_reports_can_finish(self):
        for newline in ["\n", "\r\n"]:
            report = ("boot log" + newline + "TINYCORE-PERFORMANCE-PASS "
                      "DESKTOP-PERF unix-recvmsg-2048-ns=123" + newline
                      + "DESKTOP-PERFORMANCE-PASS" + newline)
            self.assertTrue(self.complete(report))


if __name__ == "__main__":
    unittest.main()
