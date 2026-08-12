#!/usr/bin/python3

from __future__ import annotations

import importlib.util
import json
import os
import signal
import socket
import subprocess
import tempfile
import time
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path


DRIVER_PATH = Path(__file__).resolve().parents[1] / "controlled_ssh_e2e.py"
SPEC = importlib.util.spec_from_file_location("controlled_ssh_e2e", DRIVER_PATH)
assert SPEC is not None and SPEC.loader is not None
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)

WORKSPACE_ID = "10000000-0000-4000-8000-000000000002"
CLIENT_A = "10000000-0000-4000-8000-000000000003"
CLIENT_B = "10000000-0000-4000-8000-000000000004"

FAKE_SSH = r'''#!/usr/bin/python3
import selectors
import signal
import socket
import sys

ports = []
arguments = iter(sys.argv[1:])
for argument in arguments:
    if argument == "-L":
        forward = next(arguments)
        ports.append(int(forward.split(":", 3)[1]))

if len(ports) != 2 or len(set(ports)) != 2:
    raise SystemExit(64)

running = True
def stop(_signal, _frame):
    global running
    running = False

signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)
selector = selectors.DefaultSelector()
listeners = []
for port in ports:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", port))
    listener.listen(16)
    listener.setblocking(False)
    selector.register(listener, selectors.EVENT_READ)
    listeners.append(listener)

while running:
    for key, _events in selector.select(0.05):
        connection, _address = key.fileobj.accept()
        connection.close()

for listener in listeners:
    listener.close()
'''

FAKE_TEST_SYNC = r'''#!/usr/bin/python3
import json
import os
import pathlib
import subprocess
import sys

arguments = sys.argv[1:]
if not arguments or arguments.pop(0) != "run":
    raise SystemExit(64)

values = {}
while arguments:
    key = arguments.pop(0)
    if not key.startswith("--") or not arguments:
        raise SystemExit(64)
    values[key] = arguments.pop(0)

os.fstat(int(values["--token-fd"]))
evidence = pathlib.Path(__EVIDENCE_ROOT__) / values["--run-id"]
if pathlib.Path(values["--evidence-root"]) != pathlib.Path(__EVIDENCE_ROOT__):
    raise SystemExit(66)
evidence.mkdir(mode=0o700, parents=True)
context = [
    "--workspace-id", values["--workspace-id"],
    "--client-id-a", values["--client-id-a"],
    "--client-id-b", values["--client-id-b"],
    "--agent-pid-a", "41001",
    "--agent-pid-b", "41002",
]

def invoke(program, action, *extra):
    completed = subprocess.run(
        [program, "--action", action, *extra, *context],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={"LANG": "C", "PATH": "/usr/bin:/bin"},
        timeout=10,
    )
    return json.loads(completed.stdout)

for action, hook in (
    ("reconnect", values["--reconnect-hook"]),
    ("app_restart", values["--app-restart-hook"]),
):
    before = invoke(values["--effect-observer"], action, "--phase", "before")
    receipt = invoke(hook, action)
    after = invoke(values["--effect-observer"], action, "--phase", "after")
    if before["identity"] != receipt["old"] or after["identity"] != receipt["new"]:
        raise SystemExit(65)

(evidence / "fake-test-sync.json").write_text(json.dumps({
    "endpoint_a": values["--endpoint-a"],
    "endpoint_b": values["--endpoint-b"],
    "evidence_root": values["--evidence-root"],
}), encoding="ascii")
'''

BLOCKING_FAKE_TEST_SYNC = r'''#!/usr/bin/python3
import os
import pathlib
import signal
import sys
import time

arguments = sys.argv[1:]
if not arguments or arguments.pop(0) != "run":
    raise SystemExit(64)
values = {}
while arguments:
    key = arguments.pop(0)
    if not key.startswith("--") or not arguments:
        raise SystemExit(64)
    values[key] = arguments.pop(0)
os.fstat(int(values["--token-fd"]))
signal.signal(signal.SIGINT, signal.SIG_IGN)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path(os.environ["FNS_BLOCKING_CHILD_PID"]).write_text(str(os.getpid()), encoding="ascii")
while True:
    time.sleep(1)
'''


