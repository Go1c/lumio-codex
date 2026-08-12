#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
target=aarch64-apple-darwin
export CARGO_TARGET_DIR="$repo_root/target/macos-arm64-acceptance"
APPLE_SIGNING_IDENTITY=$("$script_dir/resolve-macos-signing-identity.sh")
export APPLE_SIGNING_IDENTITY

"$script_dir/prepare-remote-linux-x86_64-release.sh"
FNS_SERVER_LINUX_X86_64_ARTIFACT="$repo_root/target/release-assets/linux-x86_64/fns-server"
FNS_AGENT_LINUX_X86_64_ARTIFACT="$repo_root/target/release-assets/linux-x86_64/fns-agent"
FNS_REMOTE_LINUX_X86_64_PROVENANCE="$repo_root/target/release-assets/linux-x86_64/release-provenance.json"
export FNS_SERVER_LINUX_X86_64_ARTIFACT FNS_AGENT_LINUX_X86_64_ARTIFACT
export FNS_REMOTE_LINUX_X86_64_PROVENANCE
"$script_dir/stage-macos-arm64-sidecar.sh"
cd "$repo_root/apps/desktop"
npm ci
npm exec -- tauri build --debug --target "$target" --bundles app

app="$CARGO_TARGET_DIR/$target/debug/bundle/macos/FNS Workspace.app"
"$script_dir/verify-macos-arm64-bundle.sh" "$app"
printf '%s\n' "$app"
