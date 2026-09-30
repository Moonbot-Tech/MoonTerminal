#!/bin/sh
# moon-station-admin: the root commands the terminal runs on its station.
#
# The administrator logs in by key only and runs sudo without a password (bootstrap.sh, decided
# 2026-09-30); this file is the station's own vocabulary on top of that — the commands every
# terminal version agrees on — not a limit on what the administrator may do. Nothing here
# changes who logs in, the unit, sudo, or any path outside the station's own.
set -eu
umask 077

BIN=/opt/moon-station/bin/moon-station
CONF=/etc/moon-station/station.toml
# A completed removal revokes in-flight writes from other terminals until explicit setup.
REMOVED=/etc/moon-station/removed
CREDS=/etc/moon-station/creds
DROPIN_DIR=/etc/systemd/system/moon-station.service.d
DROPIN=$DROPIN_DIR/credentials.conf
# The bot token's credential, beside the cores' keys.
TOKEN=telegram-token
# The bot's pairing (chats, owner, access): the station's own state, in its data root.
PAIRING=/var/lib/moon-station/telegram.json
# The USDT valuation cache, the station's own file once written.
VALUATION=/var/lib/moon-station/valuation.sqlite
# The running station's control API (moon_core::station_api), in its runtime directory.
API_SOCKET=/run/moon-station/api.sock
UNIT=moon-station.service
# How long a station just (re)started must stay up to count as healthy: longer than its slowest
# start-up check (the report replica's integrity pass, ~11 s on 1 vCPU).
HEALTH_S=30
# The last update's or rollback's own output, so its verdict survives a dropped SSH connection;
# `status` reports its last line — `update=running` / `rollback=running` while one is under way.
UPDATE_LOG=/opt/moon-station/update.log
# Where `update-from-release` has the installed binary download the latest release: under
# /opt/moon-station, which only root writes.
RELEASE_DIR=/opt/moon-station/release
# The station's request for an update from its bot (moon-station-update.path watches it).
UPDATE_REQUEST=/var/lib/moon-station/update.request
# The last update's verdict with its time, for the station to show in its status.
UPDATE_RESULT=/var/lib/moon-station/update.result
LOCK=/run/moon-station-admin.lock

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(id -u)" -eq 0 ] || die "run it through sudo"

# One changing command at a time: two updates, or an update and a rollback, would restart and
# judge each other's binary.
lock() {
    exec 9>"$LOCK"
    flock -n 9 || die "another station command is running"
}

# The same lock, waited for: a short write that only has to come after the command holding it.
lock_wait() {
    exec 9>"$LOCK"
    flock -w 30 9 || die "another station command is running"
}

# The same lock, waited for as long as an update may take: a request from the bot's chat that
# comes during another update runs after it, rather than being dropped.
lock_wait_long() {
    exec 9>"$LOCK"
    flock -w 600 9 || die "another station command has run for ten minutes"
}

# Called under the same lock as removal, before any credential/config write or service start.
# A preflight on another SSH connection cannot provide this guarantee.
require_station() {
    if [ -f "$REMOVED" ]; then
        echo "station=removed"
        die "station was removed; run setup again"
    fi
}

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
    for f in "$CREDS"/core-*.cred "$CREDS/$TOKEN.cred"; do
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

# put-token; stdin: the bot token. Encrypted straight from stdin, like a core key.
cmd_put_token() {
    tmp="$CREDS/.$TOKEN.new"
    systemd-creds encrypt --name="$TOKEN" - "$tmp"
    mv -f "$tmp" "$CREDS/$TOKEN.cred"
    write_dropin
    echo "cred=$TOKEN"
}

# get-token: decrypted directly to the admin SSH channel, never a plaintext file.
cmd_get_token() {
    [ -f "$CREDS/$TOKEN.cred" ] || die "no bot token"
    systemd-creds decrypt --name="$TOKEN" "$CREDS/$TOKEN.cred" -
}

# get-pairing: the same Access the station reads, including a never-paired bot.
cmd_get_pairing() {
    if [ -f "$PAIRING" ]; then
        cat "$PAIRING"
    else
        printf '{}\n'
    fi
}

cmd_drop_token() {
    rm -f "$CREDS/$TOKEN.cred"
    write_dropin
    echo "dropped=$TOKEN"
}

# get-config: station.toml as it is, so a change to one section keeps the others. It carries no
# key: put-config refuses one.
cmd_get_config() {
    [ -f "$CONF" ] || die "no station.toml"
    cat "$CONF"
}

