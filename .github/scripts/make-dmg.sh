#!/usr/bin/env bash
# Package the staged macOS arm64 binary into MoonTerminal.app and a distributable .dmg.
# Runs on the macos-15 runner (native tools: sips, iconutil, codesign, hdiutil, lipo).
#
# release.yml compiles the binary in its arm64 matrix job and downloads its artifact into `stage/`.
# Assemble the bundle directly inside the DMG staging root and delete inputs once consumed to keep
# runner disk use low.
set -euo pipefail

BIN_ARM64="stage/moonterminal-arm64"
# The bundle IS the one that gets imaged: `dmg-root` is what `hdiutil` reads at the end.
DMG_ROOT="dist/dmg-root"
APP="$DMG_ROOT/MoonTerminal.app"
DMG="dist/MoonTerminal.dmg"
SRC_ICON="assets/icons/0.png"

# Version from the validated release tag (v0.0.1 -> 0.0.1), passed in positionally like every
# other release script takes it. GITHUB_REF_NAME is only the fallback: on workflow_dispatch it is
# the branch, and "main" is not a CFBundleVersion — which is why the result is validated.
VERSION="${1:-${GITHUB_REF_NAME:-0.0.0}}"
VERSION="${VERSION#v}"
if [[ ! "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))?$ ]]; then
  echo "refusing to bundle a non-numeric CFBundleVersion: $VERSION" >&2
  exit 1
fi

rm -rf dist
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

EXE="$APP/Contents/MacOS/moonterminal"
cp "$BIN_ARM64" "$EXE"
chmod +x "$EXE"
# The binary now lives inside the app; the staged copy would only consume runner disk.
rm -rf stage

# Build a multi-resolution .icns from the single PNG app icon.
ICONSET="dist/AppIcon.iconset"
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$SRC_ICON" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" "$SRC_ICON" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
# The .icns is written; the intermediate PNG set is only an input to it.
rm -rf "$ICONSET"

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

# Ad-hoc signature: not notarized, but seals the bundle so it launches once the user allows it
# (see the README written below). Full Developer-ID signing + notarization needs an Apple cert in
# secrets — still not set up. Never swallow a signing failure: Apple silicon refuses to execute
# code whose signature is missing or broken.
codesign --force --deep --sign - "$APP"

# Finish the Finder-friendly DMG staging root instead of imaging the bare .app: the app (already
# assembled in place above), a drag-to-install alias to /Applications, and a short RU readme. The
# image is built from this folder so the user sees the familiar "drag into
# Applications" layout. (Background image / .DS_Store window layout intentionally
# omitted for now — can be layered on later.)
ln -s /Applications "$DMG_ROOT/Applications"

cat > "$DMG_ROOT/README.txt" <<'README'
MoonTerminal — Installation

Apple silicon only (arm64).
Requires macOS 11 or newer.

1. Drag MoonTerminal.app into the Applications folder (alias next to it).
2. First launch: double-click the app, then open
   System Settings -> Privacy & Security and press "Open Anyway".
   macOS warns about an unidentified developer — this is expected
   (the build is ad-hoc signed, not notarized).
   On macOS 14 and older, right-click -> "Open" -> "Open" also works;
   macOS 15 (Sequoia) removed that shortcut.

Updating:
Drag the new version into Applications and confirm the replacement.
Your cores and settings are preserved — they live OUTSIDE the app, in
~/Library/Application Support/com.moonbot.moonterminal/.
README

# Verify the architecture and signature of the exact bundle that gets imaged.
STAGED_APP="$APP"
ARCHS="$(lipo -archs "$STAGED_APP/Contents/MacOS/moonterminal")"
if [ "$ARCHS" != "arm64" ]; then
  echo "make-dmg: expected an arm64-only binary (has: $ARCHS)" >&2
  exit 1
fi
codesign --verify --strict "$STAGED_APP"
echo "Imaging an Apple silicon bundle: $ARCHS"

# Size the image EXPLICITLY. Left to itself, `hdiutil create -srcfolder` derives the size from
# the source with a margin so thin that 22 KB of binary growth overflowed it: v0.32.2 failed
# twice with "hdiutil: create failed - No space left on device" and "could not access
# /Volumes/MoonTerminal/MoonTerminal.app/Contents/MacOS/moonterminal" — a path INSIDE the
# mounted image, not on the runner disk, which is what distinguishes this from the disk
# exhaustion the header describes. The margin costs nothing in the artifact: UDZO stores only
# the used blocks, so the shipped .dmg stays the size of its contents.
CONTENT_KB="$(du -sk "$DMG_ROOT" | cut -f1)"
hdiutil create -volname "MoonTerminal" -srcfolder "$DMG_ROOT" -ov -size "$((CONTENT_KB + 65536))k" -format UDZO "$DMG"
echo "Built $DMG (version $VERSION)"
