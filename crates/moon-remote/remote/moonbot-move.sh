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
#   locate  <udp port>                          — find the instance and unit that own the port
#   info    <udp port>                          — locate and report service state and resource use
#   service <start|stop|restart> <udp port> [<unit hint>]
#                                               — change the located service state
#   log     <udp port> <200|500|1000> [<dir hint>]
#                                               — print a bounded tail, using the hint only when
#                                                 no running or installed core can be located
#   retarget <instance dir> <new udp port>      — move the marker and its recorded firewall rule
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
PROC=${MB_PROC_ROOT:-/proc}

# The service unit of an instance directory.
unit_of() {
    systemd-escape --path --template=moonbot@.service "$1"
}

# Whether something listens on UDP port $1.
udp_bound() {
    test -n "$(ss -H -uan "sport = :$1")"
}

# Reject anything but a decimal UDP port in the inclusive range 1..65535.
validate_port() {
    p=$1
    case $p in ''|*[!0-9]*) exit 2;; esac
    test "$p" -ge 1 && test "$p" -le 65535 || exit 2
}

# Set dir, unit and ours for the core on UDP port $1; return 1 when no core owns it.
locate_port() {
    dir=''
    unit=''
    ours=0
    units=$(
        {
            systemctl list-units --all --plain --no-legend -- 'moonbot@*.service' 2>/dev/null || true
            systemctl list-unit-files --plain --no-legend -- 'moonbot@*.service' 2>/dev/null || true
        } | awk '{print $1}' | grep '^moonbot@.*\.service$' || true
    )
    for unit in $units; do
        instance=${unit#moonbot@}
        instance=${instance%.service}
        test -n "$instance" || continue
        candidate=$(systemd-escape --unescape --path "$instance") || continue
        if test -f "$candidate/$marker" && test "$(head -n 1 "$candidate/$marker")" = "$1"; then
            dir=$candidate
            ours=1
            return 0
        fi
    done
    unit=''
    pid=$(ss -H -ulnp "sport = :$1" | sed -n 's/.*pid=\([0-9][0-9]*\).*/\1/p' | head -n 1)
    test -n "$pid" || return 1
    dir=$(readlink "$PROC/$pid/cwd") || return 1
    nl=$(printf '\n_')
    nl=${nl%_}
    case $dir in *"$nl"*) return 1;; esac
    exe=$(readlink "$PROC/$pid/exe") || exe=''
    case ${exe##*/} in
    MoonBot) ;;
    *) test -x "$dir/MoonBot" || return 1 ;;
    esac
    unit=$(sed -n 's|.*/\([^/]*\.service\)$|\1|p' "$PROC/$pid/cgroup" | tail -n 1)
    test -n "$unit" || return 1
    test "$(systemctl show -p MainPID --value -- "$unit")" = "$pid" || return 1
    if test -f "$dir/$marker" && test "$(head -n 1 "$dir/$marker")" = "$1"; then
        ours=1
    fi
    return 0
}

