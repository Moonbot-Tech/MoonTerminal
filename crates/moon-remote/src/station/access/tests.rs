//! Removal capability and sandboxed helper regressions; these never connect to a server.

use super::require_removal;

/// Accepting a missing/false capability would issue an unknown destructive command against
/// older helpers rather than advising the user to update first.
#[test]
fn removal_refuses_older_or_unknown_helper_capabilities() {
    for status in [
        "",
        "release_update=yes\n",
        "remove_station=no\n",
        "remove_station=maybe\n",
        "remove_station=yes\n",
        "remove_station=yes\nremoval_guard=no\n",
    ] {
        let error = require_removal(status).unwrap_err();
        assert!(matches!(
            error.downcast_ref::<super::RemovalError>(),
            Some(super::RemovalError::Unsupported)
        ));
    }
    require_removal("release_update=yes\nremove_station=yes\nremoval_guard=yes\n").unwrap();
}

/// Execute the embedded helper with all absolute storage paths redirected to a temporary
/// synthetic tree and systemctl/flock mocked. No root command or real station is touched.
fn removal_fixture(fail_stop: bool) -> std::process::Output {
    let helper = crate::script::HELPER
        .replace("/opt/moon-station", "$fixture/opt")
        .replace("/etc/moon-station", "$fixture/etc")
        .replace("/var/lib/moon-station", "$fixture/data")
        .replace(
            "/etc/systemd/system/moon-station.service.d",
            "$fixture/dropin",
        )
        .replace("/run/moon-station-admin.lock", "$fixture/lock")
        .replace("/run/moon-station/api.sock", "$fixture/api.sock");
    // Assignment values must be quoted because a Windows temp directory may contain spaces.
    let helper = helper
        .lines()
        .map(|line| {
            if let Some((name, value)) = line.split_once('=') {
                if value.starts_with("$fixture/") {
                    return format!("{name}=\"{value}\"");
                }
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let before = r#"
set -eu
fixture=$(mktemp -d)
test -d "$fixture"
trap 'rm -r -- "$fixture"' EXIT
mkdir -p "$fixture/etc/creds" "$fixture/data" "$fixture/opt" "$fixture/admin"
for file in etc/creds/core-1.cred etc/creds/core-9.cred etc/creds/.core-3.new etc/creds/telegram-token.cred etc/creds/.telegram-token.new etc/station.toml etc/station.toml.new data/telegram.json data/telegram.json.new data/telegram.json.tmp data/update.request; do
    printf 'synthetic credential' >"$fixture/$file"
done
printf 'administrator key' >"$fixture/admin/authorized_keys"
printf 'retained history' >"$fixture/data/reports.sqlite"
printf 'unrelated credential' >"$fixture/etc/creds/other.cred"
id() { printf '0\n'; }
flock() { return 0; }
systemd-creds() { printf 'encrypted synthetic key' >"$4"; }
systemctl() {
    printf '%s\n' "$*" >>"$fixture/calls"
    if [ "$fail_stop" = yes ] && [ "$*" = 'disable --now moon-station.service' ]; then return 1; fi
}
helper() {
"#;
    let after = r#"
}
# Another terminal cached config=yes before removal; its commands now arrive afterward.
cached_config=yes
helper remove-station
test ! -e "$fixture/etc/creds/core-1.cred"
test ! -e "$fixture/etc/creds/core-9.cred"
test ! -e "$fixture/etc/creds/.core-3.new"
test ! -e "$fixture/etc/creds/telegram-token.cred"
test ! -e "$fixture/etc/creds/.telegram-token.new"
test ! -e "$fixture/etc/station.toml"
test ! -e "$fixture/etc/station.toml.new"
test ! -e "$fixture/data/telegram.json"
test ! -e "$fixture/data/telegram.json.new"
test ! -e "$fixture/data/telegram.json.tmp"
test ! -e "$fixture/data/update.request"
test "$(cat "$fixture/admin/authorized_keys")" = 'administrator key'
test "$(cat "$fixture/data/reports.sqlite")" = 'retained history'
test "$(cat "$fixture/etc/creds/other.cred")" = 'unrelated credential'
test "$(sed -n '1p' "$fixture/calls")" = 'disable --now moon-station-update.path'
test "$(sed -n '2p' "$fixture/calls")" = 'stop moon-station-update.service'
test "$(sed -n '3p' "$fixture/calls")" = 'disable --now moon-station.service'
test "$(cat "$fixture/dropin/credentials.conf")" = '[Service]'
test "$cached_config" = yes
test -f "$fixture/etc/removed"
if (helper put-cred 7 </dev/null); then exit 1; fi
if (helper put-token </dev/null); then exit 1; fi
if (helper put-config none </dev/null); then exit 1; fi
if (helper start); then exit 1; fi
test ! -e "$fixture/etc/creds/core-7.cred"
test ! -e "$fixture/etc/creds/telegram-token.cred"
test ! -e "$fixture/etc/station.toml"
test "$(wc -l <"$fixture/calls")" -eq 4
"#;
    let code = format!(
        "fail_stop={}\n{before}\n{helper}\n{after}",
        if fail_stop { "yes" } else { "no" }
    );
    // Stdin avoids Windows command-line length limits and shell-wrapper quoting entirely.
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git Bash sh or POSIX sh is required for the synthetic helper fixture");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/// Omitting a credential/config deletion leaves material that can trade or impersonate the
/// bot; omitting the service stops lets it continue using secrets already in memory. Omitting
/// removal revocation lets a second terminal's stale automatic push recreate keys afterward.
#[test]
fn helper_removal_wipes_secrets_and_keeps_admin_access() {
    let out = removal_fixture(false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("removed=yes"));
}

/// Ignoring a failed systemctl stop would announce removal while a process still holds keys.
#[test]
fn helper_removal_does_not_claim_success_when_stopping_fails() {
    let out = removal_fixture(true);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("removed=yes"));
}
