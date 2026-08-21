#!/bin/sh
set -u

home=HOME_PLACEHOLDER
bin=$home/.local/share/bestcodex/bin
state=STATE_PLACEHOLDER
root=ROOT_PLACEHOLDER
server_dir=$home/.local/share/bestcodex/server
server=$bin/fns-server
agent=$bin/fns-agent

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

start_missing() {
  if ! process_pid $server >/dev/null; then
    (cd $server_dir && exec $server run --config $server_dir/config/config.yaml) >>$state/server.stdout.log 2>>$state/server.stderr.log &
    echo $! > $state/server.pid
  fi
  if ! process_pid $agent $state/agent.json >/dev/null; then
    (cd $root && exec $agent run --config $state/agent.json) >>$state/agent.stdout.log 2>>$state/agent.stderr.log &
    echo $! > $state/agent.pid
  fi
}

trap exit TERM INT
while :; do
  start_missing
  sleep 2
done
