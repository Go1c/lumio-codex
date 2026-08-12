#!/usr/bin/python3
"""Run test-sync through two observable, restartable SSH forwards.

The driver never opens the JWT descriptor. It only passes that descriptor to
test-sync, which remains the sole token consumer.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import pwd
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any, Iterable


CONFIG_SCHEMA = "fns-controlled-ssh-config/1"
STATE_SCHEMA = "fns-controlled-ssh-state/1"
AUDIT_SCHEMA = "fns-controlled-ssh-audit/1"
EFFECT_RECEIPT_SCHEMA = "test-sync-effect/1"
EFFECT_OBSERVATION_SCHEMA = "test-sync-effect-observation/1"
MAX_CONTROL_BYTES = 16 * 1024
WORKSPACE_PATH = "/api/user/workspace-sync/v2"
EMBEDDED_COMMAND: str | None = None
EMBEDDED_RUNTIME_DIR: str | None = None
EMBEDDED_ALLOWED_ACTION: str | None = None


class DriverError(RuntimeError):
    pass


HANDLED_SIGNALS = (signal.SIGINT, signal.SIGTERM)


def install_interrupt_handlers() -> dict[signal.Signals, Any]:
    previous = {handled: signal.getsignal(handled) for handled in HANDLED_SIGNALS}

    def interrupted(handled: int, _frame: Any) -> None:
        raise DriverError(f"received {signal.Signals(handled).name}")

    for handled in HANDLED_SIGNALS:
        signal.signal(handled, interrupted)
    return previous


def ignore_interrupt_signals() -> None:
    for handled in HANDLED_SIGNALS:
        signal.signal(handled, signal.SIG_IGN)


def restore_signal_handlers(previous: dict[signal.Signals, Any]) -> None:
    for handled, handler in previous.items():
        signal.signal(handled, handler)


def now_millis() -> int:
    return time.time_ns() // 1_000_000


def compact_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def atomic_write_json(path: Path, value: Any, mode: int = 0o600) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.parent / f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "wb", closefd=True) as output:
            output.write(compact_json(value) + b"\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def append_json(path: Path, value: Any) -> None:
    encoded = compact_json(value) + b"\n"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        written = os.write(descriptor, encoded)
        if written != len(encoded):
            raise DriverError(f"short audit write for {path}")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def load_json(path: Path, maximum: int = MAX_CONTROL_BYTES) -> dict[str, Any]:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    try:
        stat = os.fstat(descriptor)
        if stat.st_size <= 0 or stat.st_size > maximum:
            raise DriverError(f"invalid JSON file size: {path}")
        data = os.read(descriptor, maximum + 1)
    finally:
        os.close(descriptor)
    if len(data) > maximum:
        raise DriverError(f"JSON file exceeds limit: {path}")
    value = json.loads(data)
    if not isinstance(value, dict):
        raise DriverError(f"JSON object required: {path}")
    return value


def require_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        unknown = sorted(actual - expected)
        raise DriverError(f"invalid {label} fields (missing={missing}, unknown={unknown})")


def runtime_paths(runtime_dir: Path) -> dict[str, Path]:
    return {
        "config": runtime_dir / "config.json",
        "state": runtime_dir / "state.json",
        "socket": runtime_dir / "control.sock",
        "lock": runtime_dir / "controller.lock",
        "audit": runtime_dir / "audit.jsonl",
        "log": runtime_dir / "controller.log",
    }


def validate_private_runtime(runtime_dir: Path) -> Path:
    resolved = runtime_dir.resolve(strict=True)
    stat = resolved.stat()
    if not resolved.is_dir() or stat.st_uid != os.geteuid() or stat.st_mode & 0o077:
        raise DriverError("runtime directory must be private and owned by the current user")
    socket_path = runtime_paths(resolved)["socket"]
    if len(os.fsencode(socket_path)) >= 100:
        raise DriverError("runtime path is too long for a portable Unix control socket")
    return resolved


def validate_config(value: dict[str, Any]) -> dict[str, Any]:
    expected = {
        "schema_version",
        "runtime_id",
        "driver_path",
        "python_path",
        "ssh_binary",
        "ssh_host",
        "ssh_port",
        "ssh_config",
        "identity_file",
        "local_host",
        "local_ports",
        "remote_host",
        "remote_port",
        "connect_timeout_seconds",
        "startup_timeout_seconds",
        "term_grace_seconds",
        "kill_timeout_seconds",
        "controller_environment",
        "workspace_id",
        "client_id_a",
        "client_id_b",
        "harness_evidence_dir",
    }
    require_keys(value, expected, "controller config")
    if value["schema_version"] != CONFIG_SCHEMA:
        raise DriverError("unsupported controller config schema")
    for key in (
        "runtime_id",
        "driver_path",
        "python_path",
        "ssh_binary",
        "ssh_host",
        "local_host",
        "remote_host",
        "workspace_id",
        "client_id_a",
        "client_id_b",
        "harness_evidence_dir",
    ):
        if not isinstance(value[key], str) or not value[key]:
            raise DriverError(f"invalid controller config field: {key}")
    if value["ssh_host"].startswith("-"):
        raise DriverError("SSH host must not start with a dash")
    for key in ("driver_path", "python_path", "ssh_binary"):
        path = Path(value[key])
        if not path.is_absolute() or path.resolve(strict=True) != path:
            raise DriverError(f"{key} must be a canonical absolute path")
        if not path.is_file() or not os.access(path, os.X_OK):
            raise DriverError(f"{key} must be executable")
    for optional in ("ssh_config", "identity_file"):
        raw = value[optional]
        if raw is not None:
            if not isinstance(raw, str) or not raw:
                raise DriverError(f"invalid optional path: {optional}")
            path = Path(raw)
            if not path.is_absolute() or path.resolve(strict=True) != path or not path.is_file():
                raise DriverError(f"{optional} must be a canonical regular file")
    ports = value["local_ports"]
    if (
        not isinstance(ports, list)
        or len(ports) != 2
        or len(set(ports)) != 2
        or any(not isinstance(port, int) or not 1 <= port <= 65535 for port in ports)
    ):
        raise DriverError("two distinct local TCP ports are required")
    for key in ("ssh_port", "remote_port"):
        if not isinstance(value[key], int) or not 1 <= value[key] <= 65535:
            raise DriverError(f"invalid controller config field: {key}")
    for key in (
        "connect_timeout_seconds",
        "startup_timeout_seconds",
        "term_grace_seconds",
        "kill_timeout_seconds",
    ):
        if not isinstance(value[key], (int, float)) or not 0 < value[key] <= 600:
            raise DriverError(f"invalid controller timeout: {key}")
    environment = value["controller_environment"]
    if not isinstance(environment, dict) or any(
        not isinstance(key, str) or not isinstance(item, str)
        for key, item in environment.items()
    ):
        raise DriverError("invalid controller environment")
    allowed_environment = {"HOME", "LOGNAME", "PATH", "SSH_AUTH_SOCK", "TMPDIR", "USER"}
    if not set(environment).issubset(allowed_environment):
        raise DriverError("controller environment contains an unsupported variable")
    if value["client_id_a"] == value["client_id_b"]:
        raise DriverError("two distinct client IDs are required")
    return value


def read_config(runtime_dir: Path) -> dict[str, Any]:
    runtime_dir = validate_private_runtime(runtime_dir)
    return validate_config(load_json(runtime_paths(runtime_dir)["config"], 64 * 1024))


def pid_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        pass
    try:
        status = subprocess.check_output(
            ["/bin/ps", "-p", str(pid), "-o", "stat="],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return False
    if not status or status.startswith("Z"):
        return False
    return True


def wait_pid_gone(pid: int, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not pid_alive(pid):
            return
        time.sleep(0.05)
    raise DriverError(f"process {pid} did not exit before timeout")


def tcp_open(host: str, port: int, timeout: float = 0.25) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def wait_ports(
    host: str,
    ports: Iterable[int],
    should_be_open: bool,
    timeout: float,
    process: subprocess.Popen[bytes] | None = None,
) -> None:
    ports = tuple(ports)
    deadline = time.monotonic() + timeout
    consecutive = 0
    while time.monotonic() < deadline:
        if process is not None and process.poll() is not None:
            raise DriverError(f"SSH process {process.pid} exited before its forwards were ready")
        current = all(tcp_open(host, port) == should_be_open for port in ports)
        if current:
            consecutive += 1
            if consecutive >= 2:
                return
        else:
            consecutive = 0
        time.sleep(0.05)
    state = "open" if should_be_open else "closed"
    raise DriverError(f"local SSH forward ports did not become {state} before timeout")


def terminate_group(
    process: subprocess.Popen[bytes], term_grace: float, kill_timeout: float
) -> str:
    if process.poll() is not None:
        process.wait()
        return "exited"
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=term_grace)
        return "terminated"
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=kill_timeout)
        except subprocess.TimeoutExpired as error:
            raise DriverError(f"process group {process.pid} could not be reaped") from error
        return "killed"


class Controller:
    def __init__(self, runtime_dir: Path, generation: int):
        self.runtime_dir = validate_private_runtime(runtime_dir)
        self.paths = runtime_paths(self.runtime_dir)
        self.config = read_config(self.runtime_dir)
        self.generation = generation
        self.tunnel: subprocess.Popen[bytes] | None = None
        self.listener: socket.socket | None = None
        self.stop_requested = False
        self.lock_file: Any = None

    @property
    def identity(self) -> dict[str, int]:
        return {"pid": os.getpid(), "generation": self.generation}

    def audit(self, event: str, **fields: Any) -> None:
        append_json(
            self.paths["audit"],
            {
                "schema_version": AUDIT_SCHEMA,
                "timestamp_ms": now_millis(),
                "runtime_id": self.config["runtime_id"],
                "event": event,
                "controller_pid": os.getpid(),
                "generation": self.generation,
                **fields,
            },
        )

    def write_state(self, status: str, error: str | None = None) -> None:
        atomic_write_json(
            self.paths["state"],
            {
                "schema_version": STATE_SCHEMA,
                "runtime_id": self.config["runtime_id"],
                "status": status,
                "controller_pid": os.getpid(),
                "generation": self.generation,
                "tunnel_pid": self.tunnel.pid if self.tunnel is not None else None,
                "local_host": self.config["local_host"],
                "local_ports": self.config["local_ports"],
                "remote_host": self.config["remote_host"],
                "remote_port": self.config["remote_port"],
                "updated_ms": now_millis(),
                "error": error,
            },
        )

    def acquire(self) -> None:
        self.lock_file = self.paths["lock"].open("a+b")
        os.chmod(self.paths["lock"], 0o600)
        try:
            fcntl.flock(self.lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise DriverError("another SSH controller already owns this runtime") from error
        try:
            self.paths["socket"].unlink()
        except FileNotFoundError:
            pass

    def ssh_command(self) -> list[str]:
        command = [
            self.config["ssh_binary"],
            "-N",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            f"ConnectTimeout={int(self.config['connect_timeout_seconds'])}",
            "-p",
            str(self.config["ssh_port"]),
        ]
        if self.config["ssh_config"] is not None:
            command.extend(["-F", self.config["ssh_config"]])
        if self.config["identity_file"] is not None:
            command.extend(["-i", self.config["identity_file"]])
        for port in self.config["local_ports"]:
            command.extend(
                [
                    "-L",
                    (
                        f"{self.config['local_host']}:{port}:"
                        f"{self.config['remote_host']}:{self.config['remote_port']}"
                    ),
                ]
            )
        command.append(self.config["ssh_host"])
        return command

    def start_tunnel(self, reason: str) -> int:
        self.write_state("starting")
        log_descriptor = os.open(
            self.paths["log"], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600
        )
        try:
            process = subprocess.Popen(
                self.ssh_command(),
                stdin=subprocess.DEVNULL,
                stdout=log_descriptor,
                stderr=log_descriptor,
                env=self.config["controller_environment"],
                close_fds=True,
                start_new_session=True,
            )
        finally:
            os.close(log_descriptor)
        self.tunnel = process
        self.audit("tunnel_spawned", reason=reason, tunnel_pid=process.pid)
        try:
            wait_ports(
                self.config["local_host"],
                self.config["local_ports"],
                True,
                float(self.config["startup_timeout_seconds"]),
                process,
            )
        except Exception:
            self.stop_tunnel("startup_failed", require_closed=False)
            raise
        self.write_state("ready")
        self.audit("tunnel_ready", reason=reason, tunnel_pid=process.pid)
        return process.pid

    def stop_tunnel(self, reason: str, require_closed: bool = True) -> tuple[int | None, str]:
        process = self.tunnel
        self.tunnel = None
        if process is None:
            return None, "absent"
        old_pid = process.pid
        termination = terminate_group(
            process,
            float(self.config["term_grace_seconds"]),
            float(self.config["kill_timeout_seconds"]),
        )
        if require_closed:
            wait_ports(
                self.config["local_host"],
                self.config["local_ports"],
                False,
                float(self.config["kill_timeout_seconds"]),
            )
        self.audit(
            "tunnel_stopped",
            reason=reason,
            tunnel_pid=old_pid,
            termination=termination,
            port_closure_observed=require_closed,
        )
        return old_pid, termination

    def reconnect(self) -> dict[str, Any]:
        old_identity = self.identity.copy()
        old_tunnel = self.tunnel.pid if self.tunnel is not None else None
        if old_tunnel is None:
            raise DriverError("cannot reconnect without a running SSH process")
        self.write_state("reconnecting")
        self.audit("reconnect_started", old_tunnel_pid=old_tunnel)
        self.stop_tunnel("reconnect", require_closed=True)
        self.generation += 1
        try:
            new_tunnel = self.start_tunnel("reconnect")
        except Exception as error:
            self.write_state("failed", str(error))
            self.audit("reconnect_failed", old_tunnel_pid=old_tunnel, error=str(error))
            raise
        if new_tunnel == old_tunnel:
            raise DriverError("SSH reconnect reused the old process PID")
        self.audit(
            "reconnect_completed",
            old_tunnel_pid=old_tunnel,
            new_tunnel_pid=new_tunnel,
            old_generation=old_identity["generation"],
            new_generation=self.generation,
            port_closure_observed=True,
        )
        return {
            "old": old_identity,
            "new": self.identity.copy(),
            "old_tunnel_pid": old_tunnel,
            "new_tunnel_pid": new_tunnel,
            "port_closure_observed": True,
        }

    def shutdown_for_restart(self) -> dict[str, Any]:
        old_identity = self.identity.copy()
        old_tunnel = self.tunnel.pid if self.tunnel is not None else None
        self.write_state("stopping")
        self.audit("app_restart_stop_started", old_tunnel_pid=old_tunnel)
        self.stop_tunnel("app_restart", require_closed=True)
        self.write_state("stopped")
        self.audit(
            "app_restart_stop_completed",
            old_tunnel_pid=old_tunnel,
            port_closure_observed=True,
        )
        return {
            "old": old_identity,
            "old_tunnel_pid": old_tunnel,
            "port_closure_observed": True,
        }

    def handle(self, request: dict[str, Any]) -> tuple[dict[str, Any], bool]:
        require_keys(request, {"command", "runtime_id"}, "controller request")
        if request["runtime_id"] != self.config["runtime_id"]:
            raise DriverError("controller request runtime does not match")
        command = request["command"]
        if command == "ping":
            return {"ok": True, "identity": self.identity, "status": "ready"}, False
        if command == "reconnect":
            return {"ok": True, "transition": self.reconnect()}, False
        if command == "shutdown_for_restart":
            return {"ok": True, "transition": self.shutdown_for_restart()}, True
        if command == "shutdown":
            self.write_state("stopping")
            self.stop_tunnel("shutdown", require_closed=True)
            self.write_state("stopped")
            return {"ok": True, "identity": self.identity}, True
        raise DriverError("unsupported controller command")

    def serve_connection(self, connection: socket.socket) -> bool:
        connection.settimeout(float(self.config["startup_timeout_seconds"]))
        received = bytearray()
        while b"\n" not in received:
            chunk = connection.recv(min(4096, MAX_CONTROL_BYTES + 1 - len(received)))
            if not chunk:
                break
            received.extend(chunk)
            if len(received) > MAX_CONTROL_BYTES:
                raise DriverError("controller request exceeds size limit")
        if not received:
            raise DriverError("empty controller request")
        request = json.loads(bytes(received).split(b"\n", 1)[0])
        if not isinstance(request, dict):
            raise DriverError("controller request must be an object")
        response, stop_after = self.handle(request)
        connection.sendall(compact_json(response) + b"\n")
        return stop_after

    def run(self) -> None:
        if self.generation <= 0:
            raise DriverError("controller generation must be positive")
        self.acquire()
        for handled_signal in (signal.SIGINT, signal.SIGTERM):
            signal.signal(
                handled_signal,
                lambda _signal, _frame: setattr(self, "stop_requested", True),
            )
        failure: Exception | None = None
        try:
            self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.listener.bind(str(self.paths["socket"]))
            os.chmod(self.paths["socket"], 0o600)
            self.listener.listen(8)
            self.listener.settimeout(0.25)
            self.audit("controller_started")
            self.start_tunnel("initial")
            while not self.stop_requested:
                if self.tunnel is None or self.tunnel.poll() is not None:
                    raise DriverError("controlled SSH process exited unexpectedly")
                try:
                    connection, _ = self.listener.accept()
                except socket.timeout:
                    continue
                with connection:
                    try:
                        stop_after = self.serve_connection(connection)
                    except Exception as error:
                        try:
                            connection.sendall(
                                compact_json({"ok": False, "error": str(error)}) + b"\n"
                            )
                        except OSError:
                            pass
                        self.audit("control_request_failed", error=str(error))
                        continue
                if stop_after:
                    self.stop_requested = True
        except Exception as error:
            failure = error
            self.write_state("failed", str(error))
            self.audit("controller_failed", error=str(error))
            raise
        finally:
            if self.tunnel is not None:
                try:
                    self.stop_tunnel("controller_exit", require_closed=True)
                except Exception as error:
                    self.audit("controller_cleanup_failed", error=str(error))
            if failure is None:
                self.write_state("stopped")
            else:
                self.write_state("failed", str(failure))
            self.audit("controller_stopped")
            if self.listener is not None:
                self.listener.close()
            try:
                self.paths["socket"].unlink()
            except FileNotFoundError:
                pass
            if self.lock_file is not None:
                fcntl.flock(self.lock_file.fileno(), fcntl.LOCK_UN)
                self.lock_file.close()


def control_request(runtime_dir: Path, command: str, timeout: float) -> dict[str, Any]:
    config = read_config(runtime_dir)
    request = {"command": command, "runtime_id": config["runtime_id"]}
    paths = runtime_paths(runtime_dir)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(timeout)
        client.connect(str(paths["socket"]))
        client.sendall(compact_json(request) + b"\n")
        received = bytearray()
        while b"\n" not in received:
            chunk = client.recv(min(4096, MAX_CONTROL_BYTES + 1 - len(received)))
            if not chunk:
                break
            received.extend(chunk)
            if len(received) > MAX_CONTROL_BYTES:
                raise DriverError("controller response exceeds size limit")
    if not received:
        raise DriverError("controller returned an empty response")
    response = json.loads(bytes(received).split(b"\n", 1)[0])
    if not isinstance(response, dict) or response.get("ok") is not True:
        detail = (
            response.get("error", "malformed response")
            if isinstance(response, dict)
            else "malformed response"
        )
        raise DriverError(f"controller request failed: {detail}")
    return response


def live_state(runtime_dir: Path) -> dict[str, Any]:
    config = read_config(runtime_dir)
    state = load_json(runtime_paths(runtime_dir)["state"])
    expected = {
        "schema_version",
        "runtime_id",
        "status",
        "controller_pid",
        "generation",
        "tunnel_pid",
        "local_host",
        "local_ports",
        "remote_host",
        "remote_port",
        "updated_ms",
        "error",
    }
    require_keys(state, expected, "controller state")
    if state["schema_version"] != STATE_SCHEMA or state["runtime_id"] != config["runtime_id"]:
        raise DriverError("controller state identity does not match")
    if state["status"] != "ready":
        raise DriverError(f"controller is not ready: {state['status']}")
    pid = state["controller_pid"]
    generation = state["generation"]
    tunnel_pid = state["tunnel_pid"]
    if (
        not isinstance(pid, int)
        or pid <= 0
        or not isinstance(generation, int)
        or generation <= 0
        or not isinstance(tunnel_pid, int)
        or tunnel_pid <= 0
        or not pid_alive(pid)
        or not pid_alive(tunnel_pid)
    ):
        raise DriverError("controller state contains a dead or invalid process")
    ping = control_request(runtime_dir, "ping", 2.0)
    if ping.get("identity") != {"pid": pid, "generation": generation}:
        raise DriverError("controller state and live ping identity differ")
    return state


def wait_live_state(
    runtime_dir: Path,
    timeout: float,
    expected_generation: int | None = None,
    different_pid: int | None = None,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            state = live_state(runtime_dir)
            if expected_generation is not None and state["generation"] != expected_generation:
                raise DriverError("controller generation has not reached the expected value")
            if different_pid is not None and state["controller_pid"] == different_pid:
                raise DriverError("replacement controller still has the old PID")
            return state
        except (
            DriverError,
            FileNotFoundError,
            ConnectionError,
            OSError,
            json.JSONDecodeError,
        ) as error:
            last_error = error
            time.sleep(0.05)
    raise DriverError(f"controller did not become ready: {last_error}")


def spawn_controller(runtime_dir: Path, generation: int) -> subprocess.Popen[bytes]:
    config = read_config(runtime_dir)
    log_descriptor = os.open(
        runtime_paths(runtime_dir)["log"], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600
    )
    try:
        return subprocess.Popen(
            [
                config["python_path"],
                config["driver_path"],
                "controller",
                "--runtime-dir",
                str(runtime_dir),
                "--generation",
                str(generation),
            ],
            stdin=subprocess.DEVNULL,
            stdout=log_descriptor,
            stderr=log_descriptor,
            env=config["controller_environment"],
            close_fds=True,
            start_new_session=True,
        )
    finally:
        os.close(log_descriptor)


def effect_context(arguments: argparse.Namespace, config: dict[str, Any]) -> dict[str, Any]:
    if arguments.action != arguments.allowed_action:
        raise DriverError("hook was invoked for the wrong action")
    if (
        arguments.workspace_id != config["workspace_id"]
        or arguments.client_id_a != config["client_id_a"]
        or arguments.client_id_b != config["client_id_b"]
    ):
        raise DriverError("hook context does not match this controlled run")
    if (
        arguments.agent_pid_a <= 0
        or arguments.agent_pid_b <= 0
        or arguments.agent_pid_a == arguments.agent_pid_b
    ):
        raise DriverError("hook received invalid Agent PIDs")
    return {
        "workspace_id": arguments.workspace_id,
        "client_id_a": arguments.client_id_a,
        "client_id_b": arguments.client_id_b,
        "agent_pid_a": arguments.agent_pid_a,
        "agent_pid_b": arguments.agent_pid_b,
    }


def copy_connection_audit_to_harness(runtime_dir: Path, config: dict[str, Any]) -> None:
    source = runtime_paths(runtime_dir)["audit"]
    destination_root = Path(config["harness_evidence_dir"])
    if not destination_root.is_dir() or destination_root.is_symlink():
        raise DriverError("harness evidence directory is unavailable")
    destination = destination_root / "connection.jsonl"
    data = source.read_bytes()
    if not data or len(data) > 4 * 1024 * 1024:
        raise DriverError("connection audit is empty or exceeds its size limit")
    temporary = destination_root / f".connection.{os.getpid()}.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=True) as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, destination)
    finally:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def run_observer(arguments: argparse.Namespace) -> None:
    runtime_dir = validate_private_runtime(arguments.runtime_dir)
    config = read_config(runtime_dir)
    context = effect_context(arguments, config)
    if arguments.phase not in {"before", "after"}:
        raise DriverError("invalid observer phase")
    state = wait_live_state(runtime_dir, float(config["startup_timeout_seconds"]))
    observation = {
        "schema_version": EFFECT_OBSERVATION_SCHEMA,
        "action": arguments.action,
        "context": context,
        "identity": {
            "pid": state["controller_pid"],
            "generation": state["generation"],
        },
    }
    sys.stdout.buffer.write(compact_json(observation) + b"\n")
    sys.stdout.buffer.flush()


def run_effect(arguments: argparse.Namespace) -> None:
    runtime_dir = validate_private_runtime(arguments.runtime_dir)
    config = read_config(runtime_dir)
    context = effect_context(arguments, config)
    timeout = float(config["startup_timeout_seconds"])
    previous_handlers = install_interrupt_handlers()
    replacement: subprocess.Popen[bytes] | None = None
    replacement_committed = False
    try:
        before = wait_live_state(runtime_dir, timeout)
        old = {"pid": before["controller_pid"], "generation": before["generation"]}
        if arguments.action == "reconnect":
            response = control_request(runtime_dir, "reconnect", timeout * 2)
            transition = response.get("transition")
            after = wait_live_state(runtime_dir, timeout, old["generation"] + 1)
            new = {"pid": after["controller_pid"], "generation": after["generation"]}
            if (
                not isinstance(transition, dict)
                or transition.get("old") != old
                or transition.get("new") != new
                or transition.get("port_closure_observed") is not True
                or transition.get("old_tunnel_pid") == transition.get("new_tunnel_pid")
                or new["pid"] != old["pid"]
            ):
                raise DriverError("reconnect did not prove a controlled SSH replacement")
        elif arguments.action == "app_restart":
            response = control_request(runtime_dir, "shutdown_for_restart", timeout * 2)
            transition = response.get("transition")
            if (
                not isinstance(transition, dict)
                or transition.get("old") != old
                or transition.get("port_closure_observed") is not True
            ):
                raise DriverError("old controller did not prove a complete SSH shutdown")
            wait_pid_gone(old["pid"], timeout)
            replacement = spawn_controller(runtime_dir, old["generation"] + 1)
            if replacement.pid == old["pid"]:
                raise DriverError("replacement controller reused the old PID")
            after = wait_live_state(
                runtime_dir,
                timeout,
                expected_generation=old["generation"] + 1,
                different_pid=old["pid"],
            )
            new = {"pid": after["controller_pid"], "generation": after["generation"]}
            append_json(
                runtime_paths(runtime_dir)["audit"],
                {
                    "schema_version": AUDIT_SCHEMA,
                    "timestamp_ms": now_millis(),
                    "runtime_id": config["runtime_id"],
                    "event": "app_restart_completed",
                    "controller_pid": new["pid"],
                    "generation": new["generation"],
                    "old_controller_pid": old["pid"],
                    "new_controller_pid": new["pid"],
                    "old_generation": old["generation"],
                    "new_generation": new["generation"],
                    "port_closure_observed": True,
                },
            )
        else:
            raise DriverError("unsupported controlled effect")
        copy_connection_audit_to_harness(runtime_dir, config)
        receipt = {
            "schema_version": EFFECT_RECEIPT_SCHEMA,
            "action": arguments.action,
            "context": context,
            "old": old,
            "new": new,
        }
        sys.stdout.buffer.write(compact_json(receipt) + b"\n")
        sys.stdout.buffer.flush()
        replacement_committed = True
    finally:
        if replacement is not None and not replacement_committed:
            ignore_interrupt_signals()
            cleanup_errors: list[str] = []
            try:
                stop_controller(runtime_dir, timeout)
            except Exception as error:
                cleanup_errors.append(str(error))
            if replacement.poll() is None:
                try:
                    terminate_group(
                        replacement,
                        float(config["term_grace_seconds"]),
                        float(config["kill_timeout_seconds"]),
                    )
                except Exception as error:
                    cleanup_errors.append(str(error))
            else:
                replacement.wait()
            try:
                stop_controller(runtime_dir, timeout)
            except Exception as error:
                cleanup_errors.append(str(error))
            if cleanup_errors:
                restore_signal_handlers(previous_handlers)
                raise DriverError(
                    "replacement controller cleanup failed: " + "; ".join(cleanup_errors)
                )
        restore_signal_handlers(previous_handlers)


def reserve_local_ports(
    host: str, requested: tuple[int, int] | None = None
) -> tuple[tuple[int, int], list[socket.socket]]:
    reservations: list[socket.socket] = []
    ports: list[int] = []
    try:
        for index in range(2):
            reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            reservation.bind((host, requested[index] if requested is not None else 0))
            reservations.append(reservation)
            ports.append(reservation.getsockname()[1])
        if ports[0] == ports[1]:
            raise DriverError("failed to allocate distinct local ports")
        return (ports[0], ports[1]), reservations
    except Exception:
        for reservation in reservations:
            reservation.close()
        raise


def allocate_local_ports(host: str) -> tuple[int, int]:
    ports, reservations = reserve_local_ports(host)
    for reservation in reservations:
        reservation.close()
    return ports


def canonical_executable(raw: str, label: str) -> Path:
    path = Path(raw).expanduser().resolve(strict=True)
    if not path.is_file() or not os.access(path, os.X_OK):
        raise DriverError(f"{label} must be an executable file")
    return path


def canonical_optional_file(raw: str | None, label: str) -> Path | None:
    if raw is None:
        return None
    path = Path(raw).expanduser().resolve(strict=True)
    if not path.is_file():
        raise DriverError(f"{label} must be a regular file")
    return path


def controller_environment() -> dict[str, str]:
    account = pwd.getpwuid(os.geteuid())
    environment = {
        "HOME": account.pw_dir,
        "USER": account.pw_name,
        "LOGNAME": account.pw_name,
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "TMPDIR": os.environ.get("TMPDIR", "/tmp"),
    }
    auth_socket = os.environ.get("SSH_AUTH_SOCK")
    if auth_socket:
        environment["SSH_AUTH_SOCK"] = auth_socket
    return environment


def write_wrapper(
    path: Path,
    driver: Path,
    runtime_dir: Path,
    command: str,
    action: str | None,
) -> None:
    source = driver.read_text(encoding="utf-8")
    replacements = (
        (
            "EMBEDDED_COMMAND: str | None = None",
            f"EMBEDDED_COMMAND: str | None = {command!r}",
        ),
        (
            "EMBEDDED_RUNTIME_DIR: str | None = None",
            f"EMBEDDED_RUNTIME_DIR: str | None = {str(runtime_dir)!r}",
        ),
        (
            "EMBEDDED_ALLOWED_ACTION: str | None = None",
            f"EMBEDDED_ALLOWED_ACTION: str | None = {action!r}",
        ),
    )
    for marker, replacement in replacements:
        if marker not in source:
            raise DriverError("could not create a self-contained controlled entrypoint")
        source = source.replace(marker, replacement, 1)
    path.write_text(source, encoding="utf-8")
    path.chmod(0o700)


def write_runtime_config(
    arguments: argparse.Namespace,
    runtime_dir: Path,
    ports: tuple[int, int],
) -> dict[str, Any]:
    driver = Path(__file__).resolve(strict=True)
    python = canonical_executable("/usr/bin/python3", "Python")
    ssh = canonical_executable(arguments.ssh_binary, "SSH")
    ssh_config = canonical_optional_file(arguments.ssh_config, "SSH config")
    identity = canonical_optional_file(arguments.identity_file, "SSH identity")
    evidence_root = Path(arguments.evidence_root).expanduser().resolve()
    harness_evidence = evidence_root / arguments.run_id
    value = {
        "schema_version": CONFIG_SCHEMA,
        "runtime_id": uuid.uuid4().hex,
        "driver_path": str(driver),
        "python_path": str(python),
        "ssh_binary": str(ssh),
        "ssh_host": arguments.ssh_host,
        "ssh_port": arguments.ssh_port,
        "ssh_config": str(ssh_config) if ssh_config is not None else None,
        "identity_file": str(identity) if identity is not None else None,
        "local_host": "127.0.0.1",
        "local_ports": list(ports),
        "remote_host": arguments.remote_host,
        "remote_port": arguments.remote_port,
        "connect_timeout_seconds": arguments.ssh_connect_timeout_seconds,
        "startup_timeout_seconds": arguments.connection_timeout_seconds,
        "term_grace_seconds": arguments.term_grace_seconds,
        "kill_timeout_seconds": arguments.kill_timeout_seconds,
        "controller_environment": controller_environment(),
        "workspace_id": arguments.workspace_id,
        "client_id_a": arguments.client_id_a,
        "client_id_b": arguments.client_id_b,
        "harness_evidence_dir": str(harness_evidence),
    }
    validate_config(value)
    atomic_write_json(runtime_paths(runtime_dir)["config"], value)
    return value


def test_sync_command(
    arguments: argparse.Namespace,
    ports: tuple[int, int],
    wrappers: dict[str, Path],
) -> list[str]:
    return [
        str(canonical_executable(arguments.test_sync, "test-sync")),
        "run",
        "--endpoint-a",
        f"ws://127.0.0.1:{ports[0]}{WORKSPACE_PATH}",
        "--endpoint-b",
        f"ws://127.0.0.1:{ports[1]}{WORKSPACE_PATH}",
        "--workspace-id",
        arguments.workspace_id,
        "--client-id-a",
        arguments.client_id_a,
        "--client-id-b",
        arguments.client_id_b,
        "--root-a",
        str(Path(arguments.root_a).expanduser().resolve()),
        "--root-b",
        str(Path(arguments.root_b).expanduser().resolve()),
        "--state-a",
        str(Path(arguments.state_a).expanduser().resolve()),
        "--state-b",
        str(Path(arguments.state_b).expanduser().resolve()),
        "--agent-binary",
        str(canonical_executable(arguments.agent_binary, "fns-agent")),
        "--reconnect-hook",
        str(wrappers["reconnect"]),
        "--app-restart-hook",
        str(wrappers["app_restart"]),
        "--effect-observer",
        str(wrappers["observer"]),
        "--run-id",
        arguments.run_id,
        "--evidence-root",
        str(Path(arguments.evidence_root).expanduser().resolve()),
        "--token-fd",
        str(arguments.token_fd),
        "--startup-timeout-seconds",
        str(arguments.agent_startup_timeout_seconds),
        "--checkpoint-timeout-seconds",
        str(arguments.checkpoint_timeout_seconds),
        "--sample-interval-millis",
        str(arguments.sample_interval_millis),
        "--hook-timeout-seconds",
        str(arguments.hook_timeout_seconds),
        "--term-grace-seconds",
        str(arguments.term_grace_seconds),
        "--kill-timeout-seconds",
        str(arguments.kill_timeout_seconds),
        "--large-file-bytes",
        str(arguments.large_file_bytes),
        "--max-active-transfers",
        str(arguments.max_active_transfers),
    ]


def descendants_of(root_pid: int) -> set[int]:
    try:
        output = subprocess.check_output(
            ["/bin/ps", "-axo", "pid=,ppid="], stderr=subprocess.DEVNULL, text=True
        )
    except (OSError, subprocess.CalledProcessError):
        return set()
    children: dict[int, list[int]] = {}
    for line in output.splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        try:
            pid, parent = map(int, fields)
        except ValueError:
            continue
        children.setdefault(parent, []).append(pid)
    found: set[int] = set()
    pending = list(children.get(root_pid, []))
    while pending:
        pid = pending.pop()
        if pid in found:
            continue
        found.add(pid)
        pending.extend(children.get(pid, []))
    return found


def process_group_alive(process_group: int) -> bool:
    if process_group <= 0:
        return False
    try:
        os.killpg(process_group, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def signal_processes(processes: Iterable[int], handled_signal: int) -> None:
    for pid in sorted(set(processes), reverse=True):
        try:
            os.kill(pid, handled_signal)
        except ProcessLookupError:
            pass


def wait_owned_tree_gone(
    process: subprocess.Popen[bytes], descendants: set[int], timeout: float
) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        leader_exited = process.poll() is not None
        if leader_exited:
            process.wait()
        descendants = {pid for pid in descendants if pid_alive(pid)}
        if leader_exited and not descendants and not process_group_alive(process.pid):
            return True
        time.sleep(0.05)
    return False


def bounded_child_shutdown(process: subprocess.Popen[bytes], grace: float, kill: float) -> None:
    if process.poll() is not None:
        process.wait()
        return
    descendants = descendants_of(process.pid)
    try:
        os.killpg(process.pid, signal.SIGINT)
    except ProcessLookupError:
        pass
    if wait_owned_tree_gone(process, descendants, grace):
        return
    if process.poll() is None:
        descendants.update(descendants_of(process.pid))
    signal_processes(descendants, signal.SIGTERM)
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    if wait_owned_tree_gone(process, descendants, grace):
        return
    if process.poll() is None:
        descendants.update(descendants_of(process.pid))
    signal_processes(descendants, signal.SIGKILL)
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    if not wait_owned_tree_gone(process, descendants, kill):
        raise DriverError("test-sync process tree could not be terminated and reaped")


def process_matches(pid: int, required: tuple[str, ...]) -> bool:
    try:
        command = subprocess.check_output(
            ["/bin/ps", "-p", str(pid), "-o", "command="],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return False
    return all(fragment in command for fragment in required)


def stop_controller(runtime_dir: Path, timeout: float) -> None:
    try:
        state = load_json(runtime_paths(runtime_dir)["state"])
    except (FileNotFoundError, DriverError, json.JSONDecodeError):
        return
    controller_pid = state.get("controller_pid")
    tunnel_pid = state.get("tunnel_pid")
    try:
        control_request(runtime_dir, "shutdown", timeout)
    except Exception:
        pass
    if isinstance(controller_pid, int) and controller_pid > 0:
        try:
            wait_pid_gone(controller_pid, timeout)
        except DriverError:
            if process_matches(controller_pid, (str(Path(__file__).resolve()), str(runtime_dir))):
                os.kill(controller_pid, signal.SIGTERM)
                try:
                    wait_pid_gone(controller_pid, timeout)
                except DriverError:
                    if process_matches(
                        controller_pid,
                        (str(Path(__file__).resolve()), str(runtime_dir)),
                    ):
                        os.kill(controller_pid, signal.SIGKILL)
                        wait_pid_gone(controller_pid, timeout)
    if isinstance(tunnel_pid, int) and tunnel_pid > 0 and pid_alive(tunnel_pid):
        config = read_config(runtime_dir)
        if process_matches(tunnel_pid, (config["ssh_binary"], config["ssh_host"])):
            try:
                os.killpg(tunnel_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                wait_pid_gone(tunnel_pid, timeout)
            except DriverError:
                if process_matches(tunnel_pid, (config["ssh_binary"], config["ssh_host"])):
                    os.killpg(tunnel_pid, signal.SIGKILL)
                    wait_pid_gone(tunnel_pid, timeout)


def write_sidecar_evidence(runtime_dir: Path, destination: Path) -> Path:
    if destination.exists():
        raise DriverError(f"connection evidence already exists: {destination}")
    destination.mkdir(mode=0o700, parents=True)
    paths = runtime_paths(runtime_dir)
    copied: list[Path] = []
    for name in ("audit", "state", "log"):
        source = paths[name]
        if source.is_file():
            target = destination / source.name
            shutil.copyfile(source, target)
            target.chmod(0o600)
            copied.append(target)
    checksums = destination / "SHA256SUMS"
    with checksums.open("x", encoding="ascii") as output:
        for path in sorted(copied):
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            output.write(f"{digest}  {path.name}\n")
        output.flush()
        os.fsync(output.fileno())
    checksums.chmod(0o600)
    return destination


def validate_run_arguments(arguments: argparse.Namespace) -> None:
    if arguments.token_fd < 3:
        raise DriverError("JWT pipe descriptor must be 3 or greater")
    if (arguments.local_port_a is None) != (arguments.local_port_b is None):
        raise DriverError("both explicit local ports must be provided together")
    for port in (
        arguments.local_port_a,
        arguments.local_port_b,
        arguments.ssh_port,
        arguments.remote_port,
    ):
        if port is not None and not 1 <= port <= 65535:
            raise DriverError("TCP ports must be between 1 and 65535")
    if (
        arguments.local_port_a is not None
        and arguments.local_port_a == arguments.local_port_b
    ):
        raise DriverError("both explicit local ports must be distinct")
    positive = (
        arguments.ssh_connect_timeout_seconds,
        arguments.connection_timeout_seconds,
        arguments.agent_startup_timeout_seconds,
        arguments.checkpoint_timeout_seconds,
        arguments.sample_interval_millis,
        arguments.hook_timeout_seconds,
        arguments.term_grace_seconds,
        arguments.kill_timeout_seconds,
        arguments.run_timeout_seconds,
        arguments.large_file_bytes,
        arguments.max_active_transfers,
    )
    if any(value <= 0 for value in positive):
        raise DriverError("all timeout, size, and transfer values must be positive")
    minimum_hook_timeout = (
        2 * arguments.connection_timeout_seconds
        + arguments.term_grace_seconds
        + 2 * arguments.kill_timeout_seconds
        + 5
    )
    if arguments.hook_timeout_seconds < minimum_hook_timeout:
        raise DriverError(
            "hook timeout is too short for bounded controller replacement and cleanup"
        )
    if arguments.run_timeout_seconds <= arguments.hook_timeout_seconds:
        raise DriverError("complete run timeout must exceed the hook timeout")


def run_harness(arguments: argparse.Namespace) -> int:
    if not arguments.run_id or len(arguments.run_id) > 80 or not all(
        character.isascii() and (character.isalnum() or character in "-_")
        for character in arguments.run_id
    ):
        raise DriverError("run ID must be an ASCII slug")
    validate_run_arguments(arguments)
    os.fstat(arguments.token_fd)  # Validate only; never read the JWT descriptor.
    evidence_root = Path(arguments.evidence_root).expanduser().resolve()
    if (evidence_root / arguments.run_id).exists():
        raise DriverError("test-sync evidence directory already exists")
    sidecar_root = Path(arguments.connection_evidence_root).expanduser().resolve()
    sidecar_destination = sidecar_root / arguments.run_id
    if sidecar_destination.exists():
        raise DriverError("connection evidence directory already exists")

    requested_ports: tuple[int, int] | None = None
    if arguments.local_port_a is not None:
        if arguments.local_port_b is None:
            raise DriverError("both explicit local ports must be provided together")
        requested_ports = (arguments.local_port_a, arguments.local_port_b)
    ports, port_reservations = reserve_local_ports("127.0.0.1", requested_ports)
    runtime_dir = Path(tempfile.mkdtemp(prefix="fns-e2e-ssh-", dir="/tmp")).resolve()
    runtime_dir.chmod(0o700)
    initial_controller: subprocess.Popen[bytes] | None = None
    child: subprocess.Popen[bytes] | None = None
    exit_code = 1
    previous_handlers = install_interrupt_handlers()
    try:
        write_runtime_config(arguments, runtime_dir, ports)
        driver = Path(__file__).resolve(strict=True)
        wrappers = {
            "reconnect": runtime_dir / "reconnect-hook.sh",
            "app_restart": runtime_dir / "app-restart-hook.sh",
            "observer": runtime_dir / "effect-observer.sh",
        }
        write_wrapper(wrappers["reconnect"], driver, runtime_dir, "effect", "reconnect")
        write_wrapper(wrappers["app_restart"], driver, runtime_dir, "effect", "app_restart")
        write_wrapper(wrappers["observer"], driver, runtime_dir, "observe", None)
        for reservation in port_reservations:
            reservation.close()
        port_reservations.clear()
        initial_controller = spawn_controller(runtime_dir, 1)
        wait_live_state(runtime_dir, arguments.connection_timeout_seconds, 1)
        command = test_sync_command(arguments, ports, wrappers)
        child = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=None,
            stderr=None,
            pass_fds=(arguments.token_fd,),
            close_fds=True,
            start_new_session=True,
        )
        try:
            exit_code = child.wait(timeout=arguments.run_timeout_seconds)
        except subprocess.TimeoutExpired:
            bounded_child_shutdown(
                child, arguments.term_grace_seconds, arguments.kill_timeout_seconds
            )
            raise DriverError("test-sync exceeded the complete run timeout")
        if exit_code != 0:
            raise DriverError(f"test-sync exited with status {exit_code}")
        return 0
    finally:
        ignore_interrupt_signals()
        cleanup_errors: list[str] = []
        for reservation in port_reservations:
            reservation.close()
        if child is not None and child.poll() is None:
            try:
                bounded_child_shutdown(
                    child, arguments.term_grace_seconds, arguments.kill_timeout_seconds
                )
            except Exception as error:
                cleanup_errors.append(f"test-sync cleanup: {error}")
        try:
            stop_controller(runtime_dir, arguments.connection_timeout_seconds)
        except Exception as error:
            cleanup_errors.append(f"controller cleanup: {error}")
        if initial_controller is not None:
            try:
                initial_controller.wait(timeout=arguments.kill_timeout_seconds)
            except subprocess.TimeoutExpired:
                try:
                    os.kill(initial_controller.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    initial_controller.wait(timeout=arguments.term_grace_seconds)
                except subprocess.TimeoutExpired:
                    try:
                        os.kill(initial_controller.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    try:
                        initial_controller.wait(timeout=arguments.kill_timeout_seconds)
                    except subprocess.TimeoutExpired as error:
                        cleanup_errors.append(f"initial controller reap: {error}")
                try:
                    stop_controller(runtime_dir, arguments.connection_timeout_seconds)
                except Exception as error:
                    cleanup_errors.append(f"post-kill controller cleanup: {error}")
        try:
            sidecar_root.mkdir(mode=0o700, parents=True, exist_ok=True)
            if not sidecar_destination.exists():
                evidence = write_sidecar_evidence(runtime_dir, sidecar_destination)
                print(f"connection evidence: {evidence}", file=sys.stderr)
        except Exception as error:
            cleanup_errors.append(f"connection evidence: {error}")
        if not cleanup_errors:
            try:
                shutil.rmtree(runtime_dir)
            except Exception as error:
                cleanup_errors.append(f"runtime cleanup: {error}")
        restore_signal_handlers(previous_handlers)
        if cleanup_errors:
            raise DriverError("; ".join(cleanup_errors))


def add_effect_arguments(parser: argparse.ArgumentParser, observer: bool) -> None:
    parser.add_argument("--runtime-dir", required=True, type=Path)
    parser.add_argument("--allowed-action", choices=("reconnect", "app_restart"))
    parser.add_argument("--action", required=True, choices=("reconnect", "app_restart"))
    if observer:
        parser.add_argument("--phase", required=True, choices=("before", "after"))
    parser.add_argument("--workspace-id", required=True)
    parser.add_argument("--client-id-a", required=True)
    parser.add_argument("--client-id-b", required=True)
    parser.add_argument("--agent-pid-a", required=True, type=int)
    parser.add_argument("--agent-pid-b", required=True, type=int)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    controller = commands.add_parser("controller", help="internal connection controller")
    controller.add_argument("--runtime-dir", required=True, type=Path)
    controller.add_argument("--generation", required=True, type=int)

    observe = commands.add_parser("observe", help="internal effect observer")
    add_effect_arguments(observe, observer=True)

    effect = commands.add_parser("effect", help="internal reconnect/restart hook")
    add_effect_arguments(effect, observer=False)

    stop = commands.add_parser("stop", help="stop an owned controller")
    stop.add_argument("--runtime-dir", required=True, type=Path)
    stop.add_argument("--timeout-seconds", type=float, default=10.0)

    run = commands.add_parser("run", help="run the real-service E2E matrix")
    run.add_argument("--test-sync", required=True)
    run.add_argument("--agent-binary", required=True)
    run.add_argument("--workspace-id", required=True)
    run.add_argument("--client-id-a", required=True)
    run.add_argument("--client-id-b", required=True)
    run.add_argument("--root-a", required=True)
    run.add_argument("--root-b", required=True)
    run.add_argument("--state-a", required=True)
    run.add_argument("--state-b", required=True)
    run.add_argument("--run-id", required=True)
    run.add_argument("--token-fd", required=True, type=int)
    run.add_argument("--ssh-host", required=True)
    run.add_argument("--ssh-port", type=int, default=22)
    run.add_argument("--ssh-binary", default="/usr/bin/ssh")
    default_ssh_config = Path.home() / ".ssh/config"
    run.add_argument(
        "--ssh-config", default=str(default_ssh_config) if default_ssh_config.is_file() else None
    )
    run.add_argument("--identity-file")
    run.add_argument("--remote-host", default="127.0.0.1")
    run.add_argument("--remote-port", type=int, default=9000)
    run.add_argument("--local-port-a", type=int)
    run.add_argument("--local-port-b", type=int)
    run.add_argument("--ssh-connect-timeout-seconds", type=float, default=10.0)
    run.add_argument("--connection-timeout-seconds", type=float, default=30.0)
    run.add_argument("--agent-startup-timeout-seconds", type=int, default=30)
    run.add_argument("--checkpoint-timeout-seconds", type=int, default=120)
    run.add_argument("--sample-interval-millis", type=int, default=250)
    run.add_argument("--hook-timeout-seconds", type=int, default=90)
    run.add_argument("--term-grace-seconds", type=int, default=3)
    run.add_argument("--kill-timeout-seconds", type=int, default=3)
    run.add_argument("--run-timeout-seconds", type=float, default=1800.0)
    run.add_argument("--large-file-bytes", type=int, default=33_554_432)
    run.add_argument("--max-active-transfers", type=int, default=2)
    repository = Path(__file__).resolve().parents[2]
    run.add_argument("--evidence-root", default=str(repository / "target/e2e-evidence"))
    run.add_argument(
        "--connection-evidence-root",
        default=str(repository / "target/e2e-connection-evidence"),
    )
    return parser


def main() -> int:
    raw_arguments = list(sys.argv[1:])
    if EMBEDDED_COMMAND is not None:
        if EMBEDDED_RUNTIME_DIR is None:
            print("controlled SSH E2E failed: embedded runtime is missing", file=sys.stderr)
            return 1
        prefix = [EMBEDDED_COMMAND, "--runtime-dir", EMBEDDED_RUNTIME_DIR]
        if EMBEDDED_ALLOWED_ACTION is not None:
            prefix.extend(["--allowed-action", EMBEDDED_ALLOWED_ACTION])
        raw_arguments = [*prefix, *raw_arguments]
    arguments = build_parser().parse_args(raw_arguments)
    try:
        if arguments.command == "controller":
            Controller(arguments.runtime_dir, arguments.generation).run()
        elif arguments.command == "observe":
            if arguments.allowed_action is None:
                arguments.allowed_action = arguments.action
            run_observer(arguments)
        elif arguments.command == "effect":
            if arguments.allowed_action is None:
                raise DriverError("effect hook must pin an allowed action")
            run_effect(arguments)
        elif arguments.command == "stop":
            stop_controller(arguments.runtime_dir, arguments.timeout_seconds)
        elif arguments.command == "run":
            return run_harness(arguments)
        else:
            raise DriverError("unknown driver command")
        return 0
    except (DriverError, OSError, ValueError, json.JSONDecodeError) as error:
        print(f"controlled SSH E2E failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
