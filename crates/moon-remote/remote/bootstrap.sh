#!/bin/sh
# moon-station bootstrap: the one-time, full-privilege half of preparing a server.
#
# Runs as root, delivered by `moon-remote setup` as `sh -c '<this file>' moon-bootstrap <step>
# [args]`; secrets arrive on stdin, never in argv. Every step checks the state it is about to
# change first, so running the setup again is harmless.
#
# The `key-only` pair of steps and `authorize` are the part a core's install uses as well
# (`moon-remote`'s `key_only`): they close password logins on a core's server, after a password
# login only once the terminal's own key has been authorized and proven.
set -eu
umask 022

HELPER=/usr/local/sbin/moon-station-admin
SUDOERS=/etc/sudoers.d/moon-station
SSHD_DROPIN=/etc/ssh/sshd_config.d/00-moon-station.conf
KEY_ONLY_DROPIN=/etc/ssh/sshd_config.d/00-moon-key-only.conf
# A hand-made reopener: a chat reply of 2026-11-12 told users to create it (`PasswordAuthentication
# yes`, `PermitRootLogin yes`) so a move could log in by password. It sorts between the two files
# above, so it keeps root logins open past the station's file. The closing steps move it aside,
# out of the `*.conf` pattern sshd includes, and their rollbacks move it back.
PASSWORD_DROPIN=/etc/ssh/sshd_config.d/00-moon-password.conf
PASSWORD_ASIDE=/etc/ssh/sshd_config.d/00-moon-password.conf.moon-off
# Where an aside goes once the close that made it is over: no later rollback may bring it back.
PASSWORD_SETTLED=/etc/ssh/sshd_config.d/00-moon-password.conf.moon-removed
UNIT=/etc/systemd/system/moon-station.service
REMOVED=/etc/moon-station/removed
UPDATE_PATH_UNIT=/etc/systemd/system/moon-station-update.path
UPDATE_SERVICE_UNIT=/etc/systemd/system/moon-station-update.service
F2B_JAIL=/etc/fail2ban/jail.d/moon-station.conf
AUTO_UPGRADES=/etc/apt/apt.conf.d/20auto-upgrades
SYSCTL=/etc/sysctl.d/60-moon-station.conf
RMEM_MAX=8388608

die() {
    echo "error: $*" >&2
    exit 1
}

[ "$(id -u)" -eq 0 ] || die "the bootstrap must run as root"

# A login name the setup may create or adopt: never root, never the service account.
valid_name() {
    case "$1" in
    '' | root | moon-station | *[!a-z0-9_-]*) return 1 ;;
    [a-z_]*) [ "${#1}" -le 32 ] ;;
    *) return 1 ;;
    esac
}

# Install stdin-produced content at $1 with mode $2 and owner $3, only when it differs.
# Prints "changed" or "unchanged".
put_file() {
    tmp=$(mktemp)
    cat >"$tmp"
    if [ -f "$1" ] && cmp -s "$tmp" "$1"; then
        rm -f "$tmp"
        echo unchanged
    else
        # Returned, not left to `set -e`: a caller testing put_file in an `if` has errexit off.
        if ! install -m "$2" -o "${3%%:*}" -g "${3##*:}" "$tmp" "$1"; then
            rm -f "$tmp"
            return 1
        fi
        rm -f "$tmp"
        echo changed
    fi
}

apt_install() {
    command -v apt-get >/dev/null || die "no apt-get: install $* by hand"
    export DEBIAN_FRONTEND=noninteractive
    apt-get -q -o DPkg::Lock::Timeout=300 update >/dev/null
    apt-get -q -y -o DPkg::Lock::Timeout=300 install "$@" >/dev/null
}

reload_sshd() {
    systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null ||
        systemctl restart ssh 2>/dev/null || systemctl restart sshd
}

# The sockets listening beyond this machine, one unique `<port>/<tcp|udp>` per line. Loopback and
# link-local addresses are left out. Without a working `ss` prints nothing and fails.
listeners() {
    command -v ss >/dev/null || return 1
    out=$(LC_ALL=C ss -Hlntu) || return 1
    printf '%s
' "$out" | awk 'NF{print $1, $5}' | while read -r proto addr; do
        port=${addr##*:}
        host=${addr%:*}
        case "$host" in
        127.* | '[::1]' | '[::ffff:127.'* | 169.254.* | fe80* | '[fe80'* | *%lo) continue ;;
        esac
        case "$proto" in
        tcp | udp) echo "$port/$proto" ;;
        esac
    done | sort -u
}

