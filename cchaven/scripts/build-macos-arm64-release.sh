#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
target=aarch64-apple-darwin
export CARGO_TARGET_DIR="$repo_root/target/macos-arm64-release"
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
npm exec -- tauri build --target "$target" --bundles app,dmg

# The bundle is named after `productName` in tauri.conf.json; read it rather
# than hardcoding, so renaming the product cannot silently break packaging.
product_name=$(node -p "require('$repo_root/apps/desktop/src-tauri/tauri.conf.json').productName")
app="$CARGO_TARGET_DIR/$target/release/bundle/macos/$product_name.app"
dmg_dir="$CARGO_TARGET_DIR/$target/release/bundle/dmg"
test -d "$app"
xcrun lipo "$app/Contents/MacOS/fns-workspace-desktop" -verify_arch arm64
xcrun lipo "$app/Contents/MacOS/fns-agent" -verify_arch arm64
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
dmg="$dmg_dir/${product_name}_${version}_aarch64.dmg"
test -f "$dmg"

"$script_dir/verify-macos-arm64-bundle.sh" "$app" "$dmg"
printf '%s\n' "$app"
printf '%s\n' "$dmg"
