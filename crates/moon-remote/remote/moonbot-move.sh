#!/bin/sh
# Installs a MoonBot core as a systemd instance, as root: a moved core (moon-remote's core_move)
# or a fresh one (core_new). One step per call:
#   check   <udp port>                          — the server can run MoonBot and the port is free;
#                                                 CHECK_* lines tell its OS, memory and firewall
#   install <work dir> <instance dir> <udp port> <package url>
#                                               — package + settings and the service, not started
#   install-fresh <instance dir> <udp port> <package url>
#                                               — package and the service, started on the core's
#                                                 own default settings, ready for its commands
#   start   <instance dir>                      — start the core, wait until it took its key and
#                                                 listens for MoonProto
#   finish  <instance dir>                      — drop the first-start INI switch the move added
#   mb      <instance dir> <arguments…>         — one command of the core's own CLI; stdin passes
#                                                 through
#   remove  <instance dir>                      — stop and delete an instance this script made
# The work dir holds state.zip (settings archive), key (MoonProto connection packet) and, when
# the move added it, ini-flag; install deletes it.
set -eu
# dash, /bin/sh on Debian and Ubuntu, skips the EXIT trap when a signal kills it; exiting from
# the signal runs it, so a dropped connection still cleans up.
trap 'exit 1' HUP INT TERM PIPE

marker=.moonterminal-move

# The service unit of an instance directory.
unit_of() {
    systemd-escape --path --template=moonbot@.service "$1"
}

# Whether something listens on UDP port $1.
udp_bound() {
    test -n "$(ss -H -uan "sport = :$1")"
}

# Whether ufw is installed and active.
ufw_active() {
    command -v ufw >/dev/null 2>&1 && LC_ALL=C ufw status | grep -q '^Status: active'
}

# The absolute instance directory for $1: a relative name lives under /root. Every step resolves
# it the same way, so finish and remove find what install made.
resolve_dir() {
    case "$1" in
    /*) dir=$1 ;;
    *) dir="/root/$1" ;;
    esac
    realpath -m -- "$dir"
}

step=$1
shift
case "$step" in
check)
    port=$1
    arch=$(uname -m)
    test "$arch" = x86_64 || { echo "MoonBot for Linux needs x86_64; this server is $arch" >&2; exit 1; }
    libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || echo 'glibc 0')
    version=${libc#glibc }
    lowest=$(printf '2.38\n%s\n' "$version" | sort -V | head -n 1)
    test "$lowest" = 2.38 || { echo "MoonBot for Linux needs glibc 2.38 or newer; this server has $libc" >&2; exit 1; }
    test -d /run/systemd/system || { echo 'MoonBot for Linux runs as a systemd service; this server has no systemd' >&2; exit 1; }
    missing=''
    for tool in curl unzip ss; do
        command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
    done
    if test -n "$missing"; then
        command -v apt-get >/dev/null 2>&1 || { echo "Install on the server:$missing" >&2; exit 1; }
        packages=$(echo "$missing" | sed 's/ ss$/ iproute2/; s/ ss / iproute2 /')
        echo "Installing:$packages"
        apt-get update >/dev/null
        DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends $packages >/dev/null
    fi
    ! udp_bound "$port" || { echo "UDP port $port is already in use on this server: choose another" >&2; exit 1; }
    os=$( (. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-}") || true)
    printf 'CHECK_OS=%s\nCHECK_GLIBC=%s\n' "$os" "$version"
    awk '/^MemTotal:/ { printf "CHECK_MEM_TOTAL_MB=%d\n", $2 / 1024 }
         /^MemAvailable:/ { printf "CHECK_MEM_AVAILABLE_MB=%d\n", $2 / 1024 }' /proc/meminfo
    if ufw_active; then echo CHECK_UFW=active; else echo CHECK_UFW=off; fi
    echo CHECK_OK
    ;;
install)
    work=$1
    base=$2
    port=$3
    url=$4
    # The work dir carries the exchange API keys: it goes on every exit from here on.
    trap 'rm -rf -- "$work"' EXIT
    base=$(resolve_dir "$base")
    test "$base" != / || { echo 'Choose an instance directory, not /' >&2; exit 1; }
    dest=$base
    n=2
    while test -e "$dest" || test -L "$dest"; do
        dest="$base-$n"
        n=$((n + 1))
    done
    unit=$(unit_of "$dest")
    test ! -e "/etc/systemd/system/$unit" || { echo "The service $unit already exists" >&2; exit 1; }
    umask 077
    mkdir -p -- "$(dirname -- "$dest")"
    mkdir -- "$dest"
    opened=''
    cleanup() {
        code=$?
        if test "$code" != 0; then
            if test -n "$opened"; then
                ufw delete allow "$port/udp" >/dev/null 2>&1 || true
            fi
            rm -rf -- "$dest"
        fi
        rm -rf -- "$work"
        exit "$code"
    }
    trap cleanup EXIT
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 600 --output "$work/package.zip" "$url"
    unzip -q "$work/package.zip" -d "$dest"
    (cd "$dest" && sha256sum --quiet --check SHA256SUMS)
    unzip -Z1 "$work/state.zip" | while IFS= read -r name; do
        case "$name" in */) continue ;; esac
        test ! -e "$dest/$name" || { echo "The settings would overwrite the program file $name" >&2; exit 1; }
    done
    unzip -q "$work/state.zip" -d "$dest"
    test -f "$dest/BotConfig.bcfg" || { echo 'The settings archive has no BotConfig.bcfg' >&2; exit 1; }
    install -m 600 "$work/key" "$dest/MoonProtoSetup.key"
    printf '%s\n' "$port" > "$dest/$marker"
    if test -e "$work/ini-flag"; then
        : > "$dest/$marker.ini-flag"
    fi
    chmod 700 "$dest/MoonBot"
    install -m 644 "$dest/moonbot@.service" /etc/systemd/system/moonbot@.service
    # An inbound IPv4 allow from anywhere that was there before stays the user's: only a rule
    # opened here is marked for removal. Any other rule for the port (deny, v6 only, one source)
    # does not let the core be reached, so the allow is still added.
    if ufw_active && ! LC_ALL=C ufw status |
        grep -q -E "^${port}/udp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+Anywhere[[:space:]]*(#.*)?\$"; then
        ufw allow "$port/udp" comment "MoonBot $dest" >/dev/null
        opened=1
        echo ufw >> "$dest/$marker"
    fi
    systemctl daemon-reload
    # Not started here: the old core must be down first, or two cores trade one account.
    trap - EXIT
    rm -rf -- "$work"
    printf 'MOVE_DIRECTORY=%s\nMOVE_SERVICE=%s\n' "$dest" "$unit"
    ;;
