#!/bin/sh
# Installs a MoonBot core as a systemd instance, as root: a moved core (moon-remote's core_move)
# or a fresh one (core_new). One step per call:
#   check   <udp port>                          — the server can run MoonBot and the port is free;
#                                                 CHECK_* lines tell its OS, memory and firewall
#   install <work dir> <instance dir> <udp port> <package url>
#                                               — package + settings and the service, not started
#   install-fresh <instance dir> <udp port> <package url> [settings]
#                                               — package and the service, started on the core's
#                                                 own default settings, ready for its commands;
#                                                 with `settings`, a settings archive read from
#                                                 stdin is unpacked first and the core starts on it
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
#   logdays <udp port> [<dir hint>]             — list the daily trading logs the core still has,
#                                                 one `LOGDAY=<yyyy-mm-dd> <bytes>` line each
#   logday  <udp port> <yyyy-mm-dd> <max bytes> [<dir hint>]
#                                               — print that day's whole trading log, gzipped, after
#                                                 a `LOGDAY_BEGIN` line; `LOGDAY_NONE=1` when the
#                                                 core has no file for that day, and
#                                                 `LOGDAY_TOO_BIG=<bytes>` when it is over the limit
#   retarget <instance dir> <new udp port>      — move the marker and its recorded firewall rule
#   mb      <instance dir> <arguments…>         — one command of the core's own CLI; stdin passes
#                                                 through; runs as the account the core runs as
#   remove  <instance dir>                      — stop and delete an instance this script made
#   egress  <udp port> <ipv4|off>               — bind the located core's outgoing connections to
#                                                 one of the server's addresses, or unbind them;
#                                                 a running core is restarted on it, a stopped one
#                                                 stays stopped and takes it on its next start
# The work dir holds state.zip (settings archive), key (MoonProto connection packet) and, when
# the move added it, ini-flag; install deletes it.
# install and install-fresh take `egress=<ipv4>` after their arguments: the core is bound before
# its first start. check and info print the EGRESS_* lines the choice is made from.
#
# A bound core: a drop-in of its unit records the address and names an nftables set through
# systemd's NFTSet= (systemd 255+), which systemd fills with the unit's cgroup on every start —
# a cgroup ID changes with each restart, so no static rule could name it. Our own table marks
# that set's sockets and SNATs their new connections to the address; replies to connections
# the core accepted (MoonProto from the terminal) are conntrack replies and stay as they are.
# The table is loaded by our oneshot unit, part of nftables.service: a firewall reload that
# flushes the ruleset reloads it too, and its daemon-reload puts the running cores back into
# their sets.
set -eu
# dash, /bin/sh on Debian and Ubuntu, skips the EXIT trap when a signal kills it; exiting from
# the signal runs it, so a dropped connection still cleans up.
trap 'exit 1' HUP INT TERM PIPE

marker=.moonterminal-move
PROC=${MB_PROC_ROOT:-/proc}
UFW_CONF=${MB_UFW_CONF:-/etc/ufw/ufw.conf}
SYSTEMD_DIR=${MB_SYSTEMD_DIR:-/etc/systemd/system}
EGRESS_NFT=${MB_EGRESS_NFT:-/etc/moonbot-egress.nft}
EGRESS_UNIT=moonbot-egress.service
EGRESS_TABLE=moonbot_egress
EGRESS_DROPIN=moonterminal-egress.conf
# NFTSet= first shipped in systemd 255.
EGRESS_MIN_SYSTEMD=255

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

