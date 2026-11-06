import json
import pathlib
import tempfile
import unittest
from unittest import mock

from tcg_profile import Capture, QMP


class Connection:
    def __init__(self, replies):
        self.chunks = [json.dumps(reply).encode() + b"\n" for reply in replies]
        self.sent = []

    def settimeout(self, timeout):
        if timeout <= 0 or timeout > 5:
            raise AssertionError("unbounded QMP timeout")

    def recv(self, capacity):
        if not self.chunks:
            return b""
        chunk = self.chunks[0]
        take = min(3, len(chunk), capacity)
        self.chunks[0] = chunk[take:]
        if not self.chunks[0]:
            self.chunks.pop(0)
        return chunk[:take]

    def sendall(self, data):
        self.sent.append(json.loads(data))


class ProfileTests(unittest.TestCase):
    def test_qmp_matches_fragmented_acknowledgements_and_skips_events(self):
        connection = Connection([{"QMP": {}}, {"return": {}, "id": 1},
                                 {"event": "STOP"}, {"return": {}, "id": 2}])
        qmp = QMP(connection)
        self.assertEqual(qmp.execute("stop"), {})
        self.assertEqual(connection.sent, [{"execute": "qmp_capabilities", "id": 1},
                                           {"execute": "stop", "id": 2}])

    def test_wrong_identity_error_closed_and_oversized_responses_fail(self):
        for response in ({"return": {}, "id": 7}, {"error": {}, "id": 1}):
            with self.subTest(response=response):
                with self.assertRaises(RuntimeError):
                    QMP(Connection([{"QMP": {}}, response]))
        with self.assertRaises(RuntimeError):
            QMP(Connection([{"QMP": {}}]))
        connection = Connection([])
        connection.chunks = [b"x" * 70000]
        with self.assertRaises(RuntimeError):
            QMP(connection)

    def test_snapshot_requires_stop_ack_and_paused_status_before_plugin(self):
        with tempfile.TemporaryDirectory(prefix="tcg-control-") as directory:
            root = pathlib.Path(directory)
            plugin = root / "plugin"
            plugin.touch()
            capture = Capture(root, plugin)
            order = []

            def execute(command):
                order.append(command)
                if command == "query-status":
                    return {"running": False, "status": "paused"}
                return {}

            capture.qmp = mock.Mock()
            capture.qmp.execute.side_effect = execute
            with mock.patch("tcg_profile.plugin_command", side_effect=lambda *args: order.append(args[1])):
                capture.snapshot("begin")
                capture.snapshot("end")
            self.assertEqual(order, ["stop", "query-status", "begin", "cont", "stop", "query-status", "end", "cont"])
            metadata = json.loads(capture.metadata.read_text())
            self.assertTrue(metadata["diagnostic_only"])
            self.assertEqual(len(metadata["events"]), 2)
            with self.assertRaises(RuntimeError):
                capture.snapshot("begin")

    def test_running_vm_never_receives_snapshot_and_plugin_error_resumes(self):
        with tempfile.TemporaryDirectory(prefix="tcg-control-") as directory:
            root = pathlib.Path(directory)
            plugin = root / "plugin"
            plugin.touch()
            capture = Capture(root, plugin)
            capture.qmp = mock.Mock()
            capture.qmp.execute.return_value = {"running": True, "status": "running"}
            with mock.patch("tcg_profile.plugin_command") as command:
                with self.assertRaises(RuntimeError):
                    capture.snapshot("begin")
                command.assert_not_called()
            capture.qmp.execute.side_effect = [{}, {"running": False, "status": "paused"}, {}]
            with mock.patch("tcg_profile.plugin_command", side_effect=RuntimeError("incomplete")):
                with self.assertRaises(RuntimeError):
                    capture.snapshot("begin")
            self.assertEqual(capture.qmp.execute.call_args.args, ("cont",))
            self.assertEqual(capture.events, [])


if __name__ == "__main__":
    unittest.main()
