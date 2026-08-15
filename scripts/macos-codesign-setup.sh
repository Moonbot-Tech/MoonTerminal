#!/usr/bin/env bash
# Creates a SELF-SIGNED code-signing certificate in the login keychain and makes it
# TRUSTED for code signing (once per machine).
#
# Why: MoonTerminal stores the config encryption key in macOS Keychain (the `keyring`
# crate with the apple-native backend; see crates/moon-core). Keychain ties the permission
# "this application may read the secret" to the binary's SIGNATURE. The regular ad-hoc
# signature from `cargo build` changes with every build, so Keychain sees a "new application"
# and asks for the password again. A stable self-signed signature fixes the designated
# requirement, BUT while the certificate is UNTRUSTED, macOS does not remember "Always Allow"
# and still asks for the password for every new version. Therefore, the certificate must be
# trusted once (trustRoot, codeSign policy), after which "Always Allow" is remembered permanently.
#
# The private key is personal and is never committed to the repository; only this script is.
# Each developer runs it once locally (`make codesign-setup`). The trust step asks for your
# login password ONCE (in the GUI or terminal), which is expected.
#
# There is NO need to modify full Xcode, SIP, or the firewall.
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

# 1) Create the key and certificate if it does not exist (search WITHOUT -v: untrusted also counts).
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
  # -T /usr/bin/codesign lets codesign use this key without prompting.
  security import "$TMP/id.p12" -k "$LOGIN_KC" -P moonterminal \
    -T /usr/bin/codesign -T /usr/bin/security
  security set-key-partition-list -S apple-tool:,apple: -s -k "" "$LOGIN_KC" >/dev/null 2>&1 || true
  echo ">> Сертификат создан."
fi

# 2) Make the certificate TRUSTED for code signing if it is not already. Note:
#    `find-identity -v` shows the certificate EVEN when it is untrusted, with the
#    "CSSMERR_TP_NOT_TRUSTED" string. Therefore, "trusted" means the line exists AND lacks this marker.
#    The alternation needs -E: as a basic regex `\|` is a GNU extension that BSD grep reads as a
#    literal pipe, so the marker never matched and every certificate looked trusted.
if security find-identity -v -p codesigning | grep -F "$IDENTITY" | grep -qvE "CSSMERR|NOT_TRUSTED"; then
  echo ">> Сертификат '$IDENTITY' уже доверен для code signing — готово."
else
  echo ">> Делаю сертификат доверенным для code signing."
  echo ">> Сейчас macOS ОДИН РАЗ спросит твой логин-пароль — это нужно, чтобы 'Always"
  echo ">> Allow' в Keychain запоминалось и пароль при запуске больше не спрашивался."
  security find-certificate -c "$IDENTITY" -p "$LOGIN_KC" > "$TMP/trust.pem"
  security add-trusted-cert -r trustRoot -p codeSign -k "$LOGIN_KC" "$TMP/trust.pem"
  echo ">> Готово. Сертификат теперь доверенный."
fi

# 3) Re-sign the debug binary with the current (now trusted) signature if it exists.
BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/debug/moonterminal"
if [[ -e "$BIN" ]]; then
  codesign --force --sign "$IDENTITY" --identifier "${MOON_BUNDLE_ID:-pro.moonbot.terminal}" "$BIN"
  echo ">> Переподписан: $BIN"
fi

echo ">> Всё. Следующий 'make run' ОДИН раз попросит 'Always Allow' → жми — и пароль"
echo ">> при запуске больше спрашиваться не будет."