def effect_arguments(action: str, workspace: str = WORKSPACE_ID) -> list[str]:
    return [
        "--action",
        action,
        "--workspace-id",
        workspace,
        "--client-id-a",
        CLIENT_A,
        "--client-id-b",
        CLIENT_B,
        "--agent-pid-a",
        "31001",
        "--agent-pid-b",
        "31002",
    ]


class ControlledSshTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(
            prefix="fns-controlled-ssh-test-", dir="/tmp"
        )
        self.root = Path(self.temporary.name).resolve()
        self.root.chmod(0o700)
        self.runtime = self.root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.evidence = self.root / "evidence"
        self.evidence.mkdir(mode=0o700)
        self.fake_ssh = self.root / "fake-ssh"
        self.fake_ssh.write_text(FAKE_SSH, encoding="ascii")
        self.fake_ssh.chmod(0o700)
        self.ports = DRIVER.allocate_local_ports("127.0.0.1")
        self.initial_controller: subprocess.Popen[bytes] | None = None
        self.write_config()

    def tearDown(self) -> None:
        try:
            DRIVER.stop_controller(self.runtime, 2.0)
        except Exception:
            pass
        if self.initial_controller is not None:
            try:
                self.initial_controller.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                self.initial_controller.kill()
                self.initial_controller.wait(timeout=2.0)
        self.temporary.cleanup()

    def write_config(self) -> None:
        value = {
            "schema_version": DRIVER.CONFIG_SCHEMA,
            "runtime_id": "offline-test-runtime",
            "driver_path": str(DRIVER_PATH),
            "python_path": str(Path("/usr/bin/python3").resolve(strict=True)),
            "ssh_binary": str(self.fake_ssh.resolve(strict=True)),
            "ssh_host": "offline-fake-host",
            "ssh_port": 22,
            "ssh_config": None,
            "identity_file": None,
            "local_host": "127.0.0.1",
            "local_ports": list(self.ports),
            "remote_host": "127.0.0.1",
            "remote_port": 9000,
            "connect_timeout_seconds": 1.0,
            "startup_timeout_seconds": 3.0,
            "term_grace_seconds": 1.0,
            "kill_timeout_seconds": 1.0,
            "controller_environment": {
                "HOME": str(self.root),
                "LOGNAME": "offline-test",
                "PATH": "/usr/bin:/bin",
                "TMPDIR": str(self.root),
                "USER": "offline-test",
            },
            "workspace_id": WORKSPACE_ID,
            "client_id_a": CLIENT_A,
            "client_id_b": CLIENT_B,
            "harness_evidence_dir": str(self.evidence),
        }
        DRIVER.validate_config(value)
        DRIVER.atomic_write_json(DRIVER.runtime_paths(self.runtime)["config"], value)

    def start_controller(self) -> dict:
        self.initial_controller = DRIVER.spawn_controller(self.runtime, 1)
        return DRIVER.wait_live_state(self.runtime, 3.0, expected_generation=1)

    def run_driver(self, command: str, action: str, *extra: str) -> dict:
        completed = subprocess.run(
            [
                "/usr/bin/python3",
                str(DRIVER_PATH),
                command,
                "--runtime-dir",
                str(self.runtime),
                *extra,
                *effect_arguments(action),
            ],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        self.assertNotIn(b"eyJ", completed.stdout + completed.stderr)
        return json.loads(completed.stdout)

    def test_run_requires_an_explicit_ssh_host(self) -> None:
        arguments = [
            "run",
            "--test-sync",
            "/usr/bin/true",
            "--agent-binary",
            "/usr/bin/true",
            "--workspace-id",
            WORKSPACE_ID,
            "--client-id-a",
            CLIENT_A,
            "--client-id-b",
            CLIENT_B,
            "--root-a",
            str(self.root / "root-a"),
            "--root-b",
            str(self.root / "root-b"),
            "--state-a",
            str(self.root / "state-a"),
            "--state-b",
            str(self.root / "state-b"),
            "--run-id",
            "explicit-host-test",
            "--token-fd",
            "3",
        ]
        with redirect_stderr(StringIO()), self.assertRaises(SystemExit) as missing:
            DRIVER.build_parser().parse_args(arguments)
        self.assertEqual(missing.exception.code, 2)

        parsed = DRIVER.build_parser().parse_args(
            [*arguments, "--ssh-host", "offline-fake-host"]
        )
        self.assertEqual(parsed.ssh_host, "offline-fake-host")

    def test_reconnect_and_app_restart_are_real_observed_process_transitions(self) -> None:
        initial = self.start_controller()
        initial_controller_pid = initial["controller_pid"]
        initial_tunnel_pid = initial["tunnel_pid"]

        before_reconnect = self.run_driver("observe", "reconnect", "--phase", "before")
        reconnect = self.run_driver(
            "effect", "reconnect", "--allowed-action", "reconnect"
        )
        after_reconnect = self.run_driver("observe", "reconnect", "--phase", "after")
        reconnected_state = DRIVER.live_state(self.runtime)

        self.assertEqual(before_reconnect["identity"], reconnect["old"])
        self.assertEqual(after_reconnect["identity"], reconnect["new"])
        self.assertEqual(initial_controller_pid, reconnected_state["controller_pid"])
        self.assertEqual(reconnect["old"]["pid"], reconnect["new"]["pid"])
        self.assertEqual(reconnect["old"]["generation"] + 1, reconnect["new"]["generation"])
        self.assertNotEqual(initial_tunnel_pid, reconnected_state["tunnel_pid"])
        self.assertFalse(DRIVER.pid_alive(initial_tunnel_pid))

        before_restart = self.run_driver("observe", "app_restart", "--phase", "before")
        app_restart = self.run_driver(
            "effect", "app_restart", "--allowed-action", "app_restart"
        )
        after_restart = self.run_driver("observe", "app_restart", "--phase", "after")
        restarted_state = DRIVER.live_state(self.runtime)

        self.assertEqual(before_restart["identity"], app_restart["old"])
        self.assertEqual(after_restart["identity"], app_restart["new"])
        self.assertNotEqual(app_restart["old"]["pid"], app_restart["new"]["pid"])
        self.assertEqual(
            app_restart["old"]["generation"] + 1,
            app_restart["new"]["generation"],
        )
        self.assertEqual(restarted_state["controller_pid"], app_restart["new"]["pid"])
        self.assertFalse(DRIVER.pid_alive(app_restart["old"]["pid"]))

        events = [
            json.loads(line)
            for line in DRIVER.runtime_paths(self.runtime)["audit"].read_text().splitlines()
        ]
        reconnect_events = [event for event in events if event["event"] == "reconnect_completed"]
        restart_events = [event for event in events if event["event"] == "app_restart_completed"]
        self.assertEqual(len(reconnect_events), 1)
        self.assertEqual(len(restart_events), 1)
        self.assertTrue(reconnect_events[0]["port_closure_observed"])
        self.assertNotEqual(
            reconnect_events[0]["old_tunnel_pid"], reconnect_events[0]["new_tunnel_pid"]
        )
        self.assertTrue(restart_events[0]["port_closure_observed"])
        self.assertNotEqual(
            restart_events[0]["old_controller_pid"],
            restart_events[0]["new_controller_pid"],
        )
        copied_events = [
            json.loads(line)
            for line in (self.evidence / "connection.jsonl").read_text().splitlines()
        ]
        self.assertEqual(copied_events, events)

    def test_wrong_context_is_rejected_without_mutating_connection(self) -> None:
        initial = self.start_controller()
        completed = subprocess.run(
            [
                "/usr/bin/python3",
                str(DRIVER_PATH),
                "effect",
                "--runtime-dir",
                str(self.runtime),
                "--allowed-action",
                "reconnect",
                *effect_arguments("reconnect", workspace="wrong-workspace"),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
        )
        self.assertNotEqual(completed.returncode, 0)
        current = DRIVER.live_state(self.runtime)
        self.assertEqual(initial["controller_pid"], current["controller_pid"])
        self.assertEqual(initial["generation"], current["generation"])
        self.assertEqual(initial["tunnel_pid"], current["tunnel_pid"])

    def test_generated_effect_entrypoints_are_self_contained_and_independent(self) -> None:
        paths = [
            self.root / "reconnect-entrypoint",
            self.root / "restart-entrypoint",
            self.root / "observer-entrypoint",
        ]
        DRIVER.write_wrapper(paths[0], DRIVER_PATH, self.runtime, "effect", "reconnect")
        DRIVER.write_wrapper(paths[1], DRIVER_PATH, self.runtime, "effect", "app_restart")
        DRIVER.write_wrapper(paths[2], DRIVER_PATH, self.runtime, "observe", None)
        self.assertEqual(len({path.stat().st_ino for path in paths}), 3)
        observer = paths[2].read_text(encoding="utf-8")
        self.assertIn("EMBEDDED_COMMAND: str | None = 'observe'", observer)
        self.assertIn(f"EMBEDDED_RUNTIME_DIR: str | None = {str(self.runtime)!r}", observer)
        self.assertNotIn(f"exec /usr/bin/python3 {DRIVER_PATH}", observer)
        paths[0].write_text("#!/bin/sh\nexit 99\n", encoding="ascii")
        self.assertIn("EMBEDDED_COMMAND: str | None = 'observe'", paths[2].read_text())

    def test_shutdown_reaps_tunnel_and_closes_both_ports(self) -> None:
        initial = self.start_controller()
        DRIVER.stop_controller(self.runtime, 3.0)
        if self.initial_controller is not None:
            self.initial_controller.wait(timeout=3.0)
        self.assertFalse(DRIVER.pid_alive(initial["controller_pid"]))
        self.assertFalse(DRIVER.pid_alive(initial["tunnel_pid"]))
        for port in self.ports:
            self.assertFalse(DRIVER.tcp_open("127.0.0.1", port))

    def test_bounded_harness_cleanup_kills_and_reaps_signal_resistant_process(self) -> None:
        process = subprocess.Popen(
            [
                "/bin/sh",
                "-c",
                "trap '' INT TERM; while :; do /bin/sleep 1; done",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        pid = process.pid
        time.sleep(0.1)
        DRIVER.bounded_child_shutdown(process, 0.1, 2.0)
        self.assertIsNotNone(process.returncode)
        self.assertFalse(DRIVER.pid_alive(pid))

    def test_local_ports_remain_reserved_until_controller_spawn(self) -> None:
        ports, reservations = DRIVER.reserve_local_ports("127.0.0.1")
        try:
            for port in ports:
                contender = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                try:
                    with self.assertRaises(OSError):
                        contender.bind(("127.0.0.1", port))
                finally:
                    contender.close()
        finally:
            for reservation in reservations:
                reservation.close()
        for port in ports:
            available = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            try:
                available.bind(("127.0.0.1", port))
            finally:
                available.close()

    def test_bounded_cleanup_kills_descendant_after_leader_exits_on_interrupt(self) -> None:
        marker = self.root / "escaped-descendant.pid"
        source = "\n".join(
            (
                "import os, pathlib, signal, sys, time",
                "signal.signal(signal.SIGINT, lambda _signal, _frame: sys.exit(0))",
                "pid = os.fork()",
                "if pid == 0:",
                "    os.setsid()",
                "    signal.signal(signal.SIGINT, signal.SIG_IGN)",
                "    signal.signal(signal.SIGTERM, signal.SIG_IGN)",
                f"    pathlib.Path({str(marker)!r}).write_text(str(os.getpid()))",
                "    while True: time.sleep(1)",
                "while True: time.sleep(1)",
            )
        )
        process = subprocess.Popen(
            ["/usr/bin/python3", "-c", source],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        deadline = time.monotonic() + 3.0
        while not marker.is_file() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertTrue(marker.is_file(), "descendant did not publish its PID")
        descendant = int(marker.read_text())
        try:
            DRIVER.bounded_child_shutdown(process, 0.2, 2.0)
        finally:
            if DRIVER.pid_alive(descendant):
                os.kill(descendant, 9)
        self.assertIsNotNone(process.returncode)
        self.assertFalse(DRIVER.pid_alive(descendant))

    def test_controller_start_failure_remains_observable_and_reaped(self) -> None:
        config_path = DRIVER.runtime_paths(self.runtime)["config"]
        config = DRIVER.load_json(config_path, 64 * 1024)
        config["ssh_binary"] = str(Path("/usr/bin/false").resolve(strict=True))
        config["startup_timeout_seconds"] = 0.5
        DRIVER.validate_config(config)
        DRIVER.atomic_write_json(config_path, config)
        self.initial_controller = DRIVER.spawn_controller(self.runtime, 1)
        status = self.initial_controller.wait(timeout=5.0)
        self.assertNotEqual(status, 0)
        state = DRIVER.load_json(DRIVER.runtime_paths(self.runtime)["state"])
        self.assertEqual(state["status"], "failed")
        self.assertIsNone(state["tunnel_pid"])
        self.assertIn("exited before", state["error"])
        self.assertFalse(DRIVER.pid_alive(state["controller_pid"]))

    def test_run_command_owns_hooks_evidence_timeout_and_final_cleanup(self) -> None:
        run_id = "offline-runner"
        harness_evidence_root = self.root / "harness-evidence"
        connection_evidence_root = self.root / "connection-evidence"
        fake_test_sync = self.root / "fake-test-sync"
        fake_test_sync.write_text(
            FAKE_TEST_SYNC.replace(
                "__EVIDENCE_ROOT__", repr(str(harness_evidence_root))
            ),
            encoding="ascii",
        )
        fake_test_sync.chmod(0o700)
        ssh_config = self.root / "ssh-config"
        ssh_config.write_text("Host *\n  BatchMode yes\n", encoding="ascii")
        reader, writer = os.pipe()
        os.close(writer)
        try:
            completed = subprocess.run(
                [
                    "/usr/bin/python3",
                    str(DRIVER_PATH),
                    "run",
                    "--test-sync",
                    str(fake_test_sync),
                    "--agent-binary",
                    "/usr/bin/true",
                    "--workspace-id",
                    WORKSPACE_ID,
                    "--client-id-a",
                    CLIENT_A,
                    "--client-id-b",
                    CLIENT_B,
                    "--root-a",
                    str(self.root / "runner-root-a"),
                    "--root-b",
                    str(self.root / "runner-root-b"),
                    "--state-a",
                    str(self.root / "runner-state-a"),
                    "--state-b",
                    str(self.root / "runner-state-b"),
                    "--run-id",
                    run_id,
                    "--token-fd",
                    str(reader),
                    "--ssh-binary",
                    str(self.fake_ssh),
                    "--ssh-config",
                    str(ssh_config),
                    "--ssh-host",
                    "offline-fake-host",
                    "--connection-timeout-seconds",
                    "3",
                    "--hook-timeout-seconds",
                    "15",
                    "--term-grace-seconds",
                    "1",
                    "--kill-timeout-seconds",
                    "1",
                    "--run-timeout-seconds",
                    "30",
                    "--evidence-root",
                    str(harness_evidence_root),
                    "--connection-evidence-root",
                    str(connection_evidence_root),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                pass_fds=(reader,),
                timeout=40,
            )
        finally:
            os.close(reader)
        self.assertEqual(completed.returncode, 0, completed.stderr.decode())
        self.assertNotIn(b"eyJ", completed.stdout + completed.stderr)
        harness = harness_evidence_root / run_id
        arguments = json.loads((harness / "fake-test-sync.json").read_text())
        self.assertNotEqual(arguments["endpoint_a"], arguments["endpoint_b"])
        self.assertTrue(arguments["endpoint_a"].startswith("ws://127.0.0.1:"))
        self.assertTrue(arguments["endpoint_b"].startswith("ws://127.0.0.1:"))
        self.assertEqual(arguments["evidence_root"], str(harness_evidence_root.resolve()))
        harness_events = [
            json.loads(line) for line in (harness / "connection.jsonl").read_text().splitlines()
        ]
        self.assertTrue(any(event["event"] == "reconnect_completed" for event in harness_events))
        self.assertTrue(any(event["event"] == "app_restart_completed" for event in harness_events))
        sidecar = connection_evidence_root / run_id
        self.assertTrue((sidecar / "SHA256SUMS").is_file())
        sidecar_events = [
            json.loads(line) for line in (sidecar / "audit.jsonl").read_text().splitlines()
        ]
        self.assertEqual(sidecar_events[-1]["event"], "controller_stopped")
        final_state = json.loads((sidecar / "state.json").read_text())
        self.assertEqual(final_state["status"], "stopped")
        self.assertIsNone(final_state["tunnel_pid"])
        self.assertFalse(DRIVER.pid_alive(final_state["controller_pid"]))

    def test_sigterm_runs_bounded_cleanup_and_leaves_no_owned_process(self) -> None:
        run_id = "offline-sigterm"
        harness_evidence_root = self.root / "signal-harness-evidence"
        connection_evidence_root = self.root / "signal-connection-evidence"
        child_marker = self.root / "blocking-child.pid"
        fake_test_sync = self.root / "blocking-test-sync"
        fake_test_sync.write_text(BLOCKING_FAKE_TEST_SYNC, encoding="ascii")
        fake_test_sync.chmod(0o700)
        ssh_config = self.root / "signal-ssh-config"
        ssh_config.write_text("Host *\n  BatchMode yes\n", encoding="ascii")
        reader, writer = os.pipe()
        os.close(writer)
        environment = dict(os.environ)
        environment["FNS_BLOCKING_CHILD_PID"] = str(child_marker)
        process = subprocess.Popen(
            [
                "/usr/bin/python3",
                str(DRIVER_PATH),
                "run",
                "--test-sync",
                str(fake_test_sync),
                "--agent-binary",
                "/usr/bin/true",
                "--workspace-id",
                WORKSPACE_ID,
                "--client-id-a",
                CLIENT_A,
                "--client-id-b",
                CLIENT_B,
                "--root-a",
                str(self.root / "signal-root-a"),
                "--root-b",
                str(self.root / "signal-root-b"),
                "--state-a",
                str(self.root / "signal-state-a"),
                "--state-b",
                str(self.root / "signal-state-b"),
                "--run-id",
                run_id,
                "--token-fd",
                str(reader),
                "--ssh-binary",
                str(self.fake_ssh),
                "--ssh-config",
                str(ssh_config),
                "--ssh-host",
                "offline-fake-host",
                "--local-port-a",
                str(self.ports[0]),
                "--local-port-b",
                str(self.ports[1]),
                "--connection-timeout-seconds",
                "3",
                "--hook-timeout-seconds",
                "20",
                "--term-grace-seconds",
                "1",
                "--kill-timeout-seconds",
                "2",
                "--run-timeout-seconds",
                "30",
                "--evidence-root",
                str(harness_evidence_root),
                "--connection-evidence-root",
                str(connection_evidence_root),
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            pass_fds=(reader,),
        )
        os.close(reader)
        child_pid: int | None = None
        try:
            deadline = time.monotonic() + 5.0
            while not child_marker.is_file() and time.monotonic() < deadline:
                if process.poll() is not None:
                    break
                time.sleep(0.05)
            if not child_marker.is_file():
                stdout, stderr = process.communicate(timeout=3.0)
                self.fail(f"test-sync child did not start: {(stdout + stderr).decode()}")
            child_pid = int(child_marker.read_text())
            DRIVER.wait_ports("127.0.0.1", self.ports, True, 3.0)
            process.send_signal(signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=15)
            self.assertEqual(process.returncode, 1, stderr.decode())
            self.assertIn(b"received SIGTERM", stderr)
            self.assertNotIn(b"eyJ", stdout + stderr)
        finally:
            if process.poll() is None:
                process.kill()
            process.communicate(timeout=3.0)
            if child_pid is not None and DRIVER.pid_alive(child_pid):
                os.kill(child_pid, 9)
        self.assertIsNotNone(child_pid)
        self.assertFalse(DRIVER.pid_alive(child_pid))
        for port in self.ports:
            self.assertFalse(DRIVER.tcp_open("127.0.0.1", port))
        sidecar = connection_evidence_root / run_id
        final_state = json.loads((sidecar / "state.json").read_text())
        self.assertEqual(final_state["status"], "stopped")
        self.assertIsNone(final_state["tunnel_pid"])
        self.assertFalse(DRIVER.pid_alive(final_state["controller_pid"]))


if __name__ == "__main__":
    unittest.main()
