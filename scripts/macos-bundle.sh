#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE="${PROFILE:-release}"
APP_DIR="${APP_DIR:-$ROOT/target/macos/MoonTerminal.app}"
SIGN_IDENTITY="${MOON_CODESIGN_IDENTITY:--}"
export TOOLCHAINS="${TOOLCHAINS:-com.apple.dt.toolchain.Metal}"

# Version shown by Finder and About. Taken from the same input as the update baseline the binary
# embeds (crates/moon-ui-gpui/build.rs): the greatest stable tag reachable from the built commit.
# A hardcoded constant here claimed 0.1.0 for every build a developer inspected.
#
# The value is interpolated into Info.plist, so it is validated rather than trusted — including
# MOON_BUNDLE_VERSION, which is the one path that does not come from the tag filter.
VERSION="${MOON_BUNDLE_VERSION:-}"
if [[ -z "$VERSION" ]]; then
  VERSION="$(git -C "$ROOT" tag --merged HEAD --sort=-version:refname --list 'v*' 2>/dev/null \
    | grep -E '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))?$' | head -n 1 || true)"
  if [[ -z "$VERSION" ]]; then
    echo "warning: no stable tag reachable from HEAD; bundling as 0.0.0" >&2
  fi
fi
VERSION="${VERSION#v}"
VERSION="${VERSION:-0.0.0}"
if [[ ! "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))?$ ]]; then
  echo "refusing to bundle a non-numeric CFBundleVersion: $VERSION" >&2
  exit 1
fi

build_args=(build -p moon-ui-gpui --bin moonterminal)
if [[ "$PROFILE" == "release" ]]; then
  build_args+=(--release)
fi
if [[ -n "${FEATURES:-}" ]]; then
  build_args+=(--features "$FEATURES")
fi

cargo "${build_args[@]}"

BIN_DIR="$ROOT/target/$PROFILE"
BIN="$BIN_DIR/moonterminal"
if [[ ! -x "$BIN" ]]; then
  echo "missing built binary: $BIN" >&2
  exit 1
fi

rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>MoonTerminal</string>
  <key>CFBundleIdentifier</key>
  <string>pro.moonbot.terminal</string>
  <key>CFBundleName</key>
  <string>MoonTerminal</string>
  <key>CFBundleDisplayName</key>
  <string>MoonTerminal</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$APP_DIR/Contents/PkgInfo"
cp "$BIN" "$APP_DIR/Contents/MacOS/MoonTerminal"
chmod +x "$APP_DIR/Contents/MacOS/MoonTerminal"

codesign --force --sign "$SIGN_IDENTITY" "$APP_DIR"
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

echo "$APP_DIR"
