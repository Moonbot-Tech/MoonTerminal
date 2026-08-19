#!/usr/bin/env bash
# Package the macOS release binaries into a UNIVERSAL MoonTerminal.app and a distributable .dmg.
# Runs on the macos-14 runner (native tools: sips, iconutil, codesign, hdiutil, lipo).
#
# Expects BOTH slices to be staged already: release.yml builds them in two steps and moves each
# out of its target dir, because the runner cannot hold two `--release` trees at once. They are
# joined here so Apple Silicon and Intel Macs share one download.
#
# Disk is the scarce resource on this runner, not time — `hdiutil` has failed with "No space left
# on device" on a tree that merely LOOKED finished. So the bundle is assembled directly inside the
# DMG staging root (never built beside it and copied, which held two universal binaries at once),
# and every input is deleted the moment it has been consumed.
set -euo pipefail

BIN_ARM64="stage/moonterminal-arm64"
BIN_X86_64="stage/moonterminal-x86_64"
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
lipo -create "$BIN_ARM64" "$BIN_X86_64" -output "$EXE"
chmod +x "$EXE"
# Both slices now live inside the universal binary; keeping them costs the size of a third copy.
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
# secrets — still not set up.
#
# Deliberately NOT swallowed with `|| true` any more: Apple Silicon refuses to execute code whose
# signature is missing or broken, while Intel does not care, so a silently failed codesign now
# ships one download that starts on the Intel half of the audience and is killed on the other.
codesign --force --deep --sign - "$APP"

# Finish the Finder-friendly DMG staging root instead of imaging the bare .app: the app (already
# assembled in place above), a drag-to-install alias to /Applications, and a short RU readme. The
# image is built from this folder so the user sees the familiar "drag into
# Applications" layout. (Background image / .DS_Store window layout intentionally
# omitted for now — can be layered on later.)
ln -s /Applications "$DMG_ROOT/Applications"

cat > "$DMG_ROOT/README.txt" <<'README'
MoonTerminal — Installation

Universal build: one app for both Apple Silicon and Intel Macs.
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

# Check the bundle that actually gets imaged — which is this one, assembled inside `dmg-root` from
# the start rather than copied in. Both properties are the ones a user meets on first launch and
# neither has a second chance once the .dmg is on the Releases page: every architecture present,
# and a signature that verifies.
STAGED_APP="$APP"
ARCHS="$(lipo -archs "$STAGED_APP/Contents/MacOS/moonterminal")"
for want in arm64 x86_64; do
  case " $ARCHS " in
  *" $want "*) ;;
  *)
    echo "make-dmg: the imaged binary is missing the $want slice (has: $ARCHS)" >&2
    exit 1
    ;;
  esac
done
codesign --verify --strict "$STAGED_APP"
echo "Imaging a universal bundle: $ARCHS"

hdiutil create -volname "MoonTerminal" -srcfolder "$DMG_ROOT" -ov -format UDZO "$DMG"
echo "Built $DMG (version $VERSION)"
