import unittest
from scope_report import validate_fixture, validate_checked_fixture


def report():
    lines = ["[       syscall.scope] owner=23 epoch=1 mode=Count start=10 end=100 ledger-valid=true clock=local-unverified"]
    for number, calls in {39: 7, 57: 1, 60: 1, 61: 1, 157: 2, 512: 1}.items():
        lines.append(f"[syscall.scope.row] epoch=1 syscall={number} calls={calls} ticks=0")
    lines += ["[syscall.scope.end] epoch=1",
              "[syscall.scope] owner=23 epoch=2 mode=Time start=200 end=300 ledger-valid=true clock=local-unverified",
              "[syscall.scope.row] epoch=2 syscall=39 calls=5 ticks=40",
              "[syscall.scope.end] epoch=2",
              "[syscall.scope.abort] owner=25 reason=exit",
              "[syscall.scope.abort] owner=26 reason=exec",
              "SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled=1"]
    return "\n".join(lines) + "\n"


class ScopeReportTests(unittest.TestCase):
    def test_checked_transport_requires_native_validation_of_all_reports(self):
        for enabled in [False, True]:
            reports = 2 if enabled else 0
            footer = f"SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled={int(enabled)} readout={reports}\n"
            result = validate_checked_fixture(footer, enabled)
            self.assertEqual(result["validated_reports"], reports)
            self.assertFalse(result["performance_admitted"])
            for bad in [footer.rstrip("\n"), footer.replace(f"readout={reports}", "readout=1"), report()]:
                with self.assertRaises(ValueError):
                    validate_checked_fixture(bad, enabled)

    def test_complete_count_time_and_retirement_are_functional_only(self):
        for text in [report(), report().replace("\n", "\r\n")]:
            result = validate_fixture(text, True)
            self.assertTrue(result["functional"])
            self.assertFalse(result["performance_admitted"])

    def test_inactive_control_requires_no_scope_output(self):
        text = "SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled=0\n"
        self.assertFalse(validate_fixture(text, False)["enabled"])
        with self.assertRaises(ValueError):
            validate_fixture(report().replace("enabled=1", "enabled=0"), False)

    def test_partial_malformed_or_wrong_counts_never_pass(self):
        for text in [report().rstrip("\n"),
                     report().replace("[syscall.scope.end] epoch=2\n", ""),
                     report().replace("syscall=39 calls=7", "syscall=39 calls=8"),
                     report().replace("ledger-valid=true", "ledger-valid=false", 1),
                     report().replace("calls=5 ticks=40", "calls=5 ticks=400"),
                     report().replace("reason=exec", "reason=retire"),
                     report().replace("owner=26 reason=exec", "owner=23 reason=exec"),
                     report().replace("[syscall.scope.row]", "noise[syscall.scope.row]", 1)]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                validate_fixture(text, True)
