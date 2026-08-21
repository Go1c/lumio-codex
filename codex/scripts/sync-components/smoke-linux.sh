#!/bin/sh
set -eu

if [ $(uname -s) != Linux ]; then
  echo smoke-linux.sh requires Linux >&2
  exit 2
fi
if [ $# -ne 1 ]; then
  echo usage: smoke-linux.sh artifact-dir >&2
  exit 2
fi

script_dir=$(CDPATH= cd -- $(dirname -- $0) && pwd)
artifact_dir=$(CDPATH= cd -- $1 && pwd)
node $script_dir/verify.mjs remote $artifact_dir

scratch=$(mktemp -d ${TMPDIR:-/tmp}/bestcodex-sync-smoke.XXXXXX)
smoke_home=$scratch/home
bin=$smoke_home/.local/share/bestcodex/bin
state_root=$smoke_home/.local/share/bestcodex/state
state_one=$state_root/workspaces/6b657374-c0de-4000-8000-000000000001
state_two=$state_root/workspaces/6b657374-c0de-4000-8000-000000000003
server_dir=$smoke_home/.local/share/bestcodex/server
workspace_one=$scratch/workspace-one
workspace_two=$scratch/workspace-two

process_pid() {
  wanted=$1
  required_arg=${2-}
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    if [ "x$target" = "x$wanted" ]; then
      pid=${proc_exe#/proc/}
      pid=${pid%/exe}
      if [ -z "$required_arg" ] || { [ -r /proc/$pid/cmdline ] && tr '\000' '\n' < /proc/$pid/cmdline | grep -Fx "$required_arg" >/dev/null 2>&1; }; then
        echo $pid
        return 0
      fi
    fi
  done
  return 1
}

cleanup() {
  status=$?
  set +e
  for state in $state_one $state_two; do
    if [ -f $state/watchdog.pid ]; then
      kill $(cat $state/watchdog.pid) 2>/dev/null
    fi
    pid=$(process_pid $bin/fns-agent $state/agent.json || true)
    if [ x$pid != x ]; then kill $pid 2>/dev/null; fi
  done
  pid=$(process_pid $bin/fns-server || true)
  if [ x$pid != x ]; then kill $pid 2>/dev/null; fi
  sleep 1
  if [ $status -ne 0 ]; then
    echo smoke failed: scratch=$scratch >&2
    for state in $state_one $state_two; do
      tail -n 80 $state/watchdog.stderr.log >&2 2>/dev/null
      tail -n 80 $state/server.stderr.log >&2 2>/dev/null
      tail -n 80 $state/agent.stderr.log >&2 2>/dev/null
    done
  else
    rm -r $scratch
  fi
  return $status
}
trap cleanup EXIT INT TERM

mkdir -p $bin $state_one $state_two $server_dir $workspace_one $workspace_two
cp $artifact_dir/fns-server $bin/fns-server
cp $artifact_dir/fns-agent $bin/fns-agent
chmod 0755 $bin/fns-server $bin/fns-agent
port=$(node -e 'const net=require("node:net");const s=net.createServer();s.listen(0,"127.0.0.1",()=>{process.stdout.write(String(s.address().port));s.close()})')
$bin/fns-server bootstrap-workspace --config $server_dir/config/config.yaml --token-file $state_one/token --workspace-id 6b657374-c0de-4000-8000-000000000001 --workspace-root $workspace_one --listen 127.0.0.1:$port
$bin/fns-server bootstrap-workspace --config $server_dir/config/config.yaml --token-file $state_two/token --workspace-id 6b657374-c0de-4000-8000-000000000003 --workspace-root $workspace_two --listen 127.0.0.1:$port

write_config() {
  state=$1
  workspace=$2
  workspace_id=$3
  client_id=$4
  SMOKE_PORT=$port SMOKE_WORKSPACE=$workspace SMOKE_STATE=$state SMOKE_WORKSPACE_ID=$workspace_id SMOKE_CLIENT_ID=$client_id node -e '
  const fs = require("node:fs");
  const state = process.env.SMOKE_STATE;
  const config = {
    schemaVersion: "fns-agent-config/1",
    endpoint: `ws://127.0.0.1:${process.env.SMOKE_PORT}/api/user/workspace-sync/v2`,
    workspaceId: process.env.SMOKE_WORKSPACE_ID,
    clientId: process.env.SMOKE_CLIENT_ID,
    workspaceRoot: process.env.SMOKE_WORKSPACE,
    stateDir: state,
    tokenFile: `${state}/token`,
    sync: { includes: ["**/*"], excludes: [], protectSecrets: true },
    transport: { maxActiveTransfers: 2 },
  };
  fs.writeFileSync(`${state}/agent.json`, JSON.stringify(config, null, 2));
'
}

write_config $state_one $workspace_one 6b657374-c0de-4000-8000-000000000001 6b657374-c0de-4000-8000-000000000002
write_config $state_two $workspace_two 6b657374-c0de-4000-8000-000000000003 6b657374-c0de-4000-8000-000000000004

for state in $state_one $state_two; do
  chmod 0600 $state/token $state/agent.json
done

render_watchdog() {
  state=$1
  workspace=$2
  sed s#HOME_PLACEHOLDER#$smoke_home#g $script_dir/watchdog.sh | sed s#STATE_PLACEHOLDER#$state#g | sed s#ROOT_PLACEHOLDER#$workspace#g | sed s#PORT_PLACEHOLDER#$port#g > $state/watchdog.sh
  chmod 0700 $state/watchdog.sh
  nohup sh $state/watchdog.sh >/dev/null 2>>$state/watchdog.stderr.log &
  echo $! > $state/watchdog.pid
}

render_watchdog $state_one $workspace_one
render_watchdog $state_two $workspace_two

attempt=0
while [ $attempt -lt 20 ]; do
  server_pid=$(process_pid $bin/fns-server || true)
  agent_one_pid=$(process_pid $bin/fns-agent $state_one/agent.json || true)
  agent_two_pid=$(process_pid $bin/fns-agent $state_two/agent.json || true)
  if [ x$server_pid != x ] && [ x$agent_one_pid != x ] && [ x$agent_two_pid != x ]; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
[ x$server_pid != x ] && [ x$agent_one_pid != x ] && [ x$agent_two_pid != x ]
[ $(wc -c < $bin/fns-server) -gt 1024 ]
[ $(wc -c < $bin/fns-agent) -gt 1024 ]
echo first probe
ps -o pid,ppid,stat,etime,comm,args -p $server_pid,$agent_one_pid,$agent_two_pid
$bin/fns-agent status --config $state_one/agent.json --json
$bin/fns-agent status --config $state_two/agent.json --json
[ -f $state_one/state.sqlite ]
[ -f $state_two/state.sqlite ]
[ $state_one/state.sqlite != $state_two/state.sqlite ]

sleep 4
second_server_pid=$(process_pid $bin/fns-server)
second_agent_one_pid=$(process_pid $bin/fns-agent $state_one/agent.json)
second_agent_two_pid=$(process_pid $bin/fns-agent $state_two/agent.json)
[ x$second_server_pid != x ] && [ x$second_agent_one_pid != x ] && [ x$second_agent_two_pid != x ]
[ $second_server_pid = $server_pid ]
[ $second_agent_one_pid = $agent_one_pid ]
[ $second_agent_two_pid = $agent_two_pid ]
echo second probe
ps -o pid,ppid,stat,etime,comm,args -p $second_server_pid,$second_agent_one_pid,$second_agent_two_pid
kill $second_agent_one_pid

attempt=0
replacement_agent_one_pid=
while [ $attempt -lt 20 ]; do
  replacement_agent_one_pid=$(process_pid $bin/fns-agent $state_one/agent.json || true)
  if [ x$replacement_agent_one_pid != x ] && [ $replacement_agent_one_pid != $second_agent_one_pid ]; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
[ x$replacement_agent_one_pid != x ]
[ $replacement_agent_one_pid != $second_agent_one_pid ]
[ $(process_pid $bin/fns-server) = $second_server_pid ]
[ $(process_pid $bin/fns-agent $state_two/agent.json) = $second_agent_two_pid ]
echo replacement probe
ps -o pid,ppid,stat,etime,comm,args -p $second_server_pid,$replacement_agent_one_pid,$second_agent_two_pid
echo smoke ok: server=$second_server_pid agent_one=$second_agent_one_pid replacement_one=$replacement_agent_one_pid agent_two=$second_agent_two_pid