# Set dir, and unit when a core owns UDP port $1, for the log steps. The directory hint $2 is used
# only when no running or installed core can be located, and must name an instance directory: a
# stopped core still has its log files.
log_instance() {
    if ! locate_port "$1"; then
        dir=${2:-}
        case $dir in /*) ;; *) exit 1;; esac
        nl=$(printf '\n_')
        nl=${nl%_}
        case $dir in *"$nl"*) exit 1;; esac
        test -x "$dir/MoonBot" || exit 1
        unit=''
    fi
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

# Put the package's `mb` wrapper on PATH as `mb` and `moonbot`, the commands every MoonBot guide
# uses, as the release's own install does: a newer package's copy replaces an older one, and the
# wrapper serves every instance on the server, so remove leaves it in place.
install_mb() {
    test -f "$1/mb" || return 0
    if test ! -e /usr/local/bin/mb || test "$1/mb" -nt /usr/local/bin/mb; then
        install -p -m 0755 "$1/mb" /usr/local/bin/mb
    fi
    ln -sfn mb /usr/local/bin/moonbot
}

# Whether ufw is installed and active.
ufw_active() {
    command -v ufw >/dev/null 2>&1 && LC_ALL=C ufw status | grep -q '^Status: active'
}

# Whether UFW enables IPv6; an unreadable config retains UFW's default enabled behavior.
ufw_ipv6() {
    test ! -r "$UFW_CONF" || grep -q '^IPV6=yes' "$UFW_CONF"
}

# Whether UDP port $1 already has a general IPv4 allow.
ufw_has_v4() {
    LC_ALL=C ufw status |
        grep -q -E "^$1/udp[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+Anywhere[[:space:]]*(#.*)?\$"
}

# Whether UDP port $1 already has a general IPv6 allow.
ufw_has_v6() {
    LC_ALL=C ufw status |
        grep -q -E "^$1/udp[[:space:]]+\(v6\)[[:space:]]+ALLOW([[:space:]]+IN)?[[:space:]]+Anywhere[[:space:]]+\(v6\)[[:space:]]*(#.*)?\$"
}

# Whether the general allow for port $1 and family $2 carries this instance's terminal marker.
ufw_has_our_comment() {
    LC_ALL=C ufw status | awk -v port="$1/udp " -v family="$2" -v tag="# MoonBot $3" '
        index($0, port) == 1 && substr($0, length($0) - length(tag) + 1) == tag {
            v6 = index($0, "(v6)") > 0
            if ((family == "6" && v6) || (family == "4" && !v6)) found = 1
        }
        END { exit !found }
    '
}

# Open port $1 for whichever families lack an allow. Family-specific sources keep a user's rule in
# the other family untouched, and insertion ahead of broader denies makes the new endpoint usable.
# The new_ufw4/new_ufw6 globals also recover ownership after an interrupted marker rewrite.
ufw_open() {
    new_ufw4=0
    new_ufw6=0
    if ! ufw_has_v4 "$1"; then
        ufw prepend allow from 0.0.0.0/0 to any port "$1" proto udp comment "MoonBot $2" >/dev/null
        new_ufw4=1
    elif ufw_has_our_comment "$1" 4 "$2"; then
        new_ufw4=1
    fi
    if ufw_ipv6 && ! ufw_has_v6 "$1"; then
        ufw prepend allow from ::/0 to any port "$1" proto udp comment "MoonBot $2" >/dev/null
        new_ufw6=1
    elif ufw_ipv6 && ufw_has_our_comment "$1" 6 "$2"; then
        new_ufw6=1
    fi
}

# Delete only the numbered allow for port $1, family $2, and this instance's terminal comment.
ufw_delete_family() {
    numbers=$(LC_ALL=C ufw status numbered | awk -v port="$1/udp " -v family="$2" -v tag="# MoonBot $3" '
        {
            rule = $0
            sub(/^[[:space:]]*\[[[:space:]]*[0-9]+[[:space:]]*\][[:space:]]*/, "", rule)
            if (index(rule, port) != 1 || index(rule, "ALLOW") == 0) next
            if (substr($0, length($0) - length(tag) + 1) != tag) next
            v6 = index(rule, "(v6)") > 0
            if ((family == "6" && !v6) || (family == "4" && v6)) next
            line = $0
            sub(/^[[:space:]]*\[[[:space:]]*/, "", line)
            sub(/[[:space:]]*\].*$/, "", line)
            print line
        }
    ' | sort -rn)
    for number in $numbers; do
        ufw --force delete "$number" >/dev/null 2>&1 || true
    done
}

# Delete the firewall allows recorded in marker $1 for port $2 and instance $3.
ufw_delete_marked() {
    if grep -qx ufw "$1"; then
        ufw delete allow "$2/udp" >/dev/null 2>&1 || true
    fi
    if grep -qx ufw4 "$1"; then
        ufw_delete_family "$2" 4 "$3"
    fi
    if grep -qx ufw6 "$1"; then
        ufw_delete_family "$2" 6 "$3"
    fi
}

# The running systemd's major version, 0 when it cannot be read.
systemd_version() {
    v=$(systemctl --version 2>/dev/null | awk 'NR == 1 { print $2 + 0 }') || v=0
    case $v in ''|*[!0-9]*) v=0 ;; esac
    printf '%s\n' "$v"
}

