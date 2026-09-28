#!/bin/sh
# moon-station-admin: the root commands the terminal runs on its station without a password.
#
# sudoers: `<admin> ALL=(root) NOPASSWD: /usr/local/sbin/moon-station-admin`. That rule is only
# narrow because this file is: nothing here changes who logs in, the unit, sudo, or any path
# outside the station's own. Everything else needs the administrator's password.
set -eu
umask 077

BIN=/opt/moon-station/bin/moon-station
CONF=/etc/moon-station/station.toml
CREDS=/etc/moon-station/creds
DROPIN_DIR=/etc/systemd/system/moon-station.service.d
DROPIN=$DROPIN_DIR/credentials.conf
UNIT=moon-station.service

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(id -u)" -eq 0 ] || die "run it through sudo"

valid_uid() {
    case "$1" in
    '' | 0 | *[!0-9]*) return 1 ;;
    esac
    [ "${#1}" -le 20 ]
}

# The unit loads exactly the credentials that exist, each under its own name.
write_dropin() {
    tmp=$(mktemp)
    echo "[Service]" >"$tmp"
    for f in "$CREDS"/core-*.cred; do
        [ -e "$f" ] || continue
        echo "LoadCredentialEncrypted=$(basename "$f" .cred):$f" >>"$tmp"
    done
    install -d -m 755 "$DROPIN_DIR"
    if [ -f "$DROPIN" ] && cmp -s "$tmp" "$DROPIN"; then
        rm -f "$tmp"
    else
        install -m 644 "$tmp" "$DROPIN"
        rm -f "$tmp"
        systemctl daemon-reload
    fi
}

# put-cred <uid>; stdin: the core key. Encrypted straight from stdin: the key is never a file.
cmd_put_cred() {
    uid=${1:-}
    valid_uid "$uid" || die "not a core uid: $uid"
    tmp="$CREDS/.core-$uid.new"
    systemd-creds encrypt --name="core-$uid" - "$tmp"
    mv -f "$tmp" "$CREDS/core-$uid.cred"
    write_dropin
    echo "cred=core-$uid"
}

cmd_drop_cred() {
    uid=${1:-}
    valid_uid "$uid" || die "not a core uid: $uid"
    rm -f "$CREDS/core-$uid.cred"
    write_dropin
    echo "dropped=core-$uid"
}

# put-config; stdin: station.toml. Refused if it carries a key: keys live in credentials only.
cmd_put_config() {
    tmp=$(mktemp)
    cat >"$tmp"
    grep -q '^\[\[core\]\]' "$tmp" || {
        rm -f "$tmp"
        die "station.toml lists no [[core]]"
    }
    if grep -Eq '^[[:space:]]*key[[:space:]]*=' "$tmp"; then
        rm -f "$tmp"
        die "station.toml must not carry a key"
    fi
    install -m 640 -o root -g moon-station "$tmp" "$CONF"
    rm -f "$tmp"
    echo "config=written"
}

# install-bin <sha256>; stdin: the binary. Checked before it replaces anything; the old one stays
# as .prev for a rollback.
cmd_install_bin() {
    want=${1:-}
    case "$want" in
    *[!0-9a-f]* | '') die "not a sha256: $want" ;;
    esac
    [ "${#want}" -eq 64 ] || die "not a sha256: $want"
    tmp="$BIN.new"
    cat >"$tmp"
    got=$(sha256sum "$tmp" | cut -d' ' -f1)
    if [ "$got" != "$want" ]; then
        rm -f "$tmp"
        die "sha256 mismatch: got $got"
    fi
    chmod 755 "$tmp"
    [ -f "$BIN" ] && ln -f "$BIN" "$BIN.prev"
    mv -f "$tmp" "$BIN"
    echo "bin=$got"
}

cmd_status() {
    echo "active=$(systemctl is-active "$UNIT" 2>/dev/null || true)"
    echo "enabled=$(systemctl is-enabled "$UNIT" 2>/dev/null || true)"
    if [ -f "$BIN" ]; then
        echo "bin=$(sha256sum "$BIN" | cut -d' ' -f1)"
    else
        echo "bin=none"
    fi
    [ -f "$CONF" ] && echo "config=yes" || echo "config=no"
    creds=$( (cd "$CREDS" && ls core-*.cred 2>/dev/null | sed 's/\.cred$//' | tr '\n' ' ') || true)
    echo "creds=${creds% }"
}

cmd_logs() {
    lines=${1:-100}
    case "$lines" in
    '' | *[!0-9]*) die "not a line count: $lines" ;;
    esac
    [ "$lines" -le 5000 ] || lines=5000
    journalctl -u "$UNIT" -n "$lines" --no-pager -o short-iso
}

cmd=${1:-}
[ "$#" -gt 0 ] && shift
case "$cmd" in
put-cred) cmd_put_cred "$@" ;;
drop-cred) cmd_drop_cred "$@" ;;
put-config) cmd_put_config ;;
install-bin) cmd_install_bin "$@" ;;
# start: enabled for every boot, and (re)started so new credentials and config take effect.
start)
    systemctl enable --quiet "$UNIT"
    systemctl restart "$UNIT"
    ;;
restart) systemctl restart "$UNIT" ;;
stop) systemctl stop "$UNIT" ;;
status) cmd_status ;;
logs) cmd_logs "$@" ;;
*) die "unknown command: $cmd" ;;
esac