# Settle an aside an EARLIER run left, so this run's rollback must not resurrect it. Called first
# by every closing step; afterwards an aside is this run's alone. An earlier run whose own
# rollback never ran (killed by its deadline, or a rollback that failed) is settled the same way:
# the server then stays closed, the state #1352 asks for, and the file stays as `.moon-removed`
# for a hand restore.
password_dropin_settle() {
    if [ -f "$PASSWORD_ASIDE" ]; then
        mv -f "$PASSWORD_ASIDE" "$PASSWORD_SETTLED"
    fi
}

# Move the hand-made reopener out of sshd's reach. Prints nothing; succeeds when there is none.
password_dropin_aside() {
    if [ -f "$PASSWORD_DROPIN" ]; then
        mv -f "$PASSWORD_DROPIN" "$PASSWORD_ASIDE"
    fi
}

# Put the reopener back where the user left it, for a rollback.
password_dropin_back() {
    if [ -f "$PASSWORD_ASIDE" ]; then
        mv -f "$PASSWORD_ASIDE" "$PASSWORD_DROPIN"
    fi
}

sshd_includes_dropins() {
    grep -Eq '^[[:space:]]*Include[[:space:]]+/etc/ssh/sshd_config\.d/\*\.conf' /etc/ssh/sshd_config
}

# sshd's effective settings, in lower case. OpenSSH prints the keywords of `sshd -T` in lower
# case up to 10.0 and capitalised since (`PasswordAuthentication no`, seen on 10.5): compared as
# printed, a newer sshd would read as one that never took the setting.
sshd_effective() {
    LC_ALL=C sshd -T 2>/dev/null | tr '[:upper:]' '[:lower:]'
}

# Whether sshd, as its files stand now, takes no password of either kind.
sshd_refuses_passwords() {
    effective=$(sshd_effective || true)
    for want in 'passwordauthentication no' 'kbdinteractiveauthentication no'; do
        printf '%s\n' "$effective" | grep -qx "$want" || return 1
    done
}

# The ports sshd really listens on, one per line; nothing when sshd cannot say. A provider's NAT
# may forward the port the client dials to another one here.
ssh_ports() {
    sshd_effective | awk '$1=="port"{print $2}' | sort -u || true
}

# fw_ports <given_port>: the ports the firewall must keep open for SSH — sshd's own and the one
# the client dials, unique.
fw_ports() {
    { ssh_ports; echo "$1"; } | sort -un
}

ufw_state() {
    if command -v ufw >/dev/null; then
        LC_ALL=C ufw status | awk 'NR==1{print ($2 == "active") ? "active" : "inactive"}'
    else
        echo none
    fi
}

# probe <ssh_port>: read-only. Every other listening port is listed: the firewall will close it.
step_probe() {
    sshport=${1:-22}
    echo "arch=$(uname -m)"
    if [ -r /etc/os-release ]; then
        . /etc/os-release
        echo "os=${ID:-unknown} ${VERSION_ID:-}"
    fi
    if [ -d /run/systemd/system ]; then
        echo "systemd=$(LC_ALL=C systemctl --version | awk 'NR==1{print $2}')"
    else
        echo "systemd=none"
    fi
    echo "tpm2=$(systemd-creds has-tpm2 2>/dev/null | head -n1 || true)"
    echo "disk_free_mb=$(LC_ALL=C df -Pm /var/lib | awk 'NR==2{print $4}')"
    echo "rmem_max=$(cat /proc/sys/net/core/rmem_max)"
    if command -v apt-get >/dev/null; then echo "apt=yes"; else echo "apt=no"; fi
    if sshd_includes_dropins; then echo "sshd_include=yes"; else echo "sshd_include=no"; fi
    echo "ufw=$(ufw_state)"
    if command -v ufw >/dev/null; then
        echo "ufw_saved=$(LC_ALL=C ufw show added 2>/dev/null | grep -c '^ufw ' || true)"
    else
        echo "ufw_saved=0"
    fi
    keep=$(fw_ports "$sshport")
    if found=$(listeners); then
        for listen in $found; do
            open=no
            for p in $keep; do
                [ "$listen" = "$p/tcp" ] && open=yes
            done
            [ "$open" = yes ] || echo "listen=$listen"
        done
    else
        echo "listen=unknown"
    fi
}