# Field $1 of the default IPv4 route ("dev" or "src"); empty without one. `route get` sends
# nothing: it only asks the kernel which way such a packet would leave.
default_route() {
    command -v ip >/dev/null 2>&1 || return 0
    ip -4 route get 1.1.1.1 2>/dev/null | awk -v key="$1" '{
        for (i = 1; i < NF; i++) if ($i == key) { print $(i + 1); exit }
    }'
}

# The global IPv4 addresses of interface $1, one per line.
dev_ipv4() {
    ip -4 -o addr show dev "$1" scope global 2>/dev/null | awk '{ split($4, a, "/"); print a[1] }'
}

# What a core's outgoing address is chosen from: the systemd version, the primary address and
# every address of the interface the default route leaves through. Addresses of other interfaces
# (a provider's private network) are not offered: they do not reach an exchange.
print_egress_options() {
    printf 'EGRESS_SYSTEMD=%s\n' "$(systemd_version)"
    primary=$(default_route src)
    test -z "$primary" || printf 'EGRESS_PRIMARY=%s\n' "$primary"
    dev=$(default_route dev)
    test -n "$dev" || return 0
    for address in $(dev_ipv4 "$dev"); do
        printf 'EGRESS_ADDRESS=%s\n' "$address"
    done
}

# Whether $1 is a dotted IPv4 address without leading zeros.
valid_ipv4() {
    case $1 in ''|*[!0-9.]*|.*|*.|*..*) return 1 ;; esac
    old_ifs=$IFS
    IFS=.
    # Word splitting on dots is the point here; the case above admits only digits and dots.
    # shellcheck disable=SC2086
    set -- $1
    IFS=$old_ifs
    test $# = 4 || return 1
    for part in "$@"; do
        case $part in 0?*) return 1 ;; esac
        test "${#part}" -le 3 && test "$part" -le 255 || return 1
    done
}

# The nftables set of outgoing address $1.
egress_set() {
    printf 'e%s\n' "$1" | tr . _
}

# The drop-in of unit $1 that binds it.
egress_dropin() {
    printf '%s\n' "$SYSTEMD_DIR/$1.d/$EGRESS_DROPIN"
}

# The outgoing address recorded for unit $1, empty when it has none.
egress_of() {
    f=$(egress_dropin "$1")
    test -f "$f" || return 0
    sed -n 's/^# MOONTERMINAL_EGRESS=//p' "$f" | head -n 1
}

# Refuse, before anything changes, unless a core can be bound to address $1 here: systemd new
# enough for NFTSet=, the address on the default route's interface, and nft (installed through
# apt when missing).
egress_require() {
    v=$(systemd_version)
    test "$v" -ge "$EGRESS_MIN_SYSTEMD" || {
        echo "Binding a core to its own outgoing address needs systemd $EGRESS_MIN_SYSTEMD or newer (Debian 13, Ubuntu 24.04 or newer); this server has systemd $v" >&2
        exit 1
    }
    valid_ipv4 "$1" || { echo "Not an IPv4 address: $1" >&2; exit 1; }
    dev=$(default_route dev)
    { test -n "$dev" && dev_ipv4 "$dev" | grep -qx -F "$1"; } || {
        echo "The address $1 is not on this server's main network interface" >&2
        exit 1
    }
    if ! command -v nft >/dev/null 2>&1; then
        command -v apt-get >/dev/null 2>&1 || { echo 'Install on the server: nftables' >&2; exit 1; }
        echo 'Installing: nftables'
        # Both wait out an unattended upgrade holding apt's locks, as bootstrap.sh does.
        apt-get -o DPkg::Lock::Timeout=300 update >/dev/null
        DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y \
            --no-install-recommends nftables >/dev/null
        # The package's own service loads /etc/nftables.conf, whose stock first line flushes the
        # whole ruleset — ufw's tables with it — at the next boot. Only the nft tool was wanted:
        # left enabled by the install, it is disabled again, never stopped (its stop flushes too).
        if systemctl is-enabled --quiet nftables.service 2>/dev/null; then
            systemctl disable nftables.service >/dev/null 2>&1 || true
        fi
    fi
}

# Serialize every change of the outgoing-address rules on this server: two steps for two cores
# of one host would otherwise write the table and the drop-ins over each other. The descriptor
# stays open, and the lock held, until the step ends.
egress_lock() {
    test -z "${egress_locked:-}" || return 0
    exec 9>"${MB_EGRESS_LOCK:-/run/moonbot-egress.lock}"
    if command -v flock >/dev/null 2>&1; then
        flock -w 120 9 || {
            echo 'Another change of outgoing addresses is still running on this server' >&2
            exit 1
        }
    fi
    egress_locked=1
}

