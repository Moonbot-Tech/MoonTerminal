#!/bin/sh
# Install only the Station's dedicated HTTPS service. Its public listener has no admin routes.
set -eu
umask 077
host=${1:?public host required}
case "$host" in ''|*[!a-zA-Z0-9.:-]*) echo 'invalid public host' >&2; exit 1;; esac
[ "$(id -u)" -eq 0 ]
exec 9>/run/moon-station-admin.lock
flock -w 30 9
[ ! -f /etc/moon-station/removed ] || { echo 'station was removed' >&2; exit 1; }
unit=moon-station-https.service
root=/opt/moon-station-https
conf=/etc/moon-station-https
for tool in curl tar sha256sum ss; do command -v "$tool" >/dev/null; done
if ! systemctl is-active --quiet "$unit" && [ -n "$(ss -H -ltn 'sport = :443')" ]; then
    echo 'TCP 443 is already occupied; configure the existing HTTPS proxy explicitly' >&2
    exit 1
fi
case "$(uname -m)" in
    x86_64) arch=amd64; digest=5b8cb7faf8726a2028ad96a66e54e6e81e8f361bc6175e5d22bbd997cc5177fb;;
    aarch64) arch=arm64; digest=5e377cff994fc9362a6fcf695c53f226cfca3a3ef85bd3a3687cf252d6310165;;
    *) echo 'unsupported HTTPS server architecture' >&2; exit 1;;
esac
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT HUP INT TERM
# Pinned official release digests: a failed download never replaces the installed executable.
curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --max-time 180 \
    "https://github.com/caddyserver/caddy/releases/download/v2.11.9/caddy_2.11.9_linux_$arch.tar.gz" -o "$tmp/caddy.tar.gz"
echo "$digest  $tmp/caddy.tar.gz" | sha256sum --check --status
tar -xzf "$tmp/caddy.tar.gz" -C "$tmp" caddy
id moon-station-https >/dev/null 2>&1 || useradd --system --home-dir /var/lib/moon-station-https --shell /usr/sbin/nologin moon-station-https
install -d -m 755 "$root" "$conf"
install -d -m 700 -o moon-station-https -g moon-station-https /var/lib/moon-station-https
# IP clients commonly send no SNI. HTTP-01 and redirects are disabled: only TCP 443 is needed.
cat >"$tmp/Caddyfile" <<EOF
{
    admin off
    auto_https disable_redirects
    default_sni $host
    servers {
        timeouts {
            read_header 10s
            read_body 10s
            write 60s
            idle 120s
        }
    }
}
https://$host {
    tls {
        issuer acme {
            dir https://acme-v02.api.letsencrypt.org/directory
            profile shortlived
            disable_http_challenge
        }
    }
    header Strict-Transport-Security "max-age=31536000"
    reverse_proxy 127.0.0.1:19791
}
EOF
# Brackets belong in an IPv6 URL, but not in default_sni.
case "$host" in *:*) sed -i "s|https://$host {|https://[$host] {|" "$tmp/Caddyfile";; esac
"$tmp/caddy" fmt --overwrite "$tmp/Caddyfile"
"$tmp/caddy" adapt --config "$tmp/Caddyfile" --adapter caddyfile >/dev/null
install -m 755 "$tmp/caddy" "$root/caddy.new"
mv -f "$root/caddy.new" "$root/caddy"
install -m 644 "$tmp/Caddyfile" "$conf/Caddyfile"
cat >/etc/systemd/system/moon-station-https.service <<'EOF'
[Unit]
Description=MoonStation MCP HTTPS
Wants=network-online.target
After=network-online.target moon-station.service
[Service]
User=moon-station-https
Group=moon-station-https
ExecStart=/opt/moon-station-https/caddy run --config /etc/moon-station-https/Caddyfile --adapter caddyfile
Environment=XDG_DATA_HOME=/var/lib/moon-station-https
Environment=XDG_CONFIG_HOME=/var/lib/moon-station-https
StateDirectory=moon-station-https
StateDirectoryMode=0700
UMask=0077
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
[Install]
WantedBy=multi-user.target
EOF
if command -v ufw >/dev/null && ufw status | grep -q '^Status: active'; then
    ufw allow 443/tcp comment 'MoonStation MCP HTTPS'
fi
systemctl daemon-reload
systemctl enable "$unit"
systemctl restart "$unit"
echo 'https=started'