install-fresh)
    base=$1
    port=$2
    url=$3
    base=$(resolve_dir "$base")
    test "$base" != / || { echo 'Choose an instance directory, not /' >&2; exit 1; }
    dest=$base
    n=2
    while test -e "$dest" || test -L "$dest"; do
        dest="$base-$n"
        n=$((n + 1))
    done
    unit=$(unit_of "$dest")
    test ! -e "/etc/systemd/system/$unit" || { echo "The service $unit already exists" >&2; exit 1; }
    umask 077
    work=''
    made=''
    opened=''
    started=''
    cleanup() {
        code=$?
        if test "$code" != 0; then
            if test -n "$started"; then
                journalctl -u "$unit" -n 30 --no-pager >&2 || true
                systemctl disable --now "$unit" >/dev/null 2>&1 || true
            fi
            if test -n "$opened"; then
                ufw delete allow "$port/udp" >/dev/null 2>&1 || true
            fi
            if test -n "$made"; then
                rm -rf -- "$dest"
            fi
        fi
        if test -n "$work"; then
            rm -rf -- "$work"
        fi
        exit "$code"
    }
    trap cleanup EXIT
    work=$(mktemp -d /tmp/moonbot-move.XXXXXXXX)
    mkdir -p -- "$(dirname -- "$dest")"
    mkdir -- "$dest"
    made=1
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 600 --output "$work/package.zip" "$url"
    unzip -q "$work/package.zip" -d "$dest"
    (cd "$dest" && sha256sum --quiet --check SHA256SUMS)
    test -f "$dest/mb" || { echo 'This MoonBot package has no command line (mb): a newer package is needed' >&2; exit 1; }
    printf '%s\n' "$port" > "$dest/$marker"
    chmod 700 "$dest/MoonBot" "$dest/mb"
    install -m 644 "$dest/moonbot@.service" /etc/systemd/system/moonbot@.service
    # As in install: only an allow opened here is marked for removal.
    if ufw_active && ! LC_ALL=C ufw status |
        grep -q -E "^${port}/udp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+Anywhere[[:space:]]*(#.*)?\$"; then
        ufw allow "$port/udp" comment "MoonBot $dest" >/dev/null
        opened=1
        echo ufw >> "$dest/$marker"
    fi
    systemctl daemon-reload
    started=1
    systemctl enable --now "$unit" >/dev/null 2>&1
    # A fresh core writes its default settings on the first start; its command channel answers
    # once it is up.
    # Bounded by the clock, not by passes: one hung probe takes its whole 10 s.
    deadline=$(($(date +%s) + 90))
    until (cd "$dest" && timeout 10 ./mb --status >/dev/null 2>&1); do
        systemctl is-active --quiet "$unit" || { echo 'The core stopped right after its start' >&2; exit 1; }
        test "$(date +%s)" -lt "$deadline" || { echo 'The core does not answer its commands within 90 s' >&2; exit 1; }
        sleep 1
    done
    trap - EXIT
    rm -rf -- "$work"
    printf 'MOVE_DIRECTORY=%s\nMOVE_SERVICE=%s\n' "$dest" "$unit"
    ;;