# Write our table from every bound core's address and (re)load it through its unit; with no core
# bound any more, the table, its file and its unit go. An address no longer on the server drops
# its cores' connections instead of letting them out through the primary address unnoticed.
#
# A reload never empties a set: the file declares the sets (an existing one keeps its elements),
# flushes only our two chains and writes their rules again, all in one transaction. Deleting and
# recreating the table would leave every other bound core outside its rules until systemd refilled
# the sets, and its new connections would leave from the primary address meanwhile.
egress_reload() {
    list=$(
        for f in "$SYSTEMD_DIR"/*.service.d/"$EGRESS_DROPIN"; do
            test -f "$f" || continue
            sed -n 's/^# MOONTERMINAL_EGRESS=//p' "$f" | head -n 1
        done | sort -u
    )
    if test -z "$list"; then
        if test -f "${SYSTEMD_DIR:?}/${EGRESS_UNIT:?}"; then
            systemctl disable --now -- "$EGRESS_UNIT" >/dev/null 2>&1 || true
            rm -f -- "${SYSTEMD_DIR:?}/${EGRESS_UNIT:?}"
        fi
        rm -f -- "${EGRESS_NFT:?}"
        if command -v nft >/dev/null 2>&1; then
            nft delete table inet "$EGRESS_TABLE" >/dev/null 2>&1 || true
        fi
        systemctl daemon-reload
        return 0
    fi
    dev=$(default_route dev)
    here=''
    test -z "$dev" || here=$(dev_ipv4 "$dev")
    new=$(mktemp "$EGRESS_NFT.XXXXXX")
    {
        echo '# Written by MoonTerminal: the outgoing address of each bound MoonBot core.'
        echo "table inet $EGRESS_TABLE {"
        for address in $list; do
            printf '    set %s { type cgroupsv2; }\n' "$(egress_set "$address")"
        done
        echo '    chain egress_mark { type filter hook output priority mangle; policy accept; }'
        echo '    chain egress_snat { type nat hook postrouting priority srcnat; policy accept; }'
        echo '}'
        echo "flush chain inet $EGRESS_TABLE egress_mark"
        echo "flush chain inet $EGRESS_TABLE egress_snat"
        echo "table inet $EGRESS_TABLE {"
        echo '    chain egress_mark {'
        echo '        oifname "lo" accept'
        n=0
        for address in $list; do
            n=$((n + 1))
            set_name=$(egress_set "$address")
            # The set holds unit cgroups, and `level` names the depth of the socket's ancestor
            # compared with it: a plain service sits at 2 (system.slice/x.service), an instance of
            # a template at 3 (system.slice/system-moonbot.slice/moonbot@….service), a unit in a
            # nested slice at 4. Every depth is asked: an ancestor matches only when it is the
            # bound unit's own cgroup, so a deeper rule never catches anything else.
            for level in 2 3 4; do
                # IPv6 would leave through the primary address whatever the IPv4 rule says;
                # refused locally, a connection fails at once and a dual-stack client falls back.
                printf '        socket cgroupv2 level %s @%s meta nfproto ipv6 counter drop\n' "$level" "$set_name"
                if printf '%s\n' "$here" | grep -qx -F "$address"; then
                    printf '        socket cgroupv2 level %s @%s meta mark set 0x4d54%04x\n' "$level" "$set_name" "$n"
                else
                    printf '        socket cgroupv2 level %s @%s counter drop\n' "$level" "$set_name"
                fi
            done
        done
        echo '    }'
        echo '    chain egress_snat {'
        n=0
        for address in $list; do
            n=$((n + 1))
            if printf '%s\n' "$here" | grep -qx -F "$address"; then
                printf '        meta mark 0x4d54%04x oifname "%s" counter snat ip to %s\n' "$n" "$dev" "$address"
            fi
        done
        echo '    }'
        echo '}'
    } > "$new"
    chmod 644 "$new"
    mv -f -- "$new" "$EGRESS_NFT"
    cat > "$SYSTEMD_DIR/$EGRESS_UNIT" <<EOF
# Written by MoonTerminal: loads the outgoing addresses of bound MoonBot cores.
[Unit]
Description=MoonTerminal: outgoing addresses of MoonBot cores
After=nftables.service
# nftables.service's start, restart and reload flush the whole ruleset, ours with it: each of
# them loads our table again.
PartOf=nftables.service
ReloadPropagatedFrom=nftables.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=nft -f $EGRESS_NFT
# After a flush the sets come back empty; a reload of systemd puts every running bound core back
# into its own.
ExecStartPost=-systemctl daemon-reload
ExecReload=nft -f $EGRESS_NFT
ExecReload=-systemctl daemon-reload

[Install]
WantedBy=multi-user.target nftables.service
EOF
    systemctl daemon-reload
    systemctl enable -- "$EGRESS_UNIT" >/dev/null 2>&1
    systemctl reload-or-restart -- "$EGRESS_UNIT" >/dev/null 2>&1 || {
        journalctl -u "$EGRESS_UNIT" -n 30 --no-pager >&2 || true
        echo 'The outgoing-address rules did not load' >&2
        exit 1
    }
}

# Record address $2 as unit $1's outgoing address ("off": none) and reload the rules. The core
# takes it on its next start.
egress_write() {
    egress_lock
    f=$(egress_dropin "$1")
    if test "$2" = off; then
        test -f "$f" || return 0
        rm -f -- "${f:?}"
        rmdir -- "${f%/*}" 2>/dev/null || true
    else
        mkdir -p -- "${f%/*}"
        set_name=$(egress_set "$2")
        cat > "$f" <<EOF
# Written by MoonTerminal: this core's outgoing connections leave from one address.
# MOONTERMINAL_EGRESS=$2
[Unit]
Wants=$EGRESS_UNIT
After=$EGRESS_UNIT

[Service]
NFTSet=cgroup:inet:$EGRESS_TABLE:$set_name
# systemd ignores a set it cannot fill: without the table the core would start unbound and leave
# from the primary address. It does not start instead. Run as root ("+"), whoever the core runs as.
ExecStartPre=+nft list set inet $EGRESS_TABLE $set_name
EOF
    fi
    egress_reload
}

# Whether unit $1, started, sits in the set of its recorded address; true for an unbound unit.
# The listing quotes each element as its cgroup path, so the unit is matched as the path's last
# component, never as the tail of another unit's name.
egress_bound() {
    address=$(egress_of "$1")
    test -n "$address" || return 0
    nft list set inet "$EGRESS_TABLE" "$(egress_set "$address")" 2>/dev/null | grep -q -F "/$1\""
}

# Unbind unit $1 on a failure path: never fails, so the cleanup that calls it goes on.
egress_undo() {
    (egress_write "$1" off) >/dev/null 2>&1 || true
}

# The INFO_EGRESS_* lines of unit $1: its address, whether the server still has that address,
# whether the core sits in its set, and how many connections the address has carried since the
# rules were last loaded (every core bound to it).
print_egress_info() {
    address=$(egress_of "$1")
    printf 'INFO_EGRESS=%s\n' "$address"
    test -n "$address" || return 0
    present=0
    dev=$(default_route dev)
    if test -n "$dev" && dev_ipv4 "$dev" | grep -qx -F "$address"; then
        present=1
    fi
    bound=0
    ! egress_bound "$1" || bound=1
    connections=$(nft list chain inet "$EGRESS_TABLE" egress_snat 2>/dev/null | awk -v address="$address" '
        $NF == address { for (i = 1; i < NF; i++) if ($i == "packets") { print $(i + 1); exit } }
    ') || connections=''
    printf 'INFO_EGRESS_PRESENT=%s\nINFO_EGRESS_BOUND=%s\nINFO_EGRESS_CONNECTIONS=%s\n' \
        "$present" "$bound" "$connections"
}

# Set settings ("settings" when given) and egress (the address of `egress=<ipv4>`) from an install
# step's trailing options; anything else is refused before the step changes a thing.
install_options() {
    settings=''
    egress=''
    for option in "$@"; do
        case $option in
        settings) settings=settings ;;
        egress=*) egress=${option#egress=} ;;
        *) echo "Unknown install option $option" >&2; exit 1 ;;
        esac
    done
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

# Unpack settings archive $1 into instance directory $2, over an unpacked package. The archive
# may not replace a file of the package and must carry the bot configuration.
unpack_settings() {
    unzip -Z1 "$1" | while IFS= read -r name; do
        case "$name" in */) continue ;; esac
        test ! -e "$2/$name" || { echo "The settings would overwrite the program file $name" >&2; exit 1; }
    done
    unzip -q "$1" -d "$2"
    test -f "$2/BotConfig.bcfg" || { echo 'The settings archive has no BotConfig.bcfg' >&2; exit 1; }
}