# admin <name>; stdin: public key lines — the app's first, then the user's own when the first login
# was by key, so the user keeps a way in of their own.
step_admin() {
    name=${1:-}
    valid_name "$name" || die "not a usable administrator name: $name"
    keys_in=$(mktemp)
    cat >"$keys_in"
    [ -s "$keys_in" ] || die "no public key on stdin"
    while IFS= read -r pub; do
        case "$pub" in
        "ssh-ed25519 "* | "ecdsa-sha2-"* | "ssh-rsa "*) ;;
        *) die "not an ed25519, ECDSA or RSA public key" ;;
        esac
    done <"$keys_in"
    if id "$name" >/dev/null 2>&1; then
        echo "admin=exists"
    else
        useradd -m -s /bin/bash "$name"
        echo "admin=created"
    fi
    home=$(getent passwd "$name" | cut -d: -f6)
    primary=$(id -gn "$name")
    install -d -m 700 -o "$name" -g "$primary" "$home/.ssh"
    keys="$home/.ssh/authorized_keys"
    [ -f "$keys" ] || install -m 600 -o "$name" -g "$primary" /dev/null "$keys"
    while IFS= read -r pub; do
        body=$(printf '%s\n' "$pub" | awk '{print $2}')
        if grep -qF "$body" "$keys"; then
            echo "key=present"
        else
            printf '%s\n' "$pub" >>"$keys"
            echo "key=added"
        fi
    done <"$keys_in"
    rm -f "$keys_in"
    chown "$name:$primary" "$keys"
    chmod 600 "$keys"
}

# helper <admin>; stdin: the helper script. Installs it root-owned; the administrator runs sudo
# without a password.
step_helper() {
    name=${1:-}
    valid_name "$name" || die "not a usable administrator name: $name"
    id "$name" >/dev/null 2>&1 || die "no user $name"
    tmp=$(mktemp)
    cat >"$tmp"
    head -n1 "$tmp" | grep -q '^#!/bin/sh' || die "the helper does not look like a script"
    echo "helper=$(put_file "$HELPER" 755 root:root <"$tmp")"
    rm -f "$tmp"
    rule=$(mktemp)
    # The administrator has no password (step_admin), so sudo asks for none. Whoever holds the
    # terminal's key already holds the station's core keys through the helper's `update`; root on a
    # server that runs nothing else adds no reach, and lets the terminal update the helper itself.
    printf '# moon-station: the administrator, by key only, without a password\n%s ALL=(ALL) NOPASSWD: ALL\n' \
        "$name" >"$rule"
    visudo -cf "$rule" >/dev/null || die "visudo rejected the rule"
    echo "sudoers=$(put_file "$SUDOERS" 440 root:root <"$rule")"
    rm -f "$rule"
    # Key logins only (decided 2026-09-30): a server set up with an administrator password loses
    # it — only here, once the rule above lets sudo go without it, or root would go away from the
    # very login doing this. `*` (no password), not `passwd -l`: sshd without PAM refuses a LOCKED
    # account ("!") even its key logins, and a fresh `useradd` account starts locked.
    usermod -p '*' "$name"
    echo "password=none"
    put_update_units
}

# The station's update from its bot's chat: the station (unprivileged) files a request without
# parameters, the path unit starts the helper's update-from-release as root. Installed with the
# helper they run, so a terminal that replaces an older helper brings them too.
put_update_units() {
    path=$(printf '%s\n' \
        '# moon-station: an update asked for from the station bot (update-from-release).' \
        '[Unit]' \
        'Description=MoonTerminal station update request' \
        '' \
        '[Path]' \
        'PathExists=/var/lib/moon-station/update.request' \
        'Unit=moon-station-update.service' \
        '' \
        '[Install]' \
        'WantedBy=paths.target' | put_file "$UPDATE_PATH_UNIT" 644 root:root)
    service=$(printf '%s\n' \
        '# moon-station: the latest release installed, health-checked, rolled back on failure.' \
        '[Unit]' \
        'Description=MoonTerminal station update from the latest release' \
        '' \
        '[Service]' \
        'Type=oneshot' \
        'ExecStartPre=/bin/rm -f /var/lib/moon-station/update.request' \
        "ExecStart=$HELPER update-from-release" \
        'TimeoutStartSec=15min' | put_file "$UPDATE_SERVICE_UNIT" 644 root:root)
    if [ "$path" = changed ] || [ "$service" = changed ]; then
        systemctl daemon-reload
    fi
    systemctl enable --now --quiet moon-station-update.path
    echo "update_units=$(systemctl is-active moon-station-update.path)"
}