# Print one stopped candidate whose executable exists and whose INI or marker confirms the port when known.
print_stopped_candidate() {
    requested=$1
    candidate_unit=$2
    working=$3
    exec_start=$4
    port_known=0
    case $candidate_unit in
    moonbot@*.service)
        instance=${candidate_unit#moonbot@}
        instance=${instance%.service}
        if test -n "$instance"; then
            instance_dir=$(systemd-escape --unescape --path "$instance") || return 1
            if test -f "$instance_dir/$marker"; then
                test "$(head -n 1 "$instance_dir/$marker")" = "$requested" || return 1
            fi
        fi
        ;;
    esac
    candidate=$working
    if test -z "$candidate" || test ! -x "$candidate/MoonBot"; then
        case $exec_start in
        *path=*)
            binary=${exec_start#*path=}
            binary=${binary%% ;*}
            binary=${binary%%\}*}
            ;;
        *) binary=${exec_start%% *} ;;
        esac
        case $binary in
        /*) candidate=${binary%/*} ;;
        *) return 1 ;;
        esac
    fi
    test -n "$candidate" && test -x "$candidate/MoonBot" || return 1
    candidate_port=''
    if test -f "$candidate/$marker"; then
        candidate_port=$(head -n 1 "$candidate/$marker")
    elif test -f "$candidate/MoonBot.ini"; then
        candidate_port=$(sed -n 's/^[[:space:]]*[Mm]oon[Pp]roto[Pp]ort[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$candidate/MoonBot.ini" | head -n 1)
    fi
    if test -n "$candidate_port"; then
        test "$candidate_port" = "$requested" || return 1
        port_known=1
    fi
    has_mb=0
    test ! -x "$candidate/mb" || has_mb=1
    printf 'LOCATE_CANDIDATE=%s|%s|%s|%s\n' "$candidate_unit" "$candidate" "$has_mb" "$port_known"
}

# Print inactive MoonBot services from one keyed snapshot of installed units.
locate_stopped() {
    found=0
    units=$(systemctl list-unit-files --type=service --no-legend --plain 2>/dev/null | awk '$1 ~ /\.service$/ && $1 !~ /@\.service$/ {print $1}') || return 1
    test -n "$units" || return 1
    details=$(systemctl show -p Id,ExecStart,WorkingDirectory,ActiveState -- $units 2>/dev/null) || return 1
    unit=''
    working=''
    exec_start=''
    active=''
    while IFS= read -r line || test -n "$line"; do
        if test -z "$line"; then
            if test -n "$unit" && test "$active" != active; then
                if print_stopped_candidate "$1" "$unit" "$working" "$exec_start"; then
                    found=1
                fi
            fi
            unit=''
            working=''
            exec_start=''
            active=''
            continue
        fi
        case $line in
        Id=*) unit=${line#Id=} ;;
        ExecStart=*) exec_start=${line#ExecStart=} ;;
        WorkingDirectory=*) working=${line#WorkingDirectory=} ;;
        ActiveState=*) active=${line#ActiveState=} ;;
        esac
    done <<EOF
$details

EOF
    test "$found" = 1
}

# Print the frozen LOCATE_* reply for the globals set by locate_port.
print_locate() {
    has_mb=0
    test ! -x "$dir/mb" || has_mb=1
    printf 'LOCATE_DIRECTORY=%s\nLOCATE_SERVICE=%s\nLOCATE_OURS=%s\nLOCATE_MB=%s\n' \
        "$dir" "$unit" "$ours" "$has_mb"
}

# Print the recent service journal before failing a service operation.
service_failed() {
    journalctl -u "$unit" -n 30 --no-pager >&2 || true
    exit 1
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
    test "$lowest" = 2.38 || { echo "MoonBot for Linux needs glibc 2.38 or newer; this server has $libc. Choose Ubuntu 24.04 or newer, or Debian 13 or newer, on x86-64." >&2; exit 1; }
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
         /^MemAvailable:/ { printf "CHECK_MEM_AVAILABLE_MB=%d\n", $2 / 1024 }' "$PROC/meminfo"
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
                systemctl disable --now -- "$unit" >/dev/null 2>&1 || true
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
    systemctl enable --now -- "$unit" >/dev/null 2>&1
    # A fresh core writes its default settings on the first start; its command channel answers
    # once it is up.
    # Bounded by the clock, not by passes: one hung probe takes its whole 10 s.
    deadline=$(($(date +%s) + 90))
    until (cd "$dest" && timeout 10 ./mb --status >/dev/null 2>&1); do
        systemctl is-active --quiet -- "$unit" || { echo 'The core stopped right after its start' >&2; exit 1; }
        test "$(date +%s)" -lt "$deadline" || { echo 'The core does not answer its commands within 90 s' >&2; exit 1; }
        sleep 1
    done
    trap - EXIT
    rm -rf -- "$work"
    printf 'MOVE_DIRECTORY=%s\nMOVE_SERVICE=%s\n' "$dest" "$unit"
    ;;
locate)
    validate_port "${1:-}"
    if locate_port "$p"; then
        print_locate
    elif ! locate_stopped "$p"; then
        echo LOCATE_NONE=1
    fi
    ;;
info)
    validate_port "${1:-}"
    if ! locate_port "$p"; then
        echo LOCATE_NONE=1
        exit 0
    fi
    print_locate
    active=$(systemctl show -p ActiveState --value -- "$unit")
    entered=$(systemctl show -p ActiveEnterTimestampMonotonic --value -- "$unit")
    memory=$(systemctl show -p MemoryCurrent --value -- "$unit")
    read now _ < "$PROC/uptime"
    uptime=''
    if test "$active" = active; then
        uptime=$(awk -v entered="$entered" -v now="$now" 'BEGIN {
            if (entered ~ /^[0-9]+$/ && entered > 0) {
                seconds = now - entered / 1000000
                if (seconds >= 0) printf "%d", seconds
            }
        }')
    fi
    case $memory in ''|*[!0-9]*) memory='';; esac
    printf 'INFO_ACTIVE=%s\nINFO_UPTIME=%s\nINFO_MEMORY=%s\n' "$active" "$uptime" "$memory"
    ;;
service)
    action=${1:-}
    case $action in start|stop|restart);; *) exit 2;; esac
    validate_port "${2:-}"
    if ! locate_port "$p"; then
        test "$action" = start || exit 1
        hint=${3:-}
        case $hint in *.service) unit=$hint;; *) exit 1;; esac
        case $unit in -*) exit 1;; esac
        # systemd escapes hyphens and other instance characters as literal \xHH sequences.
        plain_unit=$(printf '%s\n' "$unit" | sed 's/[\\]x[0-9A-Fa-f][0-9A-Fa-f]//g')
        case $plain_unit in *[!A-Za-z0-9@._:-]*) exit 1;; esac
        dir=$(systemctl show -p WorkingDirectory --value -- "$unit")
        if test -z "$dir"; then
            case $unit in
            moonbot@*.service)
                instance=${unit#moonbot@}
                instance=${instance%.service}
                dir=$(systemd-escape --unescape --path "$instance") || exit 1
                ;;
            esac
        fi
        test -n "$dir" && test -x "$dir/MoonBot" || exit 1
        ours=0
        if test -f "$dir/$marker" && test "$(head -n 1 "$dir/$marker")" = "$p"; then
            ours=1
        fi
    fi
    systemctl "$action" -- "$unit" >/dev/null 2>&1 || service_failed
    case $action in
    start|restart)
        deadline=$(($(date +%s) + 60))
        until udp_bound "$p"; do
            systemctl is-active --quiet -- "$unit" || service_failed
            test "$(date +%s)" -lt "$deadline" || service_failed
            sleep 1
        done
        ;;
    esac
    active=$(systemctl show -p ActiveState --value -- "$unit") || service_failed
    printf 'SERVICE_OK=%s\n' "$active"
    ;;
log)
    validate_port "${1:-}"
    n=${2:-}
    case $n in 200|500|1000);; *) exit 2;; esac
    if ! locate_port "$p"; then
        dir=${3:-}
        case $dir in /*) ;; *) exit 1;; esac
        nl=$(printf '\n_')
        nl=${nl%_}
        case $dir in *"$nl"*) exit 1;; esac
        test -x "$dir/MoonBot" || exit 1
        unit=''
    fi
    log_file=''
    # ISO dates sort lexically, so the last matching path is the newest daily trading log.
    for candidate in "$dir"/logs/LOG_????-??-??.log; do
        test ! -f "$candidate" || log_file=$candidate
    done
    test -n "$log_file" || test -n "$unit" || exit 1
    echo LOG_BEGIN
    if test -n "$log_file" && tail -n "$n" "$log_file"; then
        exit 0
    fi
    journalctl -u "$unit" -n "$n" --no-pager --utc -o short-iso
    ;;
retarget)
    dest=$(resolve_dir "${1:-}")
    test -f "$dest/$marker" || { echo "$dest was not installed by MoonTerminal" >&2; exit 1; }
    validate_port "${2:-}"
    old=$(head -n 1 "$dest/$marker")
    new_ufw=0
    if test "$old" != "$p" && ufw_active; then
        # A closed new port needs an allow whoever owned the old one. A matching user allow stays
        # unmarked, so a later retarget or remove can never delete access MoonTerminal did not open.
        if ! LC_ALL=C ufw status |
            grep -q -E "^${p}/udp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+Anywhere[[:space:]]*(#.*)?\$"; then
            ufw allow "$p/udp" comment "MoonBot $dest" >/dev/null
            new_ufw=1
        fi
        if grep -qx ufw "$dest/$marker"; then
            ufw delete allow "$old/udp" >/dev/null 2>&1 || true
        fi
    fi
    kept="$dest/$marker.retarget"
    {
        printf '%s\n' "$p"
        if test "$old" = "$p"; then
            tail -n +2 "$dest/$marker"
        else
            tail -n +2 "$dest/$marker" | while IFS= read -r line; do
                if test "$line" = ufw; then
                    test "$new_ufw" = 0 || echo ufw
                else
                    printf '%s\n' "$line"
                fi
            done
            if ! grep -qx ufw "$dest/$marker" && test "$new_ufw" = 1; then
                echo ufw
            fi
        fi
    } > "$kept"
    cat "$kept" > "$dest/$marker"
    rm -f -- "$kept"
    printf 'RETARGET_OK=%s\n' "$p"
    ;;
mb)
    dest=$(resolve_dir "$1")
    shift
    # Hand-installed cores get read-only status and a login retry, never a reconfiguring command.
    if test ! -f "$dest/$marker"; then
        case ${1:-} in
        --status|--retry-login) ;;
        *) echo "$dest was not installed by MoonTerminal" >&2; exit 1 ;;
        esac
    fi
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
            systemctl disable --now -- "$unit" >/dev/null 2>&1 || true
        fi
        exit "$code"
    }
    trap stop_on_error EXIT
    systemctl enable --now -- "$unit" >/dev/null 2>&1
    # The core takes the key file on its first start and deletes it.
    i=0
    while test -e "$dest/MoonProtoSetup.key"; do
        systemctl is-active --quiet -- "$unit" || { echo 'The core stopped before taking its MoonProto key' >&2; exit 1; }
        i=$((i + 1))
        test "$i" -le 60 || { echo 'The core did not take its MoonProto key within 60 s' >&2; exit 1; }
        sleep 1
    done
    i=0
    until udp_bound "$port"; do
        systemctl is-active --quiet -- "$unit" || { echo 'The core stopped before listening for MoonProto' >&2; exit 1; }
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
    systemctl disable --now -- "$unit" >/dev/null 2>&1 || true
    # Never delete the folder of a core that is still running.
    ! systemctl is-active --quiet -- "$unit" || { echo "$unit did not stop: nothing removed" >&2; exit 1; }
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
