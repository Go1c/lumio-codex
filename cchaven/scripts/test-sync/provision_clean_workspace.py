#!/usr/bin/env python3
"""Provision an isolated clean workspace root on the official test server.

Creates:
  - workspace-id: 80000000-0000-4000-8000-<12 hex>
  - remote root:  /root/fns-selftest-<12 hex>
  - appends to ~/.config/fns-workspace/server.yaml roots (uid=1)
  - restarts a single fns-server instance listening on 127.0.0.1:9000

Does NOT touch /root/my-workspace or other production-ish roots.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import uuid


REMOTE_SCRIPT = r'''
set -euo pipefail
WS_ID="$1"
SUFFIX="$2"
ROOT="/root/fns-selftest-${SUFFIX}"
CFG="/root/.config/fns-workspace/server.yaml"
BIN="/root/.local/share/fns-workspace/current/fns-server"
PIDFILE="/root/fns-test-state/fns-server.pid"
LOG="/root/fns-test-state/fns-server-selftest.log"
# Permanent roots kept across provisions (cap is max-workspaces-per-user: 8).
KEEP_IDS="10000000-0000-4000-8000-000000000002 b24e6291-f409-4e2f-8f36-4e72cc17e620 80000000-0000-4000-8000-202608100001"
mkdir -p /root/fns-test-state "$ROOT"
chmod 700 "$ROOT"
find "$ROOT" -mindepth 1 -delete 2>/dev/null || true
cp -a "$CFG" "$CFG.bak.$(date +%s)"

# Prune ephemeral selftest roots so uid=1 stays under max-workspaces-per-user.
python3 - <<'PY' "$CFG" "$WS_ID" "$ROOT" "$KEEP_IDS"
from pathlib import Path
import re, shutil, sys

cfg_path = Path(sys.argv[1])
ws, root = sys.argv[2], sys.argv[3]
keep_ids = set(sys.argv[4].split())
keep_ids.add(ws)

text = cfg_path.read_text()
# Match root entries: "  - uid: N\n    workspace-id: ...\n    root: ...\n"
pattern = re.compile(
    r"(^[ \t]*- uid:\s*\d+\n[ \t]*workspace-id:\s*([0-9a-fA-F-]{36})\n[ \t]*root:\s*(.+)\n)",
    re.M,
)
blocks = list(pattern.finditer(text))
if not blocks:
    raise SystemExit("no workspace root entries found in server.yaml")

kept_blocks = []
pruned_roots = []
for m in blocks:
    full, wid, rpath = m.group(1), m.group(2).strip(), m.group(3).strip()
    is_ephemeral = rpath.startswith("/root/fns-selftest-") and not rpath.endswith("/fns-selftest-clean")
    if wid in keep_ids or not is_ephemeral:
        kept_blocks.append(full)
    else:
        pruned_roots.append(rpath)

# Always ensure current WS is present once.
has_current = any(ws in b for b in kept_blocks)
if not has_current:
    kept_blocks.append(f"  - uid: 1\n    workspace-id: {ws}\n    root: {root}\n")
    print(f"registered {ws} -> {root}")
else:
    print(f"workspace already registered: {ws}")

# Rebuild: replace first-through-last root block span with kept list.
start = blocks[0].start()
end = blocks[-1].end()
new_text = text[:start] + "".join(kept_blocks) + text[end:]
cfg_path.write_text(new_text)

# Remove orphaned ephemeral directories (best-effort).
for rpath in pruned_roots:
    p = Path(rpath)
    if p.is_dir() and str(p).startswith("/root/fns-selftest-") and p.name != "fns-selftest-clean":
        shutil.rmtree(p, ignore_errors=True)
        print(f"pruned dir {rpath}")
print(f"workspace_roots_kept={len(kept_blocks)} pruned={len(pruned_roots)}")
# Hard cap check (server max-workspaces-per-user is 8 for uid=1).
uid1 = sum(1 for b in kept_blocks if re.search(r"- uid:\s*1\b", b))
if uid1 > 8:
    raise SystemExit(f"uid=1 still has {uid1} roots after prune (max 8)")
PY

# Stop every fns-server "run" process (avoid dual listeners / orphans).
python3 - <<'PY'
import os, signal, time
target = "/root/.local/share/fns-workspace/current/fns-server"
for _round, sig in ((1, signal.SIGTERM), (2, signal.SIGKILL)):
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
            continue
        try:
            cmd = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
        except Exception:
            continue
        if target in cmd and " run " in f" {cmd} ":
            try:
                os.kill(int(pid), sig)
            except ProcessLookupError:
                pass
    time.sleep(1.0 if _round == 1 else 0.5)
# Ensure port is free.
import subprocess
subprocess.run(["fuser", "-k", "9000/tcp"], check=False, capture_output=True)
time.sleep(0.4)
print("stopped previous fns-server instances")
PY

# Relative sqlite paths in server.yaml resolve against process cwd. The
# official deploy always uses /root/fns-deploy (admin user DB lives there).
WORKDIR="/root/fns-deploy"
if [ ! -d "$WORKDIR" ]; then
  echo "missing workdir $WORKDIR" >&2
  exit 1
fi
mkdir -p "$WORKDIR/config"
cd "$WORKDIR"
: >"$LOG"
nohup "$BIN" run --config "$CFG" >>"$LOG" 2>&1 &
echo $! >"$PIDFILE"

# Health probe with retries (cold start can exceed 2s).
python3 - <<'PY'
import json, os, time, urllib.request
from pathlib import Path

url = "http://127.0.0.1:9000/api/health"
last_err = None
for attempt in range(1, 21):
    try:
        with urllib.request.urlopen(url, timeout=2) as resp:
            body = resp.read().decode()
        d = json.loads(body)
        if d.get("code") == 1 and (d.get("data") or {}).get("status") == "healthy":
            Path("/tmp/fns-health.json").write_text(body)
            print("health ok", (d.get("data") or {}).get("version"), f"attempt={attempt}")
            break
        last_err = f"unexpected health payload: {d}"
    except Exception as e:
        last_err = str(e)
    time.sleep(0.5)
else:
    log_tail = Path("/root/fns-test-state/fns-server-selftest.log").read_text()[-2000:]
    raise SystemExit(f"health failed after retries: {last_err}\n--- log ---\n{log_tail}")

# Count "run" processes; binary may leave 1-2 PIDs (main + worker). Require:
# all cwd == /root/fns-deploy, and exactly one listener on 127.0.0.1:9000.
pids = []
owners = set()
for pid in os.listdir("/proc"):
    if not pid.isdigit():
        continue
    try:
        cmd = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode(errors="replace")
    except Exception:
        continue
    if "/fns-server" in cmd and " run " in f" {cmd} ":
        try:
            owner = os.readlink(f"/proc/{pid}/cwd")
        except Exception:
            owner = "?"
        pids.append((pid, owner))
        owners.add(owner)
        print(f"server pid={pid} cwd={owner}")
print(f"server_count={len(pids)}")
if not pids:
    raise SystemExit("no fns-server run process found after start")
if owners != {"/root/fns-deploy"}:
    raise SystemExit(f"fns-server cwd must be /root/fns-deploy, got {owners}")

# Listener check via /proc/net/tcp (IPv4 127.0.0.1:9000 = 0100007F:2328)
listen_inode_owners = 0
port_hex = f"{9000:04X}"
try:
    for line in Path("/proc/net/tcp").read_text().splitlines()[1:]:
        parts = line.split()
        if len(parts) < 10:
            continue
        local, state = parts[1], parts[3]
        if local.endswith(":" + port_hex) and state == "0A":  # LISTEN
            # Confirm local is 127.0.0.1
            ip_hex = local.split(":")[0]
            if ip_hex.upper() == "0100007F":
                listen_inode_owners += 1
except Exception as e:
    print(f"warn: listener probe failed: {e}")
    listen_inode_owners = -1
print(f"listeners_9000={listen_inode_owners}")
if listen_inode_owners == 0:
    raise SystemExit("no listener on 127.0.0.1:9000")
if listen_inode_owners > 1:
    raise SystemExit(f"expected one listener on 9000, found {listen_inode_owners}")
PY
echo "WORKSPACE_ID=${WS_ID}"
echo "REMOTE_ROOT=${ROOT}"
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ssh-host", default="vps-108-80-81-15")
    parser.add_argument("--ssh-binary", default="/usr/bin/ssh")
    parser.add_argument("--workspace-id")
    args = parser.parse_args()

    if args.workspace_id:
        workspace_id = args.workspace_id
        if not re.fullmatch(
            r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
            workspace_id,
        ):
            print("invalid workspace-id UUID", file=sys.stderr)
            return 2
        suffix = workspace_id.replace("-", "")[-12:]
    else:
        suffix = uuid.uuid4().hex[:12]
        workspace_id = f"80000000-0000-4000-8000-{suffix}"

    cmd = [
        args.ssh_binary,
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        args.ssh_host,
        "bash",
        "-s",
        "--",
        workspace_id,
        suffix,
    ]
    completed = subprocess.run(cmd, input=REMOTE_SCRIPT, text=True, capture_output=True)
    sys.stdout.write(completed.stdout)
    if completed.returncode != 0:
        sys.stderr.write(completed.stderr)
        return completed.returncode
    # Machine-readable last lines for wrappers.
    print(f"PROVISIONED_WORKSPACE_ID={workspace_id}")
    print(f"PROVISIONED_REMOTE_ROOT=/root/fns-selftest-{suffix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
