#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.0.0}"
ARCH="${2:-$(uname -m)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DIST="$ROOT/dist/macos"
BINARY_DIR="${BINARY_DIR:-$ROOT/target/release}"
DMG="$DIST/LumioCodex-${VERSION}-macos-${ARCH}-internal-unsigned.dmg"
ICON_SOURCE="$ROOT/apps/codex-plus-manager/src-tauri/icons/icon.icns"
ICON_NAME="lumio-codex.icns"
STAGE_PARENT="${TMPDIR:-/tmp}"
STAGE_PARENT="${STAGE_PARENT%/}"

case "$STAGE_PARENT" in
  /*) ;;
  *)
    echo "error: staging parent must be absolute: $STAGE_PARENT" >&2
    exit 1
    ;;
esac

STAGE="$(mktemp -d "$STAGE_PARENT/lumio-codex-package.XXXXXX")"
APP_DIR="$STAGE/BestCodex.app"

cleanup_stage() {
  case "$STAGE" in
    "$STAGE_PARENT"/lumio-codex-package.*) ;;
    *)
      echo "error: refusing to clean unexpected staging path: $STAGE" >&2
      return 1
      ;;
  esac
  if [ -d "$STAGE" ]; then
    /usr/bin/find "$STAGE" -depth -delete
  fi
}
trap cleanup_stage EXIT

for required_binary in "$BINARY_DIR/lumio-codex" "$BINARY_DIR/lumio-codex-launcher"; do
  if [ ! -x "$required_binary" ]; then
    echo "error: binary not found or not executable: $required_binary" >&2
    exit 1
  fi
done
if [ ! -f "$ICON_SOURCE" ]; then
  echo "error: icon not found: $ICON_SOURCE" >&2
  exit 1
fi

mkdir -p "$DIST"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Helpers" "$APP_DIR/Contents/Resources"
cp "$BINARY_DIR/lumio-codex" "$APP_DIR/Contents/MacOS/lumio-codex"
cp "$BINARY_DIR/lumio-codex-launcher" "$APP_DIR/Contents/Helpers/lumio-codex-launcher"
cp "$ICON_SOURCE" "$APP_DIR/Contents/Resources/$ICON_NAME"
chmod +x "$APP_DIR/Contents/MacOS/lumio-codex" "$APP_DIR/Contents/Helpers/lumio-codex-launcher"
printf 'APPL????' > "$APP_DIR/Contents/PkgInfo"
cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>BestCodex</string>
  <key>CFBundleDisplayName</key>
  <string>BestCodex</string>
  <key>CFBundleIdentifier</key>
  <string>games.lumio.codex</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleSignature</key>
  <string>????</string>
  <key>CFBundleExecutable</key>
  <string>lumio-codex</string>
  <key>CFBundleIconFile</key>
  <string>$ICON_NAME</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

plutil -lint "$APP_DIR/Contents/Info.plist" >/dev/null
if [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APP_DIR/Contents/Info.plist")" != "games.lumio.codex" ]; then
  echo "error: unexpected bundle identifier" >&2
  exit 1
fi

ln -s /Applications "$STAGE/Applications"

hdiutil create -volname "BestCodex Internal" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
echo "$DMG"