# Print "<uid> <gid>" of the account a command of instance $1 must run under, nothing for root.
# A core's command channel answers only the account the core runs as, so a running core decides.
# A core running as root gets root: its service already runs these files as root, so a command
# adds no reach. Without a running core nothing proves root is needed, so whoever owns the directory
# or a program file runs it. The caller stands inside the instance and this reads it relatively, so
# whoever owns a directory above cannot swap it after the check. An owner it cannot read fails
# rather than means root.
command_account() {
    root_core=0
    for exe in "$PROC"/[0-9]*/exe; do
        target=$(readlink "$exe") || continue
        target=${target% (deleted)}
        test "${target##*/}" = MoonBot || continue
        test "$(realpath -m -- "$target")" = "$1/MoonBot" || continue
        ids=$(awk '$1 == "Uid:" { uid = $2 } $1 == "Gid:" { gid = $2 } END { print uid, gid }' \
            "${exe%/exe}/status") || continue
        case $ids in
        '0 '*) root_core=1 ;;
        [0-9]*' '[0-9]*) printf '%s\n' "$ids"; return 0 ;;
        esac
    done
    test "$root_core" = 0 || return 0
    for path in . ./MoonBot ./mb ./lib ./libmoonbot_sqlite.so; do
        test -e "$path" || continue
        uid=$(ls -ldLn -- "$path" | awk '{ print $3 }')
        case $uid in
        0) ;;
        ''|*[!0-9]*) echo "Cannot tell who owns $path" >&2; return 1 ;;
        *) printf '%s %s\n' "$uid" "$(id -g "$uid" 2>/dev/null || echo "$uid")"; return 0 ;;
        esac
    done
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
    print_egress_options
    echo CHECK_OK
    ;;
