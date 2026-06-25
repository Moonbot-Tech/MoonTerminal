#!/usr/bin/env bash
# Package the macOS release binary into MoonTerminal.app and a distributable .dmg.
# Runs on the macos-14 runner (native tools: sips, iconutil, codesign, hdiutil).
set -euo pipefail

BIN="target/aarch64-apple-darwin/release/moonterminal"
APP="dist/MoonTerminal.app"
DMG="dist/MoonTerminal.dmg"
SRC_ICON="assets/icons/0.png"

# Version from the tag (v0.0.1 -> 0.0.1); fall back to 0.0.0 for manual runs.
VERSION="${GITHUB_REF_NAME:-0.0.0}"
VERSION="${VERSION#v}"

rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/moonterminal"
chmod +x "$APP/Contents/MacOS/moonterminal"

# Build a multi-resolution .icns from the single PNG app icon.
ICONSET="dist/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SRC_ICON" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" "$SRC_ICON" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>MoonTerminal</string>
  <key>CFBundleDisplayName</key><string>MoonTerminal</string>
  <key>CFBundleExecutable</key><string>moonterminal</string>
  <key>CFBundleIdentifier</key><string>com.moonbot.moonterminal</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: not notarized, but seals the bundle so it launches after a
# right-click -> Open (or `xattr -dr com.apple.quarantine`). Full Developer-ID
# signing + notarization needs an Apple cert in secrets — out of scope for v0.0.1.
codesign --force --deep --sign - "$APP" || true

# Assemble a Finder-friendly DMG staging root instead of imaging the bare .app:
# the app, a drag-to-install alias to /Applications, and a short RU readme. The
# image is built from this folder so the user sees the familiar "drag into
# Applications" layout. (Background image / .DS_Store window layout intentionally
# omitted for now — can be layered on later.)
DMG_ROOT="dist/dmg-root"
rm -rf "$DMG_ROOT"
mkdir -p "$DMG_ROOT"
cp -R "$APP" "$DMG_ROOT/MoonTerminal.app"
ln -s /Applications "$DMG_ROOT/Applications"

cat > "$DMG_ROOT/README.txt" <<'README'
MoonTerminal — Installation

1. Drag MoonTerminal.app into the Applications folder (alias next to it).
2. First launch: right-click the app -> "Open" -> "Open".
   macOS may warn about an unidentified developer — this is expected
   (the build is ad-hoc signed, not notarized).
   Alternative: System Settings -> Privacy & Security -> "Open Anyway".

Updating:
Drag the new version into Applications and confirm the replacement.
Your cores and settings are preserved — they live OUTSIDE the app, in
~/Library/Application Support/com.moonbot.moonterminal/.
README

hdiutil create -volname "MoonTerminal" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG"
echo "Built $DMG (version $VERSION)"
