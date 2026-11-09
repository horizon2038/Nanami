"""Check report completion without importing the top-level QEMU runner."""
import ast
from pathlib import Path
import re
import subprocess
import sys
import unittest
import io
from types import SimpleNamespace


class FakeClock:
    def __init__(self):
        self.now = 0

    def monotonic_ns(self):
        return self.now

    def monotonic(self):
        return self.now / 1e9

    def sleep(self, seconds):
        if seconds < 0:
            raise AssertionError("negative sleep")
        self.now += int(seconds * 1e9)


class LaunchObservationTests(unittest.TestCase):
    def test_paint_decoding_waits_for_vm_reap_even_after_forced_kill(self):
        source = ast.parse(Path(__file__).with_name("qemu-smoke.py").read_text())
        finalizers = [node.finalbody for node in ast.walk(source) if isinstance(node, ast.Try)
                      and any(isinstance(value, ast.Attribute) and value.attr == "audit_timeline"
                              for statement in node.finalbody for value in ast.walk(statement))]
        self.assertEqual(len(finalizers), 1)
        code = compile(ast.Module(body=finalizers[0], type_ignores=[]), "vm-finalizer", "exec")
        import json
        import tempfile
        for forced in (False, True):
            events = []
            def wait(timeout=None):
                events.append("wait")
                if forced and timeout is not None:
                    raise subprocess.TimeoutExpired("qemu", timeout)
                events.append("reaped")
            def audit(directory):
                self.assertEqual(events[-1], "reaped")
                events.append("audit")
                return {"performance_admitted": False}
            with tempfile.TemporaryDirectory() as temporary:
                logs = Path(temporary)
                process = SimpleNamespace(terminate=lambda: events.append("terminate"),
                                          kill=lambda: events.append("kill"), wait=wait)
                exec(code, dict(process=process, subprocess=subprocess,
                                args=SimpleNamespace(app_launch_timeline=True), logs=logs,
                                files_paint=SimpleNamespace(audit_timeline=audit), json=json))
                self.assertFalse(json.loads((logs / "app-launch-paint-reference.json").read_text())
                                 ["performance_admitted"])
            self.assertEqual(events[-1], "audit")
            self.assertEqual("kill" in events, forced)

    def test_file_mapping_delivery_accepts_compositor_with_interleaved_font_log(self):
        source = ast.parse(Path(__file__).with_name("qemu-smoke.py").read_text())
        branches = [node for node in ast.walk(source) if isinstance(node, ast.If)
                    and {"scope_disk_readout", "app_syscall_scope", "tinycore_file_mapping"}.issubset(
                        {value.attr for value in ast.walk(node.test) if isinstance(value, ast.Attribute)})]
        self.assertEqual(len(branches), 1)
        code = compile(ast.Module(body=branches, type_ignores=[]), "functional-bootstrap", "exec")
        serial = "[shell] font init e[another service] nd\n[              honoka] initial render done\n"
        for file_mapping, timeline in [(True, False), (False, True), (False, False)]:
            clock, observations = FakeClock(), []
            args = SimpleNamespace(scope_disk_readout=False, app_syscall_scope=None,
                                   tinycore_file_mapping=file_mapping, app_launch_timeline=timeline)
            exec(code, dict(args=args, time=clock, re=re,
                            wait_for=lambda predicate: observations.append(bool(predicate(serial)))))
            diagnostic = file_mapping or timeline
            self.assertEqual(observations, [diagnostic])
            self.assertEqual(clock.now, 10_000_000_000 if diagnostic else 0)

    def test_greeting_is_consumed_before_command_and_reply_is_not_one_command_late(self):
        process = SimpleNamespace(stdin=io.BytesIO(), stdout=SimpleNamespace(fileno=lambda: 17), poll=lambda: None)
        reads = iter([b"QEMU greeting\r\n(qe", b"mu) ", b"VM status: running\r\n(qemu) "])
        send = self.runner_function("send", process=process, time=FakeClock(),
                                    select=SimpleNamespace(select=lambda *args: ([process.stdout], [], [])),
                                    os=SimpleNamespace(read=lambda *args: next(reads)))
        self.assertIn(b"QEMU greeting", send())
        self.assertEqual(process.stdin.getvalue(), b"")
        self.assertIn(b"VM status: running", send("info status"))
        self.assertEqual(process.stdin.getvalue(), b"info status\n")
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        tree = ast.parse(source)
        greeting = next(node for node in ast.walk(tree) if isinstance(node, ast.Call)
                        and isinstance(node.func, ast.Name) and node.func.id == "send" and not node.args)
        wait = next(node for node in ast.walk(tree) if isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name) and node.func.id == "wait_for")
        self.assertLess(greeting.lineno, wait.lineno)

    def test_monitor_eof_and_exited_process_fail_without_spinning(self):
        process = SimpleNamespace(stdin=io.BytesIO(), stdout=SimpleNamespace(fileno=lambda: 17), poll=lambda: None)
        clock = FakeClock()
        reads = iter([b"partial reply", b""])
        send = self.runner_function("send", process=process, time=clock,
                                    select=SimpleNamespace(select=lambda *args: ([process.stdout], [], [])),
                                    os=SimpleNamespace(read=lambda *args: next(reads)))
        with self.assertRaisesRegex(RuntimeError, "monitor closed"):
            send("screendump failed.png")
        self.assertEqual(process.stdin.getvalue(), b"screendump failed.png\n")
        process.stdin = io.BytesIO()
        process.poll = lambda: -11
        with self.assertRaisesRegex(RuntimeError, "status=-11"):
            send("info cpus")
        self.assertEqual(process.stdin.getvalue(), b"")

    def test_monitor_deadline_covers_continuous_partial_replies(self):
        clock = FakeClock()
        process = SimpleNamespace(stdin=io.BytesIO(), stdout=SimpleNamespace(fileno=lambda: 17), poll=lambda: None)
        def select_ready(*args):
            clock.sleep(1)
            return ([process.stdout], [], [])
        send = self.runner_function("send", process=process, time=clock,
                                    select=SimpleNamespace(select=select_ready),
                                    os=SimpleNamespace(read=lambda *args: b"incomplete"))
        with self.assertRaisesRegex(RuntimeError, "monitor timed out"):
            send("info cpus")
        self.assertEqual(clock.now, 5_000_000_000)

    def test_monitor_prompt_can_span_multiple_reads(self):
        process = SimpleNamespace(stdin=io.BytesIO(), stdout=SimpleNamespace(fileno=lambda: 17), poll=lambda: None)
        reads = iter([b"reply(qe", b"mu)"])
        send = self.runner_function("send", process=process, time=FakeClock(),
                                    select=SimpleNamespace(select=lambda *args: ([process.stdout], [], [])),
                                    os=SimpleNamespace(read=lambda *args: next(reads)))
        self.assertEqual(send("info cpus"), b"reply(qemu)")

    def test_host_wait_uses_declared_poll_interval(self):
        clock = FakeClock()
        replies = iter(["waiting", "waiting", "ready"])
        args = type("Arguments", (), dict(timeout=1, host_poll_ms=10))()
        wait = self.runner_function("wait_for", time=clock, args=args,
                                    checked_serial=lambda: next(replies))
        wait(lambda text: text == "ready")
        self.assertEqual(clock.now, 20_000_000)

    def test_hmp_paths_translate_period_instead_of_silently_losing_it(self):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        maps = [node.value for node in ast.walk(ast.parse(source))
                if isinstance(node, ast.Assign)
                and any(isinstance(target, ast.Name) and target.id == "keys"
                        for target in node.targets)]
        self.assertEqual(len(maps), 1)
        self.assertEqual(ast.literal_eval(maps[0])["."], "dot")

    @staticmethod
    def runner_function(name, **environment):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        matches = [node for node in ast.walk(ast.parse(source))
                   if isinstance(node, ast.FunctionDef) and node.name == name]
        if len(matches) != 1:
            raise AssertionError(f"expected one runner function: {name}")
        module = ast.Module(body=matches, type_ignores=[])
        exec(compile(module, "runner-observation", "exec"), environment)
        return environment[name]

    def test_timeline_is_opt_in_and_requires_files_ping_mode(self):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        guards = [node for node in ast.parse(source).body
                  if isinstance(node, ast.If)
                  and any(isinstance(value, ast.Attribute)
                          and value.attr == "app_launch_timeline"
                          for value in ast.walk(node.test))
                  and not any(isinstance(value, ast.Attribute)
                              and value.attr == "app_launch_rtt"
                              for value in ast.walk(node.test))]
        self.assertEqual(len(guards), 1)
        code = compile(ast.Module(body=guards, type_ignores=[]), "timeline-guard", "exec")

        class Parser:
            @staticmethod
            def error(message):
                raise ValueError(message)

        for enabled, application, mode in [
            (False, None, "viewable"), (True, "files", "ping"),
            (True, None, "viewable"), (True, "files", "viewable"),
            (True, "editor", "ping"), (True, "terminal", "ping"),
        ]:
            args = type("Arguments", (), dict(app_launch_timeline=enabled,
                        tinycore_app_launch_perf=application, app_launch_ready=mode))()
            environment = dict(args=args, parser=Parser)
            if enabled and (application != "files" or mode != "ping"):
                with self.assertRaisesRegex(ValueError, "requires"):
                    exec(code, environment)
            else:
                exec(code, environment)

    def test_click_retains_press_and_brackets_release_command(self):
        clock, commands = FakeClock(), []

        def send(command):
            commands.append((command, clock.now))
            clock.now += 300_000  # HMP acknowledgement is not the input instant.

        click = self.runner_function("click_at", time=clock, send=send)
        pressed, release_begin, released = click(0, 0)
        self.assertEqual(commands[-2][0], "mouse_button 1")
        self.assertEqual(commands[-1], ("mouse_button 0", release_begin))
        self.assertEqual(pressed, commands[-2][1])
        self.assertGreaterEqual(release_begin - pressed, 100_000_000)
        self.assertEqual(released - release_begin, 300_000)

    def test_pointer_move_without_click_does_not_invent_a_release(self):
        commands = []
        click = self.runner_function("click_at", time=FakeClock(), send=commands.append)
        self.assertIsNone(click(0, 0, click=False))
        self.assertFalse(any(command.startswith("mouse_button") for command in commands))

    def test_screenshot_waits_past_hmp_ack_for_complete_png(self):
        clock = FakeClock()

        class PendingImage:
            reads = 0

            def exists(self):
                return True

            def read_bytes(self):
                self.reads += 1
                if self.reads == 1:
                    return b"partial PNG"
                return b"PNG IEND\xaeB`\x82"

        path = PendingImage()
        capture = self.runner_function("screenshot_at", time=clock, send=lambda _: None)
        begin, end = capture(path)
        self.assertEqual(path.reads, 2)
        self.assertEqual(begin, 0)
        self.assertGreater(end, begin)

    def test_incomplete_screenshot_is_a_failure_not_a_ready_sample(self):
        class MissingImage:
            def exists(self):
                return False

        capture = self.runner_function("screenshot_at", time=FakeClock(), send=lambda _: None)
        with self.assertRaisesRegex(RuntimeError, "did not finish"):
            capture(MissingImage())

    def test_timeline_records_actual_capture_and_only_challenges_real_input(self):
        clock, commands, reports, captures = FakeClock(), [], [], []
        clock.now = 900_000_000  # Already late: never label this as a 200ms capture.

        def capture(path):
            captures.append(path)
            begin = clock.now
            clock.now += 1_000_000
            return begin, clock.now

        import json
        import tempfile
        with tempfile.TemporaryDirectory() as temporary:
            observe = self.runner_function(
                "launch_timeline", time=clock, send=commands.append,
                screenshot_at=capture, logs=Path(temporary), json=json,
                keys={"/": "slash", "\n": "ret"},
                print=lambda line, **_: reports.append(line),
            )
            observe(0, 0)
            short = json.loads((Path(temporary) / "app-launch-single-input.json").read_text())
        self.assertEqual(len(captures), 7)
        self.assertIn("requested-ms=200 capture-begin-ns=900000000", reports[0])
        self.assertEqual(commands[:2], ["sendkey ctrl-l 40", "sendkey ctrl-a 40"])
        self.assertEqual(commands[-1], "sendkey ret 40")
        self.assertIn("phase=keyboard-challenge", reports[-1])
        self.assertNotIn("PASS", "\n".join(reports))
        self.assertEqual(short["command"], "sendkey ctrl-l 40")
        self.assertFalse(short["painted_input_readiness_verified"])
        self.assertFalse(short["performance_admitted"])
        self.assertEqual([item["requested_ms"] for item in short["captures"]], [50, 500])
        self.assertEqual(short["captures"][0]["capture_begin_ns"] - short["input_begin_ns"],
                         50_000_000)