install)
    work=$1
    base=$2
    port=$3
    url=$4
    # The work dir carries the exchange API keys: it goes on every exit from here on.
    trap 'rm -rf -- "$work"' EXIT
    shift 4
    install_options "$@"
    test -z "$settings" || { echo "Unknown install option $settings" >&2; exit 1; }
    test -z "$egress" || egress_require "$egress"
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
    new_ufw4=0
    new_ufw6=0
    egress_made=''
    cleanup() {
        code=$?
        if test "$code" != 0; then
            test "$new_ufw4" = 0 || ufw_delete_family "$port" 4 "$dest"
            test "$new_ufw6" = 0 || ufw_delete_family "$port" 6 "$dest"
            test -z "$egress_made" || egress_undo "$unit"
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
    unpack_settings "$work/state.zip" "$dest"
    install -m 600 "$work/key" "$dest/MoonProtoSetup.key"
    printf '%s\n' "$port" > "$dest/$marker"
    if test -e "$work/ini-flag"; then
        : > "$dest/$marker.ini-flag"
    fi
    chmod 700 "$dest/MoonBot"
    install -m 644 "$dest/moonbot@.service" /etc/systemd/system/moonbot@.service
    install_mb "$dest"
    # Only rules opened here are marked for removal, independently for IPv4 and IPv6.
    if ufw_active; then
        ufw_open "$port" "$dest"
        test "$new_ufw4" = 0 || echo ufw4 >> "$dest/$marker"
        test "$new_ufw6" = 0 || echo ufw6 >> "$dest/$marker"
    fi
    systemctl daemon-reload
    # Bound before its first start: `start` then confirms the core sits in its set.
    if test -n "$egress"; then
        egress_made=1
        egress_write "$unit" "$egress"
    fi
    # Not started here: the old core must be down first, or two cores trade one account.
    trap - EXIT
    rm -rf -- "$work"
    printf 'MOVE_DIRECTORY=%s\nMOVE_SERVICE=%s\n' "$dest" "$unit"
    ;;
install-fresh)
    base=$1
    port=$2
    url=$3
    shift 3
    install_options "$@"
    test -z "$egress" || egress_require "$egress"
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
    new_ufw4=0
    new_ufw6=0
    started=''
    egress_made=''
    cleanup() {
        code=$?
        if test "$code" != 0; then
            if test -n "$started"; then
                journalctl -u "$unit" -n 30 --no-pager >&2 || true
                systemctl disable --now -- "$unit" >/dev/null 2>&1 || true
            fi
            test "$new_ufw4" = 0 || ufw_delete_family "$port" 4 "$dest"
            test "$new_ufw6" = 0 || ufw_delete_family "$port" 6 "$dest"
            test -z "$egress_made" || egress_undo "$unit"
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
    # The settings archive is the whole of stdin: taken before any other command could read it.
    if test -n "$settings"; then
        cat > "$work/state.zip"
    fi
    mkdir -p -- "$(dirname -- "$dest")"
    mkdir -- "$dest"
    made=1
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 600 --output "$work/package.zip" "$url"
    unzip -q "$work/package.zip" -d "$dest"
    (cd "$dest" && sha256sum --quiet --check SHA256SUMS)
    test -f "$dest/mb" || { echo 'This MoonBot package has no command line (mb): a newer package is needed' >&2; exit 1; }
    # With settings the core starts on that configuration instead of writing its defaults.
    if test -n "$settings"; then
        unpack_settings "$work/state.zip" "$dest"
    fi
    printf '%s\n' "$port" > "$dest/$marker"
    chmod 700 "$dest/MoonBot" "$dest/mb"
    install -m 644 "$dest/moonbot@.service" /etc/systemd/system/moonbot@.service
    install_mb "$dest"
    # As in install: only an allow opened here is marked for removal.
    if ufw_active; then
        ufw_open "$port" "$dest"
        test "$new_ufw4" = 0 || echo ufw4 >> "$dest/$marker"
        test "$new_ufw6" = 0 || echo ufw6 >> "$dest/$marker"
    fi
    systemctl daemon-reload
    # Bound before the first start, so not one connection of the core leaves from elsewhere.
    if test -n "$egress"; then
        egress_made=1
        egress_write "$unit" "$egress"
    fi
    started=1
    systemctl enable --now -- "$unit" >/dev/null 2>&1
    # A fresh core writes its default settings on the first start, or starts on the settings
    # unpacked above; either way its command channel answers once it is up.
    # Bounded by the clock, not by passes: one hung probe takes its whole 10 s.
    deadline=$(($(date +%s) + 90))
    until (cd "$dest" && timeout 10 ./mb --status >/dev/null 2>&1); do
        systemctl is-active --quiet -- "$unit" || { echo 'The core stopped right after its start' >&2; exit 1; }
        test "$(date +%s)" -lt "$deadline" || { echo 'The core does not answer its commands within 90 s' >&2; exit 1; }
        sleep 1
    done
    egress_bound "$unit" || { echo "The core started outside its outgoing address $egress" >&2; exit 1; }
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
    print_egress_info "$unit"
    print_egress_options
    ;;
egress)
    validate_port "${1:-}"
    wanted=${2:-}
    test -n "$wanted" || exit 2
    locate_port "$p" || { echo "No core is installed for UDP $p" >&2; exit 1; }
    current=$(egress_of "$unit")
    running=0
    ! systemctl is-active --quiet -- "$unit" || running=1
    # Asked for what it already has: a trading core is not restarted for nothing. The rules are
    # still rewritten — a table from an older script is replaced — and the reload keeps the
    # running core in its set.
    if test "$wanted" = off && test -z "$current"; then
        printf 'EGRESS_OK=%s\n' "$wanted"
        exit 0
    fi
    if test "$wanted" = "$current" && { test "$running" = 0 || egress_bound "$unit"; }; then
        # Under the lock like every write: a reload from a stale drop-in list, racing a step that
        # binds another core, would flush that core's fresh rules.
        egress_lock
        egress_reload
        test "$running" = 0 || egress_bound "$unit" ||
            { echo "The core left its outgoing address $wanted on a reload" >&2; exit 1; }
        printf 'EGRESS_OK=%s\n' "$wanted"
        exit 0
    fi
    test "$wanted" = off || egress_require "$wanted"
    # A failure from here on puts the previous binding back: a drop-in whose rules never loaded
    # would let the core's next start leave from the primary address while it reads as bound.
    restore_egress() {
        code=$?
        if test "$code" != 0; then
            (egress_write "$unit" "${current:-off}") >/dev/null 2>&1 || true
        fi
        exit "$code"
    }
    trap restore_egress EXIT
    egress_write "$unit" "$wanted"
    # A stopped core stays stopped — it may be the old core of a move, or one its user stopped on
    # purpose: it takes the address on its next start, and the drop-in refuses that start without
    # the rules.
    if test "$running" = 0; then
        trap - EXIT
        printf 'EGRESS_OK=%s\n' "$wanted"
        exit 0
    fi
    # Connections already open keep the address they have: only a restart moves all of them.
    # The journal goes first and the reason last: the terminal shows the last line.
    restart_failed() {
        journalctl -u "$unit" -n 30 --no-pager >&2 || true
        echo "The core did not come back up after its restart on the outgoing address $wanted" >&2
        exit 1
    }
    systemctl restart -- "$unit" >/dev/null 2>&1 || restart_failed
    deadline=$(($(date +%s) + 60))
    until udp_bound "$p"; do
        systemctl is-active --quiet -- "$unit" || restart_failed
        test "$(date +%s)" -lt "$deadline" || restart_failed
        sleep 1
    done
    egress_bound "$unit" || { echo "The core restarted outside its outgoing address $wanted" >&2; exit 1; }
    trap - EXIT
    printf 'EGRESS_OK=%s\n' "$wanted"
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
    log_instance "$p" "${3:-}"
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
logdays)
    validate_port "${1:-}"
    log_instance "$p" "${2:-}"
    echo LOGDAYS_BEGIN
    for candidate in "$dir"/logs/LOG_????-??-??.log; do
        test -f "$candidate" || continue
        day=${candidate##*/LOG_}
        # The arithmetic expansion drops the padding a BSD `wc` prints before the count. A file
        # that went away since the glob matched is skipped, not the end of the listing.
        size=$(wc -c 2>/dev/null < "$candidate") || continue
        printf 'LOGDAY=%s %s\n' "${day%.log}" "$((size))"
    done
    ;;
logday)
    validate_port "${1:-}"
    day=${2:-}
    # The day becomes part of a path: nothing but a date may reach it.
    case $day in
    [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
    *) exit 2 ;;
    esac
    max=${3:-}
    case $max in ''|*[!0-9]*) exit 2;; esac
    log_instance "$p" "${4:-}"
    log_file=$dir/logs/LOG_$day.log
    if test ! -f "$log_file"; then
        echo LOGDAY_NONE=1
        exit 0
    fi
    # A file past the caller's limit is refused before a byte of it is sent: the caller holds the
    # whole reply in memory. A file still being written can grow past the limit after this check,
    # which is why the caller caps what it unpacks as well.
    size=$(wc -c < "$log_file")
    if test "$((size))" -gt "$max"; then
        printf 'LOGDAY_TOO_BIG=%s\n' "$((size))"
        exit 0
    fi
    # A day is tens of megabytes of text; compressed it is a tenth of that on the wire. The service
    # journal is never substituted: it is a different log. The file goes in through stdin: given
    # its name, gzip compares the size before and after and complains about a log that grew
    # meanwhile, which today's always does.
    echo LOGDAY_BEGIN
    gzip -c < "$log_file"
    ;;
