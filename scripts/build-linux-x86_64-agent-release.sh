#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
output_dir=${1:-"$repo_root/target/release-assets/linux-x86_64"}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || fail "fns-agent release artifact must be built on Linux"
case "$(uname -m)" in
  x86_64|amd64) ;;
  *) fail "fns-agent release artifact must be built on Linux x86_64" ;;
esac
[ -z "$(git -C "$repo_root" status --porcelain=v1)" ] \
  || fail "client worktree must be clean before building the Linux fns-agent"

client_commit=$(git -C "$repo_root" rev-parse HEAD)
build_timestamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
rust_version=$(rustc --version | awk '{ print $2; exit }')
builder="linux-x86_64/rustc-$rust_version"
build_command='cargo build --locked --release --target x86_64-unknown-linux-gnu -p fns-agent --bin fns-agent'

cd "$repo_root"
cargo build --locked --release --target x86_64-unknown-linux-gnu \
  -p fns-agent --bin fns-agent

source_binary=${CARGO_TARGET_DIR:-"$repo_root/target"}/x86_64-unknown-linux-gnu/release/fns-agent
[ -f "$source_binary" ] || fail "Linux fns-agent build output is missing: $source_binary"
case "$(file -b "$source_binary")" in
  *ELF*64-bit*x86-64*) ;;
  *) fail "fns-agent build output is not a Linux x86_64 ELF binary" ;;
esac

mkdir -p "$output_dir"
artifact="$output_dir/fns-agent"
temporary=$(mktemp "$output_dir/.fns-agent.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$source_binary" "$temporary"
chmod 0755 "$temporary"
mv -f "$temporary" "$artifact"
trap - EXIT HUP INT TERM

artifact_sha256=$(sha256sum "$artifact" | awk '{ print $1 }')
provenance="$output_dir/fns-agent.provenance.json"
provenance_temporary=$(mktemp "$output_dir/.fns-agent.provenance.XXXXXX")
trap 'rm -f "$provenance_temporary"' EXIT HUP INT TERM
printf '%s\n' \
  '{' \
  '  "schemaVersion": "fns-linux-artifact-provenance/1",' \
  '  "artifactName": "fns-agent",' \
  "  \"sourceCommit\": \"$client_commit\"," \
  "  \"sha256\": \"$artifact_sha256\"," \
  '  "os": "linux",' \
  '  "architecture": "x86_64",' \
  "  \"buildTimestamp\": \"$build_timestamp\"," \
  "  \"builder\": \"$builder\"," \
  "  \"buildCommand\": \"$build_command\"" \
  '}' >"$provenance_temporary"
mv -f "$provenance_temporary" "$provenance"
trap - EXIT HUP INT TERM

printf 'fns-agent commit=%s sha256=%s\n' "$client_commit" "$artifact_sha256"
printf '%s\n' "$artifact"
printf '%s\n' "$provenance"
