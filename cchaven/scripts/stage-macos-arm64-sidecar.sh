#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
target=aarch64-apple-darwin
target_dir=${CARGO_TARGET_DIR:-"$repo_root/target"}
: "${FNS_SERVER_LINUX_X86_64_ARTIFACT:?set the explicit final fns-server artifact}"
: "${FNS_AGENT_LINUX_X86_64_ARTIFACT:?set the explicit final fns-agent artifact}"
: "${FNS_REMOTE_LINUX_X86_64_PROVENANCE:?set the explicit final release provenance JSON}"
export FNS_SERVER_LINUX_X86_64_ARTIFACT FNS_AGENT_LINUX_X86_64_ARTIFACT
export FNS_REMOTE_LINUX_X86_64_PROVENANCE

case "$target_dir" in
  /*) ;;
  *) target_dir="$repo_root/$target_dir" ;;
esac

cd "$repo_root"
cargo build --locked --release --target "$target" -p fns-agent --bin fns-agent

source_binary="$target_dir/$target/release/fns-agent"
destination_dir="$repo_root/apps/desktop/src-tauri/binaries"
destination="$destination_dir/fns-agent-$target"
mkdir -p "$destination_dir"
temporary=$(mktemp "$destination.tmp.XXXXXX")
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$source_binary" "$temporary"
chmod 0755 "$temporary"
xcrun lipo "$temporary" -verify_arch arm64
mv -f "$temporary" "$destination"
trap - EXIT HUP INT TERM

shasum -a 256 "$destination"
file "$destination"

"$script_dir/stage-remote-linux-x86_64-artifacts.sh"
