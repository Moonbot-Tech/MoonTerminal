#!/usr/bin/env bash
# Подписывает собранный бинарь moonterminal СТАБИЛЬНОЙ самоподписанной подписью,
# чтобы macOS Keychain не спрашивал пароль при каждом запуске. Подробнее про причину —
# в macos-codesign-setup.sh. Вызывается автоматически из `make build`/`make run`.
#
# Использование: scripts/macos-sign.sh <путь-к-бинарю>
# На не-macOS — тихий no-op (Keychain-проблемы там нет).
set -euo pipefail

BIN="${1:?usage: macos-sign.sh <path-to-binary>}"
IDENTITY="${MOON_CODESIGN_IDENTITY:-MoonTerminal Dev}"
# Фиксированный identifier — вторая половина стабильного designated requirement.
# Совпадает с CFBundleIdentifier из scripts/macos-bundle.sh.
BUNDLE_ID="${MOON_BUNDLE_ID:-pro.moonbot.terminal}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

if [[ ! -e "$BIN" ]]; then
  echo ">> macos-sign: бинарь '$BIN' не найден, пропускаю." >&2
  exit 0
fi

# Создать сертификат при первом запуске (идемпотентно).
if ! security find-identity -p codesigning | grep -qF "$IDENTITY"; then
  "$(dirname "${BASH_SOURCE[0]}")/macos-codesign-setup.sh" "$IDENTITY"
fi

codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$BIN"
echo ">> Подписано '$IDENTITY' / $BUNDLE_ID: $BIN"
