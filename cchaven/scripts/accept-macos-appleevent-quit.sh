#!/bin/sh
set -eu

usage() {
  printf '%s\n' "usage: FNS_ACCEPTANCE_SSH_HOST_ALIAS=your-ssh-host $0 /absolute/path/FNS\ Workspace.app /absolute/path/runtime-status.json" >&2
  exit 64
}

[ "$#" -eq 2 ] || usage
ssh_host_alias=${FNS_ACCEPTANCE_SSH_HOST_ALIAS:-}
[ -n "$ssh_host_alias" ] || usage
app=$1
runtime_status=$2
desktop="$app/Contents/MacOS/fns-workspace-desktop"
worker="$app/Contents/MacOS/fns-agent"
startup_timeout=${FNS_ACCEPTANCE_STARTUP_TIMEOUT_SECONDS:-120}
shutdown_timeout=${FNS_ACCEPTANCE_SHUTDOWN_TIMEOUT_SECONDS:-45}

case "$app" in
  /*) ;;
  *) usage ;;
esac
case "$runtime_status" in
  /*) ;;
  *) usage ;;
esac
[ -x "$desktop" ] || { printf 'desktop executable missing: %s\n' "$desktop" >&2; exit 1; }
[ -x "$worker" ] || { printf 'worker executable missing: %s\n' "$worker" >&2; exit 1; }

matching_pids() {
  needle=$1
  /bin/ps -ww -axo pid=,command= | /usr/bin/awk -v needle="$needle" '
    {
      pid = $1
      line = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", line)
      if (index(line, needle) == 1) print pid
    }
  '
}

tunnel_pids() {
  /bin/ps -ww -axo pid=,command= | /usr/bin/awk -v host="$ssh_host_alias" '
    {
      pid = $1
      line = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", line)
      if (line ~ /^([^[:space:]]*\/)?ssh[[:space:]]/ \
          && index(line, host) \
          && (index(line, ":9000") || index(line, "ControlMaster=yes"))) print pid
    }
  '
}

has_matching_pid() {
  [ -n "$(matching_pids "$1")" ]
}

has_tunnel() {
  [ -n "$(tunnel_pids)" ]
}

terminate_pid_list() {
  signal=$1
  shift
  for pid in "$@"; do
    case "$pid" in
      ''|*[!0-9]*|0|1) continue ;;
    esac
    /bin/kill -"$signal" "$pid" 2>/dev/null || true
  done
}

cleanup() {
  desktop_pids=$(matching_pids "$desktop")
  worker_pids=$(matching_pids "$worker")
  ssh_pids=$(tunnel_pids)
  # shellcheck disable=SC2086
  terminate_pid_list TERM $desktop_pids $worker_pids $ssh_pids
  sleep 1
  desktop_pids=$(matching_pids "$desktop")
  worker_pids=$(matching_pids "$worker")
  ssh_pids=$(tunnel_pids)
  # shellcheck disable=SC2086
  terminate_pid_list KILL $desktop_pids $worker_pids $ssh_pids
}
trap cleanup EXIT HUP INT TERM

if has_matching_pid "$desktop" || has_matching_pid "$worker" || has_tunnel; then
  printf '%s\n' 'preflight failed: an acceptance App, Worker, or SSH tunnel is already running' >&2
  exit 1
fi

printf 'started_at_utc=%s\n' "$(date -u +%FT%TZ)"
printf 'app=%s\n' "$app"
printf 'runtime_status=%s\n' "$runtime_status"
printf 'desktop_sha256='; /usr/bin/shasum -a 256 "$desktop" | /usr/bin/awk '{ print $1 }'
printf 'worker_sha256='; /usr/bin/shasum -a 256 "$worker" | /usr/bin/awk '{ print $1 }'

/usr/bin/open -n "$app"
deadline=$(( $(date +%s) + startup_timeout ))
while :; do
  if has_matching_pid "$desktop" \
    && has_matching_pid "$worker" \
    && has_tunnel \
    && [ -f "$runtime_status" ] \
    && /usr/bin/grep -Eq '"running"[[:space:]]*:[[:space:]]*true' "$runtime_status" \
    && /usr/bin/grep -Eq '"phase"[[:space:]]*:[[:space:]]*"online"' "$runtime_status" \
    && /usr/bin/grep -Eq '"connected"[[:space:]]*:[[:space:]]*true' "$runtime_status" \
    && /usr/bin/grep -Eq '"pendingCommands"[[:space:]]*:[[:space:]]*0' "$runtime_status" \
    && /usr/bin/grep -Eq '"queuedWatcherBatches"[[:space:]]*:[[:space:]]*0' "$runtime_status" \
    && /usr/bin/grep -Eq '"activeTransfers"[[:space:]]*:[[:space:]]*0' "$runtime_status"
  then
    break
  fi
  if [ "$(date +%s)" -ge "$deadline" ]; then
    printf '%s\n' 'startup timed out before Desktop, Worker, SSH, and a fully drained connected runtime were all observed' >&2
    exit 1
  fi
  sleep 1
done

printf 'desktop_pid_before=%s\n' "$(matching_pids "$desktop" | /usr/bin/tr '\n' ' ')"
printf 'worker_pid_before=%s\n' "$(matching_pids "$worker" | /usr/bin/tr '\n' ' ')"
printf 'ssh_pid_before=%s\n' "$(tunnel_pids | /usr/bin/tr '\n' ' ')"
printf '%s\n' 'runtime_before_begin'
/bin/cat "$runtime_status"
printf '%s\n' 'runtime_before_end'

/usr/bin/osascript -e 'tell application id "com.go1c.fns-workspace" to quit'
deadline=$(( $(date +%s) + shutdown_timeout ))
while has_matching_pid "$desktop" || has_matching_pid "$worker" || has_tunnel; do
  if [ "$(date +%s)" -ge "$deadline" ]; then
    printf '%s\n' 'shutdown timed out with a Desktop, Worker, or SSH process still present' >&2
    exit 1
  fi
  sleep 1
done

[ -f "$runtime_status" ] || { printf '%s\n' 'runtime status disappeared after shutdown' >&2; exit 1; }
if /usr/bin/grep -Eq '"lastErrorCode"[[:space:]]*:[[:space:]]*"abnormal_exit"' "$runtime_status"; then
  printf '%s\n' 'runtime recorded abnormal_exit after a normal AppleEvent quit' >&2
  exit 1
fi
if ! /usr/bin/grep -Eq '"running"[[:space:]]*:[[:space:]]*false' "$runtime_status"; then
  printf '%s\n' 'runtime did not record a stopped state after AppleEvent quit' >&2
  exit 1
fi

printf 'desktop_pid_after=%s\n' "$(matching_pids "$desktop" | /usr/bin/tr '\n' ' ')"
printf 'worker_pid_after=%s\n' "$(matching_pids "$worker" | /usr/bin/tr '\n' ' ')"
printf 'ssh_pid_after=%s\n' "$(tunnel_pids | /usr/bin/tr '\n' ' ')"
printf '%s\n' 'runtime_after_begin'
/bin/cat "$runtime_status"
printf '%s\n' 'runtime_after_end'
printf 'completed_at_utc=%s\n' "$(date -u +%FT%TZ)"
printf '%s\n' 'result=passed'

trap - EXIT HUP INT TERM