# service; stdin: the unit file. The service account, its directories, the unit, the UDP receive
# ceiling. Does not start the station — it has nothing to connect to until its credentials exist —
# but restarts one already running when the ceiling was raised, so its socket gets it.
step_service() {
    getent group moon-station >/dev/null || groupadd --system moon-station
    if ! id moon-station >/dev/null 2>&1; then
        useradd --system --gid moon-station --home-dir /var/lib/moon-station \
            --no-create-home --shell /usr/sbin/nologin moon-station
    fi
    install -d -m 755 -o root -g root /opt/moon-station /opt/moon-station/bin
    install -d -m 750 -o root -g moon-station /etc/moon-station
    install -d -m 700 -o root -g root /etc/moon-station/creds
    install -d -m 700 -o moon-station -g moon-station /var/lib/moon-station
    # Only explicit setup re-arms a removed station. Helper refreshes must retain revocation.
    rm -f "$REMOVED"
    echo "unit=$(put_file "$UNIT" 644 root:root)"
    systemctl daemon-reload
    # moonproto asks for an 8 MB UDP receive buffer; the kernel caps it at rmem_max, 208 KB on a
    # stock Ubuntu. Raised, never lowered.
    # A running station keeps the buffer it opened with: restarted, so the new ceiling applies.
    if [ "$(cat /proc/sys/net/core/rmem_max)" -lt "$RMEM_MAX" ]; then
        echo "net.core.rmem_max = $RMEM_MAX" | put_file "$SYSCTL" 644 root:root >/dev/null
        sysctl -q -p "$SYSCTL"
        if systemctl is-active --quiet moon-station; then
            systemctl restart moon-station
            echo "station=restarted"
        fi
    fi
    echo "rmem_max=$(cat /proc/sys/net/core/rmem_max)"
}

# harden: keys only, no root login. Our file sorts before cloud-init's 50-cloud-init.conf, and
# sshd takes the first value it reads. Verified through `sshd -T`, rolled back on any doubt. The
# hand-made reopener sorts before our file and would keep root open, so it is moved aside first.
step_harden() {
    sshd_includes_dropins ||
        die "sshd_config does not include sshd_config.d"
    password_dropin_settle
    moved=no
    if [ -f "$PASSWORD_DROPIN" ]; then
        password_dropin_aside
        moved=yes
    fi
    if ! state=$(printf '%s\n' \
        '# moon-station: keys only, no root. Sorted first so no later file can reopen it.' \
        'PermitRootLogin no' \
        'PasswordAuthentication no' \
        'KbdInteractiveAuthentication no' | put_file "$SSHD_DROPIN" 644 root:root); then
        password_dropin_back
        die "could not write $SSHD_DROPIN"
    fi
    [ "$moved" = no ] || state=changed
    if ! sshd -t; then
        rm -f "$SSHD_DROPIN"
        password_dropin_back
        die "sshd rejected the configuration; removed it"
    fi
    effective=$(sshd_effective || true)
    for want in 'permitrootlogin no' 'passwordauthentication no' 'kbdinteractiveauthentication no'; do
        if ! printf '%s\n' "$effective" | grep -qx "$want"; then
            rm -f "$SSHD_DROPIN"
            password_dropin_back
            reload_sshd
            die "sshd still does not say \"$want\"; removed our file"
        fi
    done
    reload_sshd
    echo "sshd=$state"
}

# unharden: the rollback of harden, for when the new logins do not work.
step_unharden() {
    rm -f "$SSHD_DROPIN"
    password_dropin_back
    sshd -t
    reload_sshd
    echo "sshd=reopened"
}