# put-config [base]; stdin: station.toml. With <base> — the sha256 of the file the caller read, or
# `none` for no file yet — it is written only while the file is still that one, so what another
# terminal wrote in between is not overwritten: `config=changed`, exit 3, and the caller reads it
# again. Without it (a terminal older than this helper, which still takes it for its own) it is
# written as it comes. Replaced in one rename: a reader never sees half a file. Refused if it
# carries a key: keys live in credentials only.
cmd_put_config() {
    base=${1:-}
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
    if [ -n "$base" ]; then
        now=none
        [ -f "$CONF" ] && now=$(sha256sum "$CONF" | cut -d' ' -f1)
        if [ "$now" != "$base" ]; then
            rm -f "$tmp"
            echo "config=changed"
            exit 3
        fi
    fi
    install -m 640 -o root -g moon-station "$tmp" "$CONF.new"
    mv -f "$CONF.new" "$CONF"
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
    chmod 755 "$tmp" || die "chmod $tmp failed"
    if [ -f "$BIN" ]; then
        ln -f "$BIN" "$BIN.prev" || die "keeping the previous binary failed"
    fi
    mv -f "$tmp" "$BIN" || die "putting the new binary in place failed"
    echo "bin=$got"
}

# The station, just (re)started, stays up for $HEALTH_S seconds: active throughout, the same
# process, no restart by systemd (the unit restarts a crash after 5 s). Until the station has its
# API this is all a health check can see — not whether its cores connected.
healthy() {
    pid=$(systemctl show -p MainPID --value "$UNIT")
    restarts=$(systemctl show -p NRestarts --value "$UNIT")
    [ "$pid" != 0 ] || return 1
    i=0
    while [ "$i" -lt "$HEALTH_S" ]; do
        sleep 1
        i=$((i + 1))
        systemctl is-active --quiet "$UNIT" || return 1
        [ "$(systemctl show -p MainPID --value "$UNIT")" = "$pid" ] || return 1
        [ "$(systemctl show -p NRestarts --value "$UNIT")" = "$restarts" ] || return 1
    done
}

# Put .prev back, restart and check it. Fails when there is no .prev or it does not stay up. The
# .prev stays: a rollback that fails can be retried.
restore_prev() {
    [ -f "$BIN.prev" ] || die "no previous binary to roll back to"
    ln -f "$BIN.prev" "$BIN.back"
    mv -f "$BIN.back" "$BIN"
    systemctl restart "$UNIT"
    healthy || die "rolled back to $(sha256sum "$BIN" | cut -d' ' -f1), which does not stay up either"
    echo "rolled_back=$(sha256sum "$BIN" | cut -d' ' -f1)"
}

# update <sha256>; stdin: the binary. Installed as install-bin does; a station already enabled is
# restarted on it and must stay healthy, or the previous binary goes back and the update fails.
# One not enabled yet (the setup, before its cores) is only installed: `start` comes with them.
#
# The restart, the check and the rollback run detached (`finish-update`, its own session, its
# output in $UPDATE_LOG): a connection that drops during the minute they may take must not cut a
# rollback in half. This command waits for it and prints what it wrote.
cmd_update() {
    lock
    require_station
    echo "update=running" >"$UPDATE_LOG"
    install_and_restart "$@"
}

# The binary on stdin, checked against <sha256>, in place; then restarted and judged as `update`
# says. Under the lock, with `update=running` already in $UPDATE_LOG.
install_and_restart() {
    # Not a pipe: `sh` has no pipefail, and a refused binary must stop here.
    installed=$(cmd_install_bin "$@") || {
        echo "update=refused" >>"$UPDATE_LOG"
        exit 1
    }
    echo "$installed"
    if ! systemctl is-enabled --quiet "$UNIT"; then
        echo "health=not-started" | tee -a "$UPDATE_LOG"
        return
    fi
    # `update=running` stays the last line until the detached half writes its verdict.
    detached finish-update
}

# update-from-release: the latest release's station binary, installed as `update` installs one.
# The installed binary finds the release and downloads it (`moon-station release-fetch`: GitHub's
# immutable release, its SHA-256 digest — the terminal updater's own rules); nothing the station
# could write chooses it. Started by moon-station-update.service when the station files its
# request from the bot's chat, or by the terminal's "Update the service". All of it runs detached,
# the download too: a dropped connection leaves a verdict, not `update=running`.
cmd_update_from_release() {
    lock_wait_long
    require_station
    rm -f "$UPDATE_REQUEST"
    # The watcher of the bot's requests up again, should it have stopped or failed: the terminal
    # runs this command when a request from the chat was never taken.
    systemctl reset-failed moon-station-update.path 2>/dev/null || true
    systemctl start moon-station-update.path 2>/dev/null || true
    echo "update=running" >"$UPDATE_LOG"
    detached finish-update-from-release
}

