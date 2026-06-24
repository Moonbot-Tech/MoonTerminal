#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE="${PROFILE:-release}"
APP_DIR="${APP_DIR:-$ROOT/target/macos/MoonTerminal.app}"
SIGN_IDENTITY="${MOON_CODESIGN_IDENTITY:--}"
export TOOLCHAINS="${TOOLCHAINS:-com.apple.dt.toolchain.Metal}"

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

cat > "$APP_DIR/Contents/Info.plist" <<'PLIST'
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
  <string>0.1.0</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
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