class PerformanceReportTests(unittest.TestCase):
    def test_cli_rejects_input_or_tcg_work_in_kvm_rtt_trials(self):
        runner = str(Path(__file__).with_name("qemu-smoke.py"))
        for flags, message in [
            (["--accel", "kvm", "--tcg-threads", "multi"], "--tcg-threads requires --accel tcg"),
            (["--tinycore-app-launch-perf", "files", "--app-launch-ready", "ping",
              "--app-launch-rtt", "--app-launch-timeline"], "cannot overlap timeline"),
        ]:
            result = subprocess.run([sys.executable, runner, "--image", "/nonexistent", *flags],
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 2)
            self.assertIn(message, result.stderr)

    def test_rtt_requires_all_six_samples_and_a_complete_line(self):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        candidates = [node for node in ast.walk(ast.parse(source))
                      if isinstance(node, ast.Lambda)
                      and any(isinstance(value, ast.Constant) and isinstance(value.value, str)
                              and value.value.startswith("(?m)^APP-X11-RTT-PASS")
                              for value in ast.walk(node))]
        self.assertEqual(len(candidates), 1)
        complete = eval(compile(ast.Expression(candidates[0]), "rtt-predicate", "eval"), {"re": re})
        for report in ["APP-X11-RTT-FAIL files\n",
                       "APP-X11-RTT-PASS count=128 raw-ns=1,2,3 xlib-ns=4,5\n",
                       "APP-X11-RTT-PASS count=128 raw-ns=1,2,3 xlib-ns=4,5,6"]:
            self.assertFalse(complete(report))
        for newline in ["\n", "\r\n"]:
            self.assertTrue(complete("APP-X11-RTT-PASS count=128 raw-ns=1,2,3 xlib-ns=4,5,6" + newline))

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

    def test_readonly_fork_requires_source_reaping_and_a_complete_line(self):
        source = Path(__file__).with_name("qemu-smoke.py").read_text()
        candidates = [node for node in ast.walk(ast.parse(source))
                      if isinstance(node, ast.Lambda)
                      and any(isinstance(value, ast.Constant)
                              and value.value == r" source-reap=true\r?\n"
                              for value in ast.walk(node))]
        self.assertEqual(len(candidates), 1)
        marker = "PRIVATE-READONLY-FORK-PASS ram=true guarded-write=true isolation=true"
        complete = eval(compile(ast.Expression(candidates[0]), "readonly-predicate", "eval"),
                        {"re": re, "marker": marker})
        for report in [marker, marker + "\n", marker + " source-reap=true"]:
            self.assertFalse(complete(report))
        for newline in ["\n", "\r\n"]:
            self.assertTrue(complete(marker + " source-reap=true" + newline))


if __name__ == "__main__":
    unittest.main()
