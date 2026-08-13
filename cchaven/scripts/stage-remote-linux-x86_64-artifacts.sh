#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
destination_dir="$repo_root/apps/desktop/src-tauri/resources/remote/linux-x86_64"

: "${FNS_SERVER_LINUX_X86_64_ARTIFACT:?set the explicit final fns-server artifact}"
: "${FNS_AGENT_LINUX_X86_64_ARTIFACT:?set the explicit final fns-agent artifact}"
: "${FNS_REMOTE_LINUX_X86_64_PROVENANCE:?set the explicit final release provenance JSON}"
server_source=$FNS_SERVER_LINUX_X86_64_ARTIFACT
agent_source=$FNS_AGENT_LINUX_X86_64_ARTIFACT

"$script_dir/verify-remote-linux-x86_64-provenance.sh" \
  "$server_source" "$agent_source" "$FNS_REMOTE_LINUX_X86_64_PROVENANCE"

mkdir -p "$destination_dir"
for artifact in fns-server fns-agent release-provenance.json; do
  case "$artifact" in
    fns-server) source_path=$server_source ;;
    fns-agent) source_path=$agent_source ;;
    release-provenance.json) source_path=$FNS_REMOTE_LINUX_X86_64_PROVENANCE ;;
  esac
  destination="$destination_dir/$artifact"
  temporary=$(mktemp "$destination.tmp.XXXXXX")
  trap 'rm -f "$temporary"' EXIT HUP INT TERM
  cp "$source_path" "$temporary"
  if [ "$artifact" = release-provenance.json ]; then
    chmod 0644 "$temporary"
  else
    chmod 0755 "$temporary"
  fi
  mv -f "$temporary" "$destination"
  trap - EXIT HUP INT TERM
  shasum -a 256 "$destination"
  file "$destination"
done