# authorize <login>; stdin: public key lines. Appends to that login's authorized_keys every key
# it lacks — the terminal's own, so a server closed to passwords after a password login still lets
# the terminal in. Root is a valid login here: a provider's image often has no other account. The
# file is appended to, never replaced, and a directory or file that exists keeps its mode. Prints
# `key=added` or `key=present` per key.
step_authorize() {
    name=${1:-}
    case "$name" in
    '' | *[!a-z0-9_.-]*) die "not a usable login name: $name" ;;
    esac
    entry=$(getent passwd "$name") || die "no user $name"
    home=$(printf '%s\n' "$entry" | cut -d: -f6)
    [ -n "$home" ] && [ -d "$home" ] || die "$name has no home directory"
    keys_in=$(mktemp)
    cat >"$keys_in"
    if [ ! -s "$keys_in" ]; then
        rm -f "$keys_in"
        die "no public key on stdin"
    fi
    while IFS= read -r pub; do
        case "$pub" in
        "ssh-ed25519 "* | "ecdsa-sha2-"* | "ssh-rsa "*) ;;
        *)
            rm -f "$keys_in"
            die "not an ed25519, ECDSA or RSA public key"
            ;;
        esac
    done <"$keys_in"
    primary=$(id -gn "$name")
    if [ ! -d "$home/.ssh" ]; then
        mkdir -p "$home/.ssh"
        chown "$name:$primary" "$home/.ssh"
        chmod 700 "$home/.ssh"
    fi
    keys="$home/.ssh/authorized_keys"
    if [ ! -f "$keys" ]; then
        : >"$keys"
        chown "$name:$primary" "$keys"
        chmod 600 "$keys"
    fi
    # A last line without its newline would glue the appended key onto it.
    if [ -s "$keys" ] && [ -n "$(tail -c 1 "$keys")" ]; then
        echo >>"$keys"
    fi
    while IFS= read -r pub; do
        body=$(printf '%s\n' "$pub" | awk '{print $2}')
        if grep -qF "$body" "$keys"; then
            echo "key=present"
        else
            printf '%s\n' "$pub" >>"$keys"
            echo "key=added"
        fi
    done <"$keys_in"
    rm -f "$keys_in"
}

# key-only: password logins off, nothing else touched — for a core's server. The way in that
# stays is the login's own key, or after a password login the terminal's key that `authorize`
# added first; this step adds no account and no key, and root's own login stays as it was: the
# core runs as root and its user logs in as root. `PermitRootLogin` is
# left out on purpose: this file sorts before the station's, and naming it here would reopen a
# root login the station closed. Prints `sshd=already` when sshd takes no password as it stands,
# `sshd=changed` when this step closed it. The hand-made reopener is moved aside on the way: moving
# it is a change too, so a server it sat on is never reported as closed already.
step_key_only() {
    password_dropin_settle
    moved=no
    if [ -f "$PASSWORD_DROPIN" ]; then
        password_dropin_aside
        moved=yes
    fi
    # Asked first: a server closed in its main sshd_config needs no drop-in directory at all.
    if [ "$moved" = no ] && sshd_refuses_passwords; then
        echo "sshd=already"
        return 0
    fi
    if ! sshd_includes_dropins; then
        password_dropin_back
        die "sshd_config does not include sshd_config.d"
    fi
    if ! printf '%s\n' \
        '# MoonBot core: key logins only. Sorted first so no later file can reopen it.' \
        'PasswordAuthentication no' \
        'KbdInteractiveAuthentication no' | put_file "$KEY_ONLY_DROPIN" 644 root:root >/dev/null; then
        password_dropin_back
        die "could not write $KEY_ONLY_DROPIN"
    fi
    if ! sshd -t; then
        rm -f "$KEY_ONLY_DROPIN"
        password_dropin_back
        die "sshd rejected the configuration; removed it"
    fi
    if ! sshd_refuses_passwords; then
        rm -f "$KEY_ONLY_DROPIN"
        password_dropin_back
        die "sshd would still take a password; removed our file"
    fi
    # A file left behind an sshd that never read it would make the next run say `already`.
    if ! reload_sshd; then
        rm -f "$KEY_ONLY_DROPIN"
        password_dropin_back
        die "sshd did not reload; removed our file"
    fi
    echo "sshd=changed"
}

# key-only-off: the rollback of key-only, for when the key login does not work afterwards.
step_key_only_off() {
    rm -f "$KEY_ONLY_DROPIN"
    password_dropin_back
    sshd -t
    reload_sshd
    echo "sshd=reopened"
}

valid_port() {
    case "$1" in
    '' | *[!0-9]*) return 1 ;;
    esac
    [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

# ufw-install: the firewall tool, when the server has none.
step_ufw_install() {
    if command -v ufw >/dev/null; then
        echo "ufw=present"
    else
        apt_install ufw
        echo "ufw=installed"
    fi
}

# Prints `firewall_added=<port>` when `ufw allow` output $2 says a rule was added for port $1 and
# none existed: `ufw delete` removes both families, so a half-existing port is never noted.
note_added() {
    case "$2" in
        *"Skipping adding existing rule"*) return 1 ;;
    esac
    if printf '%s
' "$2" | grep -q '^Rule added'; then
        echo "firewall_added=$1"
        return 0
    fi
    return 1
}