retarget)
    dest=$(resolve_dir "${1:-}")
    test -f "$dest/$marker" || { echo "$dest was not installed by MoonTerminal" >&2; exit 1; }
    validate_port "${2:-}"
    old=$(head -n 1 "$dest/$marker")
    new_ufw4=0
    new_ufw6=0
    if test "$old" != "$p" && ufw_active; then
        # The new port needs admission whoever owned the old rule. Existing user rules stay
        # unmarked; a terminal comment left by an interrupted attempt retains its ownership.
        ufw_open "$p" "$dest"
        ufw_delete_marked "$dest/$marker" "$old" "$dest"
    fi
    kept="$dest/$marker.retarget"
    {
        printf '%s\n' "$p"
        if test "$old" = "$p"; then
            tail -n +2 "$dest/$marker"
        else
            tail -n +2 "$dest/$marker" | while IFS= read -r line; do
                case $line in
                ufw|ufw4|ufw6) ;;
                *) printf '%s\n' "$line" ;;
                esac
            done
            test "$new_ufw4" = 0 || echo ufw4
            test "$new_ufw6" = 0 || echo ufw6
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
    cd "$dest"
    account=$(command_account "$dest")
    if test -z "$account"; then
        # The wrapper finds its instance by the working directory and runs as root, without the
        # service's library path: system tools it calls must not see the core's libraries.
        exec ./mb "$@"
    fi
    # The wrapper refuses everyone but root, so another account runs its program directly.
    exec setpriv --reuid="${account% *}" --regid="${account#* }" --clear-groups --no-new-privs \
        -- env -i PATH="$PATH" ./MoonBot "$@"
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
    egress_bound "$unit" || { echo "The core started outside its outgoing address $(egress_of "$unit")" >&2; exit 1; }
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
    # Only rules this move opened go: either port family may have been open before it.
    if ufw_active; then
        ufw_delete_marked "$dest/$marker" "$port" "$dest"
    fi
    # Never fails: a rule reload that breaks for other cores must not leave this one half-removed.
    egress_undo "$unit"
    rm -rf -- "$dest"
    echo REMOVE_OK
    ;;
*)
    echo "unknown step $step" >&2
    exit 2
    ;;
esac