mb)
    dest=$(resolve_dir "$1")
    shift
    # Only an instance this script made: the commands reconfigure a running core.
    test -f "$dest/$marker" || { echo "$dest was not installed by MoonTerminal" >&2; exit 1; }
    # The wrapper finds its instance by the working directory and runs as root, without the
    # service's library path: system tools it calls must not see the core's libraries.
    cd "$dest"
    exec ./mb "$@"
    ;;
start)
    dest=$(resolve_dir "$1")
    test -f "$dest/$marker" || { echo "$dest was not installed by a MoonTerminal move" >&2; exit 1; }
    unit=$(unit_of "$dest")
    port=$(head -n 1 "$dest/$marker")
    # A failed start stops the core again and keeps the instance, for a retry or remove.
    stop_on_error() {
        code=$?
        if test "$code" != 0; then
            journalctl -u "$unit" -n 30 --no-pager >&2 || true
            systemctl disable --now "$unit" >/dev/null 2>&1 || true
        fi
        exit "$code"
    }
    trap stop_on_error EXIT
    systemctl enable --now "$unit" >/dev/null 2>&1
    # The core takes the key file on its first start and deletes it.
    i=0
    while test -e "$dest/MoonProtoSetup.key"; do
        systemctl is-active --quiet "$unit" || { echo 'The core stopped before taking its MoonProto key' >&2; exit 1; }
        i=$((i + 1))
        test "$i" -le 60 || { echo 'The core did not take its MoonProto key within 60 s' >&2; exit 1; }
        sleep 1
    done
    i=0
    until udp_bound "$port"; do
        systemctl is-active --quiet "$unit" || { echo 'The core stopped before listening for MoonProto' >&2; exit 1; }
        i=$((i + 1))
        test "$i" -le 30 || { echo "The core does not listen on UDP $port" >&2; exit 1; }
        sleep 1
    done
    trap - EXIT
    echo START_OK
    ;;
finish)
    dest=$(resolve_dir "$1")
    test -f "$dest/$marker" || { echo "$dest was not installed by a MoonTerminal move" >&2; exit 1; }
    if test -e "$dest/$marker.ini-flag" && test -f "$dest/MoonBot.ini"; then
        cr=$(printf '\r')
        kept="$dest/.MoonBot.ini.move"
        # grep exits 1 when no line is left, 2 on an error: only the error keeps the INI as it is.
        rc=0
        grep -v -i -E "^KeepOrdersAfterCrash=1${cr}?\$" "$dest/MoonBot.ini" > "$kept" || rc=$?
        test "$rc" -le 1 || { rm -f -- "$kept"; echo 'Cannot read MoonBot.ini' >&2; exit 1; }
        # Rewrite in place: the file keeps its owner and mode.
        cat "$kept" > "$dest/MoonBot.ini"
        rm -f -- "$kept"
    fi
    rm -f -- "$dest/$marker.ini-flag"
    echo FINISH_OK
    ;;
remove)
    dest=$(resolve_dir "$1")
    test -f "$dest/$marker" || { echo "$dest was not installed by MoonTerminal: not removed" >&2; exit 1; }
    unit=$(unit_of "$dest")
    port=$(head -n 1 "$dest/$marker")
    systemctl disable --now "$unit" >/dev/null 2>&1 || true
    # Never delete the folder of a core that is still running.
    ! systemctl is-active --quiet "$unit" || { echo "$unit did not stop: nothing removed" >&2; exit 1; }
    # Only a rule this move opened goes: the port may have been open before it.
    if grep -qx ufw "$dest/$marker" && ufw_active; then
        ufw delete allow "$port/udp" >/dev/null 2>&1 || true
    fi
    rm -rf -- "$dest"
    echo REMOVE_OK
    ;;
*)
    echo "unknown step $step" >&2
    exit 2
    ;;
esac
