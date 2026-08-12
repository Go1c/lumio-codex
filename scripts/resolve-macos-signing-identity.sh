#!/bin/sh
set -eu

fail() {
  printf '%s\n' "$1" >&2
  exit 1
}

if [ "$(uname -s)" != Darwin ]; then
  fail "macOS code signing is only available on Darwin"
fi

identity=${APPLE_SIGNING_IDENTITY:-${FNS_MACOS_SIGNING_IDENTITY:-}}
if [ -n "$identity" ]; then
  if [ "$identity" = "-" ]; then
    fail "ad-hoc signing is not allowed; set APPLE_SIGNING_IDENTITY to a valid identity"
  fi
  case "$identity" in
    *'
'*) fail "APPLE_SIGNING_IDENTITY must be a single line" ;;
  esac
  identity_matches=$(
    /usr/bin/security find-identity -v -p codesigning \
      | /usr/bin/awk -v requested="$identity" '
          length($2) == 40 && $2 ~ /^[0-9A-F]+$/ && index($0, requested) > 0 {
            matches += 1
          }
          END { print matches + 0 }
        '
  )
  if [ "$identity_matches" -eq 0 ]; then
    fail "APPLE_SIGNING_IDENTITY does not match a valid code-signing identity"
  fi
  if [ "$identity_matches" -ne 1 ]; then
    fail "APPLE_SIGNING_IDENTITY matches multiple valid code-signing identities"
  fi
  printf '%s\n' "$identity"
  exit 0
fi

identities=$(
  /usr/bin/security find-identity -v -p codesigning \
    | /usr/bin/awk '
        /"Apple Development:/ && length($2) == 40 && $2 ~ /^[0-9A-F]+$/ {
          print $2
        }
      '
)

set -- $identities
case $# in
  0)
    fail "no valid Apple Development identity found; set APPLE_SIGNING_IDENTITY"
    ;;
  1)
    printf '%s\n' "$1"
    ;;
  *)
    fail "multiple valid Apple Development identities found; set APPLE_SIGNING_IDENTITY explicitly"
    ;;
esac
