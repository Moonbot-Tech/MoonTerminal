#!/bin/sh
# moon-station bootstrap: the one-time, full-privilege half of preparing a server.
#
# Runs as root, delivered by `moon-remote setup` as `sh -c '<this file>' moon-bootstrap <step>
# [args]`; secrets arrive on stdin, never in argv. Every step checks the state it is about to
# change first, so running the setup again is harmless.
set -eu
umask 022

HELPER=/usr/local/sbin/moon-station-admin
SUDOERS=/etc/sudoers.d/moon-station
SSHD_DROPIN=/etc/ssh/sshd_config.d/00-moon-station.conf
UNIT=/etc/systemd/system/moon-station.service
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
        install -m "$2" -o "${3%%:*}" -g "${3##*:}" "$tmp" "$1"
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

step_probe() {
    echo "arch=$(uname -m)"
    if [ -r /etc/os-release ]; then
        . /etc/os-release
        echo "os=${ID:-unknown} ${VERSION_ID:-}"
    fi
    if [ -d /run/systemd/system ]; then
        echo "systemd=$(systemctl --version | awk 'NR==1{print $2}')"
    else
        echo "systemd=none"
    fi
    echo "tpm2=$(systemd-creds has-tpm2 2>/dev/null | head -n1 || true)"
    echo "disk_free_mb=$(df -Pm /var/lib | awk 'NR==2{print $4}')"
    echo "rmem_max=$(cat /proc/sys/net/core/rmem_max)"
    if command -v ufw >/dev/null; then
        echo "ufw=$(ufw status | awk 'NR==1{print $2}')"
    else
        echo "ufw=none"
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
        "ssh-ed25519 "* | "ecdsa-sha2-"*) ;;
        *) die "not an ed25519 or ECDSA public key" ;;
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
# sshd takes the first value it reads. Verified through `sshd -T`, rolled back on any doubt.
step_harden() {
    grep -Eq '^[[:space:]]*Include[[:space:]]+/etc/ssh/sshd_config\.d/\*\.conf' /etc/ssh/sshd_config ||
        die "sshd_config does not include sshd_config.d"
    state=$(printf '%s\n' \
        '# moon-station: keys only, no root. Sorted first so no later file can reopen it.' \
        'PermitRootLogin no' \
        'PasswordAuthentication no' \
        'KbdInteractiveAuthentication no' | put_file "$SSHD_DROPIN" 644 root:root)
    if ! sshd -t; then
        rm -f "$SSHD_DROPIN"
        die "sshd rejected the configuration; removed it"
    fi
    effective=$(sshd -T 2>/dev/null || true)
    for want in 'permitrootlogin no' 'passwordauthentication no' 'kbdinteractiveauthentication no'; do
        if ! printf '%s\n' "$effective" | grep -qx "$want"; then
            rm -f "$SSHD_DROPIN"
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
    sshd -t
    reload_sshd
    echo "sshd=reopened"
}

# firewall: incoming denied except SSH. SSH is allowed BEFORE the firewall is enabled.
step_firewall() {
    command -v ufw >/dev/null || apt_install ufw
    ufw allow 22/tcp >/dev/null
    ufw default deny incoming >/dev/null
    ufw default allow outgoing >/dev/null
    ufw --force enable >/dev/null
    ufw status | sed 's/^/ufw: /'
}

# firewall-off: the rollback of firewall.
step_firewall_off() {
    ufw --force disable >/dev/null
    echo "ufw=disabled"
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
probe) step_probe ;;
admin) step_admin "$@" ;;
helper) step_helper "$@" ;;
service) step_service ;;
harden) step_harden ;;
unharden) step_unharden ;;
firewall) step_firewall ;;
firewall-off) step_firewall_off ;;
extras) step_extras ;;
*) die "unknown step: $step" ;;
esac
