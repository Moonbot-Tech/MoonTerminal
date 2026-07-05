#!/usr/bin/env bash
# Создаёт САМОПОДПИСАННЫЙ code-signing сертификат в login keychain и делает его
# ДОВЕРЕННЫМ для code signing (один раз на машину).
#
# Зачем: MoonTerminal хранит ключ шифрования конфига в macOS Keychain (крейт `keyring`
# c бэкендом apple-native, см. crates/moon-core). Keychain привязывает разрешение
# «этому приложению можно читать секрет» к ПОДПИСИ бинаря. Обычная ad-hoc подпись от
# `cargo build` меняется каждую сборку → Keychain видит «новое приложение» и заново
# требует пароль. Стабильная самоподписанная подпись фиксирует designated requirement,
# НО пока сертификат НЕДОВЕРЕННЫЙ, macOS не запоминает «Always Allow» и всё равно просит
# пароль каждую новую версию. Поэтому cert надо один раз сделать доверенным (trustRoot,
# политика codeSign) — тогда «Always Allow» запоминается навсегда.
#
# Приватный ключ личный и в репозиторий не коммитится — только этот скрипт, который
# каждый разработчик прогоняет у себя один раз (`make codesign-setup`). Шаг доверия
# ОДИН РАЗ спросит твой логин-пароль (GUI/терминал) — это нормально.
#
# Полный Xcode/SIP/firewall трогать НЕ нужно.
set -euo pipefail

IDENTITY="${1:-${MOON_CODESIGN_IDENTITY:-MoonTerminal Dev}}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo ">> codesign-setup: не macOS, пропускаю."
  exit 0
fi

LOGIN_KC="$(security login-keychain -d user 2>/dev/null | tr -d ' "' || true)"
[[ -n "$LOGIN_KC" ]] || LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# 1) Создать ключ+cert, если сертификата ещё нет (ищем БЕЗ -v: недоверенный тоже считается).
if security find-identity -p codesigning | grep -qF "$IDENTITY"; then
  echo ">> Сертификат '$IDENTITY' уже есть."
else
  cat > "$TMP/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions    = v3
prompt             = no
[dn]
CN = $IDENTITY
[v3]
basicConstraints   = critical,CA:false
keyUsage           = critical,digitalSignature
extendedKeyUsage   = critical,codeSigning
EOF
  echo ">> Генерирую ключ и самоподписанный сертификат '$IDENTITY'…"
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$TMP/key.pem" -out "$TMP/cert.pem" \
    -days 3650 -config "$TMP/openssl.cnf" >/dev/null 2>&1
  openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
    -out "$TMP/id.p12" -passout pass:moonterminal -name "$IDENTITY" >/dev/null 2>&1
  # -T /usr/bin/codesign → codesign сможет подписывать этим ключом без запроса.
  security import "$TMP/id.p12" -k "$LOGIN_KC" -P moonterminal \
    -T /usr/bin/codesign -T /usr/bin/security
  security set-key-partition-list -S apple-tool:,apple: -s -k "" "$LOGIN_KC" >/dev/null 2>&1 || true
  echo ">> Сертификат создан."
fi

# 2) Сделать cert ДОВЕРЕННЫМ для code signing, если ещё не доверен. Внимание:
#    `find-identity -v` показывает cert ДАЖЕ когда он недоверен — со строкой
#    "CSSMERR_TP_NOT_TRUSTED". Поэтому «доверен» = строка есть И без этой пометки.
if security find-identity -v -p codesigning | grep -F "$IDENTITY" | grep -qv "CSSMERR\|NOT_TRUSTED"; then
  echo ">> Сертификат '$IDENTITY' уже доверен для code signing — готово."
else
  echo ">> Делаю сертификат доверенным для code signing."
  echo ">> Сейчас macOS ОДИН РАЗ спросит твой логин-пароль — это нужно, чтобы 'Always"
  echo ">> Allow' в Keychain запоминалось и пароль при запуске больше не спрашивался."
  security find-certificate -c "$IDENTITY" -p "$LOGIN_KC" > "$TMP/trust.pem"
  security add-trusted-cert -r trustRoot -p codeSign -k "$LOGIN_KC" "$TMP/trust.pem"
  echo ">> Готово. Сертификат теперь доверенный."
fi

# 3) Переподписать debug-бинарь текущей (теперь доверенной) подписью, если он есть.
BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/debug/moonterminal"
if [[ -e "$BIN" ]]; then
  codesign --force --sign "$IDENTITY" --identifier "${MOON_BUNDLE_ID:-pro.moonbot.terminal}" "$BIN"
  echo ">> Переподписан: $BIN"
fi

echo ">> Всё. Следующий 'make run' ОДИН раз попросит 'Always Allow' → жми — и пароль"
echo ">> при запуске больше спрашиваться не будет."
