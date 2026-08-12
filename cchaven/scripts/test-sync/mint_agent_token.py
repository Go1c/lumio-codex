#!/usr/bin/env python3
"""Mint a fresh fns-agent WS token on the official test server and install it.

Uses WebGUI login over SSH LocalForward or remote localhost curl, then
POST /api/token with protocol/client/function dimensions (not a free-form
scope string that can double-prefix to p:p:ws).

Writes token to:
  remote: /root/.config/fns-workspace/agent-c31e5757-4a04-4dce-8147-0e1881ae5278.token
  optional local --out path

Never prints the JWT body.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REMOTE_INSTALL = r'''
set -euo pipefail
PASS="$1"
OUT="$2"
BASE="http://127.0.0.1:9000"
LOGIN=$(curl -fsS -m 10 -X POST "$BASE/api/user/login" \
  -H 'Content-Type: application/json' \
  -H 'X-Client: webgui' \
  -H 'X-Client-Name: FNS-WebGUI' \
  -d "{\"credentials\":\"admin\",\"password\":$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$PASS")}")
WEB_TOKEN=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); t=(d.get("data") or {}).get("token") or "";
assert t, d; print(t)' "$LOGIN")
CREATE=$(curl -fsS -m 10 -X POST "$BASE/api/token" \
  -H "Authorization: Bearer $WEB_TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'X-Client: webgui' \
  -H 'X-Client-Name: FNS-WebGUI' \
  -d '{"clientType":"fns-agent","protocol":"ws","client":"fns-agent","function":"workspace_rw","expiredDays":30}')
python3 - <<'PY' "$CREATE" "$OUT"
import json,sys
from pathlib import Path
d=json.loads(sys.argv[1])
data=d.get("data") or {}
token=data.get("token") or ""
scope=data.get("scope") or ""
if not token:
    raise SystemExit(f"mint failed: {d}")
if not scope.startswith("p:ws"):
    raise SystemExit(f"unexpected scope: {scope}")
if scope.startswith("p:p:"):
    raise SystemExit(f"double-prefixed scope: {scope}")
path=Path(sys.argv[2])
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(token)
path.chmod(0o600)
print(f"TOKEN_SCOPE={scope}")
print(f"TOKEN_LEN={len(token)}")
print(f"TOKEN_PATH={path}")
PY
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh-host", default="vps-108-80-81-15")
    parser.add_argument(
        "--admin-password",
        default="",
        help="defaults to env FNS_TEST_ADMIN_PASSWORD or the known test-server password",
    )
    parser.add_argument(
        "--remote-out",
        default="/root/.config/fns-workspace/agent-c31e5757-4a04-4dce-8147-0e1881ae5278.token",
    )
    parser.add_argument("--out", type=Path, help="also copy token to this local path")
    args = parser.parse_args()

    import os
    import shlex

    password = (
        args.admin_password
        or os.environ.get("FNS_TEST_ADMIN_PASSWORD")
        or "FnsTest!2026-Wave0"
    )

    def try_mint() -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=15",
                args.ssh_host,
                "bash",
                "-s",
                "--",
                password,
                args.remote_out,
            ],
            input=REMOTE_INSTALL,
            text=True,
            capture_output=True,
        )

    completed = try_mint()
    if completed.returncode != 0:
        # Password may have drifted on the test host — reset then retry once.
        reset = subprocess.run(
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=15",
                args.ssh_host,
                "bash",
                "-lc",
                "cd /root/fns-deploy && /root/.local/share/fns-workspace/current/fns-server "
                f"reset-password -c /root/.config/fns-workspace/server.yaml -u admin -p {shlex.quote(password)}",
            ],
            text=True,
            capture_output=True,
        )
        if reset.returncode != 0:
            sys.stderr.write(reset.stderr or reset.stdout)
        completed = try_mint()
    sys.stdout.write(completed.stdout)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        return completed.returncode

    if args.out:
        scp = subprocess.run(
            [
                "scp",
                "-o",
                "BatchMode=yes",
                f"{args.ssh_host}:{args.remote_out}",
                str(args.out),
            ],
            text=True,
            capture_output=True,
        )
        if scp.returncode != 0:
            sys.stderr.write(scp.stderr)
            return scp.returncode
        args.out.chmod(0o600)
        print(f"LOCAL_OUT={args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
