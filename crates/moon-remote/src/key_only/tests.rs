use std::io::Write;
use std::process::Stdio;

use super::*;

/// Run `body` under `sh` after the bootstrap's functions, on a fixture where `sshd -T` answers
/// the way OpenSSH 10.5 does — capitalised keywords — and, like sshd, takes each keyword from the
/// first drop-in that names it, in file-name order: `00-moon-key-only` (passwords off), then the
/// hand-made `00-moon-password` (passwords and root on), then `00-moon-station` (both off). `deaf`
/// makes it keep saying that passwords are taken whatever the files hold. `getent` points every
/// login at `$HOME_DIR`. Returns stdout.
fn bootstrap_fixture(deaf: bool, body: &str) -> String {
    let (functions, _) = script::BOOTSTRAP
        .split_once("\nstep=${1:-}\n")
        .expect("bootstrap.sh has a dispatcher");
    let deaf = if deaf { "yes" } else { "no" };
    let code = format!(
        r#"FIX=$(mktemp -d)
trap 'rm -rf "$FIX"' EXIT
id() {{ printf '0\n'; }}
{functions}
KEY_ONLY_DROPIN="$FIX/00-moon-key-only.conf"
SSHD_DROPIN="$FIX/00-moon-station.conf"
PASSWORD_DROPIN="$FIX/00-moon-password.conf"
PASSWORD_ASIDE="$FIX/00-moon-password.conf.moon-off"
PASSWORD_SETTLED="$FIX/00-moon-password.conf.moon-removed"
HOME_DIR="$FIX/home"
mkdir -p "$HOME_DIR"
getent() {{ printf 'root:x:0:0:root:%s:/bin/sh\n' "$HOME_DIR"; }}
chown() {{ :; }}
sshd_includes_dropins() {{ :; }}
install() {{ cp "$7" "$8"; }}
systemctl() {{ :; }}
sshd() {{
    case "$1" in
    -t) return 0 ;;
    -T)
        printf 'Port 22\nKbdInteractiveAuthentication no\n'
        if [ {deaf} = yes ]; then
            pa=yes
        elif [ -f "$KEY_ONLY_DROPIN" ]; then
            pa=no
        elif [ -f "$PASSWORD_DROPIN" ]; then
            pa=yes
        elif [ -f "$SSHD_DROPIN" ]; then
            pa=no
        else
            pa=yes
        fi
        if [ -f "$PASSWORD_DROPIN" ]; then
            root=yes
        elif [ -f "$SSHD_DROPIN" ]; then
            root=no
        else
            root=yes
        fi
        printf 'PasswordAuthentication %s\nPermitRootLogin %s\n' "$pa" "$root"
        ;;
    esac
}}
{body}
"#
    );
    // Stdin carries the code, as in the setup's fixtures: no argv length limit, no quoting.
    let mut child = std::process::Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git Bash sh or POSIX sh is required for the sshd fixture");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).replace('\r', "")
}

/// Comparing `sshd -T` as printed reads OpenSSH 10.5's `PasswordAuthentication no` as a setting
/// that never took: the step would then remove its own file and fail on every such server. The
/// second run must find the server closed and leave it alone.
#[test]
fn the_step_closes_a_server_whose_sshd_prints_capitalised_keywords() {
    let out = bootstrap_fixture(
        false,
        r#"step_key_only
step_key_only
echo '==FILE=='
cat "$KEY_ONLY_DROPIN"
echo '==PORTS=='
ssh_ports"#,
    );
    assert_eq!(
        out,
        "sshd=changed\nsshd=already\n==FILE==\n\
         # MoonBot core: key logins only. Sorted first so no later file can reopen it.\n\
         PasswordAuthentication no\nKbdInteractiveAuthentication no\n==PORTS==\n22\n"
    );
}

/// A drop-in that sshd does not follow must not stay: it sorts first, so it would silently take
/// effect the day the setting that overrode it goes away.
#[test]
fn a_file_sshd_does_not_follow_is_removed_again() {
    let out = bootstrap_fixture(
        true,
        r#"if (step_key_only) 2>/dev/null; then echo closed; else echo failed; fi
if [ -f "$KEY_ONLY_DROPIN" ]; then echo left; else echo removed; fi"#,
    );
    assert_eq!(out, "failed\nremoved\n");
}

/// The station's own closing step compared `sshd -T` in lower case as well, so on OpenSSH 10.5
/// it rolled itself back with "sshd still does not say".
#[test]
fn the_station_step_accepts_capitalised_keywords_too() {
    assert_eq!(bootstrap_fixture(false, "step_harden"), "sshd=changed\n");
}

