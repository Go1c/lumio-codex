#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

read_json() {
  /usr/bin/plutil -extract "$2" raw -o - "$1" 2>/dev/null \
    || fail "release provenance is missing field: $2"
}

[ "$#" -eq 3 ] || fail "usage: $0 /path/fns-server /path/fns-agent /path/release-provenance.json"
server=$1
agent=$2
provenance=$3
: "${FNS_SERVER_SOURCE_DIR:?set FNS_SERVER_SOURCE_DIR to the server checkout used for this release}"

[ -f "$server" ] || fail "fns-server artifact is missing: $server"
[ -f "$agent" ] || fail "fns-agent artifact is missing: $agent"
[ -f "$provenance" ] || fail "release provenance is missing: $provenance"
[ -z "$(git -C "$repo_root" status --porcelain=v1)" ] \
  || fail "client worktree must be clean when verifying release provenance"
[ -z "$(git -C "$FNS_SERVER_SOURCE_DIR" status --porcelain=v1)" ] \
  || fail "server worktree must be clean when verifying release provenance"

[ "$(read_json "$provenance" schemaVersion)" = 'fns-release-provenance/1' ] \
  || fail "release provenance has an unsupported schemaVersion"
client_commit=$(git -C "$repo_root" rev-parse HEAD)
server_commit=$(git -C "$FNS_SERVER_SOURCE_DIR" rev-parse HEAD)
[ "$(read_json "$provenance" clientCommit)" = "$client_commit" ] \
  || fail "release provenance clientCommit does not match the client checkout"
[ "$(read_json "$provenance" serverCommit)" = "$server_commit" ] \
  || fail "release provenance serverCommit does not match the server checkout"
for field in \
  buildTimestamp builder \
  artifacts.fns-agent.buildTimestamp artifacts.fns-agent.builder artifacts.fns-agent.buildCommand \
  artifacts.fns-server.buildTimestamp artifacts.fns-server.builder artifacts.fns-server.buildCommand
do
  [ -n "$(read_json "$provenance" "$field")" ] \
    || fail "release provenance field is empty: $field"
done

case "$(file -b "$server")" in
  *ELF*64-bit*x86-64*) ;;
  *) fail "fns-server is not a Linux x86_64 ELF binary" ;;
esac
case "$(file -b "$agent")" in
  *ELF*64-bit*x86-64*) ;;
  *) fail "fns-agent is not a Linux x86_64 ELF binary" ;;
esac
[ "$(read_json "$provenance" artifacts.fns-server.os)" = linux ] \
  || fail "fns-server provenance OS is not linux"
[ "$(read_json "$provenance" artifacts.fns-agent.os)" = linux ] \
  || fail "fns-agent provenance OS is not linux"
[ "$(read_json "$provenance" artifacts.fns-server.architecture)" = x86_64 ] \
  || fail "fns-server provenance architecture is not x86_64"
[ "$(read_json "$provenance" artifacts.fns-agent.architecture)" = x86_64 ] \
  || fail "fns-agent provenance architecture is not x86_64"

server_sha256=$(shasum -a 256 "$server" | awk '{ print $1 }')
agent_sha256=$(shasum -a 256 "$agent" | awk '{ print $1 }')
[ "$(read_json "$provenance" artifacts.fns-server.sha256)" = "$server_sha256" ] \
  || fail "fns-server SHA-256 does not match release provenance"
[ "$(read_json "$provenance" artifacts.fns-agent.sha256)" = "$agent_sha256" ] \
  || fail "fns-agent SHA-256 does not match release provenance"

server_build_info=$(go version -m "$server")
embedded_revision=$(printf '%s\n' "$server_build_info" \
  | awk '$1 == "build" && $2 ~ /^vcs[.]revision=/ { sub(/^vcs[.]revision=/, "", $2); print $2; exit }')
embedded_modified=$(printf '%s\n' "$server_build_info" \
  | awk '$1 == "build" && $2 ~ /^vcs[.]modified=/ { sub(/^vcs[.]modified=/, "", $2); print $2; exit }')
[ "$embedded_revision" = "$server_commit" ] \
  || fail "fns-server embedded vcs.revision does not match the server checkout"
[ "$embedded_modified" = false ] \
  || fail "fns-server embedded vcs.modified is not false"

printf 'verified Linux release provenance: client=%s server=%s\n' \
  "$client_commit" "$server_commit"
printf 'fns-agent sha256=%s\n' "$agent_sha256"
printf 'fns-server sha256=%s\n' "$server_sha256"