# finish-update-from-release: the detached half of `update-from-release`.
cmd_finish_update_from_release() {
    trap 'rm -rf "$RELEASE_DIR"; publish_verdict "update from release"' EXIT
    rm -rf "$RELEASE_DIR"
    # Its stdout only: why it failed goes to $UPDATE_LOG through stderr.
    found=$("$BIN" release-fetch --out "$RELEASE_DIR/moon-station") || {
        echo "update=failed: the installed station could not fetch the latest release (the lines above say why)"
        exit 1
    }
    case "$found" in
    release=current | release=unversioned)
        echo "update=none $found"
        return
        ;;
    esac
    sha=$(printf '%s\n' "$found" | sed -n 's/^release=v[0-9.]* sha256=\([0-9a-f]\{64\}\)$/\1/p')
    if [ -z "$sha" ]; then
        echo "update=failed: unexpected answer from release-fetch: $found"
        exit 1
    fi
    echo "$found"
    cmd_install_bin "$sha" <"$RELEASE_DIR/moon-station" || {
        echo "update=refused"
        exit 1
    }
    if ! systemctl is-enabled --quiet "$UNIT"; then
        echo "health=not-started"
        return
    fi
    cmd_finish_update
}

# rollback: the previous binary back, by hand — detached like an update.
cmd_rollback() {
    lock
    require_station
    echo "rollback=running" >"$UPDATE_LOG"
    detached finish-rollback
}

# Run this helper's `$1` in a session of its own with its output appended to $UPDATE_LOG, wait,
# and print what it wrote; fail when it failed.
detached() {
    from=$(wc -l <"$UPDATE_LOG")
    rc=0
    setsid -w "$0" "$1" >>"$UPDATE_LOG" 2>&1 </dev/null || rc=$?
    tail -n +"$((from + 1))" "$UPDATE_LOG"
    [ "$rc" -eq 0 ] || exit "$rc"
}

# `<UTC time> <what>: <last line of $UPDATE_LOG>` where the station reads it: its status says how
# the last update or rollback ended — the chat that pressed "Update" learns it there. Called by
# the detached half as it exits, so a caller that went away does not leave an older verdict.
# Best-effort: a verdict that cannot be left does not fail the update.
publish_verdict() {
    [ -d "$(dirname "$UPDATE_RESULT")" ] || return 0
    tmp=$(mktemp) || return 0
    printf '%s %s: %s\n' "$(date -u +%Y-%m-%dT%H:%MZ)" "$1" "$(tail -n1 "$UPDATE_LOG")" >"$tmp"
    install -m 600 -o moon-station -g moon-station "$tmp" "$UPDATE_RESULT" || true
    rm -f "$tmp"
}

# finish-update: the detached half of `update`.
cmd_finish_update() {
    systemctl restart "$UNIT"
    if healthy; then
        echo "health=ok"
        return
    fi
    echo "health=failed"
    restore_prev
    die "the new binary did not stay up; the previous one is back"
}

# finish-rollback: the detached half of `rollback`.
cmd_finish_rollback() {
    restore_prev
    echo "health=ok"
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
    [ -f "$CREDS/$TOKEN.cred" ] && echo "token=yes" || echo "token=no"
    [ -f "$PAIRING" ] && echo "pairing=yes" || echo "pairing=no"
    [ -f "$VALUATION" ] && echo "valuation=yes" || echo "valuation=no"
    [ -S "$API_SOCKET" ] && echo "api=yes" || echo "api=no"
    echo "config_cas=yes"
    echo "update_path=$(systemctl is-active moon-station-update.path 2>/dev/null || true)"
    # The marker of this helper's version (the line added last): a terminal that does not see it
    # puts its own helper in place first.
    echo "release_update=yes"
    echo "bot_return=yes"
    echo "remove_station=yes"
    echo "removal_guard=yes"
    [ -s "$UPDATE_LOG" ] && echo "last_update=$(tail -n1 "$UPDATE_LOG")"
    return 0
}

# put-pairing; stdin: telegram.json — the chats, owner and access a terminal hands over with its
# bot. The station owns the file and rewrites it on every /pair, so it is written with the station
# stopped; the caller starts it again.
cmd_put_pairing() {
    tmp=$(mktemp)
    cat >"$tmp"
    head -c 1 "$tmp" | grep -q '{' || {
        rm -f "$tmp"
        die "telegram.json is not a JSON object"
    }
    systemctl stop "$UNIT"
    install -m 600 -o moon-station -g moon-station "$tmp" "$PAIRING"
    rm -f "$tmp"
    echo "pairing=written"
}