/// A hand-made `00-moon-password.conf` sorts before the station's file and keeps root open, so the
/// station's check failed and the whole install rolled back (#1352). Harden must set it aside, and
/// its rollback must give it back: a user who still needs it keeps it after a failed install.
#[test]
fn the_station_step_sets_a_reopener_aside_and_its_rollback_restores_it() {
    let out = bootstrap_fixture(
        false,
        r#"printf 'PasswordAuthentication yes\nPermitRootLogin yes\n' >"$PASSWORD_DROPIN"
step_harden
if [ -f "$PASSWORD_DROPIN" ]; then echo reopener-live; else echo reopener-off; fi
step_unharden
if [ -f "$PASSWORD_DROPIN" ]; then echo restored; else echo lost; fi"#,
    );
    assert_eq!(out, "sshd=changed\nreopener-off\nsshd=reopened\nrestored\n");
}

/// The core's close sets the reopener aside too, and moving it is a change: a server it sat on is
/// never reported closed already, so the outside check and its rollback still run.
#[test]
fn the_core_step_sets_a_reopener_aside_and_its_rollback_restores_it() {
    let out = bootstrap_fixture(
        false,
        r#"printf 'PasswordAuthentication yes\nPermitRootLogin yes\n' >"$PASSWORD_DROPIN"
step_key_only
if [ -f "$PASSWORD_ASIDE" ]; then echo aside; fi
step_key_only_off
if [ -f "$PASSWORD_DROPIN" ]; then echo restored; else echo lost; fi"#,
    );
    assert_eq!(out, "sshd=changed\naside\nsshd=reopened\nrestored\n");
}

/// A close that fails on the server must leave the reopener where the user put it: dropping it on
/// a failed attempt would take away the password login without a key to replace it.
#[test]
fn a_failed_core_close_gives_the_reopener_back() {
    let out = bootstrap_fixture(
        true,
        r#"printf 'PasswordAuthentication yes\n' >"$PASSWORD_DROPIN"
if (step_key_only) 2>/dev/null; then echo closed; else echo failed; fi
if [ -f "$PASSWORD_DROPIN" ]; then echo restored; else echo lost; fi"#,
    );
    assert_eq!(out, "failed\nrestored\n");
}

/// A reopener an earlier, successful close set aside belongs to that close: the rollback of a
/// later run — a station setup whose check failed — must not bring `PermitRootLogin yes` back.
#[test]
fn a_later_rollback_does_not_resurrect_an_earlier_closes_reopener() {
    let out = bootstrap_fixture(
        false,
        r#"printf 'PasswordAuthentication yes\nPermitRootLogin yes\n' >"$PASSWORD_DROPIN"
step_key_only
step_harden
step_unharden
if [ -f "$PASSWORD_DROPIN" ]; then echo resurrected; else echo stays-off; fi
if [ -f "$PASSWORD_SETTLED" ]; then echo kept; fi"#,
    );
    assert_eq!(
        out,
        "sshd=changed\nsshd=changed\nsshd=reopened\nstays-off\nkept\n"
    );
}

/// The terminal's key goes in beside the user's own keys, once, and on a line of its own even when
/// the provider's file ends without a newline; anything that is not a public key is refused.
#[test]
fn authorize_appends_the_key_once_and_keeps_the_existing_lines() {
    let out = bootstrap_fixture(
        false,
        r#"mkdir -p "$HOME_DIR/.ssh"
printf 'ssh-ed25519 AAAAown user@pc' >"$HOME_DIR/.ssh/authorized_keys"
printf 'ssh-ed25519 AAAAapp moonterminal@pc\n' | step_authorize root
printf 'ssh-ed25519 AAAAapp moonterminal@pc\n' | step_authorize root
cat "$HOME_DIR/.ssh/authorized_keys"
if (echo 'not a key' | step_authorize root) 2>/dev/null; then echo took; else echo refused; fi"#,
    );
    assert_eq!(
        out,
        "key=added\nkey=present\nssh-ed25519 AAAAown user@pc\n\
         ssh-ed25519 AAAAapp moonterminal@pc\nrefused\n"
    );
}

/// Closed means the server names no password method; reading the flag the other way round
/// would report an open server as closed.
#[test]
fn only_a_refusal_without_password_methods_counts_as_closed() {
    let refused = |password_offered| -> Result<(), OpenError> {
        Err(OpenError::Refused { password_offered })
    };
    assert!(password_refused(refused(false), "root").is_ok());
    let open = password_refused(refused(true), "root").expect_err("a password is still offered");
    assert!(open.to_string().contains("still offers a password"));
    assert!(password_refused(Ok(()), "root").is_err());
}
