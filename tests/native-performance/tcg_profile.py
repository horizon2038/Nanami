"""Control diagnostic snapshots; never infer a paused VM from an HMP prompt."""
import json
import pathlib
import socket
import time


class QMP:
    def __init__(self, connection):
        self.connection = connection
        self.pending = bytearray()
        self.sequence = 0
        greeting = self.receive(time.monotonic() + 5)
        if "QMP" not in greeting:
            raise RuntimeError("missing QMP greeting")
        self.execute("qmp_capabilities")

    @classmethod
    def connect(cls, path):
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(5)
        try:
            connection.connect(str(path))
            return cls(connection)
        except BaseException:
            connection.close()
            raise

    def receive(self, deadline):
        while b"\n" not in self.pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("QMP response deadline")
            self.connection.settimeout(remaining)
            chunk = self.connection.recv(4096)
            if not chunk:
                raise RuntimeError("QMP closed before acknowledgement")
            self.pending.extend(chunk)
            if len(self.pending) > 65536:
                raise RuntimeError("QMP response exceeds bound")
        line, _, rest = self.pending.partition(b"\n")
        self.pending = bytearray(rest)
        response = json.loads(line)
        if not isinstance(response, dict):
            raise RuntimeError("invalid QMP response")
        return response

    def execute(self, command):
        self.sequence += 1
        sequence = self.sequence
        self.connection.sendall(json.dumps({"execute": command, "id": sequence}).encode() + b"\n")
        deadline = time.monotonic() + 5
        for _ in range(1024):
            response = self.receive(deadline)
            if "event" in response:
                continue
            if response.get("id") != sequence:
                raise RuntimeError("QMP acknowledgement identity mismatch")
            if "error" in response or "return" not in response:
                raise RuntimeError(f"QMP {command} failed: {response}")
            return response["return"]
        raise RuntimeError("QMP event bound exceeded")

    def close(self):
        self.connection.close()


def plugin_command(path, command):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(5)
        connection.connect(str(path))
        connection.sendall(command.encode() + b"\n")
        reply = bytearray()
        deadline = time.monotonic() + 5
        while b"\n" not in reply:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("plugin response deadline")
            connection.settimeout(remaining)
            chunk = connection.recv(64)
            if not chunk:
                raise RuntimeError("plugin closed before acknowledgement")
            reply.extend(chunk)
            if len(reply) > 128:
                raise RuntimeError("plugin response exceeds bound")
        if reply != f"OK {command}\n".encode():
            raise RuntimeError(f"plugin rejected {command}: {reply!r}")


class Capture:
    def __init__(self, root, plugin):
        self.root = pathlib.Path(root).resolve()
        self.plugin = pathlib.Path(plugin).resolve()
        self.qmp_path = self.root / "profile-qmp.sock"
        self.control_path = self.root / "profile-control.sock"
        self.output = self.root / "profile.tsv"
        self.metadata = self.root / "profile-boundaries.json"
        if not self.root.is_dir() or not self.plugin.is_file():
            raise ValueError("profile directory and plugin must exist")
        for path in (self.qmp_path, self.control_path, self.output, self.metadata):
            if len(str(path).encode()) >= 104 or path.exists():
                raise ValueError(f"profile path too long or already exists: {path}")
        if any("," in str(path) for path in (self.plugin, self.root)):
            raise ValueError("QEMU plugin paths cannot contain commas")
        self.qmp = None
        self.events = []

    def arguments(self):
        return ["-qmp", f"unix:{self.qmp_path},server=on,wait=off", "-plugin",
                f"{self.plugin},output={self.output},socket={self.control_path},max=131072,top=512"]

    def snapshot(self, command):
        if command not in ("begin", "end") or len(self.events) != (0 if command == "begin" else 1):
            raise RuntimeError("invalid profile phase order")
        if self.qmp is None:
            self.qmp = QMP.connect(self.qmp_path)
        event = {"command": command, "stop_request_host_ns": time.monotonic_ns()}
        self.qmp.execute("stop")
        event["stop_ack_host_ns"] = time.monotonic_ns()
        paused = self.qmp.execute("query-status")
        if not isinstance(paused, dict) or paused.get("running") is not False or paused.get("status") != "paused":
            raise RuntimeError(f"VM not confirmed paused: {paused}")
        event["paused_confirmed_host_ns"] = time.monotonic_ns()
        try:
            plugin_command(self.control_path, command)
            event["snapshot_ack_host_ns"] = time.monotonic_ns()
        finally:
            self.qmp.execute("cont")
        event["resume_ack_host_ns"] = time.monotonic_ns()
        self.events.append(event)
        print("TCG-PROFILE-BOUNDARY " + json.dumps(event, sort_keys=True), flush=True)
        if command == "end":
            with self.metadata.open("x") as output:
                json.dump({"diagnostic_only": True, "events": self.events,
                           "scope": "before mouse press/resume through first-ping observation/stop acknowledgement",
                           "limits": "TCG TB-entry counts and translated-slot weights, not elapsed CPU time or fully painted/input readiness"},
                          output, indent=2)
                output.write("\n")

    def close(self):
        if self.qmp is not None:
            self.qmp.close()
