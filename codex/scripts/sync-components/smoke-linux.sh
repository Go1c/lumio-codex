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
state=$smoke_home/.local/share/bestcodex/state
server_dir=$smoke_home/.local/share/bestcodex/server
workspace=$scratch/workspace

process_pid() {
  wanted=$1
  for proc_exe in /proc/[0-9]*/exe; do
    [ -L $proc_exe ] || continue
    target=$(readlink $proc_exe 2>/dev/null || true)
    if [ "x$target" = "x$wanted" ]; then
      pid=${proc_exe#/proc/}
      echo ${pid%/exe}
      return 0
    fi
  done
  return 1
}

cleanup() {
  status=$?
  set +e
  if [ -f $state/watchdog.pid ]; then
    kill $(cat $state/watchdog.pid) 2>/dev/null
  fi
  for wanted in $bin/fns-agent $bin/fns-server; do
    pid=$(process_pid $wanted || true)
    if [ x$pid != x ]; then kill $pid 2>/dev/null; fi
  done
  sleep 1
  if [ $status -ne 0 ]; then
    echo smoke failed: scratch=$scratch >&2
    tail -n 80 $state/watchdog.stderr.log >&2 2>/dev/null
    tail -n 80 $state/server.stderr.log >&2 2>/dev/null
    tail -n 80 $state/agent.stderr.log >&2 2>/dev/null
  else
    rm -r $scratch
  fi
  return $status
}
trap cleanup EXIT INT TERM

mkdir -p $bin $state $server_dir $workspace
cp $artifact_dir/fns-server $bin/fns-server
cp $artifact_dir/fns-agent $bin/fns-agent
chmod 0755 $bin/fns-server $bin/fns-agent
port=$(node -e 'const net=require("node:net");const s=net.createServer();s.listen(0,"127.0.0.1",()=>{process.stdout.write(String(s.address().port));s.close()})')
printf %s bestcodex-local-token > $state/token
SMOKE_PORT=$port SMOKE_WORKSPACE=$workspace SMOKE_STATE=$state node -e '
  const fs = require("node:fs");
  const state = process.env.SMOKE_STATE;
  const config = {
    schemaVersion: "fns-agent-config/1",
    endpoint: `ws://127.0.0.1:${process.env.SMOKE_PORT}/api/user/workspace-sync/v2`,
    workspaceId: "6b657374-c0de-4000-8000-000000000001",
    clientId: "6b657374-c0de-4000-8000-000000000002",
    workspaceRoot: process.env.SMOKE_WORKSPACE,
    stateDir: state,
    tokenFile: `${state}/token`,
    sync: { includes: ["**/*"], excludes: [], protectSecrets: true },
    transport: { maxActiveTransfers: 2 },
  };
  fs.writeFileSync(`${state}/agent.json`, JSON.stringify(config, null, 2));
'
chmod 0600 $state/token $state/agent.json
sed s#HOME_PLACEHOLDER#$smoke_home#g $script_dir/watchdog.sh | sed s#ROOT_PLACEHOLDER#$workspace#g | sed s#PORT_PLACEHOLDER#$port#g > $state/watchdog.sh
chmod 0700 $state/watchdog.sh
nohup sh $state/watchdog.sh >/dev/null 2>>$state/watchdog.stderr.log &
echo $! > $state/watchdog.pid

attempt=0
while [ $attempt -lt 20 ]; do
  server_pid=$(process_pid $bin/fns-server || true)
  agent_pid=$(process_pid $bin/fns-agent || true)
  if [ x$server_pid != x ] && [ x$agent_pid != x ]; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
[ x$server_pid != x ] && [ x$agent_pid != x ]
[ $(wc -c < $bin/fns-server) -gt 1024 ]
[ $(wc -c < $bin/fns-agent) -gt 1024 ]
echo first probe
ps -o pid,ppid,stat,etime,comm,args -p $server_pid,$agent_pid

sleep 4
second_server_pid=$(process_pid $bin/fns-server)
second_agent_pid=$(process_pid $bin/fns-agent)
[ x$second_server_pid != x ] && [ x$second_agent_pid != x ]
[ $second_server_pid = $server_pid ]
[ $second_agent_pid = $agent_pid ]
echo second probe
ps -o pid,ppid,stat,etime,comm,args -p $second_server_pid,$second_agent_pid
kill $second_agent_pid

attempt=0
replacement_agent_pid=
while [ $attempt -lt 20 ]; do
  replacement_agent_pid=$(process_pid $bin/fns-agent || true)
  if [ x$replacement_agent_pid != x ] && [ $replacement_agent_pid != $second_agent_pid ]; then break; fi
  attempt=$((attempt + 1))
  sleep 1
done
[ x$replacement_agent_pid != x ]
[ $replacement_agent_pid != $second_agent_pid ]
process_pid $bin/fns-server >/dev/null
echo replacement probe
ps -o pid,ppid,stat,etime,comm,args -p $second_server_pid,$replacement_agent_pid
echo smoke ok: server=$second_server_pid agent=$second_agent_pid replacement=$replacement_agent_pid
