#!/usr/bin/env bash
# Signs the built moonterminal binary with a STABLE self-signed signature so that
# macOS Keychain does not ask for the password on every launch. See
# macos-codesign-setup.sh for details. Called automatically by `make build`/`make run`.
#
# Usage: scripts/macos-sign.sh <path-to-binary>
# On non-macOS systems, this is a silent no-op (the Keychain issue does not apply).
set -euo pipefail

BIN="${1:?usage: macos-sign.sh <path-to-binary>}"
IDENTITY="${MOON_CODESIGN_IDENTITY:-MoonTerminal Dev}"
# The fixed identifier is the second half of the stable designated requirement.
# It matches the CFBundleIdentifier from scripts/macos-bundle.sh.
BUNDLE_ID="${MOON_BUNDLE_ID:-pro.moonbot.terminal}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

if [[ ! -e "$BIN" ]]; then
  echo ">> macos-sign: бинарь '$BIN' не найден, пропускаю." >&2
  exit 0
fi

# Create the certificate on the first run (idempotently).
if ! security find-identity -p codesigning | grep -qF "$IDENTITY"; then
  "$(dirname "${BASH_SOURCE[0]}")/macos-codesign-setup.sh" "$IDENTITY"
fi

codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$BIN"
echo ">> Подписано '$IDENTITY' / $BUNDLE_ID: $BIN"
