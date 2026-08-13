#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
output_dir="$repo_root/target/release-assets/linux-x86_64"

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

read_json() {
  /usr/bin/plutil -extract "$2" raw -o - "$1" 2>/dev/null \
    || fail "fns-agent build provenance is missing field: $2"
}

: "${FNS_SERVER_SOURCE_DIR:?set FNS_SERVER_SOURCE_DIR to the final server checkout}"
: "${FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT:?set FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT to the final Linux build}"
: "${FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE:?set FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE to its provenance JSON}"
[ -f "$FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT" ] \
  || fail "fns-agent build artifact is missing"
[ -f "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" ] \
  || fail "fns-agent build provenance is missing"
[ -z "$(git -C "$repo_root" status --porcelain=v1)" ] \
  || fail "client worktree must be clean before preparing release artifacts"
[ -z "$(git -C "$FNS_SERVER_SOURCE_DIR" status --porcelain=v1)" ] \
  || fail "server worktree must be clean before preparing release artifacts"

client_commit=$(git -C "$repo_root" rev-parse HEAD)
server_commit=$(git -C "$FNS_SERVER_SOURCE_DIR" rev-parse HEAD)
agent_sha256=$(shasum -a 256 "$FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT" | awk '{ print $1 }')
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" schemaVersion)" = 'fns-linux-artifact-provenance/1' ] \
  || fail "fns-agent build provenance has an unsupported schemaVersion"
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" artifactName)" = fns-agent ] \
  || fail "fns-agent build provenance has the wrong artifactName"
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" sourceCommit)" = "$client_commit" ] \
  || fail "fns-agent build provenance does not match the final client commit"
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" sha256)" = "$agent_sha256" ] \
  || fail "fns-agent build artifact does not match its provenance SHA-256"
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" os)" = linux ] \
  || fail "fns-agent build provenance OS is not linux"
[ "$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" architecture)" = x86_64 ] \
  || fail "fns-agent build provenance architecture is not x86_64"
agent_build_timestamp=$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" buildTimestamp)
agent_builder=$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" builder)
agent_build_command=$(read_json "$FNS_AGENT_LINUX_X86_64_BUILD_PROVENANCE" buildCommand)
case "$agent_build_timestamp" in
  ????-??-??T??:??:??Z) ;;
  *) fail "fns-agent build provenance has an invalid buildTimestamp" ;;
esac
case "$agent_builder" in
  linux-x86_64/rustc-[0-9]*) ;;
  *) fail "fns-agent build provenance has an invalid builder" ;;
esac
[ "$agent_build_command" = 'cargo build --locked --release --target x86_64-unknown-linux-gnu -p fns-agent --bin fns-agent' ] \
  || fail "fns-agent build provenance has an unexpected buildCommand"
case "$(file -b "$FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT")" in
  *ELF*64-bit*x86-64*) ;;
  *) fail "fns-agent build artifact is not a Linux x86_64 ELF binary" ;;
esac

build_timestamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
server_tag=$(git -C "$FNS_SERVER_SOURCE_DIR" describe --tags --abbrev=0)
server_module=github.com/haierkeys/fast-note-sync-service
server_build_command='CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -buildvcs=true -trimpath -ldflags <release-metadata> -o fns-server .'
server_builder="darwin/$(go version | awk '{ print $3; exit }')"
ldflags="-X $server_module/internal/app.Version=$server_tag -X $server_module/internal/app.GitTag=$server_commit -X $server_module/internal/app.BuildTime=$build_timestamp"

mkdir -p "$output_dir"
server_temporary=$(mktemp "$output_dir/.fns-server.XXXXXX")
server_build_root=$(mktemp -d "${TMPDIR:-/tmp}/fns-server-release.XXXXXX")
trap 'rm -f "$server_temporary"; rm -rf "$server_build_root"' EXIT HUP INT TERM
git clone --shared --no-checkout --quiet "$FNS_SERVER_SOURCE_DIR" "$server_build_root/source"
git -C "$server_build_root/source" checkout --detach --quiet "$server_commit"
[ -z "$(git -C "$server_build_root/source" status --porcelain=v1)" ] \
  || fail "temporary server build checkout is not clean"
(
  cd "$server_build_root/source"
  CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
    go build -buildvcs=true -trimpath -ldflags "$ldflags" -o "$server_temporary" .
)
chmod 0755 "$server_temporary"
mv -f "$server_temporary" "$output_dir/fns-server"
rm -rf "$server_build_root"
trap - EXIT HUP INT TERM

agent_destination="$output_dir/fns-agent"
agent_temporary=$(mktemp "$output_dir/.fns-agent.XXXXXX")
trap 'rm -f "$agent_temporary"' EXIT HUP INT TERM
cp "$FNS_AGENT_LINUX_X86_64_BUILD_ARTIFACT" "$agent_temporary"
chmod 0755 "$agent_temporary"
mv -f "$agent_temporary" "$agent_destination"
trap - EXIT HUP INT TERM

server_sha256=$(shasum -a 256 "$output_dir/fns-server" | awk '{ print $1 }')
provenance="$output_dir/release-provenance.json"
provenance_temporary=$(mktemp "$output_dir/.release-provenance.XXXXXX")
trap 'rm -f "$provenance_temporary"' EXIT HUP INT TERM
printf '%s\n' \
  '{' \
  '  "schemaVersion": "fns-release-provenance/1",' \
  "  \"buildTimestamp\": \"$build_timestamp\"," \
  "  \"builder\": \"$server_builder\"," \
  "  \"clientCommit\": \"$client_commit\"," \
  "  \"serverCommit\": \"$server_commit\"," \
  '  "artifacts": {' \
  '    "fns-agent": {' \
  "      \"sha256\": \"$agent_sha256\"," \
  '      "os": "linux",' \
  '      "architecture": "x86_64",' \
  "      \"buildTimestamp\": \"$agent_build_timestamp\"," \
  "      \"builder\": \"$agent_builder\"," \
  "      \"buildCommand\": \"$agent_build_command\"" \
  '    },' \
  '    "fns-server": {' \
  "      \"sha256\": \"$server_sha256\"," \
  '      "os": "linux",' \
  '      "architecture": "x86_64",' \
  "      \"buildTimestamp\": \"$build_timestamp\"," \
  "      \"builder\": \"$server_builder\"," \
  "      \"buildCommand\": \"$server_build_command\"" \
  '    }' \
  '  }' \
  '}' >"$provenance_temporary"
mv -f "$provenance_temporary" "$provenance"
trap - EXIT HUP INT TERM

"$script_dir/verify-remote-linux-x86_64-provenance.sh" \
  "$output_dir/fns-server" "$output_dir/fns-agent" "$provenance"
printf '%s\n' "$output_dir/fns-server"
printf '%s\n' "$output_dir/fns-agent"
printf '%s\n' "$provenance"