# firewall <ssh_port>: the SSH ports (sshd's own and the given one) are allowed; an active ufw
# gains only them, an inactive one is turned on denying everything else incoming, enabled LAST.
# Prints every `firewall_added=<port>`, `firewall_prev_in=<policy>` when it enables, and as its
# last line `firewall=unchanged|rule-added|enabled` for the rollback.
step_firewall() {
    port=${1:-}
    valid_port "$port" || die "not a usable SSH port: $port"
    command -v ufw >/dev/null || die "no ufw on this server"
    ports=$(fw_ports "$port")
    if [ "$(ufw_state)" = active ]; then
        any=no
        for p in $ports; do
            out=$(LC_ALL=C ufw allow "$p/tcp")
            if note_added "$p" "$out"; then any=yes; fi
        done
        LC_ALL=C ufw status | sed 's/^/ufw: /' || true
        if [ "$any" = yes ]; then echo "firewall=rule-added"; else echo "firewall=unchanged"; fi
    else
        prev=$(sed -n 's/^DEFAULT_INPUT_POLICY=//p' /etc/default/ufw 2>/dev/null | tr -d '"' | head -n1 || true)
        echo "firewall_prev_in=${prev:-unknown}"
        for p in $ports; do
            out=$(LC_ALL=C ufw allow "$p/tcp")
            note_added "$p" "$out" || true
        done
        LC_ALL=C ufw default deny incoming >/dev/null
        LC_ALL=C ufw default allow outgoing >/dev/null
        LC_ALL=C ufw --force enable >/dev/null
        LC_ALL=C ufw status | sed 's/^/ufw: /' || true
        echo "firewall=enabled"
    fi
}

# firewall-off <prev_in> <port>...: the rollback of a firewall this setup enabled — off, the
# rules it added gone, the incoming policy back to what it was.
step_firewall_off() {
    if ! command -v ufw >/dev/null; then
        echo "ufw=absent"
        return 0
    fi
    prev=${1:-unknown}
    [ "$#" -gt 0 ] && shift
    LC_ALL=C ufw --force disable >/dev/null
    for p in "$@"; do
        valid_port "$p" || die "not a usable port: $p"
        LC_ALL=C ufw delete allow "$p/tcp" >/dev/null || true
    done
    case "$prev" in
    DROP) policy=deny ;;
    REJECT) policy=reject ;;
    *) policy=allow ;;
    esac
    LC_ALL=C ufw default "$policy" incoming >/dev/null
    echo "ufw=disabled"
}

# firewall-unallow <port>...: the rollback of the rules added to an already active firewall.
step_firewall_unallow() {
    for p in "$@"; do
        valid_port "$p" || die "not a usable port: $p"
    done
    for p in "$@"; do
        LC_ALL=C ufw delete allow "$p/tcp" >/dev/null
    done
    echo "ufw=rule-removed"
}

# extras: fail2ban on sshd, unattended security upgrades.
step_extras() {
    apt_install fail2ban python3-systemd unattended-upgrades
    jail=$(printf '%s\n' '[sshd]' 'enabled = true' 'backend = systemd' |
        put_file "$F2B_JAIL" 644 root:root)
    systemctl enable fail2ban >/dev/null 2>&1
    if [ "$jail" = changed ] || ! systemctl is-active --quiet fail2ban; then
        systemctl restart fail2ban
    fi
    echo "fail2ban=$(systemctl is-active fail2ban)"
    printf '%s\n' 'APT::Periodic::Update-Package-Lists "1";' 'APT::Periodic::Unattended-Upgrade "1";' |
        put_file "$AUTO_UPGRADES" 644 root:root >/dev/null
    systemctl enable --now unattended-upgrades >/dev/null 2>&1 || true
    echo "unattended_upgrades=$(systemctl is-active unattended-upgrades || true)"
}

step=${1:-}
[ "$#" -gt 0 ] && shift
case "$step" in
probe) step_probe "$@" ;;
admin) step_admin "$@" ;;
helper) step_helper "$@" ;;
service) step_service ;;
harden) step_harden ;;
unharden) step_unharden ;;
authorize) step_authorize "$@" ;;
key-only) step_key_only ;;
key-only-off) step_key_only_off ;;
ufw-install) step_ufw_install ;;
firewall) step_firewall "$@" ;;
firewall-off) step_firewall_off "$@" ;;
firewall-unallow) step_firewall_unallow "$@" ;;
extras) step_extras ;;
*) die "unknown step: $step" ;;
esac
