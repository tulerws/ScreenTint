#!/usr/bin/env bash
# Builds dist/ScreenTint.app and a .dmg from the two per-architecture release binaries.
# Usage: packaging/macos/build-dmg.sh <version>   (run from anywhere, after both cargo builds)
set -euo pipefail

VERSION="${1:?usage: build-dmg.sh <version>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist"
APP="$OUT/ScreenTint.app"

rm -rf "$APP" "$OUT/dmg" "$OUT/icon.iconset"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$OUT/dmg"

lipo -create \
  "$ROOT/target/aarch64-apple-darwin/release/ScreenTint" \
  "$ROOT/target/x86_64-apple-darwin/release/ScreenTint" \
  -output "$APP/Contents/MacOS/ScreenTint"

# .icns from the 512 px PNG
ICONSET="$OUT/icon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$ROOT/assets/icon.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  if [ "$s" -lt 512 ]; then
    d=$((s * 2))
    sips -z "$d" "$d" "$ROOT/assets/icon.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
  fi
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"
rm -rf "$ICONSET"

# LSUIElement: menu bar app, no Dock icon
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ScreenTint</string>
  <key>CFBundleDisplayName</key><string>ScreenTint</string>
  <key>CFBundleIdentifier</key><string>io.github.tulerws.screentint</string>
  <key>CFBundleExecutable</key><string>ScreenTint</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# ad-hoc signature (no Developer ID): required for arm64 binaries to launch at all
codesign --force --deep --sign - "$APP"

cp -R "$APP" "$OUT/dmg/"
ln -s /Applications "$OUT/dmg/Applications"
hdiutil create -volname "ScreenTint" -srcfolder "$OUT/dmg" -ov -format UDZO \
  "$OUT/ScreenTint-${VERSION}-macos-universal.dmg"
rm -rf "$OUT/dmg"
echo "built $OUT/ScreenTint-${VERSION}-macos-universal.dmg"
