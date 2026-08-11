#!/usr/bin/env python3
"""One-command official test-server E2E against vps-108-80-81-15.

1. Optionally provision a brand-new clean remote workspace (default: yes).
2. Pass JWT only via an anonymous pipe FD into controlled_ssh_e2e.
3. Run the dual-agent matrix with empty local roots/states.

Token sources (first match wins):
  --token-file PATH
  env FNS_TEST_JWT_FILE
  remote path via --remote-token-file (scp to a private temp file)

Never prints the JWT.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path


REPO_CLIENT = Path(__file__).resolve().parents[2]
DEFAULT_SSH_HOST = "vps-108-80-81-15"
DEFAULT_REMOTE_TOKEN = (
    "/root/.config/fns-workspace/agent-c31e5757-4a04-4dce-8147-0e1881ae5278.token"
)


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, text=True, capture_output=True, **kwargs)


def provision(ssh_host: str) -> str:
    script = Path(__file__).with_name("provision_clean_workspace.py")
    completed = run(
        [sys.executable, str(script), "--ssh-host", ssh_host],
        check=False,
    )
    sys.stdout.write(completed.stdout)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        raise SystemExit(completed.returncode)
    for line in completed.stdout.splitlines():
        if line.startswith("PROVISIONED_WORKSPACE_ID="):
            return line.split("=", 1)[1].strip()
    raise SystemExit("provision script did not emit PROVISIONED_WORKSPACE_ID")


def mint_fresh_token(ssh_host: str) -> None:
    """Refresh the remote agent JWT so restarts do not leave a revoked token."""
    mint = Path(__file__).with_name("mint_agent_token.py")
    completed = run([sys.executable, str(mint), "--ssh-host", ssh_host], check=False)
    sys.stdout.write(completed.stdout)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        raise SystemExit(f"mint_agent_token failed: {completed.returncode}")


def load_token_bytes(arguments: argparse.Namespace) -> bytes:
    if arguments.token_file:
        path = Path(arguments.token_file).expanduser()
        data = path.read_bytes().strip()
    elif os.environ.get("FNS_TEST_JWT_FILE"):
        path = Path(os.environ["FNS_TEST_JWT_FILE"]).expanduser()
        data = path.read_bytes().strip()
    else:
        if not arguments.no_mint:
            print(f"minting fresh agent JWT on {arguments.ssh_host} ...", flush=True)
            mint_fresh_token(arguments.ssh_host)
        temporary = Path(tempfile.mkdtemp(prefix="fns-jwt-")) / "token"
        temporary.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        scp = run(
            [
                "scp",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=15",
                f"{arguments.ssh_host}:{arguments.remote_token_file}",
                str(temporary),
            ]
        )
        if scp.returncode != 0:
            sys.stderr.write(scp.stderr)
            raise SystemExit(f"failed to scp remote token: {scp.returncode}")
        os.chmod(temporary, 0o600)
        data = temporary.read_bytes().strip()
        # Best-effort cleanup of token material from disk after read.
        try:
            temporary.unlink()
            temporary.parent.rmdir()
        except OSError:
            pass
    if data.count(b".") != 2:
        raise SystemExit("token does not look like a JWT (expected 3 segments)")
    return data


def ensure_empty_dir(path: Path) -> None:
    if path.exists():
        if any(path.iterdir()):
            raise SystemExit(f"directory must be empty: {path}")
    else:
        path.mkdir(parents=True, mode=0o700)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh-host", default=DEFAULT_SSH_HOST)
    parser.add_argument("--test-sync", default=str(REPO_CLIENT / "target/debug/test-sync"))
    parser.add_argument("--agent-binary", default=str(REPO_CLIENT / "target/debug/fns-agent"))
    parser.add_argument("--workspace-id", help="use existing workspace (skips provision)")
    parser.add_argument(
        "--no-provision",
        action="store_true",
        help="do not auto-create a clean remote workspace",
    )
    parser.add_argument("--token-file")
    parser.add_argument("--remote-token-file", default=DEFAULT_REMOTE_TOKEN)
    parser.add_argument(
        "--no-mint",
        action="store_true",
        help="do not mint a fresh agent JWT before the run",
    )
    parser.add_argument("--run-id")
    parser.add_argument("--work-root", type=Path)
    parser.add_argument(
        "--evidence-root",
        default=str(REPO_CLIENT / "target/e2e-evidence"),
    )
    arguments = parser.parse_args()

    test_sync = Path(arguments.test_sync).resolve()
    agent = Path(arguments.agent_binary).resolve()
    if not test_sync.is_file() or not os.access(test_sync, os.X_OK):
        raise SystemExit(f"test-sync binary missing/unexecutable: {test_sync}")
    if not agent.is_file() or not os.access(agent, os.X_OK):
        raise SystemExit(f"fns-agent binary missing/unexecutable: {agent}")

    if arguments.workspace_id:
        workspace_id = arguments.workspace_id
    elif arguments.no_provision:
        raise SystemExit("--no-provision requires --workspace-id")
    else:
        print(f"provisioning clean workspace on {arguments.ssh_host} ...", flush=True)
        workspace_id = provision(arguments.ssh_host)

    if not re.fullmatch(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        workspace_id,
    ):
        raise SystemExit("workspace-id must be a UUID")

    run_id = arguments.run_id or f"official-{uuid.uuid4().hex[:12]}"
    work_root = (
        arguments.work_root.expanduser().resolve()
        if arguments.work_root
        else Path(tempfile.mkdtemp(prefix=f"fns-e2e-{run_id}-")).resolve()
    )
    roots = {
        "root_a": work_root / "root-a",
        "root_b": work_root / "root-b",
        "state_a": work_root / "state-a",
        "state_b": work_root / "state-b",
        "evidence": Path(arguments.evidence_root).expanduser().resolve(),
    }
    for key in ("root_a", "root_b", "state_a", "state_b"):
        ensure_empty_dir(roots[key])
    roots["evidence"].mkdir(parents=True, exist_ok=True)

    client_a = str(uuid.uuid4())
    client_b = str(uuid.uuid4())
    token = load_token_bytes(arguments)

    r_fd, w_fd = os.pipe()
    try:
        os.write(w_fd, token)
    finally:
        os.close(w_fd)

    driver = Path(__file__).with_name("controlled_ssh_e2e.py")
    cmd = [
        sys.executable,
        str(driver),
        "run",
        "--test-sync",
        str(test_sync),
        "--agent-binary",
        str(agent),
        "--workspace-id",
        workspace_id,
        "--client-id-a",
        client_a,
        "--client-id-b",
        client_b,
        "--root-a",
        str(roots["root_a"]),
        "--root-b",
        str(roots["root_b"]),
        "--state-a",
        str(roots["state_a"]),
        "--state-b",
        str(roots["state_b"]),
        "--run-id",
        run_id,
        "--evidence-root",
        str(roots["evidence"]),
        "--token-fd",
        "3",
        "--ssh-host",
        arguments.ssh_host,
        "--remote-host",
        "127.0.0.1",
        "--remote-port",
        "9000",
    ]
    print(
        f"running official e2e run_id={run_id} workspace_id={workspace_id} work_root={work_root}",
        flush=True,
    )
    os.dup2(r_fd, 3)
    os.close(r_fd)
    completed = subprocess.run(cmd, pass_fds=(3,))
    return completed.returncode


if __name__ == "__main__":
    # Avoid keeping JWT bytes longer than needed in locals after fork.
    raise SystemExit(main())
