#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
expected_remote_server=${FNS_SERVER_LINUX_X86_64_ARTIFACT:-"$repo_root/target/release-assets/linux-x86_64/fns-server"}
expected_remote_agent=${FNS_AGENT_LINUX_X86_64_ARTIFACT:-"$repo_root/target/release-assets/linux-x86_64/fns-agent"}
expected_remote_provenance=${FNS_REMOTE_LINUX_X86_64_PROVENANCE:-"$repo_root/target/release-assets/linux-x86_64/release-provenance.json"}

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

usage() {
  fail "usage: $0 /absolute/path/FNS Workspace.app [/absolute/path/FNS Workspace.dmg]"
}

details_for() {
  /usr/bin/codesign -dvvv --verbose=4 "$1" 2>&1
}

team_for() {
  details_for "$1" | /usr/bin/awk -F= '/^TeamIdentifier=/{ print $2; exit }'
}

verify_component() {
  component=$1
  expected_team=$2
  label=$3

  [ -e "$component" ] || fail "$label is missing: $component"
  /usr/bin/codesign --verify --strict --verbose=2 "$component"
  details=$(details_for "$component")
  if printf '%s\n' "$details" | /usr/bin/grep -F 'Signature=adhoc' >/dev/null; then
    fail "$label uses an ad-hoc signature"
  fi
  if ! printf '%s\n' "$details" | /usr/bin/grep -F 'Authority=' >/dev/null; then
    fail "$label has no certificate authority"
  fi
  component_team=$(printf '%s\n' "$details" | /usr/bin/awk -F= '/^TeamIdentifier=/{ print $2; exit }')
  if [ -z "$component_team" ] || [ "$component_team" = "not set" ]; then
    fail "$label has no TeamIdentifier"
  fi
  if [ -n "$expected_team" ] && [ "$component_team" != "$expected_team" ]; then
    fail "$label TeamIdentifier does not match the app"
  fi
}

verify_remote_resource() {
  resource=$1
  expected_source=$2
  label=$3

  [ -f "$expected_source" ] || fail "$label source is missing: $expected_source"
  [ -f "$resource" ] || fail "$label is missing: $resource"
  [ -x "$resource" ] || fail "$label is not executable: $resource"
  case "$(/usr/bin/file -b "$resource")" in
    *ELF*64-bit*x86-64*) ;;
    *) fail "$label is not a Linux x86_64 ELF binary" ;;
  esac
  /usr/bin/cmp -s "$expected_source" "$resource" \
    || fail "$label does not match its staged release artifact"
  /usr/bin/shasum -a 256 "$resource"
}

verify_app() {
  app=$1
  expected_team=${2:-}
  expected_identifier=${FNS_MACOS_BUNDLE_IDENTIFIER:-com.go1c.fns-workspace}
  expected_arch=${FNS_MACOS_ARCHITECTURE:-arm64}
  main="$app/Contents/MacOS/fns-workspace-desktop"
  sidecar="$app/Contents/MacOS/fns-agent"
  remote_server="$app/Contents/Resources/remote/linux-x86_64/fns-server"
  remote_agent="$app/Contents/Resources/remote/linux-x86_64/fns-agent"
  remote_provenance="$app/Contents/Resources/remote/linux-x86_64/release-provenance.json"
  plist="$app/Contents/Info.plist"

  [ -d "$app" ] || fail "app bundle is missing: $app"
  [ -f "$plist" ] || fail "app Info.plist is missing: $plist"
  bundle_identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$plist")
  [ "$bundle_identifier" = "$expected_identifier" ] \
    || fail "unexpected app bundle identifier: $bundle_identifier"

  /usr/bin/codesign --verify --deep --strict --verbose=2 "$app"
  app_team=$(team_for "$app")
  if [ -z "$app_team" ] || [ "$app_team" = "not set" ]; then
    fail "app bundle has no TeamIdentifier"
  fi
  if [ -n "$expected_team" ] && [ "$app_team" != "$expected_team" ]; then
    fail "app bundle TeamIdentifier does not match the expected team"
  fi

  verify_component "$main" "$app_team" "app main executable"
  verify_component "$sidecar" "$app_team" "bundled fns-agent"
  verify_remote_resource "$remote_server" "$expected_remote_server" "bundled remote fns-server"
  verify_remote_resource "$remote_agent" "$expected_remote_agent" "bundled remote fns-agent"
  [ -f "$remote_provenance" ] || fail "bundled remote release provenance is missing"
  /usr/bin/cmp -s "$expected_remote_provenance" "$remote_provenance" \
    || fail "bundled remote release provenance does not match the staged manifest"
  "$script_dir/verify-remote-linux-x86_64-provenance.sh" \
    "$remote_server" "$remote_agent" "$remote_provenance"
  /usr/bin/lipo "$main" -verify_arch "$expected_arch"
  /usr/bin/lipo "$sidecar" -verify_arch "$expected_arch"

  requirement=$(/usr/bin/codesign -d -r- "$main" 2>&1)
  if printf '%s\n' "$requirement" | /usr/bin/grep -F 'designated => cdhash' >/dev/null; then
    fail "app main executable has a build-specific designated requirement"
  fi
  if ! printf '%s\n' "$requirement" \
    | /usr/bin/grep -F "identifier \"$expected_identifier\"" >/dev/null
  then
    fail "app main executable designated requirement has the wrong identifier"
  fi

  printf 'verified app: %s (team %s, architecture %s)\n' \
    "$app" "$app_team" "$expected_arch"
  VERIFIED_APP_TEAM=$app_team
}

[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage
case "$1" in
  /*) ;;
  *) fail "app bundle path must be absolute" ;;
esac

verify_app "$1"
outer_team=$VERIFIED_APP_TEAM

if [ "$#" -eq 2 ]; then
  dmg=$2
  case "$dmg" in
    /*) ;;
    *) fail "DMG path must be absolute" ;;
  esac
  [ -f "$dmg" ] || fail "DMG is missing: $dmg"
  verify_component "$dmg" "$outer_team" "DMG"

  mount_dir=$(mktemp -d "${TMPDIR:-/tmp}/fns-workspace-dmg.XXXXXX")
  mounted=false
  cleanup() {
    if [ "$mounted" = true ]; then
      /usr/bin/hdiutil detach "$mount_dir" -quiet >/dev/null 2>&1 \
        || /usr/bin/hdiutil detach "$mount_dir" -force -quiet >/dev/null 2>&1 \
        || true
    fi
    rmdir "$mount_dir" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT HUP INT TERM

  /usr/bin/hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount_dir" -quiet
  mounted=true
  set -- "$mount_dir"/*.app
  if [ "$#" -ne 1 ] || [ ! -d "$1" ]; then
    fail "DMG must contain exactly one app bundle"
  fi
  verify_app "$1" "$outer_team"
  /usr/bin/hdiutil detach "$mount_dir" -quiet
  mounted=false
  rmdir "$mount_dir"
  trap - EXIT HUP INT TERM
  printf 'verified DMG: %s\n' "$dmg"
fi