# ctl; stdin: one API request (JSON) — relayed to the running station by its own binary, the
# reply printed as one JSON line. The station checks the request; this only carries it.
cmd_ctl() {
    [ -S "$API_SOCKET" ] || die "the station is not running (no $API_SOCKET)"
    exec "$BIN" ctl --socket "$API_SOCKET"
}

# put-valuation; stdin: the terminal's valuation.sqlite, gzipped — its cached USDT rates, so a new
# station does not ask the exchanges for years of minutes again. Written with the station stopped;
# the caller starts it. The station re-derives the values from its own replica.
cmd_put_valuation() {
    tmp=$(mktemp)
    gzip -dc >"$tmp" || {
        rm -f "$tmp"
        die "valuation.sqlite: not a gzip stream"
    }
    [ "$(head -c 15 "$tmp")" = "SQLite format 3" ] || {
        rm -f "$tmp"
        die "valuation.sqlite: not an SQLite database"
    }
    systemctl stop "$UNIT"
    install -m 600 -o moon-station -g moon-station "$tmp" "$VALUATION"
    rm -f "$tmp" "$VALUATION-wal" "$VALUATION-shm"
    echo "valuation=written $(stat -c %s "$VALUATION")"
}

# drop-pairing: the chats of a bot taken off the station, so the next bot starts unpaired.
cmd_drop_pairing() {
    rm -f "$PAIRING"
    echo "dropped=pairing"
}

# Remove only material that can trade or act as the bot, after all station processes stop.
# Keep the administrator, authorized keys and SSH configuration for key-only access.
cmd_remove_station() {
    lock
    systemctl disable --now moon-station-update.path
    systemctl stop moon-station-update.service
    systemctl disable --now "$UNIT"
    # Publish revocation before deleting anything; failed deletions remain retryable, and no
    # other terminal can put a credential back after this lock is released.
    : >"$REMOVED"
    rm -f "$CREDS"/core-*.cred "$CREDS"/.core-*.new \
        "$CREDS/$TOKEN.cred" "$CREDS/.$TOKEN.new" \
        "$PAIRING" "$PAIRING.new" "$PAIRING.tmp" "$CONF" "$CONF.new" "$UPDATE_REQUEST"
    write_dropin
    echo "removed=yes"
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
put-cred)
    lock_wait
    require_station
    cmd_put_cred "$@"
    ;;
drop-cred)
    lock_wait
    require_station
    cmd_drop_cred "$@"
    ;;
put-token)
    lock_wait
    require_station
    cmd_put_token
    ;;
drop-token)
    lock_wait
    require_station
    cmd_drop_token
    ;;
get-token) cmd_get_token ;;
get-pairing) cmd_get_pairing ;;
get-config) cmd_get_config ;;
put-config)
    lock_wait
    require_station
    cmd_put_config "$@"
    ;;
install-bin)
    lock
    require_station
    cmd_install_bin "$@"
    ;;
update) cmd_update "$@" ;;
update-from-release) cmd_update_from_release ;;
rollback) cmd_rollback ;;
finish-update)
    trap 'publish_verdict update' EXIT
    cmd_finish_update
    ;;
finish-update-from-release) cmd_finish_update_from_release ;;
finish-rollback)
    trap 'publish_verdict rollback' EXIT
    cmd_finish_rollback
    ;;
# start: enabled for every boot, and (re)started so new credentials and config take effect. Every
# command that restarts or stops the station waits for no update: a restart in the middle of an
# update's health check would roll a good binary back.
start)
    lock
    require_station
    systemctl enable --quiet "$UNIT"
    systemctl restart "$UNIT"
    ;;
restart)
    lock
    require_station
    systemctl restart "$UNIT"
    ;;
# reload: station.toml again without a restart (SIGHUP) — for a changed tape window or a core
# taken out; a core added still needs `start` for its credential.
reload)
    lock
    require_station
    systemctl reload "$UNIT"
    ;;
stop)
    lock
    systemctl stop "$UNIT"
    ;;
status) cmd_status ;;
remove-station) cmd_remove_station ;;
logs) cmd_logs "$@" ;;
put-pairing)
    lock
    require_station
    cmd_put_pairing
    ;;
ctl) cmd_ctl ;;
put-valuation)
    lock
    require_station
    cmd_put_valuation
    ;;
drop-pairing)
    lock
    cmd_drop_pairing
    ;;
*) die "unknown command: $cmd" ;;
esac
